"""Every child process the backend starts, in one registry, so a quit can kill them all.

Windows ends only the process it is told to end. Electron quits by calling ``backend.kill()``
on the Python process, and a Windows process's children are *not* killed with it, so an ffmpeg
that was encoding a master keeps encoding: the file stays locked, the job is left in no state,
and the next attempt at the same source collides with a process nobody can see. The engine
does kill its ffmpeg when a cancel token is set, but a quit never sets one.

The fix is not "remember to cancel" -- it is that the children must be *findable* at the
moment the backend is about to die. That is what this module is: one place that records what
was started and one call that kills what is still there. The registry is module-level because
the children are process-wide: the worker thread that runs a cut does not share its locals
with the HTTP handler that receives the quit.

Only the standard library is used. A dependency here would be a dependency the quit path
cannot afford to import.
"""

from __future__ import annotations

import os
import subprocess
import sys
import threading
from typing import Any

#: Windows gives every console child of a *windowed* process (pythonw) its own console
#: window, and a trim is a dozen ffmpeg and ffprobe calls. Without this flag the app looks
#: like it is opening and closing command prompts instead of working; the output is read
#: through pipes either way, so nothing is lost. The flag does not exist off Windows, where
#: this is 0 and subprocess ignores it.
NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)

#: A child must be the root of its own process group, not a member of the console group this
#: backend happens to own. Console control events (Ctrl+C, Ctrl+Break) are delivered to a
#: *group*, so sharing one would let a signal aimed at the backend take the encoder down (or
#: the reverse) instead of the backend deciding when its own children stop. It also means
#: killing the child's group cannot reach this process. Does not exist off Windows (0).
NEW_PROCESS_GROUP = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)

_WINDOWS = sys.platform == "win32"

#: Children started and not yet forgotten, keyed by object identity. A set because a process
#: object is unique and discarding one twice must be a no-op; a list would make a double
#: release raise, and every release would need a membership test before the removal.
_REGISTRY: set[subprocess.Popen] = set()

#: Guards the registry, and only the registry. It is never held while waiting on a child:
#: the quit path kills a child while the worker thread may be finishing one, and a lock
#: around a five-second wait would make a clean shutdown hang behind a job already ending.
_LOCK = threading.Lock()

#: Rights asked of Windows for a liveness probe. SYNCHRONIZE is the least privilege that can
#: answer "has it exited?"; it permits waiting, not reading or writing the target.
_SYNCHRONIZE = 0x00100000
#: WaitForSingleObject's "the object is signalled", which for a process means it has exited.
_WAIT_OBJECT_0 = 0x00000000
#: OpenProcess's error for a PID that names no process. The only failure read as death.
_ERROR_INVALID_PARAMETER = 87

if _WINDOWS:
    import ctypes
    from ctypes import wintypes

    _KERNEL32 = ctypes.WinDLL("kernel32", use_last_error=True)
    _OPEN_PROCESS = _KERNEL32.OpenProcess
    _OPEN_PROCESS.restype = wintypes.HANDLE
    _OPEN_PROCESS.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    _WAIT_SINGLE = _KERNEL32.WaitForSingleObject
    _WAIT_SINGLE.restype = wintypes.DWORD
    _WAIT_SINGLE.argtypes = (wintypes.HANDLE, wintypes.DWORD)
    _CLOSE_HANDLE = _KERNEL32.CloseHandle
    _CLOSE_HANDLE.restype = wintypes.BOOL
    _CLOSE_HANDLE.argtypes = (wintypes.HANDLE,)


