"""The proof sheet's decisions, and one real sheet built by ffmpeg.

Most of what can go wrong here is arithmetic: which frames are seams, which frames each
seam's window covers, and whether the filtergraph names exactly those frames. Those are
pure functions and are tested as such, because a real render only proves that *some*
sheet was written -- it cannot say whether frame 37 was in it.

The one real render is here because the escaping and tiling grammar are ffmpeg's, not
ours: a string assertion cannot prove that ``select='eq(n\\,10)+...'`` is a command
ffmpeg accepts. The clip is generated from ``lavfi`` so the test carries no fixture.
"""

from __future__ import annotations

import pytest

from trimmer import ffmpeg as ff
from trimmer import proof

try:
    FFMPEG = ff.tool("ffmpeg")
except ff.FFmpegError:
    FFMPEG = None

requires_ffmpeg = pytest.mark.skipif(FFMPEG is None, reason="ffmpeg is not installed")


# ---------------------------------------------------------------------------------------
# Which frames are seams
# ---------------------------------------------------------------------------------------

def test_a_range_that_starts_on_a_keyframe_has_no_head_join():
    """The best case: the body opens on the in point, so nothing is re-encoded there."""
    got = proof.seams(in_frame=100, out_frame=400, keyframe=100, tail_keyframe=300)
    assert [(seam.label, seam.frame) for seam in got] == [
        ("in", 100),
        ("tail join", 300),
        ("out", 400),
    ]


def test_a_head_patched_range_shows_both_joins():
    """Both re-encoded ends are seams, and they sort between the marks."""
    got = proof.seams(in_frame=100, out_frame=400, keyframe=137, tail_keyframe=350)
    assert [(seam.label, seam.frame) for seam in got] == [
        ("in", 100),
        ("head join", 137),
        ("tail join", 350),
        ("out", 400),
    ]


def test_a_join_that_lands_on_a_mark_keeps_the_mark():
    """A join is where the encoder could start; the mark is what was asked for.

    Here the first keyframe after the in point *is* the out point, so the head join and
    the out mark name one frame. The mark was emitted first, so the join is the one
    dropped: two labels on one tile would be ambiguous, and dropping the out mark would
    hide the frame the cut was asked to end on.
    """
    got = proof.seams(in_frame=100, out_frame=120, keyframe=120, tail_keyframe=110)
    assert [(seam.label, seam.frame) for seam in got] == [
        ("in", 100),
        ("tail join", 110),
        ("out", 120),
    ]


def test_a_join_one_frame_from_its_mark_is_not_a_seam():
    """One frame of re-encode is not a run worth judging separately from the mark."""
    got = proof.seams(in_frame=100, out_frame=400, keyframe=101, tail_keyframe=399)
    assert [(seam.label, seam.frame) for seam in got] == [
        ("in", 100),
        ("out", 400),
    ]


# ---------------------------------------------------------------------------------------
# Which frames each seam shows
# ---------------------------------------------------------------------------------------

def test_a_window_at_the_first_frame_shows_only_frames_that_exist():
    """Negative frame numbers clamp to 0 and collapse, so frame 0 is not repeated."""
    assert proof.windows([0], 5, 100) == [0, 1, 2]


def test_a_window_at_the_last_frame_stops_there():
    """The same clamp at the other end, where the duplicates are the last frame."""
    assert proof.windows([99], 5, 100) == [97, 98, 99]


def test_overlapping_windows_show_each_frame_once():
    """Two seams close together must not spend two tiles on one picture."""
    assert proof.windows([10, 12], 3, 100) == [9, 10, 11, 12, 13]


# ---------------------------------------------------------------------------------------
# The filtergraph: exact strings, because this is a command line
# ---------------------------------------------------------------------------------------

def test_the_filter_selects_by_frame_number_and_tiles_in_one_pass():
    """The escaping is grammar; a command line is the one place an exact string is right."""
    assert proof.sheet_filter(25.0, [10, 11, 12, 13]) == (
        r"select='eq(n\,10)+eq(n\,11)+eq(n\,12)+eq(n\,13)',"
        r"scale=320:-2,tile=6x1"
    )


def test_rows_round_up_for_a_short_last_row():
    """Four frames over three columns is two rows, not one and a third."""
    assert proof.sheet_filter(25.0, [3, 4, 5, 6], width=160, columns=3) == (
        r"select='eq(n\,3)+eq(n\,4)+eq(n\,5)+eq(n\,6)',"
        r"scale=160:-2,tile=3x2"
    )


# ---------------------------------------------------------------------------------------
# What the UI reads
# ---------------------------------------------------------------------------------------

def test_describe_carries_the_timecode_the_window_draws():
    """The sheet's caption and the box the operator typed in must say the same time."""
    got = proof.describe([proof.Seam("head join", 37), proof.Seam("out", 75)], 30.0)
    assert got == [
        {"label": "head join", "frame": 37, "timecode": "00:00:01:07"},
        {"label": "out", "frame": 75, "timecode": "00:00:02:15"},
    ]


# ---------------------------------------------------------------------------------------
# A real sheet
# ---------------------------------------------------------------------------------------

def test_a_failed_render_is_false_rather_than_an_exception(monkeypatch, tmp_path):
    """A missing sheet is a missing proof, not a reason to take the trim down with it."""
    def refuse(*args, **kwargs):
        raise ff.FFmpegError("no such source")

    monkeypatch.setattr(ff, "run", refuse)
    assert proof.render(tmp_path / "missing.mp4", [0], 25.0, tmp_path / "sheet.png") is False


@requires_ffmpeg
def test_a_four_frame_sheet_is_wider_than_a_single_tile(tmp_path):
    """The filtergraph and the muxer agree: one PNG, tiled, not one PNG per frame."""
    source = tmp_path / "clip.mp4"
    ff.run([
        ff.tool("ffmpeg"), "-hide_banner", "-v", "error", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=25:duration=1",
        "-pix_fmt", "yuv420p", "-c:v", "mpeg4", "-q:v", "2", str(source),
    ])

    out = tmp_path / "sheet.png"
    assert proof.render(source, [0, 8, 16, 24], 25.0, out, width=120, columns=2)

    data = out.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    png_width = int.from_bytes(data[16:20], "big")
    png_height = int.from_bytes(data[20:24], "big")
    assert png_width == 240, f"two 120px tiles should be 240px, not {png_width}"
    assert png_height > 0
    assert png_width > 120
