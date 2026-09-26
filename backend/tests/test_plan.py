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

import pytest

from trimmer import trim as cutter
from trimmer.ffmpeg import AudioStream, MediaInfo

SOURCE = Path("C:/media/reel.mp4")
NTSC_30 = Fraction(30000, 1001)
CINEMA = Fraction(24000, 1001)


def media(frames: int = 300, rate: Fraction = Fraction(25), codec: str = "h264") -> MediaInfo:
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
# The three modes
# ---------------------------------------------------------------------------------------

def test_an_in_point_on_a_keyframe_is_copied_whole(keyframes):
    keyframes(4.0)
    plan = cutter.plan_trim(spec(100, 200), media())
    assert plan.mode == "copy"
    assert (plan.keyframe, plan.head_frames, plan.body_frames) == (100, 0, 100)


def test_an_in_point_between_keyframes_re_encodes_only_the_head(keyframes):
    # Keyframe at frame 75; in point at frame 50. Frames 50..74 are re-encoded, 75..199 are
    # the original packets. Getting this split wrong is the slow-motion bug: the copied body
    # is rescanned against a timescale it did not have.
    keyframes(3.0)
    plan = cutter.plan_trim(spec(50, 200), media())
    assert plan.mode == "headpatch"
    assert plan.keyframe == 75
    assert plan.head_frames == 25
    assert plan.body_frames == 125
    assert plan.requested == 150


def test_a_range_with_no_keyframe_in_it_is_re_encoded_whole(keyframes):
    # The next keyframe is past the out point, so there is no packet boundary to copy from.
    keyframes(9.0)
    plan = cutter.plan_trim(spec(50, 200), media())
    assert plan.mode == "reencode"
    assert plan.keyframe == -1
    assert plan.body_frames == 0
    assert any("no keyframe inside the segment" in note for note in plan.notes)


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
        cutter.plan_trim(spec(100, 301), media())


def test_a_source_whose_codec_cannot_be_patched_is_refused_with_a_reason(keyframes):
    keyframes(4.0)
    with pytest.raises(cutter.TrimError, match="only H.264 and HEVC"):
        cutter.plan_trim(spec(100, 200), media(codec="vp9"))


# ---------------------------------------------------------------------------------------
# The two things that are warnings rather than refusals
# ---------------------------------------------------------------------------------------

def test_a_concat_offset_that_would_empty_the_head_is_clamped_and_recorded(keyframes):
    # A head of zero frames is not a head. Clamping silently would hide a bad offset, so the
    # plan carries a note saying what it did.
    keyframes(3.0)
    plan = cutter.plan_trim(spec(50, 200, concat_offset=25), media())
    assert plan.head_frames >= 1
    assert any("clamped" in note for note in plan.notes)


def test_a_variable_rate_file_is_planned_anyway_and_says_so(keyframes):
    # No check can make a mark exact against a grid that does not exist, so the honest
    # answer is a warning beside a plan that still works.
    keyframes(4.0)
    variable = media()
    variable.average_rate = Fraction(24000, 1001)
    plan = cutter.plan_trim(spec(100, 200), variable)
    assert plan.mode == "copy"
    assert any("variable frame rate" in note for note in plan.notes)


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
