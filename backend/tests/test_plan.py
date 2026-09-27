"""What a range will do, decided before ffmpeg is started.

## Why this is the test that matters most

The plan is the product's central claim made checkable: *the body is the original packets*.
It is decided here, from the keyframe list, and every branch of it is reachable without
cutting anything. `ff.keyframes` is the single process launch involved, and it is replaced —
so this file is a few milliseconds and there is no reason not to run it on every change.
"""

from __future__ import annotations

from fractions import Fraction
from pathlib import Path
from types import SimpleNamespace

import pytest

from trimmer import trim as cutter
from trimmer.ffmpeg import AudioStream, MediaInfo

SOURCE = Path("C:/media/reel.mp4")
NTSC_30 = Fraction(30000, 1001)
CINEMA = Fraction(24000, 1001)


def media(frames: int = 500, rate: Fraction = Fraction(25), codec: str = "h264") -> MediaInfo:
    """A source with the fields the planner reads. No file behind it, deliberately: the
    planner's decisions are arithmetic and arithmetic does not need a disk."""
    return MediaInfo(
        path=SOURCE,
        codec=codec,
        width=1920,
        height=1080,
        pix_fmt="yuv420p",
        rate=rate,
        average_rate=rate,
        timebase=Fraction(1, 12800),
        frames=frames,
        duration=frames / float(rate),
        audio=AudioStream(codec="aac", sample_rate=48000, channels=2),
        size_bytes=1_000_000,
    )


def spec(in_frame: int, out_frame: int, **kwargs) -> cutter.TrimSpec:
    return cutter.TrimSpec(
        source=SOURCE,
        output=Path("C:/media/out.mp4"),
        in_frame=in_frame,
        out_frame=out_frame,
        **kwargs,
    )


@pytest.fixture
def keyframes(monkeypatch):
    """Stand in for ffprobe's keyframe list, so no process is launched."""

    def install(*seconds: float) -> None:
        monkeypatch.setattr(cutter.ff, "keyframes", lambda *a, **k: [float(s) for s in seconds])

    return install


# ---------------------------------------------------------------------------------------
# The three pieces: the run to the opening keyframe, the copied packets, the run to the mark
# ---------------------------------------------------------------------------------------

def test_an_in_point_on_a_keyframe_re_encodes_only_the_tail(keyframes):
    # Keyframes at 4.0s and 8.0s (frames 100 and 200 at 25 fps). The in point is the first
    # keyframe, so nothing needs re-encoding at the front; the body is the original packets
    # from 100 to 199; the tail is re-encoded from 200 to the out point.
    keyframes(4.0, 8.0)
    plan = cutter.plan_trim(spec(100, 300), media())
    assert plan.mode == "copy"
    assert (plan.keyframe, plan.head_frames, plan.body_frames, plan.tail_frames) == \
        (100, 0, 100, 100)
    assert plan.requested == 200


def test_both_ends_are_re_encoded_and_the_middle_is_copied(keyframes):
    # Keyframes at frames 100, 200 and 300 at 25 fps; in point at frame 150, out point at 350.
    # The in point sits inside the first GOP, so frames 150..199 are re-encoded; 200..299 are
    # the original packets; and 300..349 are re-encoded again so the segment ends on the frame
    # that was asked for. Getting the first split wrong is the slow-motion bug: the copied
    # body is rescanned against a timescale it did not have.
    keyframes(4.0, 8.0, 12.0)
    plan = cutter.plan_trim(spec(150, 350), media())
    assert plan.mode == "headpatch"
    assert plan.keyframe == 200
    assert plan.head_frames == 50
    assert plan.body_frames == 100
    assert plan.tail_frames == 50
    assert plan.requested == 200


def test_the_tail_re_encodes_from_the_last_keyframe_before_the_out_point(keyframes):
    """Its length is whatever is left after that keyframe, not a whole GOP."""
    keyframes(4.0, 8.0, 12.0)
    plan = cutter.plan_trim(spec(150, 450), media())
    # Out point is frame 450; the last keyframe before it is frame 300, so 150 frames are
    # re-encoded at the tail -- not until some later keyframe, and not a whole GOP.
    assert plan.tail_frames == 150
    assert plan.tail_keyframe_seconds == 12.0
    assert plan.requested == 300


def test_a_tail_that_ends_before_the_next_keyframe_needs_no_body(keyframes):
    """The tail is a run of frames, so it does not wait for the next keyframe: a range that
    ends before one is re-encoded whole rather than lengthened to reach a boundary."""
    keyframes(4.0, 12.0)
    plan = cutter.plan_trim(spec(150, 200), media())
    assert plan.mode == "reencode"
    assert plan.tail_frames == 0
    assert any("no keyframe inside the segment" in note for note in plan.notes)


