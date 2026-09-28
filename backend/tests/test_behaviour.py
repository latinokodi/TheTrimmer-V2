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
from types import SimpleNamespace

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


@given(parsers.parse("its keyframes are at {marks}"))
def keyframes_at(world: dict, marks: str, monkeypatch) -> None:
    """One step for any number of keyframes: ``4.0 seconds`` or
    ``4.0 seconds, 8.0 seconds and 12.0 seconds``. Both the keyframe that opens the body and
    the last keyframe before the out point are read off this list, so a scenario that wants a
    copied body needs two of them and one that wants a re-encoded tail needs a third."""
    seconds = [
        float(token.replace("seconds", "").strip())
        for token in marks.replace(" and ", ",").split(",")
        if token.replace("seconds", "").strip()
    ]
    monkeypatch.setattr(cutter.ff, "keyframes", lambda *a, **k: seconds)


@given(parsers.parse("its video timescale is {ticks:d}"))
def timescale(world: dict, ticks: int) -> None:
    world["media"].timebase = Fraction(1, ticks)


@given(parsers.parse("a keyframe that opens at {opens:f} seconds"))
def a_keyframe_opens(world: dict, opens: float) -> None:
    world["keyframe_seconds"] = opens


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


@given(parsers.parse("a head patch of {head:d} frames, a copied body of {body:d}, and a "
                     "re-encoded tail of {tail:d}"),
       target_fixture="world")
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
def copy_a_body(world: dict, frames: int, monkeypatch, tmp_path) -> None:
    plan = cutter.TrimPlan(
        world["media"], "headpatch", 0, 1, frames,
        keyframe_seconds=world["keyframe_seconds"],
        tail_frames=1,
        tail_keyframe_seconds=9.0,
    )
    world["plan"] = plan
    world["spec"] = spec_for(world, 0, frames + 1)
    world["copied"] = []
    body = tmp_path / "body.mp4"

    # The copy reads the source's packets, and there is no source here, so the reader is
    # replaced by one that records the range it was asked for and writes a stub file.
    def fake(source, destination, first, last, cancel=None):
        world["copied"].append((first, last))
        Path(destination).write_bytes(b"")

    monkeypatch.setattr(cutter, "_copy_packets", fake)
    monkeypatch.setattr(cutter.ff, "inspect", lambda path: SimpleNamespace(frames=frames))
    world["body_commands"], world["body_frames"] = cutter._copy_body(
        world["media"], world["spec"], plan, body, lambda *_: None, None)


@when(parsers.parse("I name the output for frames {first:d} to {last:d} at {rate:d} fps"))
def name_the_output(world: dict, first: int, last: int, rate: int) -> None:
    world["name"] = cutter.default_output(SOURCE, first, last, Fraction(rate))


# ------------------------------------------------------------------------------------------------
#  Naming a segment
# ------------------------------------------------------------------------------------------------

#  Frames 100 to 299 at 25 fps, so the marks are 4.0s to 12.0s and the range name is a known
#  string. Only the paths are built here; nothing is written, which is why a scenario about naming
#  needs no media.
SEGMENT_IN, SEGMENT_OUT, SEGMENT_RATE = 100, 299, Fraction(25)


@when(parsers.parse('I name the segment "{name}"'))
def name_the_segment(world: dict, name: str) -> None:
    """Records either the path or the refusal, so a scenario can be about either.

    A step that raised would end the scenario at the `When`, and the assertion that belongs in a
    `Then` would never be read. So a refusal is caught and kept, and the `Then` says which of the
    two it expected.
    """
    try:
        world["segment"] = cutter.output_for(SOURCE, SEGMENT_IN, SEGMENT_OUT, SEGMENT_RATE, name=name)
        world["refusal"] = None
    except cutter.TrimError as refused:
        world["segment"] = None
        world["refusal"] = str(refused)


@when(parsers.parse('I name the segment "{name}" and ask for a folder'))
def name_the_segment_in_a_folder(world: dict, name: str) -> None:
    world["segment"] = cutter.output_for(SOURCE, SEGMENT_IN, SEGMENT_OUT, SEGMENT_RATE,
                                         name=name, in_folder=True)
    world["refusal"] = None