def spawn(args: list[str], **kwargs: Any) -> subprocess.Popen:
    """Start a child and record it, so :func:`terminate_all` can find it after a quit.

    ``kwargs`` go to :class:`subprocess.Popen` unchanged, including the pipes the caller
    asked for: this function exists to make a child *findable*, not to impose an I/O policy
    on whoever started it. The console-window and process-group flags are added here rather
    than at each call site so a new call site cannot forget them and silently reintroduce
    the orphaned-encoder bug.

    The registry is written only after ``Popen`` returns. A constructor that raises has no
    process to kill, and an entry added before the process exists would make the quit path
    spend its timeout terminating nothing.
    """
    creationflags = int(kwargs.pop("creationflags", 0))
    creationflags |= NO_WINDOW
    if _WINDOWS:
        creationflags |= NEW_PROCESS_GROUP
    process = subprocess.Popen(args, creationflags=creationflags, **kwargs)
    with _LOCK:
        _REGISTRY.add(process)
    return process


def release(process: subprocess.Popen) -> None:
    """Forget a child. Safe to call twice, and safe after the child has exited.

    Deliberately does not wait on or signal the process. Callers release from a ``finally``
    after they have already reaped it, and a ``wait()`` here would block on a child whose
    pipes nobody is draining -- turning a cleanup into a hang. Forgetting is all "release"
    means; anything that needs the exit status must read it before releasing.
    """
    with _LOCK:
        _REGISTRY.discard(process)


def terminate(process: subprocess.Popen, *, timeout: float = 5.0) -> None:
    """Ask a child to stop, give it ``timeout`` seconds, then kill it.

    Never raises for a child that is already gone. Between the ``poll`` below and the
    ``terminate`` call the child can exit on its own, and a quit path that raises ``OSError``
    halfway through would leave every child after it running -- the exact failure this module
    exists to prevent. ``timeout`` bounds each wait, not the kill, so a wedged child can hold
    the caller for at most twice the timeout.

    The child is *not* forgotten here. The caller still holds the object and may want its
    ``returncode``; forgetting it is :func:`release` or :func:`terminate_all`'s job.
    ``timeout`` is keyword-only so a call site reads as ``timeout=5``: a bare ``5`` could be
    seconds or milliseconds to the next reader, and this number decides how long a quit waits.

    On Windows ``Popen.terminate`` is already a forced end -- there is no polite signal --
    so the wait before ``kill`` matters mainly on POSIX and for the child that needs a moment
    to release its file handles before the next one is opened.
    """
    if process.poll() is not None:
        return
    try:
        process.terminate()
    except (OSError, ValueError):
        # TerminateProcess on an already-exited process, or a handle closed under us.
        return
    try:
        process.wait(timeout=timeout)
        return
    except subprocess.TimeoutExpired:
        pass
    try:
        process.kill()
    except (OSError, ValueError):
        return
    try:
        process.wait(timeout=timeout)
    except (subprocess.TimeoutExpired, OSError, ValueError):
        # The kill has been issued; the OS may still be tearing the process down. There is
        # nothing further this function can do, and the caller is usually on its way out.
        pass


def terminate_all() -> None:
    """Kill every registered child, descendants included, and forget them.

    On Windows the kill goes through ``taskkill /F /T``: Windows has no "kill this process
    group" call, and ``/T`` is the part that matters -- ffmpeg may be a grandchild of
    whatever the backend started, and killing only the PID held here would leave the encoder
    writing. On POSIX ``terminate`` signals the direct child; descendants are reparented when
    it dies and cannot be found by PID afterwards, and this module does not put them in a
    process group because the application itself ships only for Windows -- a POSIX tree kill
    here would be untested on the platform that has the bug.

    Failures are tolerated per child: one process that cannot be killed (already gone,
    access denied, a stale entry) must not stop the rest, because the caller is a quit. The
    snapshot is taken under the lock and the killing happens outside it, so ``taskkill``
    waiting on one child cannot block a worker thread that is trying to release another. A
    child spawned after the snapshot is not killed; nothing should be starting work once a
    quit is in progress.
    """
    with _LOCK:
        children = list(_REGISTRY)
    for process in children:
        try:
            if _WINDOWS:
                _kill_tree(process)
            terminate(process)
        except Exception:  # noqa: BLE001 - a quit kills the rest of the children regardless
            continue
    with _LOCK:
        _REGISTRY.difference_update(children)


