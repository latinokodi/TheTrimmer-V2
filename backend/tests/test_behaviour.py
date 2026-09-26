"""The steps behind every scenario in ``specs/features/``.

## What belongs here, and what does not

A step is a sentence from `docs/SPEC.md` made runnable, and it must not become a second place the
behaviour is described. So each one is thin on purpose: it builds the smallest state the sentence
needs, calls the engine, and records what came back. Every assertion is in a `Then`, and every
`Then` reads as the requirement it verifies.

**No step runs a cut.** A scenario about planning, rates or verdicts is arithmetic and a stubbed
keyframe list; a scenario about the HTTP contract is a test client. The engine's behaviour on real
footage is checked by using the application, which is what the proof panel is for — and, for the
two reported defects that started this, by the measurements recorded in `docs/DESIGN.md`.
"""

from __future__ import annotations

import asyncio
from fractions import Fraction
from pathlib import Path

import pytest
from aiohttp.test_utils import TestClient, TestServer
from pytest_bdd import given, parsers, scenarios, then, when

from trimmer import ffmpeg as ff
from trimmer import subtitles as subs
from trimmer import trim as cutter
from trimmer import verify as verifier
from trimmer.subtitles import Cue
from trimmer.timecode import TimecodeError, format_timecode, parse_timecode

import server

scenarios("../../specs/features/cutting.feature")
scenarios("../../specs/features/rates.feature")
scenarios("../../specs/features/verification.feature")
scenarios("../../specs/features/engine_api.feature")

SOURCE = Path("C:/media/reel.mp4")


@pytest.fixture
def world() -> dict:
    return {}


@pytest.fixture(autouse=True)
def no_ffmpeg(monkeypatch):
    """No scenario launches a process.

    ffmpeg is replaced, not stubbed at a higher level, so a step that builds a command line
    still builds the real one and a step that would have run it does not. Every scenario here is
    therefore milliseconds, which is what makes it worth running on every change.

    Keyframes come from the scenario unless it says otherwise, and an empty list is itself a
    case the planner has to answer.
    """
    monkeypatch.setattr(ff, "run", lambda *a, **k: None)
    monkeypatch.setattr(ff, "keyframes", lambda *a, **k: [])


def master(frames: int, rate: Fraction, *, start_time: float = 0.0) -> ff.MediaInfo:
    return ff.MediaInfo(
        path=SOURCE,
        codec="h264",
        width=1920,
        height=1080,
        pix_fmt="yuv420p",
        rate=rate,
        average_rate=rate,
        timebase=Fraction(1, 90000),
        frames=frames,
        duration=frames / float(rate),
        audio=ff.AudioStream(codec="aac", sample_rate=48000, channels=2),
        size_bytes=1_000_000,
        start_time=start_time,
    )


def spec_for(world: dict, in_frame: int, out_frame: int) -> cutter.TrimSpec:
    return cutter.TrimSpec(
        source=SOURCE,
        output=world.get("output", Path("C:/media/out.mp4")),
        in_frame=in_frame,
        out_frame=out_frame,
        crf=18,
        preset="veryfast",
    )


# ---------------------------------------------------------------------------------------
# Given
# ---------------------------------------------------------------------------------------

@given(parsers.parse("a {rate:d} fps master of {frames:d} frames"), target_fixture="world")
def a_master(rate: int, frames: int) -> dict:
    return {"media": master(frames, Fraction(rate)), "keyframes": []}


@given(parsers.parse("its keyframes are at {first:f} seconds and {second:f} seconds"))
def two_keyframes(world: dict, first: float, second: float, monkeypatch) -> None:
    monkeypatch.setattr(cutter.ff, "keyframes", lambda *a, **k: [first, second])


@given(parsers.parse("its keyframes are at {only:f} seconds"))
def one_keyframe(world: dict, only: float, monkeypatch) -> None:
    monkeypatch.setattr(cutter.ff, "keyframes", lambda *a, **k: [only])


@given(parsers.parse("its video timescale is {ticks:d}"))
def timescale(world: dict, ticks: int) -> None:
    world["media"].timebase = Fraction(1, ticks)


@given(parsers.parse("a keyframe that opens at {opens:f} seconds"))
def a_keyframe_opens(world: dict, opens: float) -> None:
    world["keyframe_seconds"] = opens


@given(parsers.parse("the next keyframe at {following:f} seconds"))
def the_next_keyframe(world: dict, following: float) -> None:
    world["next_keyframe_seconds"] = following


