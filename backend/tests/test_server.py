"""The HTTP contract between the window and the engine.

## What is checked, and what deliberately is not

Only the shape of the contract and the sentences it refuses with: which routes exist, that a
missing file is a 400 carrying a readable reason, that a run in flight is a 409 rather than a
second ffmpeg. The *cutting* is not checked here. A test that drove a real cut would take
seconds, would need a fixture on disk, and would still not be the thing that finds a wrong
frame — that is what the application is for, and it is a person's job to press the button.

Nothing here launches ffmpeg except `/api/health`, which asks it its version.
"""

from __future__ import annotations

import asyncio

from aiohttp.test_utils import TestClient, TestServer
import pytest

import server
from trimmer import trim as cutter


def call(coroutine):
    """Run one client interaction. `asyncio.run` rather than a pytest plugin, because the
    plugin is a dependency and this needs nothing the standard library does not have."""
    return asyncio.run(coroutine)


async def with_client(interaction):
    async with TestClient(TestServer(server.build_app())) as client:
        return await interaction(client)


# ---------------------------------------------------------------------------------------
# Getting the app up
# ---------------------------------------------------------------------------------------

def test_the_route_list_names_every_route_that_exists():
    async def check(client: TestClient):
        response = await client.get("/api")
        assert response.status == 200
        body = await response.json()
        assert body["version"] == server.VERSION
        assert set(body["routes"]) == {
            "health", "probe", "parse", "plan", "cut", "cancel", "events"
        }

    call(with_client(check))


def test_health_answers_even_when_ffmpeg_cannot_be_found():
    # The window has one question on startup — can this machine cut — and a health route that
    # raises is a window that shows nothing at all instead of showing what is missing.
    async def check(client: TestClient):
        response = await client.get("/api/health")
        assert response.status == 200
        body = await response.json()
        assert body["version"] == server.VERSION
        assert "libx264" in body
        # The full encoder list is about eleven kilobytes and the window only needs the one
        # boolean, so it must not come back.
        assert len(await response.text()) < 2000

    call(with_client(check))


# ---------------------------------------------------------------------------------------
# Refusals carry a sentence a person can act on
# ---------------------------------------------------------------------------------------

def test_probing_a_file_that_is_not_there_is_a_400_that_says_which_file():
    async def check(client: TestClient):
        response = await client.get("/api/probe", params={"path": "C:/nowhere/nothing.mp4"})
        assert response.status == 400
        body = await response.json()
        assert "nothing.mp4" in body["error"]

    call(with_client(check))


def test_probing_with_no_path_at_all_is_refused_rather_than_probing_the_cwd():
    async def check(client: TestClient):
        response = await client.get("/api/probe")
        assert response.status == 400

    call(with_client(check))


def test_planning_needs_a_source_it_can_read():
    async def check(client: TestClient):
        response = await client.post("/api/plan", json={"inFrame": 0, "endFrame": 10})
        assert response.status == 400
        assert "error" in await response.json()

    call(with_client(check))


# ------------------------------------------------------------------------------------------------
#  Naming a segment, as it reaches the engine
# ------------------------------------------------------------------------------------------------

def _body(**extra) -> dict:
    return {"source": "C:/media/reel.mp4", "inFrame": 0, "endFrame": 250, "rate": 25, **extra}


def test_a_request_with_no_name_is_resolved_the_way_it_always_was():
    """The field the window always had, and the default it has to keep."""
    spec = server._spec_from(_body())
    assert spec.output.name == "reel 00.00.00.00-00.00.09.24.mp4"


def test_a_name_in_the_request_becomes_the_segment_and_its_transcript():
    """The two sentences of the feature, checked where the window's request becomes the engine's.

    The transcript is not sent and not named here: it is `output.with_suffix(".srt")`, which is the
    whole of the guarantee that the caption file carries the segment's name. Checking it on the
    spec is checking the relationship the writer uses.
    """
    spec = server._spec_from(_body(name="Interview wide"))
    assert spec.output.name == "Interview wide.mp4"
    assert spec.output.with_suffix(".srt").name == "Interview wide.srt"


