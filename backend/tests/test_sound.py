"""The sound path: a copy when the master's packets fit the container, an encode when they cannot.

The product's claim is that a cut does not re-encode what it can carry, so the assertion that
matters here is on the **packets**: the segment's audio must be byte-identical to a contiguous run
of the master's own, and the first sample delivered must be the one at the mark. A number that
merely looks right would not prove either.

The one test that cuts runs the real engine on a two-second synthetic master -- a re-encoded head,
a copied body and a re-encoded tail, which is the technique the deliverable depends on -- so the
audio work is proven against the same splice the product ships, not against a stand-in.
"""

from __future__ import annotations

import array
import subprocess
import wave
from fractions import Fraction
from pathlib import Path
from types import SimpleNamespace

import av
import pytest

from trimmer import ffmpeg as ff
from trimmer import trim as cutter


def _tool(name: str) -> str:
    try:
        return ff.tool(name)
    except ff.FFmpegError:  # pragma: no cover - only where ffmpeg is absent
        pytest.skip(f"{name} is not installed")


# ---------------------------------------------------------------------------------------------
# Which containers can hold which packets
# ---------------------------------------------------------------------------------------------

@pytest.mark.parametrize(
    ("codec", "suffix", "expected"),
    [
        # The master's own codec, in the container the segment will be written to.
        ("aac", ".mp4", True),
        ("aac", ".m4v", True),
        ("aac", ".mov", True),
        ("aac", ".mkv", True),
        # Matroska takes anything, so nothing is decided.
        ("pcm_s24le", ".mkv", True),
        ("opus", ".webm", True),
        # A container that refuses the packets outright.
        ("pcm_s16le", ".mp4", False),
        ("aac", ".webm", False),
        ("aac", ".mpg", False),
        # Uncompressed sound into a moving-image container is legal where the codec is.
        ("pcm_s16le", ".mov", True),
        # A suffix nobody listed is not guessed at.
        ("aac", ".unknown", False),
    ],
)
def test_the_copy_is_only_attempted_where_the_container_holds_the_codec(
    codec: str, suffix: str, expected: bool
) -> None:
    assert cutter._copyable(codec, suffix) is expected


def test_the_sound_is_aligned_to_the_instant_the_picture_starts() -> None:
    """The origin is the container's stated time for the in frame, when it is known.

    A time computed from the average rate drifts past the frame it names on a long master, and
    the sound would then be aligned to a different instant than the head's seek.
    """
    media = SimpleNamespace(seconds_of=lambda frame: frame / 25.0)
    spec = SimpleNamespace(in_frame=100)
    assert cutter._picture_origin(media, spec, SimpleNamespace(in_seconds=4.004)) == 4.004
    assert cutter._picture_origin(media, spec, SimpleNamespace(in_seconds=None)) == 4.0


def test_a_container_that_cannot_hold_the_codec_encodes_instead(tmp_path, monkeypatch) -> None:
    """The fallback is a guard, not a silent second-class path: it runs and it says so."""
    seen: list[str] = []

    def encoded(*_args, **_kwargs):
        seen.append("encoded")
        return ["encoded"]

    monkeypatch.setattr(cutter, "_encode_sound", encoded)
    spec = cutter.TrimSpec(source=tmp_path / "a.mov", output=tmp_path / "b.mp4",
                           in_frame=0, out_frame=1)
    media = SimpleNamespace(audio=SimpleNamespace(codec="pcm_s16le"),
                            grid_rate=Fraction(25))
    plan = SimpleNamespace(in_seconds=0.0)
    result = cutter._write_sound(media, spec, plan, tmp_path / "sound.mp4",
                                 lambda *_: None, None)
    assert seen == ["encoded"], "a PCM master into MP4 was copied, and that cannot work"
    assert result == ["encoded"]


# ---------------------------------------------------------------------------------------------
# The cut itself
# ---------------------------------------------------------------------------------------------

def _master(folder: Path) -> Path:
    """Two seconds at 25 fps, a keyframe every 15 frames, and a click at exactly 0.800 s.

    The click is the measurement: it is the only loud sample in the file, so where it lands in
    the delivered segment says where the sound actually starts.
    """
    out = folder / "master.mp4"
    subprocess.run(
        [
            _tool("ffmpeg"), "-y", "-hide_banner", "-v", "error",
            "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25:duration=2",
            "-f", "lavfi", "-i",
            "aevalsrc='if(between(t\\,0.800\\,0.810)\\,0.8*sin(2*PI*1000*t)\\,0)':s=48000:d=2",
            "-c:v", "libx264", "-preset", "ultrafast", "-g", "15", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-b:a", "192k", "-shortest", str(out),
        ],
        check=True,
    )
    return out


def _click_seconds(segment: Path, folder: Path) -> float | None:
    wav = folder / "decoded.wav"
    subprocess.run(
        [_tool("ffmpeg"), "-y", "-hide_banner", "-v", "error", "-i", str(segment),
         "-vn", "-ac", "1", "-ar", "48000", "-c:a", "pcm_s16le", str(wav)],
        check=True,
    )
    with wave.open(str(wav), "rb") as handle:
        samples = array.array("h", handle.readframes(handle.getnframes()))
    first = next((index for index, value in enumerate(samples) if abs(value) > 3000), None)
    return None if first is None else first / 48000.0


def _payloads(path: Path) -> list[bytes]:
    with av.open(str(path)) as container:
        stream = container.streams.audio[0]
        return [bytes(packet) for packet in container.demux(stream) if packet.pts is not None]


def test_a_cut_carries_the_masters_own_audio_packets(tmp_path) -> None:
    """Head patch, copied body, tail patch -- and the sound is the master's, untouched.

    Frames 3..41 of the master: the head re-encodes the run to the keyframe at 15, the body copies
    15..29, the tail re-encodes 30..41. The audio is a single contiguous copy, so it cannot be a
    second generation unless something encoded it behind the caller's back.
    """
    source = _master(tmp_path)
    output = tmp_path / "segment.mp4"
    spec = cutter.TrimSpec(source=source, output=output, in_frame=3, out_frame=42)
    cutter.trim(spec, log=lambda *_: None)

    assert ff.inspect(output).frames == 39, "the cut is not the frame count it asked for"

    segment = _payloads(output)
    master = _payloads(source)
    assert segment, "the delivered segment has no audio at all"
    run = len(segment)
    assert any(master[i:i + run] == segment for i in range(len(master) - run + 1)), (
        "the segment's audio is not a contiguous run of the master's own packets, so it was "
        "re-encoded or re-ordered"
    )

    click = _click_seconds(output, tmp_path)
    assert click is not None, "the click is missing, so the sound lost its content"
    # Frame 20 of the master is where the click sits; the segment begins at frame 3.
    expected = (20 - 3) / 25.0
    assert abs(click - expected) < 0.001, (
        f"the click landed at {click:.4f}s, not at {expected:.4f}s: the sound is not aligned "
        f"to the mark the picture starts on"
    )