@given(parsers.parse("a master that claims {claimed:d} fps and averages {num:d}/{den:d}"))
def a_drifting_master(world: dict, claimed: int, num: int, den: int) -> None:
    media = master(52210, Fraction(claimed))
    media.average_rate = Fraction(num, den)
    media.duration = 1740.366333
    world["media"] = media


@given(parsers.parse("its picture begins at {start:f} seconds"))
def picture_begins(world: dict, start: float) -> None:
    world["media"].start_time = start


@given(parsers.parse("a master of {frames:d} frames at exactly {rate:d} fps"), target_fixture="world")
def an_exact_master(frames: int, rate: int) -> dict:
    return {"media": master(frames, Fraction(rate)), "keyframes": []}


@given(parsers.parse("a {rate:g} fps master"), target_fixture="world")
def a_rate_master(rate: str) -> dict:
    return {"media": master(300, Fraction(rate).limit_denominator(1001)), "keyframes": []}


@given(parsers.parse("a {rate:d} fps master whose picture begins at {start:f} seconds"),
       target_fixture="world")
def a_master_starting_late(rate: int, start: float) -> dict:
    return {"media": master(300, Fraction(rate), start_time=start), "keyframes": []}


@given(parsers.parse("the rates {rates}"), target_fixture="world")
def the_rates(rates: str) -> dict:
    return {"rates": [Fraction(token.strip()) for token in rates.split(",")]}


@given(parsers.parse("a check that {state}"))
def a_check(world: dict, state: str) -> None:
    world["check"] = state


@given(parsers.parse("a head patch of {head:d} frames in a segment of {frames:d} frames"),
       target_fixture="world")
def a_head_patch(head: int, frames: int) -> dict:
    media = master(frames + head, Fraction(25))
    plan = cutter.TrimPlan(media, "headpatch", head, head, frames)
    spec = spec_for({}, 0, head + frames)
    return {"media": media, "plan": plan, "spec": spec}


@given("samples that report -1, 0 and 2 frames")
def disagreeing_samples(world: dict) -> None:
    world["offsets"] = [verifier.OffsetCheck(0, -1), verifier.OffsetCheck(1, 0),
                        verifier.OffsetCheck(2, 2)]


@given(parsers.parse("samples that all report {offset:d} frames"))
def agreeing_samples(world: dict, offset: int) -> None:
    world["offsets"] = [verifier.OffsetCheck(0, offset), verifier.OffsetCheck(1, offset)]


@given("samples that could not be measured")
def unmeasurable_samples(world: dict) -> None:
    world["offsets"] = [verifier.OffsetCheck(0, None), verifier.OffsetCheck(1, None)]


@given("a transcript with a cue across the in point and one across the out point")
def cues_across_the_marks(world: dict) -> None:
    world["cues"] = [
        Cue(9.0, 11.0, "starts before the in point"),      # the segment begins at 10
        Cue(30.0, 41.0, "runs past the out point"),        # the segment ends at 40
        Cue(20.0, 21.0, "wholly inside"),
    ]


@given("a cut already running")
def a_cut_running(world: dict) -> None:
    world["running"] = server.Job(spec_for({}, 0, 10), "standard")


# ---------------------------------------------------------------------------------------
# When
# ---------------------------------------------------------------------------------------

@when(parsers.parse("I mark frames {in_frame:d} to {out_frame:d}"))
def mark_frames(world: dict, in_frame: int, out_frame: int) -> None:
    world["spec"] = spec_for(world, in_frame, out_frame)
    world["plan"] = cutter.plan_trim(world["spec"], world["media"])


@when(parsers.parse("a body of {frames:d} frames is copied from that keyframe"))
def copy_a_body(world: dict, frames: int) -> None:
    plan = cutter.TrimPlan(
        world["media"], "headpatch", 0, 1, frames,
        keyframe_seconds=world["keyframe_seconds"],
        next_keyframe_seconds=world["next_keyframe_seconds"],
    )
    world["plan"] = plan
    world["aim"], world["length"] = cutter.body_seek(plan, world["media"].grid_rate, frames)


@when(parsers.parse("I name the output for frames {first:d} to {last:d} at {rate:d} fps"))
def name_the_output(world: dict, first: int, last: int, rate: int) -> None:
    world["name"] = cutter.default_output(SOURCE, first, last, Fraction(rate))