@then(parsers.parse('the segment is called "{name}"'))
def segment_is_called(world: dict, name: str) -> None:
    assert world["segment"] is not None, "the name was refused and should not have been"
    assert world["segment"].name == name


@then("the segment sits beside its source")
def segment_is_beside_its_source(world: dict) -> None:
    assert world["segment"].parent == SOURCE.parent


@then(parsers.parse('the transcript is called "{name}"'))
def transcript_is_called(world: dict, name: str) -> None:
    # The transcript is written as `output.with_suffix(".srt")`, so this is the relationship and
    # not a second naming rule that could drift away from the first.
    assert world["segment"].with_suffix(".srt").name == name


@then("the transcript sits beside the segment")
def transcript_is_beside_the_segment(world: dict) -> None:
    assert world["segment"].with_suffix(".srt").parent == world["segment"].parent


@then(parsers.parse('the segment and its transcript are inside a folder named "{name}"'))
def both_are_in_a_folder(world: dict, name: str) -> None:
    segment = world["segment"]
    transcript = segment.with_suffix(".srt")
    assert segment.parent.name == name, f"the segment is in {segment.parent.name}"
    assert transcript.parent == segment.parent, "the transcript is not in the folder"
    assert transcript.name == f"{name}.srt"


@then("that folder sits beside the source")
def the_folder_is_beside_the_source(world: dict) -> None:
    assert world["segment"].parent.parent == SOURCE.parent


@then("the name is refused")
def the_name_is_refused(world: dict) -> None:
    assert world["refusal"] is not None, "the name was accepted and should not have been"
    assert world["segment"] is None


@then("the reason names the character that was objected to")
def the_reason_names_the_character(world: dict) -> None:
    # A refusal that does not say which character is a refusal the operator cannot act on.
    assert ":" in world["refusal"], world["refusal"]


# ------------------------------------------------------------------------------------------------
#  A name, resolved on its own — which is what makes the field work while somebody is typing
# ------------------------------------------------------------------------------------------------

@given("no range is marked")
def no_range_marked(world: dict) -> None:
    # Nothing to set up: the point of the scenario is that no marks are needed. It is stated as a
    # Given so the scenario reads as the situation it describes rather than as a missing step.
    world["marks"] = None


@when(parsers.parse('I ask where the segment named "{name}" will be written'))
def ask_where_the_segment_goes(world: dict, name: str) -> None:
    world["answer"] = call("post", "/api/name",
                           json={"source": str(SOURCE), "name": name, "inFolder": False})


@then(parsers.parse('the answer is a path called "{filename}"'))
def the_answer_is_a_path(world: dict, filename: str) -> None:
    assert world["answer"]["status"] == 200, world["answer"]["text"]
    got = world["answer"]["body"]["output"]
    assert got is not None, "no path came back for a name that is fine"
    assert got.replace("\\", "/").endswith(filename), got


@then("the source was not read")
def the_source_was_not_read(world: dict) -> None:
    """`SOURCE` does not exist, which is the proof: a path resolved from a name reads nothing.

    If this endpoint probed the file it would fail here, and the field would stop working on a
    machine whose master happens to be on a disconnected drive.
    """
    assert not SOURCE.exists(), "the fixture path exists, so this proves nothing"
    assert world["answer"]["status"] == 200


@then("the refusal is tagged as being about the name")
def the_refusal_is_tagged(world: dict) -> None:
    """The tag is what lets the window show it beside the field rather than in the run's errors."""
    assert world["answer"]["status"] == 400, world["answer"]["text"]
    assert world["answer"]["body"]["reason"] == "name"
    assert ":" in world["answer"]["body"]["error"]


# ------------------------------------------------------------------------------------------------
#  Reading a source's keyframes before the first mark
# ------------------------------------------------------------------------------------------------

