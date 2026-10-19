"""One contact sheet showing the four joins a head-patched cut is made of.

A cut is a re-encoded **head** from the in point to the next keyframe, a packet-copied
**body**, and a re-encoded **tail** from the last keyframe before the out point. The joins
an operator has to trust are therefore four: the in point, the head->body join at the
keyframe, the body->tail join at the tail keyframe, and the out point. This module turns
those into one PNG.

Everything is addressed by **frame number**. On these masters two packets can share a
presentation time -- the container's first timestamp is often carried by the first two
frames -- so a timestamp is not an identity: ``select`` on ``n`` names exactly one frame,
while a time-based seek can deliver its neighbour. The sheet is rendered in one ffmpeg
pass, because decoding the source once and tiling in the same graph costs one launch and
one decode rather than one of each per tile.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from pathlib import Path

from . import ffmpeg as ff
from .timecode import format_timecode


@dataclass(frozen=True)
class Seam:
    """One place where two differently-produced pieces of a cut meet.

    ``label`` is what the operator reads under the tile; ``frame`` is the source's own
    numbering, counted from zero, because that is the only address a mark has here.
    """

    label: str
    frame: int


def seams(
    *,
    in_frame: int,
    out_frame: int,
    keyframe: int | None,
    tail_keyframe: int | None,
) -> list[Seam]:
    """The joins worth showing, from the planner's own frame numbers.

    The two marks are emitted first, and a later seam on the same frame is dropped. The
    order matters: a join is where the encoder *could* start, while a mark is the frame
    the operator actually asked for, so a join landing on a mark must never hide it.
    That is also why ``in`` and ``out`` are unconditional -- a sheet that omits the frame
    the cut was asked to start or end on is not a proof of the cut.

    A head or tail join is dropped when it sits one frame from its mark. A head of one
    frame re-encodes a single frame, so there is no run of new pictures to inspect; a
    tail of one frame is the same. Two tiles a frame apart would also spend a tile on
    what the mark's own window already shows.
    """
    found = [Seam("in", in_frame), Seam("out", out_frame)]
    if keyframe is not None and abs(keyframe - in_frame) > 1:
        found.append(Seam("head join", keyframe))
    if tail_keyframe is not None and abs(tail_keyframe - out_frame) > 1:
        found.append(Seam("tail join", tail_keyframe))

    seen: set[int] = set()
    unique: list[Seam] = []
    for seam in found:
        if seam.frame not in seen:
            seen.add(seam.frame)
            unique.append(seam)
    return sorted(unique, key=lambda seam: seam.frame)


def windows(frames: list[int], count: int, total: int) -> list[int]:
    """The frame numbers to show: a window of ``count`` around each seam, merged.

    The windows are centred on the seam and then clamped frame by frame to the file's
    real range. The count is what the window asks for, not what it promises: at the first
    frame the negative neighbours all become frame 0 and collapse in the set, so the
    sheet shows the frames that exist instead of repeating one tile to reach a quota. A
    repeated tile would look like several different pictures to anyone reading the sheet,
    which is the opposite of what a proof is for. The last frame clamps the same way.

    Flattening into one set is what de-duplicates overlapping windows. Neighbouring
    seams often share half their frames, and tiling the same picture twice would spend
    half the sheet saying one thing. Sorting restores the order ``select`` needs.
    """
    if count <= 0 or total <= 0:
        return []
    half = count // 2
    offsets = range(-half, count - half)
    seen: set[int] = set()
    for frame in frames:
        for offset in offsets:
            seen.add(min(max(frame + offset, 0), total - 1))
    return sorted(seen)


def sheet_filter(
    rate: float,
    frames: list[int],
    *,
    width: int = 320,
    columns: int = 6,
) -> str:
    """The ``-vf`` graph that selects these frames and tiles them into one image.

    ``n`` is the decoder's frame number and the only stable address for a frame here:
    two packets on these masters can share a presentation time, so selecting by time can
    land on the neighbour. The commas inside ``eq(...)`` are escaped because the
    filtergraph parser splits filters on a comma before the expression is evaluated; an
    unescaped comma cuts the expression in half, which ffmpeg reports as a syntax error
    rather than selecting silently wrong frames -- the better failure, but still a failed
    proof. The single quotes are the same escaping layer, kept because this exact form is
    what ffmpeg's documentation shows for a ``select`` expression.

    ``rate`` is accepted because every caller has one and a future selection by
    timestamp would have to come through it, but this graph must not convert to seconds:
    see the module docstring for the measurement that makes frame numbers the identity.

    Rows round up so a short last row still belongs to the sheet, and ``tile`` flushes
    that partial row at the end of its input.
    """
    if not frames:
        raise ValueError("a contact sheet needs at least one frame")
    if columns <= 0 or width <= 0:
        raise ValueError("columns and width must be positive")
    rows = math.ceil(len(frames) / columns)
    expression = "+".join(f"eq(n\\,{frame})" for frame in frames)
    return f"select='{expression}',scale={width}:-2,tile={columns}x{rows}"


def render(
    source: Path,
    frames: list[int],
    rate: float,
    out: Path,
    *,
    width: int = 320,
    columns: int = 6,
    seek: float | None = None,
    span: float | None = None,
    timeout: float = 300.0,
) -> bool:
    """Write the contact sheet, or return ``False`` when ffmpeg could not.

    The output path is a plain argument and never part of ``-vf``. A filter option that
    takes a path must be a bare filename with the process's working directory set,
    because a Windows drive colon is parsed as a filter option separator; keeping the
    output out of the graph means a source or sheet anywhere on disk renders without
    changing the process's directory. ``-frames:v 1 -update 1`` makes the image2 muxer
    treat the file as one updated image rather than the first of a numbered sequence.

    ``seek`` and ``span`` bound the decode, and they change what ``frames`` means: with a
    seek, the numbers are counted **from the frame the seek lands on**, and the caller is
    responsible for having chosen a time the engine already knows exactly -- the container's
    own time for that frame, never ``frame / rate``. Without them the numbers are absolute
    and the whole file is decoded to find them, which on an hour-long master is minutes:
    a proof that costs more than the cut is not a proof anybody waits for.

    A missing sheet is a missing proof, not a broken cut, so a failed render is reported
    as ``False``: the caller can retry or say the sheet was unavailable while the video
    it describes is still valid. ``timeout`` is a wall-clock ceiling so a damaged source
    cannot pin the worker drawing its proof; it is minutes by default because even a
    bounded proof has to decode up to the out point.
    """
    if not frames:
        return False
    try:
        graph = sheet_filter(rate, frames, width=width, columns=columns)
        out.parent.mkdir(parents=True, exist_ok=True)
        bounds: list[str] = []
        if seek is not None:
            bounds += ["-ss", f"{max(seek, 0.0):.6f}"]
        if span is not None:
            bounds += ["-t", f"{max(span, 0.0):.6f}"]
        ff.run(
            [
                ff.tool("ffmpeg"), "-hide_banner", "-v", "error", "-y",
                *bounds, "-i", str(source), "-map", "0:v:0", "-an",
                "-vf", graph, "-frames:v", "1", "-update", "1", str(out),
            ],
            timeout=timeout,
        )
        return out.exists() and out.stat().st_size > 0
    except (ff.FFmpegError, OSError, ValueError):
        return False


def describe(seams: list[Seam], rate: float) -> list[dict]:
    """The seams as the rows the UI draws: label, frame number, and timecode.

    The timecode comes from :func:`trimmer.timecode.format_timecode` rather than being
    formatted here, so the sheet's captions and the marks the operator typed agree on
    drop-frame and on how many labels a second holds. A second copy of that arithmetic
    is how two parts of one window come to show the same frame at different timecodes.
    """
    return [
        {
            "label": seam.label,
            "frame": seam.frame,
            "timecode": format_timecode(seam.frame, rate),
        }
        for seam in seams
    ]