def test_a_request_can_ask_for_a_folder_of_that_name():
    spec = server._spec_from(_body(name="Interview wide", inFolder=True))
    assert spec.output.name == "Interview wide.mp4"
    assert spec.output.parent.name == "Interview wide"
    assert spec.output.with_suffix(".srt").parent == spec.output.parent


def test_an_explicit_output_path_still_wins_over_a_name():
    """`output` is what this API accepted before there was a name, and it keeps working.

    A caller that has already decided the whole path is not overruled by a name field it may not
    even know about.
    """
    spec = server._spec_from(_body(name="Interview wide", output="D:/elsewhere/mine.mp4"))
    assert spec.output.name == "mine.mp4"


def test_a_name_windows_refuses_is_refused_before_anything_is_read():
    """The refusal has to arrive before the engine is asked for the media.

    If the name were checked after probing the source, a person who mistyped a colon in the name
    would wait for a probe of an 11 GB master to be told about the colon.
    """
    with pytest.raises(cutter.TrimError) as refused:
        server._spec_from(_body(name="Take 1:2", inFolder=True))
    assert ":" in str(refused.value)


def test_the_plan_answers_with_the_path_the_name_produces():
    """The window shows this path under the name field, so it has to be the real one."""
    spec = server._spec_from(_body(name="Interview wide", inFolder=True))
    assert str(spec.output).endswith("Interview wide\\Interview wide.mp4") or \
        str(spec.output).endswith("Interview wide/Interview wide.mp4")


def test_a_range_against_a_file_that_is_not_there_is_a_400_not_a_traceback():
    async def check(client: TestClient):
        response = await client.post(
            "/api/plan",
            json={
                "source": "C:/nowhere/nothing.mp4",
                "inFrame": 0,
                "endFrame": 10,
                "rate": 25,
            },
        )
        assert response.status == 400
        assert "error" in await response.json()

    call(with_client(check))


def test_cancelling_nothing_is_an_answer_and_not_an_error():
    # The button is offered optimistically. A cancel that arrived a moment after the run
    # finished is a race, not a fault, and it must not read as one.
    async def check(client: TestClient):
        response = await client.post("/api/cancel", json={})
        assert response.status == 200
        body = await response.json()
        assert body["cancelled"] is False
        assert "nothing is running" in body["reason"]

    call(with_client(check))


# ---------------------------------------------------------------------------------------
# The stream
# ---------------------------------------------------------------------------------------

def test_the_event_stream_is_an_event_stream():
    async def check(client: TestClient):
        # Readiness rather than a read: `prepare()` alone is not enough for aiohttp to send
        # the headers, so a request is made and the content type is checked from it.
        response = await client.get("/api/events")
        assert response.status == 200
        assert response.headers["Content-Type"].startswith("text/event-stream")
        response.close()

    call(with_client(check))


def test_one_run_at_a_time_is_enforced_with_a_409():
    """The guard, checked without cutting: a job is put in the slot by hand and the second
    request must be refused. Two ffmpeg processes on one disk are slower than one."""
    from pathlib import Path

    from trimmer import ffmpeg as ff
    from trimmer import trim as cutter

    held = server.Job(
        cutter.TrimSpec(
            source=Path("C:/media/reel.mp4"),
            output=Path("C:/media/out.mp4"),
            in_frame=100,
            out_frame=200,
        ),
        "standard",
    )
    monkey = ff.CancelToken()
    held.cancel = monkey

    async def check(client: TestClient):
        server.CURRENT = held
        try:
            response = await client.post(
                "/api/cut",
                json={
                    "source": "C:/media/reel.mp4",
                    "inFrame": 100,
                    "endFrame": 200,
                    "rate": 25,
                },
            )
            assert response.status == 409
            assert "already running" in (await response.json())["error"]
        finally:
            server.CURRENT = None

    try:
        call(with_client(check))
    finally:
        assert server.CURRENT is None