def test_a_single_keyframe_leaves_nothing_to_copy(keyframes):
    """A copied middle needs two keyframes: one to open it and one to close it. A file that
    only has the first has no packets that can be passed through untouched."""
    keyframes(4.0)
    plan = cutter.plan_trim(spec(100, 200), media())
    assert plan.mode == "reencode"
    assert plan.body_frames == 0
    assert plan.tail_frames == 0
    assert any("no copied body fits" in note for note in plan.notes)


def test_no_keyframe_at_all_is_re_encoded_whole(keyframes):
    keyframes()
    assert cutter.plan_trim(spec(50, 200), media()).mode == "reencode"


# ---------------------------------------------------------------------------------------
# Refusals, before anything is written
# ---------------------------------------------------------------------------------------

def test_an_empty_or_backwards_range_is_refused(keyframes):
    keyframes(4.0)
    with pytest.raises(cutter.TrimError, match="not after the in point"):
        cutter.plan_trim(spec(200, 200), media())
    with pytest.raises(cutter.TrimError, match="not after the in point"):
        cutter.plan_trim(spec(200, 100), media())


def test_a_negative_in_point_is_refused(keyframes):
    keyframes(4.0)
    with pytest.raises(cutter.TrimError, match="before the start"):
        cutter.plan_trim(spec(-1, 100), media())


def test_an_out_point_past_the_end_is_refused_and_says_where_the_end_is(keyframes):
    keyframes(4.0)
    with pytest.raises(cutter.TrimError, match="past the end of the file"):
        cutter.plan_trim(spec(100, 501), media())


def test_a_source_whose_codec_cannot_be_patched_is_refused_with_a_reason(keyframes):
    keyframes(4.0)
    with pytest.raises(cutter.TrimError, match="only H.264 and HEVC"):
        cutter.plan_trim(spec(100, 200), media(codec="vp9"))


# ---------------------------------------------------------------------------------------
# The two things that are warnings rather than refusals
# ---------------------------------------------------------------------------------------

def test_a_concat_offset_that_would_empty_the_head_is_re_encoded_whole(keyframes):
    # A tail cannot absorb a concat offset the way a head does -- it is anchored to the out
    # point -- so when the offset would leave no copied middle, the segment is re-encoded
    # whole. It used to clamp the head to one frame instead, which silently left the out
    # point overshooting, so the note is the evidence that it refused rather than guessed.
    keyframes(4.0, 8.0, 12.0)
    plan = cutter.plan_trim(spec(150, 450, concat_offset=50), media())
    assert plan.mode == "reencode"
    assert any("no copied body fits" in note for note in plan.notes)

def test_a_file_whose_timestamps_are_off_its_grid_is_planned_anyway_and_says_so(keyframes):
    # No check can make a mark exact against a frame grid that does not exist, so the honest
    # answer is a warning beside a plan that still works -- and the warning carries the
    # measurement, because "0.99 frame(s) away by the end" is the number that says whether a
    # failed alignment check means the cut moved or the file has no grid to be exact against.
    keyframes(4.0)
    off_grid = media(frames=300, rate=Fraction(30))
    off_grid.average_rate = Fraction(24000, 1001)
    plan = cutter.plan_trim(spec(100, 200), off_grid)
    assert any("away from that grid" in note for note in plan.notes)


def test_a_file_whose_timestamps_accumulate_a_whole_frame_of_drift_is_flagged(keyframes):
    """The case a rate comparison misses.

    The reference master reports 30/1 and averages 29.99943 -- 0.0019% apart, under any
    sensible threshold -- yet over 52210 frames the difference is exactly one frame. A mark
    then lands a frame out in one part of the file and exactly right in another.
    """
    keyframes(4.0)
    drifting = media(frames=52210, rate=Fraction(30))
    drifting.duration = 52210 / 30 + 1 / 30           # a frame of drift by the end
    drifting.average_rate = Fraction(30)
    assert drifting.grid_drift == pytest.approx(1.0, abs=0.01)
    assert drifting.variable is True
    assert any("away from that grid" in note for note in
               cutter.plan_trim(spec(100, 200), drifting).notes)