@given("a source whose keyframes take a while to list")
def a_source_with_slow_keyframes(world: dict, tmp_path, monkeypatch) -> None:
    """A real file on disk, so the stat that keys the cache is real, and a recorded read.

    Stood in for rather than actually listed: what is being checked is *when* the engine asks, not
    how long ffprobe takes. The file exists so `probe` accepts it and so the size and modification
    time in the recorded call are the ones a plan would compute.
    """
    reel = tmp_path / "reel.mp4"
    reel.write_bytes(b"\x00" * 64)
    world["reel"] = reel
    monkeypatch.setattr(server.ff, "probe", lambda path: master(100, Fraction(25)))
    world["keyframe_reads"] = []
    monkeypatch.setattr(cutter, "_keyframes_of",
                        lambda *args: world["keyframe_reads"].append(args) or ())


@when("the window loads it")
def load_the_source(world: dict) -> None:
    async def interaction(client: TestClient):
        response = await client.get("/api/probe", params={"path": str(world["reel"])})
        # The read is scheduled and not awaited, so the answer is not delayed by it. Let the loop
        # run the task before `asyncio.run` closes the loop and cancels it.
        for _ in range(50):
            if world["keyframe_reads"]:
                break
            await asyncio.sleep(0.01)
        return {"status": response.status}

    world["answer"] = asyncio.run(with_client(interaction))


@then("the engine starts reading them before the first mark is typed")
def keyframes_are_read_on_load(world: dict) -> None:
    """Measured: the first plan of a source costs 3.5 s and every plan after it 0.12 s.

    Left until the first mark, that 3.5 s sits between typing a mark and being allowed to press
    Trim, which reads as a cut already under way.
    """
    assert world["answer"]["status"] == 200
    assert world["keyframe_reads"], "loading a source did not start reading its keyframes"


@then("it reads them under the key the plan will look them up by")
def keyframes_are_read_under_the_right_key(world: dict) -> None:
    """The three arguments *are* the cache key: a warm-up reading under a different one would do
    the whole scan and then not be found, which is worse than not warming at all."""
    stat = world["reel"].stat()
    called_path, size, modified_ns = world["keyframe_reads"][0]
    assert called_path == str(world["reel"])
    assert size == stat.st_size
    assert modified_ns == stat.st_mtime_ns


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
    """Both patches together: the run to the opening keyframe and the run from the last
    keyframe before the out point. Everything between them is the original packets."""
    plan = world["plan"]
    assert plan.head_frames + plan.tail_frames == frames


@then(parsers.parse("the plan re-encodes {head:d} frames at the head and {tail:d} at the tail"))
def plan_reencodes_ends(world: dict, head: int, tail: int) -> None:
    assert (world["plan"].head_frames, world["plan"].tail_frames) == (head, tail)


@then(parsers.parse("the plan copies {frames:d} frames untouched"))
def plan_copies(world: dict, frames: int) -> None:
    assert world["plan"].body_frames == frames


@then("the plan says there is no keyframe inside the segment")
def plan_says_no_keyframe(world: dict) -> None:
    assert any("no keyframe inside the segment" in note for note in world["plan"].notes)


@then("the copy takes exactly the frames the plan names, by index")
def copy_takes_the_named_frames(world: dict) -> None:
    """The claim the packet copy makes, and the one a time-based copy could not.

    ``ffmpeg -ss … -t … -c copy`` stops when a timestamp passes the length it was given, and
    its answer moves in steps of a whole packet group: measured on a master whose frames are
    33.3 ms apart, five milliseconds off the request moved the stop fifteen frames, and a
    request for 189 frames came back with 191. Reading the container's packets and writing the
    ones wanted is exact by construction, so the range is asserted here rather than the pixels.
    """
    plan = world["plan"]
    # The container counts rows from one and the plan's keyframe is its own row number, so the
    # body opens on the row after it.
    first = plan.keyframe + 1
    assert world["copied"] == [(first, first + plan.body_frames - 1)]
    assert world["body_frames"] == plan.body_frames


@then("the length is not asked for as a span of time")
def no_time_span(world: dict) -> None:
    """There is no ``-t``/``-ss`` pair left to be imprecise: the copy issues no command at all."""
    assert world["body_commands"] == []


@then(parsers.parse("the head pass pins the timescale to {ticks:d}"))
def head_pins_timescale(world: dict, ticks: int) -> None:
    head = world["plan"].head_frames
    world["plan"] = cutter.TrimPlan(world["media"], "headpatch", head, head,
                                    world["plan"].body_frames)
    commands = cutter._encode_head(world["media"], world["spec"], world["plan"],
                                   Path("head.mp4"), lambda *_: None, None)
    assert str(ticks) in commands
    assert commands[commands.index("-video_track_timescale") + 1] == str(ticks)


