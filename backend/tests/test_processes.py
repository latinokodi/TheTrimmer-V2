"""Tests for the child-process registry and killer.

The children here are real processes, not mocks. The bug being prevented is about what the
*operating system* does to a process when the parent quits, and a mock would agree with any
implementation, including one that forgets the Windows tree flag. A real ``sys.executable``
sleeper also keeps the tests independent of ffmpeg being installed.

Every test leaves the registry empty, including a failing one, via the autouse fixture --
otherwise a leaked entry would make an unrelated test's count wrong and the suite would
report the wrong failure.
"""

from __future__ import annotations

import os
import subprocess
import sys
import pytest

from concurrent.futures import ThreadPoolExecutor

from trimmer import processes

#: A child that would outlive the test run if nothing killed it, started through the same
#: interpreter running the tests so no external tool or PATH is involved.
SLEEP_COMMAND = [sys.executable, "-c", "import time; time.sleep(30)"]
#: A PID that cannot name a process on either platform: Windows PIDs are multiples of four,
#: and this is odd, while on POSIX it is far above any ``pid_max``.
IMPOSSIBLE_PID = 0x7FFFFFFF


@pytest.fixture(autouse=True)
def _empty_registry():
    """Start and end every test with no children, even when a test fails mid-way."""
    processes.terminate_all()
    yield
    processes.terminate_all()


def _sleeper() -> subprocess.Popen:
    """A real child that stays alive until something kills it."""
    return processes.spawn(
        SLEEP_COMMAND,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def _short_lived(script: str = "pass") -> subprocess.Popen:
    """A real child that exits on its own, for the already-exited paths."""
    return processes.spawn(
        [sys.executable, "-c", script],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def test_live_counts_spawned_children():
    """The registry count goes up on spawn and down on release, and nowhere else."""
    assert processes.live() == 0

    first = _sleeper()
    second = _sleeper()
    assert processes.live() == 2

    processes.terminate(first)
    processes.release(first)
    assert processes.live() == 1

    processes.terminate(second)
    processes.release(second)
    assert processes.live() == 0


def test_release_is_idempotent_and_works_after_the_child_exited():
    """Release is bookkeeping only: no wait, no signal, no error the second time."""
    child = _short_lived()
    assert processes.live() == 1

    child.wait(timeout=10)
    processes.release(child)
    processes.release(child)

    assert processes.live() == 0


def test_spawn_hands_the_caller_the_pipes_it_asked_for():
    """Spawn records a child; it does not decide how that child's output is read."""
    child = processes.spawn(
        [sys.executable, "-c", "print('hello')"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        out, err = child.communicate(timeout=10)
    finally:
        processes.release(child)

    assert out.strip() == "hello"
    assert err == ""
    assert processes.live() == 0


def test_terminate_kills_a_running_child():
    """After ``terminate`` the child is dead, and the OS agrees the PID is gone."""
    child = _sleeper()
    assert processes.parent_alive(child.pid)

    processes.terminate(child, timeout=5)

    assert child.poll() is not None
    assert processes.parent_alive(child.pid) is False
    processes.release(child)
    assert processes.live() == 0


def test_terminate_all_kills_and_forgets_several_children():
    """One call clears the registry and leaves no live child behind."""
    children = [_sleeper() for _ in range(3)]
    assert processes.live() == 3

    processes.terminate_all()

    assert processes.live() == 0
    for child in children:
        assert child.poll() is not None
        assert processes.parent_alive(child.pid) is False


def test_terminate_is_safe_on_an_already_exited_child():
    """A child that exited on its own must not make terminate raise or hang."""
    child = _short_lived()
    child.wait(timeout=10)

    processes.terminate(child)
    processes.terminate(child, timeout=0.01)

    processes.release(child)
    assert processes.live() == 0


def test_terminate_is_safe_after_terminate_all():
    """The quit path may race the worker: terminate after clearing must be harmless."""
    child = _sleeper()
    processes.terminate_all()

    processes.terminate(child, timeout=0.01)
    processes.release(child)

    assert processes.live() == 0


def test_parent_alive_is_true_for_this_process():
    """The process running the tests is, by definition, running."""
    assert processes.parent_alive(os.getpid()) is True


def test_parent_alive_is_false_for_a_pid_that_cannot_exist():
    """Non-existence is the one answer that is certain, and the only one read as dead."""
    assert processes.parent_alive(IMPOSSIBLE_PID) is False


def test_parent_alive_is_false_once_a_child_has_been_reaped():
    """The watchdog's real question: a child that finished is not alive."""
    child = _short_lived()
    child.wait(timeout=10)

    assert processes.parent_alive(child.pid) is False
    processes.release(child)


def test_concurrent_spawn_and_release_keep_the_count_consistent():
    """The worker thread and the HTTP thread share this registry; the lock must hold."""

    def one_round(_: int) -> None:
        child = _short_lived()
        child.wait(timeout=10)
        processes.release(child)

    with ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(one_round, range(8)))

    assert processes.live() == 0


@pytest.mark.skipif(sys.platform != "win32", reason="process groups are a Windows concept")
def test_spawn_requests_no_window_and_a_new_process_group_on_windows(monkeypatch):
    """The flags that stop a console flash and make the child's tree killable are set."""
    seen: dict[str, int] = {}
    real_popen = subprocess.Popen

    def recording_popen(args, **kwargs):
        seen["creationflags"] = kwargs.get("creationflags", 0)
        return real_popen(args, **kwargs)

    monkeypatch.setattr(processes.subprocess, "Popen", recording_popen)

    child = _sleeper()
    processes.terminate(child)
    processes.release(child)

    assert seen["creationflags"] & subprocess.CREATE_NO_WINDOW
    assert seen["creationflags"] & subprocess.CREATE_NEW_PROCESS_GROUP
