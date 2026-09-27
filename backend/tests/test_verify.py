"""The verification's own logic, tested without a media file.

`verify` spends its time reading frames out of two files, which is why the checks that matter are
written as small functions over hashes and plans: those can be tested directly. This file does
that, because the alternative -- testing them only by cutting real material -- is how three
successive faults in the checker went unnoticed while every real-file report said PASS.
"""

from __future__ import annotations

from fractions import Fraction
from pathlib import Path
from types import SimpleNamespace

from trimmer import ffmpeg as ff
from trimmer import trim as cutter
from trimmer import verify as verifier


def media(frames: int = 500, rate: Fraction = Fraction(25)) -> ff.MediaInfo:
    return ff.MediaInfo(
        path=Path("C:/media/reel.mp4"),
        codec="h264",
        width=1920,
        height=1080,
        pix_fmt="yuv420p",
        rate=rate,
        average_rate=rate,
        timebase=Fraction(1, 12800),
        frames=frames,
        duration=frames / float(rate),
        audio=ff.AudioStream(codec="aac", sample_rate=48000, channels=2),
        size_bytes=1_000_000,
    )


def plan_for(head: int, body: int, tail: int, keyframe: int = 100) -> cutter.TrimPlan:
    return cutter.TrimPlan(media(), "headpatch", keyframe, head, body, tail_frames=tail)


def fake_frames(monkeypatch, source_hashes: list[str], output_hashes: list[str],
                total_output_frames: int | None = None) -> None:
    """Stand in for reading frames: hand back the hashes each side is asked for.

    `frames_near` is given a start time, so the index into the list is worked out from it -- which
    also proves the caller is aiming at the frame it means rather than at the start.
    """
    monkeypatch.setattr(ff, "inspect",
                        lambda path: SimpleNamespace(
                            frames=total_output_frames or len(output_hashes)))
    monkeypatch.setattr(
        ff, "frames_near",
        lambda path, at, count: [
            (at + i / 25.0, (source_hashes if "src" in str(path) else output_hashes)[i])
            for i in range(count)
        ])


# ---------------------------------------------------------------------------------------
# The copied body
# ---------------------------------------------------------------------------------------

def test_a_body_that_is_the_source_packets_passes(monkeypatch):
    """The claim the product makes, checked: every copied frame is the source's own."""
    hashes = ["h%d" % i for i in range(40)]
    fake_frames(monkeypatch, hashes, hashes)
    compared, matched, wrong = verifier.body_is_intact(
        Path("C:/media/src.mp4"), Path("C:/media/out.mp4"), cutter.TrimSpec(
            source=Path("C:/media/reel.mp4"), output=Path("C:/media/out.mp4"),
            in_frame=0, out_frame=40),
        plan_for(head=10, body=30, tail=10), media())
    assert compared == 30
    assert matched == 30
    assert wrong == []


def test_a_body_that_is_not_is_reported_with_the_frame(monkeypatch):
    """A failure has to say *where*, or it cannot be acted on."""
    good = ["h%d" % i for i in range(40)]
    bad = list(good)
    bad[15] = "re-encoded"
    fake_frames(monkeypatch, good, bad)
    compared, matched, wrong = verifier.body_is_intact(
        Path("C:/media/src.mp4"), Path("C:/media/out.mp4"), cutter.TrimSpec(
            source=Path("C:/media/reel.mp4"), output=Path("C:/media/out.mp4"),
            in_frame=0, out_frame=40),
        plan_for(head=10, body=30, tail=10), media())
    assert compared == 30
    assert matched == 29
    assert len(wrong) == 1
    assert "body frame 16" in wrong[0], wrong[0]


def test_a_file_shorter_than_its_own_head_and_body_is_reported(monkeypatch):
    """A count that cannot be met is said plainly rather than compared into nonsense."""
    hashes = ["h%d" % i for i in range(40)]
    fake_frames(monkeypatch, hashes, hashes, total_output_frames=20)
    compared, matched, wrong = verifier.body_is_intact(
        Path("C:/media/src.mp4"), Path("C:/media/out.mp4"), cutter.TrimSpec(
            source=Path("C:/media/reel.mp4"), output=Path("C:/media/out.mp4"),
            in_frame=0, out_frame=40),
        plan_for(head=10, body=30, tail=10), media())
    assert compared == 0
    assert wrong and "short of" in wrong[0]


def test_a_body_of_nothing_is_not_an_error(monkeypatch):
    """A whole-segment re-encode has no copied body, so there is nothing to compare."""
    compared, matched, wrong = verifier.body_is_intact(
        Path("C:/media/src.mp4"), Path("C:/media/out.mp4"), cutter.TrimSpec(
            source=Path("C:/media/reel.mp4"), output=Path("C:/media/out.mp4"),
            in_frame=0, out_frame=40),
        plan_for(head=0, body=0, tail=40), media())
    assert (compared, matched, wrong) == (0, 0, [])


# ---------------------------------------------------------------------------------------
# The verdict
# ---------------------------------------------------------------------------------------

def test_a_verdict_is_three_valued_never_two():
    """`not checked` and `failed` are different answers and must not collapse."""
    from trimmer.verify import VerifyResult

    result = VerifyResult()
    result.checks.append("something passed")
    assert result.ok
    assert len(result.failures) == 0

    failed = VerifyResult()
    failed.failures.append("something failed")
    assert not failed.ok

    empty = VerifyResult()
    assert empty.ok, "a run with no failures is still not a failure"


# ---------------------------------------------------------------------------------------
# A whole-segment re-encode must carry the sound
# ---------------------------------------------------------------------------------------

def test_a_whole_segment_re_encode_maps_the_audio():
    """The fault this guards produced silent files, and nothing caught it.

    When the video and audio maps were split into two helpers, this pass kept using only the
    video one -- so it added -af and -c:a aac while mapping no audio stream at all, and
    ffmpeg wrote a file with a picture and no sound. It was found by cutting a real master and
    looking at the result: fprobe showed one stream where there should have been two.

    No test could have caught it by accident: every check in the suite passed, because the frame
    count, the duration and the picture were all correct.
    """
    from trimmer import trim as cutter

    spec = cutter.TrimSpec(source=Path("C:/media/reel.mp4"), output=Path("C:/media/out.mp4"),
                           in_frame=0, out_frame=100)
    args = cutter._reencode_args(media(), spec, Path("C:/media/out.mp4"))
    assert "0:a:0" in args, "the re-encode maps no audio stream, so the file comes out silent"
    assert "-c:a" in args


def test_a_source_with_no_audio_is_not_given_one():
    """And the map is not invented for a source that has none."""
    from trimmer import trim as cutter

    silent = media()
    silent.audio = None
    spec = cutter.TrimSpec(source=Path("C:/media/reel.mp4"), output=Path("C:/media/out.mp4"),
                           in_frame=0, out_frame=100)
    args = cutter._reencode_args(silent, spec, Path("C:/media/out.mp4"))
    assert "0:a:0" not in args