@then("no pass that copies packets counts frames")
def no_copy_pass_counts_frames(world: dict, monkeypatch, tmp_path) -> None:
    """A copy of the source's own packets must not pin its length with ``-frames:v``.

    With B-frames ``-frames:v`` counts packets in **decode** order, so it drops a frame near
    the end that has not been handed over yet and keeps one that comes after the out point.
    Measured on a 91-frame cut: the last frame kept was the source's 151st while the 150th
    that was asked for had gone.

    There is nothing left to assert about a command, though, because the copy issues none: it
    selects the packets it wants by frame index and writes them itself, which cannot drop a
    frame. So the check is that the packet reader runs and no ffmpeg pass is launched for it.
    """
    plan = world["plan"]
    calls = []

    def fake(source, destination, first, last, cancel=None):
        calls.append((first, last))
        Path(destination).write_bytes(b"")

    def no_pass(*args, **kwargs):
        raise AssertionError("the body copy launched an ffmpeg pass")

    monkeypatch.setattr(cutter, "_copy_packets", fake)
    monkeypatch.setattr(cutter.ff, "run", no_pass)
    monkeypatch.setattr(cutter.ff, "inspect", lambda path: SimpleNamespace(frames=plan.body_frames))
    commands, measured = cutter._copy_body(world["media"], world["spec"], plan,
                                           tmp_path / "b.mp4", lambda *_: None, None)
    # The container counts rows from one and the plan's keyframe is its own row number, so the
    # body opens on the row after it.
    first = plan.keyframe + 1
    assert calls == [(first, first + plan.body_frames - 1)]
    assert measured == plan.body_frames
    assert commands == []


@then("both re-encoded ends are bounded by the frame count the plan names")
def reencoded_ends_pin_their_length(world: dict) -> None:
    """The rule for a piece that is re-encoded is the opposite of the rule for a copy: nothing
    can be dropped by counting, because the frames are being produced rather than selected, and
    bounding it by time leaves the count to how many of the source's own frames fall inside the
    interval -- which came back one frame long on a master that averages 29.999431."""
    plan = world["plan"]
    steps = [
        cutter._encode_head(world["media"], world["spec"], plan, Path("h.mp4"),
                            lambda *_: None, None),
        cutter._encode_tail(world["media"], world["spec"], plan, Path("t.mp4"),
                            lambda *_: None, None),
    ]
    for step, frames in zip(steps, (plan.head_frames, plan.tail_frames)):
        assert "-frames:v" in step
        assert step[step.index("-frames:v") + 1] == str(frames)


@when("the plan carries the container's own time for the in point")
def plan_carries_container_time(world: dict) -> None:
    """No ffmpeg is run; the plan is built with, and without, the container's timestamps."""
    world["container_time"] = 286.533000


@then("both re-encoded ends are seeked to the time the container states")
def reencoded_ends_seek_exactly(world: dict) -> None:
    """A seek aimed at a time *computed* from the average rate lands past the frame it names on
    a long master, so the re-encoded end starts one frame late. The container's own timestamps
    are what the seek is aimed at when they are available, and the computed time remains the
    fallback for a file that does not report them."""
    media = world["media"]
    in_time = world["container_time"]
    spec = spec_for(world, 8596, 28100)

    with_container = cutter.TrimPlan(
        media, "headpatch", 8750, 154, 19250, tail_frames=100,
        keyframe_seconds=291.667, tail_keyframe_seconds=933.333,
        in_seconds=in_time, out_seconds=291.633000,
    )
    head = cutter._encode_head(media, spec, with_container, Path("h.mp4"),
                               lambda *_: None, None)
    assert head[head.index("-ss") + 1] == f"{in_time:.6f}"

    # Without one, the computed grid time is used -- which is what every build before this
    # did, and what a source with no timestamps still gets.
    without = cutter.TrimPlan(media, "headpatch", 8750, 154, 19250, tail_frames=100,
                              keyframe_seconds=291.667, tail_keyframe_seconds=933.333)
    fallback = cutter._encode_head(media, spec, without, Path("h.mp4"),
                                   lambda *_: None, None)
    assert fallback[fallback.index("-ss") + 1] == f"{media.seconds_of(8596):.6f}"