@when("I convert its last frame to a time")
def convert_last_frame(world: dict) -> None:
    media = world["media"]
    world["on_grid"] = media.seconds_of(media.frames - 1)
    world["on_claimed"] = media.start_time + (media.frames - 1) / float(media.rate)
    world["back"] = media.frame_of(world["on_grid"])


@when("I plan a range on it")
def plan_a_range(world: dict, monkeypatch) -> None:
    monkeypatch.setattr(cutter.ff, "keyframes", lambda *a, **k: [4.0])
    world["plan"] = cutter.plan_trim(spec_for(world, 100, 200), world["media"])


@when(parsers.parse("I ask when frame {frame:d} is shown"))
def ask_when_shown(world: dict, frame: int) -> None:
    world["shown"] = world["media"].seconds_of(frame)


@when(parsers.parse('I read "{text}"'))
def read_a_timecode(world: dict, text: str) -> None:
    try:
        world["frame"] = parse_timecode(text, world["media"].rate)
        world["refusal"] = None
    except TimecodeError as refused:
        world["frame"] = None
        world["refusal"] = str(refused)


@when("I write each of the frames 0, 1, 29, 1800 and 107892 and read it back")
def round_trip(world: dict) -> None:
    world["round_tripped"] = [
        (frame, parse_timecode(format_timecode(frame, rate), rate))
        for rate in world["rates"]
        for frame in (0, 1, 29, 1800, 107892)
    ]


@when("the window is told about it")
def window_is_told(world: dict) -> None:
    # The same mapping the backend sends: a check line is a name, a detail, and a status.
    passed = world["check"] == "passed"
    failed = world["check"] == "failed"
    world["rows"] = server._check_json(
        verifier.VerifyResult(
            checks=["frames 100, exactly as asked"] if passed else [],
            failures=["frames 99, one short"] if failed else [],
        )
    )
    world["verdict"] = world["rows"][0]["status"]["kind"] if world["rows"] else "not checked"


@when("I choose the frames to sample")
def choose_samples(world: dict) -> None:
    world["samples"] = verifier.sample_frames(world["spec"], world["plan"], world["media"], 3)


@when("I ask what offset they agree on")
def ask_consensus(world: dict) -> None:
    world["agreed"] = verifier.consensus(world["offsets"])


@when("the segment is cut")
def cut_the_segment(world: dict, tmp_path: Path) -> None:
    world["retimed"] = subs.retime(world["cues"], 10.0, 40.0)
    source_srt = tmp_path / "in.srt"
    source_srt.write_text(
        "1\n00:00:09,000 --> 00:00:11,000\nstarts before\n\n"
        "2\n00:00:20,000 --> 00:00:21,000\nwholly inside\n",
        encoding="utf-8",
    )
    result, written = subs.retime_file(source_srt, tmp_path / "out.srt", 10.0, 40.0)
    world["written"] = written
    world["reported"] = result.written


@when("the window asks what this machine can do")
def ask_health(world: dict) -> None:
    world["answer"] = call("get", "/api/health")


@when("I ask about a file that is not there")
def ask_about_a_missing_file(world: dict) -> None:
    world["answer"] = call("get", "/api/probe", params={"path": "C:/nowhere/nothing.mp4"})


@when("I plan a range against a file that is not there")
def plan_a_missing_file(world: dict) -> None:
    world["answer"] = call("post", "/api/plan",
                           json={"source": "C:/nowhere/nothing.mp4",
                                 "inFrame": 0, "endFrame": 10, "rate": 25})


@when("I start another")
def start_another(world: dict) -> None:
    async def interaction(client: TestClient):
        server.CURRENT = world["running"]
        try:
            response = await client.post("/api/cut", json={
                "source": str(SOURCE), "inFrame": 0, "endFrame": 10, "rate": 25})
            return {"status": response.status, "body": await response.json(),
                    "text": await response.text(),
                    "content_type": response.headers.get("Content-Type", "")}
        finally:
            server.CURRENT = None

    world["answer"] = asyncio.run(with_client(interaction))


@when("I cancel with nothing running")
def cancel_nothing(world: dict) -> None:
    world["answer"] = call("post", "/api/cancel", json={})


@when("I ask the engine what it serves")
def ask_routes(world: dict) -> None:
    world["answer"] = call("get", "/api")


@when("I ask for a page that does not exist")
def ask_for_a_missing_page(world: dict) -> None:
    world["answer"] = call("get", "/no/such/page")