def test_frames_are_counted_on_the_grid_the_file_is_actually_on():
    """The case that made alignment checks fail on real material.

    The reference master reports ``30/1`` and averages ``156630000/5221099``. Counting its
    frames at 30 puts them a whole frame away from where they are by the end of the file, so a
    mark is a frame out in one part of the file and exact in another -- which is not a grid any
    check can be exact against. The engine converts on the file's own grid instead.
    """
    master = media(frames=52210, rate=Fraction(30))
    master.average_rate = Fraction(156630000, 5221099)
    master.duration = 1740.366333
    master.start_time = 0.021

    assert master.grid_rate_text == "156630000/5221099"
    assert master.grid_drift == pytest.approx(0.99, abs=0.02)

    on_the_grid = master.seconds_of(52209)
    on_the_claimed_rate = master.start_time + 52209 / 30
    assert on_the_grid - on_the_claimed_rate == pytest.approx(1 / 30, abs=0.002)

    # And the frame a time names comes back the same way, so the two are inverses.
    assert master.frame_of(on_the_grid) == 52209


def test_an_ordinary_rate_is_its_own_grid():
    plain = media(frames=300, rate=Fraction(25))
    assert plain.grid_rate == Fraction(25)
    assert plain.grid_rate_text == "25"
    assert plain.seconds_of(100) == pytest.approx(4.0)
    assert plain.frame_of(4.0) == 100


def test_an_ordinary_constant_rate_file_is_not_flagged(keyframes):
    # The check has to be quiet on clean material or it is noise nobody reads.
    keyframes(4.0)
    clean = media(frames=300, rate=Fraction(25))
    assert clean.grid_drift == pytest.approx(0.0, abs=0.01)
    assert clean.variable is False
    assert not any("away from that grid" in note
                   for note in cutter.plan_trim(spec(100, 200), clean).notes)


# ---------------------------------------------------------------------------------------
# Naming
# ---------------------------------------------------------------------------------------

def test_the_default_output_name_says_which_range_it_holds():
    # Named by timecode rather than by frame number, because a timecode is what the operator
    # typed and what they will look for afterwards. Colons are not legal in a Windows file
    # name, so they are dotted.
    named = cutter.default_output(SOURCE, 100, 199, Fraction(25))
    assert named.suffix == ".mp4"
    assert named.parent == SOURCE.parent
    assert named.name == "reel 00.00.04.00-00.00.07.24.mp4"


def test_the_default_output_name_never_contains_a_character_windows_refuses():
    for rate in (Fraction(25), NTSC_30, CINEMA):
        named = cutter.default_output(SOURCE, 0, 1, rate)
        assert not set(':*?"<>|') & set(named.name)


# ---------------------------------------------------------------------------------------
# The body copy's contract with the rest of the trim
# ---------------------------------------------------------------------------------------

def test_the_body_copy_is_handed_the_cancellation_token(monkeypatch, tmp_path):
    """The copy is a packet read rather than a subprocess, so nothing else in the engine polls
    the token for the length of it. The first version called a method the token does not have,
    which surfaced as an `AttributeError` in the middle of a real trim on a 34-minute master
    rather than as a failing test -- so the contract is asserted here directly."""
    media_obj = media(frames=500)
    plan = cutter.TrimPlan(media_obj, "headpatch", 100, 100, 300, tail_frames=100)
    seen = []

    def fake(source, destination, first, last, cancel=None):
        seen.append(cancel)
        Path(destination).write_bytes(b"")

    monkeypatch.setattr(cutter, "_copy_packets", fake)
    monkeypatch.setattr(cutter.ff, "inspect", lambda path: SimpleNamespace(frames=300))

    token = cutter.CancelToken()
    cutter._copy_body(media_obj, spec(0, 500), plan, tmp_path / "b.mp4",
                      lambda *_: None, token)
    assert seen == [token], "the token was not handed to the copy"


def test_a_cancelled_job_does_not_start_its_body_copy(monkeypatch, tmp_path):
    media_obj = media(frames=500)
    plan = cutter.TrimPlan(media_obj, "headpatch", 100, 100, 300, tail_frames=100)
    monkeypatch.setattr(cutter, "_copy_packets",
                        lambda *a, **k: pytest.fail("the copy ran after cancellation"))

    stopped = cutter.CancelToken()
    stopped.cancel()
    with pytest.raises(cutter.Cancelled):
        cutter._copy_body(media_obj, spec(0, 500), plan, tmp_path / "b.mp4",
                          lambda *_: None, stopped)


def test_the_packet_reader_stops_when_the_job_is_cancelled():
    """Checked inside the demux loop, not only before it: reading a long file's packets is the
    longest single operation in a trim, and a token looked at once would leave Cancel appearing
    to do nothing for the whole of it."""
    import inspect as _inspect

    source_text = _inspect.getsource(cutter._copy_packets)
    assert "cancel.cancelled" in source_text, "the demux loop does not poll the token"
    assert "raise Cancelled" in source_text