@given("a 30 fps master whose frames are 1/30 of a second apart")
def frames_on_a_thirty_grid(world: dict) -> None:
    world["rate"] = 30.0
    world["wanted"] = 10048


def _answer_windows(monkeypatch, world: dict, frame_of) -> None:
    """Stand in for ffprobe answering a `-read_intervals` window.

    ``frame_of`` decides which frames come back, so one helper can serve a container that
    answers with the frames around the one asked about and one that answers from an earlier
    keyframe. The windows asked for are kept, so a scenario can check the read was aimed.
    """
    asked: list[tuple[float, float]] = []

    class Done:
        returncode = 0
        stderr = ""

        def __init__(self, text: str) -> None:
            self.stdout = text

    def fake(args, **kwargs):
        window = args[args.index("-read_intervals") + 1]
        begin_text, span_text = window.split("%+")
        begin, span = float(begin_text), float(span_text)
        asked.append((begin, span))
        return Done("\n".join(f"{at:.6f}," for at in frame_of(begin, span)))

    monkeypatch.setattr(ff, "_run_quiet", fake)
    world["asked"] = asked


@when("I ask the container for the stated time of a frame deep in the file")
def ask_for_a_deep_frame(world: dict, monkeypatch) -> None:
    """A file whose frames are exactly on the grid, so the answer is checkable by arithmetic."""
    rate = world["rate"]

    def frame_of(begin, span):
        return [begin + i / rate for i in range(int(span * rate) + 1)]

    _answer_windows(monkeypatch, world, frame_of)
    world["answer"] = ff.frame_pts_near(Path("absent.mp4"), world["wanted"], rate)


@when("the container answers with frames that are not the one asked about")
def answer_from_an_earlier_keyframe(world: dict, monkeypatch) -> None:
    """What a seek to a keyframe really returns when the window is too short to reach the frame:
    frames from *before* the one asked about, one of which is nearer the computed guess than
    anything else on offer."""
    def frame_of(begin, span):
        return [frame / 30.0 for frame in range(750, 781)]

    _answer_windows(monkeypatch, world, frame_of)
    world["answer"] = ff.frame_pts_near(Path("absent.mp4"), 1000, 30.0)
    world["fallback"] = ff.FrameTimes(Path("absent.mp4"), lambda frame: frame / 30.0,
                                      30.0, 0.0)(1000)


@then("the read opens before that frame and closes after it")
def read_contains_the_frame(world: dict) -> None:
    wanted = world["wanted"] / world["rate"]
    for begin, span in world["asked"]:
        assert begin <= wanted <= begin + span, "the window does not contain the frame"


@then("the read does not begin at the start of the file")
def read_is_aimed(world: dict) -> None:
    begin, _ = world["asked"][0]
    assert begin > 0, "the read started at the beginning of the file"
    assert world["answer"] is not None


@then("no time is returned for that frame")
def nothing_is_returned(world: dict) -> None:
    """Refusing is the point: the nearest time in the window belongs to a different frame, and
    handing it back is what put a cut's re-encoded end 7.3 s from where it was asked to be."""
    assert world["answer"] is None


@then("the plan falls back to the time computed from the grid")
def falls_back_to_the_grid(world: dict) -> None:
    assert world["fallback"] == pytest.approx(1000 / 30.0)


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


@then("the answer says the interface may not be stored")
def interface_is_not_stored(world: dict) -> None:
    """The bundle's files keep their names across builds, so a cached copy is indistinguishable
    from a current one and the window can show an interface that no longer exists.

    It is not a theoretical hazard: it looks exactly like a change that never took effect, which
    is the hardest kind of report to act on.
    """
    assert "no-store" in world["answer"]["cache_control"], world["answer"]["cache_control"]


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
                "content_type": response.headers.get("Content-Type", ""),
                "cache_control": response.headers.get("Cache-Control", "")}

    return asyncio.run(with_client(interaction))