def _kill_tree(process: subprocess.Popen) -> None:
    """``taskkill /F /T`` one child on Windows, then reap it locally.

    ``/T`` is why this exists: it kills the named process *and its descendants*, which is
    what makes closing the window stop the encoder even when ffmpeg is a grandchild of a
    wrapper. ``/F`` skips the polite request, because the only reason to get here is that
    politeness already ran out. The exit status of ``taskkill`` is deliberately not inspected:
    a child that exited a moment ago is a success for the caller, and ``taskkill``'s "not
    found" would otherwise have to be translated into one.

    ``taskkill`` runs with no console window for the same reason the children do, and its
    output is discarded so a quit cannot deadlock on a full pipe nobody reads. The local
    ``wait`` afterwards reaps the child, so ``returncode`` is set for callers that inspect
    it; a timeout there is harmless because the process was already force-killed.
    """
    try:
        subprocess.run(
            ["taskkill", "/F", "/T", "/PID", str(process.pid)],
            creationflags=NO_WINDOW,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5.0,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        # taskkill missing (a stripped image) or wedged. Fall through: terminate_all calls
        # terminate() next, so the direct child is still killed even without the tree walk.
        return
    try:
        process.wait(timeout=2.0)
    except (subprocess.TimeoutExpired, OSError, ValueError):
        pass


def parent_alive(pid: int) -> bool:
    """Whether ``pid`` still names a running process. Unknown counts as alive.

    A watchdog that kills work whenever the answer is unclear is worse than one that misses
    a dead parent: it would end a live job over a permissions quirk. So only a *definite*
    "no such process" returns ``False``; access denied, a failed wait and any unrecognised
    error return ``True``.

    On POSIX the probe is ``os.kill(pid, 0)``: signal 0 performs the permission and
    existence checks without delivering anything. On Windows that same call is
    ``TerminateProcess`` -- it would *kill* the process it was asked to inspect -- so
    Windows asks the kernel a question instead (see :func:`_windows_alive`).

    PIDs of zero and below are refused rather than probed: to ``kill`` they mean "a process
    group", so probing them would be a broadcast, not a question about one process.
    """
    if pid <= 0:
        return False
    if _WINDOWS:
        return _windows_alive(pid)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def _windows_alive(pid: int) -> bool:
    """``parent_alive`` on Windows, via a synchronisable handle and a zero-time wait.

    ``OpenProcess(SYNCHRONIZE)`` is the least privilege that can answer the question: it
    allows waiting on the process without reading or writing it. A signalled handle means
    the process has exited; ``WAIT_TIMEOUT`` means it is still running. ``OpenProcess``
    fails with ``ERROR_INVALID_PARAMETER`` when the PID does not exist, which is the only
    failure read as death -- ``ERROR_ACCESS_DENIED`` for another user's process says nothing
    about whether it is running, and claiming it is dead would kill a live job.

    The handle is closed in a ``finally``. This probe runs from a watchdog for the life of the
    backend, so a leaked handle would accumulate kernel objects on every check.
    """
    handle = _OPEN_PROCESS(_SYNCHRONIZE, False, pid)
    if not handle:
        return ctypes.get_last_error() != _ERROR_INVALID_PARAMETER
    try:
        # Zero timeout on purpose: a watchdog polls, it does not wait on the parent.
        return _WAIT_SINGLE(handle, 0) != _WAIT_OBJECT_0
    finally:
        _CLOSE_HANDLE(handle)


def live() -> int:
    """How many children are registered right now.

    Registered, not necessarily running: a caller that exits without releasing leaves a
    dead entry until :func:`release` or :func:`terminate_all` removes it. That is still the
    useful number -- it is what a quit logs before killing, and what tests assert on -- and
    pruning here would make the count depend on whether ``poll`` happened to be called.
    """
    with _LOCK:
        return len(_REGISTRY)