# ---------------------------------------------------------------------------------------
# Then
# ---------------------------------------------------------------------------------------

@then(parsers.parse("the plan is {mode}"))
def plan_is(world: dict, mode: str) -> None:
    wanted = {"a lossless copy": "copy", "a head patch": "headpatch",
              "a full re-encode": "reencode"}[mode]
    assert world["plan"].mode == wanted


@then(parsers.parse("the plan re-encodes {frames:d} frames"))
def plan_reencodes(world: dict, frames: int) -> None:
    assert world["plan"].head_frames == frames


@then(parsers.parse("the plan copies {frames:d} frames untouched"))
def plan_copies(world: dict, frames: int) -> None:
    assert world["plan"].body_frames == frames


@then("the plan says there is no keyframe inside the segment")
def plan_says_no_keyframe(world: dict) -> None:
    assert any("no keyframe inside the segment" in note for note in world["plan"].notes)


@then("the copy is aimed inside the GOP and not on its boundary")
def aimed_inside(world: dict) -> None:
    aim = float(world["aim"])
    opens = world["keyframe_seconds"]
    following = world["next_keyframe_seconds"]
    assert opens < aim < following, f"{aim} is not inside ({opens}, {following})"


@then("the length asked for is short by exactly the preroll the aim will include")
def length_gives_preroll_back(world: dict) -> None:
    frames = world["plan"].body_frames
    wanted = frames / float(world["media"].grid_rate)
    preroll = float(world["aim"]) - world["keyframe_seconds"]
    assert world["length"] == pytest.approx(wanted - preroll, abs=1e-6)


@then(parsers.parse("the head pass pins the timescale to {ticks:d}"))
def head_pins_timescale(world: dict, ticks: int) -> None:
    head = world["plan"].head_frames
    world["plan"] = cutter.TrimPlan(world["media"], "headpatch", head, head,
                                    world["plan"].body_frames)
    commands = cutter._encode_head(world["media"], world["spec"], world["plan"],
                                   Path("head.mp4"), lambda *_: None, None)
    assert str(ticks) in commands
    assert commands[commands.index("-video_track_timescale") + 1] == str(ticks)


@then("no pass counts frames")
def no_pass_counts_frames(world: dict) -> None:
    head = world["plan"].head_frames
    plan = cutter.TrimPlan(world["media"], "headpatch", head, head, world["plan"].body_frames)
    for step in [cutter._encode_head(world["media"], world["spec"], plan, Path("h.mp4"),
                                     lambda *_: None, None)]:
        assert "-frames:v" not in step
    assert "-frames:v" not in cutter._copy(world["media"], world["spec"], plan,
                                           Path("o.mp4"), lambda *_: None, None)


@then(parsers.parse('the name is "{name}"'))
def name_is(world: dict, name: str) -> None:
    assert world["name"].name == name


@then("the name holds no character Windows refuses")
def name_is_legal(world: dict) -> None:
    assert not set(':*?"<>|') & set(world["name"].name)


@then("the time is one frame later than counting at 30 would give")
def one_frame_later(world: dict) -> None:
    assert world["on_grid"] - world["on_claimed"] == pytest.approx(1 / 30, abs=0.002)


@then("converting that time back gives the same frame")
def converts_back(world: dict) -> None:
    assert world["back"] == world["media"].frames - 1


@then("the plan warns that its timestamps are away from that grid")
def plan_warns(world: dict) -> None:
    assert any("away from that grid" in note for note in world["plan"].notes)


@then("the warning carries the measured drift")
def warning_has_the_number(world: dict) -> None:
    note = next(n for n in world["plan"].notes if "away from that grid" in n)
    assert f"{world['media'].grid_drift:.2f} frame(s)" in note


@then("the plan says nothing about its frame grid")
def plan_is_quiet(world: dict) -> None:
    assert not any("grid" in note for note in world["plan"].notes)


@then(parsers.parse("the answer is {seconds:f} seconds"))
def answer_is(world: dict, seconds: float) -> None:
    assert world["shown"] == pytest.approx(seconds, abs=1e-6)


@then(parsers.parse("it names frame {frame:d}"))
def names_frame(world: dict, frame: int) -> None:
    assert world["refusal"] is None, world["refusal"]
    assert world["frame"] == frame


@then("the timecode is refused because the rate counts to 24")
def refused_for_rate(world: dict) -> None:
    assert world["refusal"] is not None and "counts to 24" in world["refusal"]


@then("the timecode is refused because that rate has no drop-frame form")
def refused_no_drop_frame(world: dict) -> None:
    assert world["refusal"] is not None and "no drop-frame form" in world["refusal"]


@then("every frame comes back unchanged")
def round_trip_holds(world: dict) -> None:
    assert all(written == frame for frame, written in world["round_tripped"])


@then(parsers.parse('it reads "{verdict}"'))
def it_reads(world: dict, verdict: str) -> None:
    assert world["verdict"] == verdict


@then("every sample is after the re-encoded head")
def samples_after_head(world: dict) -> None:
    assert all(frame >= world["plan"].head_frames for frame in world["samples"])


@then("every sample sits inside the range that was asked for")
def samples_inside(world: dict) -> None:
    assert all(frame + verifier.WINDOW <= world["spec"].frames for frame in world["samples"])


@then("at least one sample is chosen")
def one_sample(world: dict) -> None:
    assert len(world["samples"]) >= 1


@then("there is no agreed offset")
def no_agreement(world: dict) -> None:
    assert world["agreed"] is None


@then(parsers.parse("the agreed offset is {offset:d}"))
def agreement(world: dict, offset: int) -> None:
    assert world["agreed"] == offset


@then("each cue is clamped to the mark it crossed")
def cues_clamped(world: dict) -> None:
    # Cues come back on the segment's clock, where the in point is zero. So the cue that
    # crossed the in point now starts at zero, and the one that ran past the out point now
    # ends at the out point less the in point.
    kept = {cue.text: cue for cue in world["retimed"].cues}
    assert kept["starts before the in point"].start == pytest.approx(0.0)
    assert kept["runs past the out point"].end == pytest.approx(30.0)
    assert len(world["retimed"].clamped) == 2


@then("the file written is reported")
def file_reported(world: dict) -> None:
    assert world["written"] is not None
    assert world["reported"] == world["written"]


@then("the answer names the engine version")
def names_version(world: dict) -> None:
    assert world["answer"]["body"]["version"] == server.VERSION


@then("the answer says whether H.264 encoding is available")
def says_h264(world: dict) -> None:
    assert "libx264" in world["answer"]["body"]


@then("the answer does not carry the whole encoder list")
def not_the_whole_list(world: dict) -> None:
    assert len(world["answer"]["text"]) < 2000


@then("the answer is a refusal")
def is_a_refusal(world: dict) -> None:
    assert world["answer"]["status"] == 400
    assert "error" in world["answer"]["body"]


@then("the refusal names the file")
def refusal_names_the_file(world: dict) -> None:
    assert "nothing.mp4" in world["answer"]["body"]["error"]


@then("the answer is a refusal and not a traceback")
def refusal_not_traceback(world: dict) -> None:
    assert world["answer"]["status"] == 400
    assert "Traceback" not in world["answer"]["body"]["error"]


@then("the second is refused because one is already running")
def second_refused(world: dict) -> None:
    assert world["answer"]["status"] == 409
    assert "already running" in world["answer"]["body"]["error"]


@then("the answer is that nothing was running")
def nothing_was_running(world: dict) -> None:
    assert world["answer"]["body"]["cancelled"] is False
    assert "nothing is running" in world["answer"]["body"]["reason"]


@then("the route list is exactly the routes it has")
def routes_are_exact(world: dict) -> None:
    assert set(world["answer"]["body"]["routes"]) == {
        "health", "probe", "parse", "plan", "cut", "cancel", "events"
    }


@then("the interface's own document is served")
def interface_served(world: dict) -> None:
    assert world["answer"]["status"] == 200
    assert world["answer"]["content_type"].startswith("text/html")


# ---------------------------------------------------------------------------------------
# Talking to the engine without cutting anything
# ---------------------------------------------------------------------------------------

async def with_client(interaction):
    async with TestClient(TestServer(server.build_app())) as client:
        return await interaction(client)


def call(method: str, path: str, **kwargs) -> dict:
    """One request against the engine, and what it answered."""

    async def interaction(client: TestClient):
        response = await getattr(client, method)(path, **kwargs)
        text = await response.text()
        try:
            body = await response.json()
        except Exception:  # noqa: BLE001 - a document is not JSON, and that is the point
            body = None
        return {"status": response.status, "body": body, "text": text,
                "content_type": response.headers.get("Content-Type", "")}

    return asyncio.run(with_client(interaction))
