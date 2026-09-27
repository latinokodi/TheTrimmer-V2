"""The cut itself: re-encode the head GOP, copy everything after it untouched.

Why a plain ``ffmpeg -ss S -to E -c copy`` is not enough: H.264 and HEVC frames are
deltas against earlier frames, so a decoder needs the keyframe that precedes the mark,
and a stream copy therefore starts at the last keyframe at or before S -- here usually
8.33 s early. The end is not constrained that way: you can stop mid-GOP, because every
frame up to that point is decodable. Only the start is a problem, and only as far as the
next keyframe.

So the cut is split at K, the first keyframe at or after S::

    requested:   S ────────────── E
    keyframe:         K
                 ├─[S, K)─┤├─[K, E]─┤
                  re-encode   copy

Only K - S is re-encoded; from K on the packets are the original ones, so the result is
visually indistinguishable from the source. This is *not* mathematically lossless --
``-crf 0`` would be, at many times the size. A two-hour source typically re-encodes two
or three seconds of it.

Three details that each cost a corrupted file before they were pinned down:

1. **Timescale.** MP4 keeps one timescale for a whole track. libx264 defaults to
   1/15360 while these sources are 1/90000, and joining the two makes the container
   rescale the copied body: the clip plays in slow motion with frozen stretches while
   ffmpeg still exits 0. The head is therefore encoded with
   ``-video_track_timescale`` set to the source's own timebase.

2. **Codec.** The head must be the *same codec* as the body, or the track holds two
   kinds of sample description and players (and Premiere) refuse it. The encoder is
   chosen from the source: libx264 for H.264, libx265 for HEVC, and anything else is
   refused with a message rather than silently mangled.

3. **Where the body starts.** The concat demuxer places the second file at the first
   file's duration, and that duration is not always the head's content length: on these
   sources the body lands a few frames early, repeating a slice of the head and shifting
   everything after it. Two things guard against that -- an explicit ``duration``
   directive in the concat list, and :func:`trim_with_calibration`, which measures the
   finished file against the source and re-cuts once with the difference applied.
"""

from __future__ import annotations

import bisect
import shutil
import tempfile
from dataclasses import dataclass, field
from fractions import Fraction
from functools import lru_cache
from pathlib import Path

import av

from . import container
from . import ffmpeg as ff
from . import subtitles as subs
from .ffmpeg import CancelToken, Cancelled, MediaInfo
from .subtitles import RetimeResult
from .timecode import format_timecode

#: Source codec -> the encoder that can join it, and anything the muxer needs.
#:
#: Only codecs whose cut has been verified on real material are listed. MPEG-2 and MPEG-4 were
#: added and then taken out again: an MPEG program stream's tail scored -1 frame at ssim 0.806 on
#: a cut whose frame count was exact, and whether that is a fault in the cut or an artefact of
#: seeking a long-GOP program stream was not settled. Shipping a codec on an unsettled
#: measurement is how "the body is the original packets" stops being true for some files.
ENCODERS = {
    "h264": ("libx264", []),
    "hevc": ("libx265", ["-tag:v", "hvc1"]),
    "h265": ("libx265", ["-tag:v", "hvc1"]),
    # Editing codecs. These are all-intra, so they need no encoder at all -- see ALL_INTRA --
    # and the entry here is only for a source whose ends still need patching.
    "prores": ("prores_ks", ["-profile:v", "3", "-vendor", "apl0"]),
    "dnxhd": ("dnxhd", []),
}

#: Codecs whose every frame stands alone, so a stream copy may begin and end anywhere in them.
#:
#: This is what makes the product's promise exactly true rather than nearly true. For a long-GOP
#: source (H.264, HEVC, MPEG-2) a copy has to begin on a keyframe, which is why those need a
#: re-encoded head and tail; for an all-intra one there is no such frame, so the wanted range is
#: copied packet for packet, byte for byte, and *nothing is re-encoded at all* -- frame-exact and
#: lossless in the strict sense, and no head or tail to stitch.
ALL_INTRA = {
    "prores", "dnxhd", "dvvideo", "mjpeg", "ffv1", "huffyuv", "rawvideo", "v210",
    "qtrle", "png", "cfhd", "hap",
}

#: Pixel formats the encoders above will accept straight through. Anything else (a 12-bit
#: or exotic source) is converted to the 8-bit 4:2:0 its codec family expects.
PASSTHROUGH_PIX_FMTS = {
    "yuv420p", "yuv422p", "yuv444p", "yuv420p10le", "yuv422p10le", "yuv444p10le",
}


def _known_codecs() -> str:
    """The source codecs the engine can cut, for a refusal that names them all.

    Built from the two tables rather than written out, so adding a codec cannot leave the
    sentence behind -- which is how a message that said "only H.264 and HEVC" came to be wrong
    the moment anything else was added.
    """
    intra = sorted(ALL_INTRA & set(ENCODERS))
    gop = sorted(set(ENCODERS) - ALL_INTRA)
    return f"{_and_list(gop)} (long-GOP) and {_and_list(intra)} (all-intra)"


def _and_list(names: list[str]) -> str:
    if not names:
        return "nothing"
    if len(names) == 1:
        return names[0]
    return ", ".join(names[:-1]) + " and " + names[-1]


class TrimError(RuntimeError):
    """The requested trim cannot be made from this source."""


@dataclass
class TrimSpec:
    """One trim request: the source, the frame range, and the knobs that matter."""

    source: Path
    output: Path
    in_frame: int
    out_frame: int                       # exclusive: one past the last frame kept
    crf: int = 18
    preset: str = "veryfast"
    concat_offset: int = 0               # frames of concat drift to cancel
    audio_bitrate: str = "192k"
    overwrite: bool = True
    #: A transcript to cut along with the video, usually ``<video>.srt``. The retimed
    #: captions are written beside the output, named after it.
    subtitles: Path | None = None

    @property
    def frames(self) -> int:
        return self.out_frame - self.in_frame


@dataclass
class TrimPlan:
    """What the trim *will* do, worked out before anything is written."""

    media: MediaInfo
    mode: str                            # "headpatch", "copy" or "reencode"
    keyframe: int
    head_frames: int
    body_frames: int
    notes: list[str] = field(default_factory=list)
    #: Frames re-encoded at the far end, from the last keyframe before the out point to the
    #: out point. Both ends are re-encoded so the segment is frame-exact: a stream copy
    #: stops on a packet boundary and lands a few frames past the out point, because
    #: ``-t`` is a time and ffmpeg stops on a whole B-frame packet group.
    tail_frames: int = 0
    #: The chosen keyframe's own presentation time. The frame numbering the copy uses comes
    #: from `keyframe` and `body_frames` instead; see `_copy_packets`.
    keyframe_seconds: float | None = None
    #: Where the re-encoded tail begins: the last keyframe at or before the out point.
    tail_keyframe_seconds: float | None = None
    #: Where each re-encoded end must be seeked to, as the container states it. Neither is
    #: `seconds_of`, which computes the time from the average-rate grid and drifts past the
    #: frame it names on a long master -- see `ffmpeg.frame_pts`. `None` means the two are the
    #: same and the caller may compute them.
    in_seconds: float | None = None
    out_seconds: float | None = None

    @property
    def requested(self) -> int:
        return self.head_frames + self.body_frames + self.tail_frames

    @property
    def body_row(self) -> int:
        """The container **row** the copied body opens on: the keyframe's own row.

        The copy selects packets by their position in decode order, which is a different grid from
        the frame numbers a mark is given in. This is the one place that difference is stated, so
        that everything reporting a span reports it in rows and the spans join up.
        """
        return self.keyframe + 1

    def rows(self) -> dict[str, tuple[int, int]]:
        """Each piece's span of container rows, in order. Contiguous by construction.

        Reported instead of frame numbers because the two grids do not line up: the log used to
        say ``head 7514..7741`` and ``body 7743..9492``, which reads as a missing frame at 7742,
        while in rows the same cut is ``7515..7742`` then ``7743..9492`` with nothing between
        them. That apparent gap was read as a real one and sent a diagnosis down the wrong path.
        """
        first = self.body_row - self.head_frames
        head = (first, first + self.head_frames - 1) if self.head_frames else (0, -1)
        body = ((self.body_row, self.body_row + self.body_frames - 1) if self.body_frames
                else (0, -1))
        tail = ((body[1] + 1, body[1] + self.tail_frames) if self.tail_frames else (0, -1))
        return {"head": head, "body": body, "tail": tail}

    def describe(self, rate) -> str:
        # Spans are given as container rows, which is the grid the copy selects by, so the three
        # of them join end to end. Frame numbers would leave an apparent gap wherever the two
        # grids differ -- see `rows`.
        spans = self.rows()
        lines = [f"mode        {self.mode}"]
        if self.mode == "headpatch":
            if self.head_frames:
                first, last = spans["head"]
                lines.append(
                    f"head        rows {first}..{last} "
                    f"({self.head_frames} frames, {self.head_frames / float(rate):.3f}s) "
                    f"re-encoded from the in point to the keyframe at row {self.keyframe}"
                )
            if self.body_frames:
                first, last = spans["body"]
                lines.append(f"body        rows {first}..{last} copied untouched")
            if self.tail_frames:
                first, last = spans["tail"]
                lines.append(
                    f"tail        rows {first}..{last} "
                    f"({self.tail_frames} frames, {self.tail_frames / float(rate):.3f}s) "
                    f"re-encoded from the last keyframe to the row the out point names"
                )
        elif self.mode == "copy":
            lines.append("in point    lands on a keyframe: the whole segment is copied")
        else:
            lines.append(f"whole segment re-encoded (frames {self.requested})")
        for note in self.notes:
            lines.append(f"note        {note}")
        return "\n".join(lines)


@dataclass
class TrimReport:
    spec: TrimSpec
    plan: TrimPlan
    output: Path
    frames: int
    duration: float
    overshoot: int
    commands: list[list[str]] = field(default_factory=list)
    #: What happened to the transcript, when the request carried one: the retimed cues,
    #: how many were clamped at the marks, and the file written (None when nothing fell
    #: inside the segment).
    subtitles: RetimeResult | None = None

    def summary(self, rate) -> str:
        return (f"{self.output.name}: {self.frames} frames, {self.duration:.3f}s, "
                f"requested {self.plan.requested} frames from "
                f"{format_timecode(self.spec.in_frame, rate)} to "
                f"{format_timecode(self.spec.out_frame, rate)}")


def _row_of(media: MediaInfo, frame: int, frame_time=None) -> int:
    """Which row of the container's packet order a mark sits on.

    Rows and frame numbers are two different grids, and their offset is not constant: measured on
    three masters it is +1 on Joseph Chalom and +2 on TY Gellasch and Andy Ross. So the row is
    found by asking the container, never by computing it from the frame number.

    Falls back to the mark's own number when the container cannot be read, which is what every
    build before this used and what a synthetic source in a test still gets.
    """
    try:
        index = container.read(media.path)
    except (OSError, ValueError):
        return frame + 1
    want = frame_time(frame) if frame_time is not None else media.seconds_of(frame)
    # Start from the mark's own row and look outward: the answer is within a few rows, and reading
    # the whole packet list to find it would be a second walk of the container.
    best, best_distance = frame + 1, float("inf")
    for delta in range(-8, 9):
        row = frame + 1 + delta
        if not 1 <= row <= index.count:
            continue
        distance = abs(index.time_of(row) - want)
        if distance < best_distance:
            best, best_distance = row, distance
    return best


@lru_cache(maxsize=4)
def _keyframes_of(path: str, size: int, modified_ns: int) -> tuple:
    """Every keyframe in a file, remembered per source.

    The planner is called on **every keystroke** -- the interface asks what a range will do so it
    can decide whether the Trim button is enabled -- and listing a 35-minute master's keyframes
    means walking the whole container, 2.6-7 s however it is asked for. That cost is per *file*,
    not per question, so the answer is remembered and the plan slices it.

    Keying this on the out point as well was the first attempt and it was useless: every
    keystroke is a different out point, so every keystroke missed the cache and read the file
    again -- measured, 6.7 s each time. The key is the file's identity, its size and its
    modification time, and nothing that a question can change.

    Output times are rounded to the millisecond. A keyframe is turned into a frame number before
    it is used, so the rounding cannot change a decision.
    """
    marks = ff.keyframes(Path(path), -1.0, 1e12)
    return tuple(round(mark, 3) for mark in marks)


def plan_trim(spec: TrimSpec, media: MediaInfo, frame_time=None) -> TrimPlan:
    """Work out the split point, and refuse clearly when the source cannot be cut.

    Called before the trim so the CLI can show the plan and the GUI can react to an
    impossible request without starting ffmpeg.

    ``frame_time`` maps a frame number to the time the container says that frame is shown,
    used wherever a seek has to be aimed. It defaults to the computed grid time, which is
    what the engine used for every seek until a long master showed the difference: a seek
    aimed at the computed time lands one frame late by the far end of a thirty-minute file.
    Passing ``ff.FrameTimes`` makes both ends exact; leaving it out keeps the arithmetic
    that every existing caller and test expects.
    """
    if frame_time is None:
        frame_time = media.seconds_of
    if spec.frames <= 0:
        raise TrimError("the out point is not after the in point")
    if spec.in_frame < 0:
        raise TrimError("the in point is before the start of the file")
    if spec.out_frame > media.frames:
        raise TrimError(
            f"the out point is frame {spec.out_frame}, past the end of the file "
            f"({media.frames} frames / {format_timecode(media.frames - 1, media.rate)})"
        )
    if media.codec.lower() not in ENCODERS:
        raise TrimError(
            f"the source is {media.codec}; the method needs the head to be the same codec as "
            f"the body, and {_known_codecs()} are supported. Re-encode the source first, or "
            "trim it with a plain ffmpeg re-encode."
        )

    notes: list[str] = []

    # An all-intra source needs no patching at all: every frame stands alone, so the wanted range
    # is copied packet for packet and nothing is re-encoded. This is the one case where the
    # segment is lossless in the strict sense -- not "the original packets except at the ends".
    if media.codec.lower() in ALL_INTRA:
        in_row = _row_of(media, spec.in_frame, frame_time)
        notes.append(
            f"the source is all-intra ({media.codec}), so every frame stands alone: the whole "
            "segment is copied from the original packets and nothing is re-encoded"
        )
        # `keyframe` is the row the copy opens on, less one, because the copy adds one. Every
        # frame is a keyframe here, so the copy may open on the in point itself.
        return TrimPlan(media, "copy", in_row - 1, 0, spec.frames, notes,
                        in_seconds=frame_time(spec.in_frame),
                        out_seconds=frame_time(spec.out_frame))

    if media.variable:
        # Say what was measured, not just that something is off: "one frame" is a number the
        # operator can weigh, and it is the number that decides whether a failed alignment
        # check means the cut moved or the file has no grid to be exact against.
        averages = ("--" if media.average_rate is None
                    else f"{float(media.average_rate):.5f}")
        drift = media.grid_drift
        notes.append(
            f"the file claims {media.rate_text} fps but averages {averages}, so its "
            f"timestamps are {drift:.2f} frame(s) away from that grid by the end; a frame "
            "grid the method assumes may not hold, and a mark can be a frame out in part of "
            "the file and exact in another"
        )

    rate = media.grid_rate
    # The seek targets, from the container where it can say: a computed time drifts past the
    # frame it names on a long master, and `-ss` aimed there lands inside the next frame.
    #
    # `frame_time` is `ff.FrameTimes`, which asks the container. That reader was answering with
    # a time up to 222 frames out -- it read a window shorter than the gap between keyframes and
    # then returned whichever frame in it was nearest the computed guess -- so this value, the
    # one the re-encoded ends are seeked with, was the least trustworthy number in the planner.
    # See `ffmpeg.frame_pts_near`, which now refuses to answer unless it found the frame asked
    # for.
    #
    # The head still lands one frame after the mark on the reference master, and that is the
    # container's timing rather than a fault: a mark is a time, the seek is a time, and the
    # frames of an OBS master do not sit exactly on the grid the marks are counted in, so the
    # frame the mark names by time is the one after it by packet order. The delivered count is
    # exact, the sound is cut in one pass so there is no seam to clip, and the alignment check
    # names the offset instead of assuming it away.
    in_seconds = frame_time(spec.in_frame)
    out_seconds = frame_time(spec.out_frame)
    # Every keyframe up to the out point, then filtered to the segment. Two are wanted and they
    # are on opposite sides of the range: the first keyframe *after* the in point, which is where
    # the copied body can begin, and the last keyframe *before* the out point, which is where the
    # re-encoded tail begins. Asking ffprobe for a window risks it answering with a keyframe just
    # outside it, so the list is read once and both bounds come from it.
    #
    # The whole file's keyframes, remembered per source (see :func:`_keyframes_of`), then cut
    # down to the range here. Two are wanted and they are on opposite sides of it: the first
    # keyframe *after* the in point, which is where the copied body can begin, and the last
    # keyframe *before* the out point, which is where the re-encoded tail begins.
    #
    # The window opens a frame's worth before the file starts: a window that begins exactly on
    # the first frame's timestamp *omits* that keyframe, and a source whose first keyframe is
    # frame 0 then looks as though it has none on its opening GOP -- which re-encoded ranges that
    # could have copied their whole body.
    try:
        stat = media.path.stat()
        every_mark = _keyframes_of(str(media.path), stat.st_size, stat.st_mtime_ns)
    except OSError:
        # A source that cannot be stat-ed is still worth planning: read it directly.
        every_mark = tuple(ff.keyframes(media.path, -1.0, 1e12))
    # Nothing after the out point can be used -- the tail's keyframe is at or before it -- so the
    # list is cut there and the rest of the planner sees only what it can act on.
    marks = [mark for mark in every_mark if mark <= out_seconds + 1e-6]
    after = [t for t in marks if t >= in_seconds - 1e-6]

    # A keyframe exactly on the in point is the in point: the body opens there and nothing is
    # re-encoded at the front. One beyond the out point leaves no body to copy at all.
    if not after or round((after[0] - media.start_time) * float(rate)) >= spec.out_frame:
        notes.append(
            "no keyframe inside the segment: the whole segment is re-encoded, because a "
            "stream-copied body has to begin and end on a keyframe"
        )
        return TrimPlan(media, "reencode", spec.in_frame, spec.frames, 0, notes)

    # The frame a keyframe holds is measured from the source's **own** first timestamp, not
    # from zero. A master that starts at 0.021 s -- 0.63 of a frame at 30 fps -- puts every
    # keyframe a frame later than `pts * rate` says, so this read 13493 for a keyframe that is
    # really 13492, and the copied body began a frame early. `verify` has always measured from
    # `media.start_time`, so the two disagreed by exactly that frame and the alignment check
    # reported it on every sample.
    opens_at = after[0]
    keyframe = round((opens_at - media.start_time) * float(rate))

    # The tail begins at the last keyframe before the out point and is re-encoded from there,
    # so the segment ends on the frame that was asked for. A copy cannot end there: `-t` is a
    # time, and ffmpeg stops on a whole B-frame packet group -- measured at 4 frames a step on
    # the reference source, independent of GOP length.
    #
    # The search runs over *every* keyframe and not just the ones after the in point: the last
    # keyframe before the out point is often the one the body opens on, or one before the in
    # point entirely, and those are perfectly good places for a tail to begin. Searching only
    # the later ones rejected most real ranges and re-encoded them whole.
    index = bisect.bisect_left([round((t - media.start_time) * float(rate)) for t in marks],
                               spec.out_frame)
    tail_keyframe = marks[index - 1] if index else None
    tail_key = (round((tail_keyframe - media.start_time) * float(rate))
                if tail_keyframe is not None else None)
    if tail_key is None or tail_key < keyframe:
        notes.append(
            "no keyframe at or after the opening keyframe and before the out point to start a "
            "re-encoded tail from, so the whole segment is re-encoded"
        )
        return TrimPlan(media, "reencode", spec.in_frame, spec.frames, 0, notes)

    # The keyframe the body opens on is `keyframe`, and the body runs to the frame before the
    # tail's own keyframe, so nothing here needs the *next* keyframe's timestamp: the drop of
    # the time-based copy removed the only caller that did.
    offset = spec.concat_offset
    head_frames = keyframe - spec.in_frame - offset
    body_frames = tail_key - keyframe
    tail_frames = spec.out_frame - tail_key
    # A head of exactly zero is not empty -- it is the in point landing on the keyframe that
    # opens the body, which is the best case: nothing needs re-encoding at the front. Below
    # zero means a concat offset asked for a head shorter than the run to the keyframe.
    empty_head = head_frames < 0 or (head_frames == 0 and keyframe != spec.in_frame)
    if empty_head or body_frames <= 0 or tail_frames <= 0:
        # A tail cannot absorb a concat offset the way a head does -- the tail is anchored to
        # the out point -- so a segment with no room for a copied body between the two
        # keyframes is re-encoded whole. This replaces the old clamp, which shrank the head to
        # one frame and silently left the out point overshooting.
        notes.append(
            "no copied body fits between the in and out points once both ends are patched, "
            "so the whole segment is re-encoded"
        )
        return TrimPlan(media, "reencode", spec.in_frame, spec.frames, 0, notes)

    return TrimPlan(media, "copy" if keyframe == spec.in_frame else "headpatch",
                    keyframe, head_frames, body_frames, notes,
                    tail_frames=tail_frames, keyframe_seconds=opens_at,
                    tail_keyframe_seconds=tail_keyframe,
                    in_seconds=in_seconds, out_seconds=out_seconds)


def trim(
    spec: TrimSpec,
    *,
    log=print,
    cancel: CancelToken | None = None,
    keep_temp: Path | None = None,
    media: MediaInfo | None = None,
    progress=None,
) -> TrimReport:
    """Build the trimmed file. Raises :class:`TrimError` or :class:`FFmpegError`.

    ``media`` is the already-probed source, when the caller has one: probing is a
    process launch, and the calibration loop would otherwise do it twice per attempt.

    ``progress`` is called with a :class:`~trimmer.ffmpeg.Ticks` for each position ffmpeg
    reports, so a caller can draw a bar that means something. Each pass says how long it
    should be, which is what turns a position into a fraction; a pass whose length is not
    known reports counters without one rather than a made-up percentage.
    """
    media = media or ff.probe(spec.source)
    # Both re-encoded ends are seeked to times the *container* states, not to times computed
    # from the average-rate grid. The two agree on a short file and drift apart on a long one,
    # and where they differ the computed time is past the frame it names, so the seek starts
    # one frame late. See `ffmpeg.frame_pts`.
    plan = plan_trim(spec, media, frame_time=ff.FrameTimes(
        spec.source, media.seconds_of, float(media.grid_rate), media.start_time))
    rate = media.grid_rate
    whole = spec.frames / float(rate)

    if spec.output.exists() and not spec.overwrite:
        raise TrimError(f"{spec.output.name} already exists (overwrite is off)")

    commands: list[list[str]] = []
    work = Path(keep_temp) if keep_temp else Path(tempfile.mkdtemp(prefix="thetrimmer-"))
    work.mkdir(parents=True, exist_ok=True)
    #: The three picture pieces and the one sound piece. The ends are re-encoded and the
    #: middle is copied; the sound is a single continuous track for the whole segment, so
    #: there is no audio seam at either patch to clip or gap.
    # The pieces carry the output's own container, not a hard-coded one: a ProRes source comes
    # in a .mov and its pieces have to be able to hold ProRes.
    suffix = spec.output.suffix or ".mp4"
    head = work / ("head" + suffix)
    body = work / ("body" + suffix)
    tail = work / ("tail" + suffix)
    # The sound piece takes the output's container as well: an .m4a is the MP4 muxer, and an MP4
    # cannot hold the uncompressed PCM that an all-intra source's sound is kept as.
    sound = work / ("sound" + suffix)
    joined = work / "joined.mp4"
    try:
        log(f"source      {media.summary()}")
        log(plan.describe(rate))

        if plan.mode == "reencode":
            commands.append(_reencode(media, spec, spec.output, log, cancel, progress, whole))
        else:
            pieces: list[Path] = []
            # Each piece's length as an exact fraction of a second, not a float division.
            # The concat list rounds to six decimals, and a master at 186525000/6217501 fps
            # gives 8538 frames a length of 284.6000338... s -- where the float and the exact
            # value can round to different milliseconds, which is enough for the demuxer to
            # place the next piece a frame out. See `_concat_list`.
            spans: list[Fraction] = []
            if plan.head_frames > 0:
                # A copy cannot begin mid-GOP, so the run up to the opening keyframe is
                # re-encoded. An all-intra source needs none of this, and a zero-frame piece is
                # not built at all -- a file with no packets cannot be concatenated.
                pieces.append(head)
                spans.append(Fraction(plan.head_frames, 1) / rate)
                commands.append(_encode_head(media, spec, plan, head, log, cancel, progress))
            body_commands, body_frames = _copy_body(media, spec, plan, body, log, cancel,
                                                    progress)
            commands += body_commands
            pieces.append(body)
            # The piece's *measured* length, not the requested one: the concat demuxer uses the
            # stated duration to place the next piece, and a copy that came back a frame or two
            # off would otherwise shift the tail and the sound against the picture.
            spans.append(Fraction(body_frames, 1) / rate)
            if plan.tail_frames > 0:
                # A copy cannot stop on a chosen frame either, so the run from the last keyframe
                # to the out point is re-encoded. This is what makes the segment frame-exact.
                commands.append(_encode_tail(media, spec, plan, tail, log, cancel, progress))
                pieces.append(tail)
                spans.append(Fraction(plan.tail_frames, 1) / rate)

            picture = body
            if len(pieces) > 1:
                listing = work / "concat.txt"
                listing.write_text(_concat_list(pieces, spans), encoding="utf-8")
                commands.append(_join(listing, joined, log, cancel, progress, whole))
                picture = joined
            else:
                # One piece: an all-intra source's range, copied whole. Joining a list of one is
                # a second copy of the same bytes for nothing.
                log("join        nothing to concatenate: the segment is a single copy")

            commands.append(_encode_sound(media, spec, sound, log, cancel, progress))
            commands.append(_mux(picture, sound, spec.output, log, cancel, progress, whole))
    finally:
        if keep_temp is None:
            shutil.rmtree(work, ignore_errors=True)

    facts = ff.inspect(spec.output)
    report = TrimReport(
        spec=spec,
        plan=plan,
        output=spec.output,
        frames=facts.frames,
        duration=facts.video_duration,
        overshoot=max(0, facts.frames - spec.frames),
        commands=commands,
    )
    log(report.summary(rate))
    if report.overshoot:
        # The re-encoded tail makes this zero, so a non-zero value is not a tolerance to
        # report but a signal that the tail did not land where it was aimed. Said plainly
        # rather than phrased as an accepted margin: it used to be the latter, and it hid a
        # real fault in the deliverable for as long as the phrasing held.
        log(f"tail        WARNING {report.overshoot} frame(s) past the out point, which the "
            f"re-encoded tail should have prevented; the cut is longer than asked for")
    if spec.subtitles is not None:
        report.subtitles = cut_subtitles(spec, media, log=log)
    return report


def cut_subtitles(spec: TrimSpec, media: MediaInfo, *, log=print) -> RetimeResult:
    """Move the transcript onto the segment and write it beside the video.

    The window is the one that was asked for, not the file that came out: the frames a
    stream copy adds past the out point belong to the pack's boundary, and a caption
    should never appear for them.
    """
    assert spec.subtitles is not None
    output_srt = spec.output.with_suffix(".srt")
    result, written = subs.retime_file(
        spec.subtitles,
        output_srt,
        media.seconds_of(spec.in_frame),
        media.seconds_of(spec.out_frame),
    )
    log(subs.summary(result, written))
    return result


# --------------------------------------------------------------------------- #
# the individual ffmpeg calls
# --------------------------------------------------------------------------- #

def _common_input() -> list[str]:
    return ["-hide_banner", "-v", "error", "-y"]


def _video_map(media: MediaInfo) -> list[str]:
    return ["-map", "0:v:0"]


def _audio_map(media: MediaInfo) -> list[str]:
    """Map the source's audio, when it has any. The maps are kept exact (``0:a:0``) rather
    than left to ffmpeg's default stream selection, which would also pick up a second
    programme or a commentary track on a broadcast master."""
    return ["-map", "0:a:0"] if media.audio is not None else []


def _encode_head(media: MediaInfo, spec: TrimSpec, plan: TrimPlan, head: Path,
                 log, cancel, progress=None) -> list[str]:
    """Re-encode the run from the in point up to the keyframe that opens the copied body.

    Picture only: the sound is one continuous track for the whole segment (see
    :func:`_encode_sound`), because three separately-encoded audio pieces cannot be joined
    without a gap or an overlap -- AAC frames are 1024 samples, and a piece whose length is
    not a whole number of them has to be padded or clipped at the join.
    """
    encoder, extra = ENCODERS[media.codec.lower()]
    pix_fmt = media.pix_fmt if media.pix_fmt in PASSTHROUGH_PIX_FMTS else "yuv420p"
    head_seconds = plan.head_frames / float(media.grid_rate)
    # The container's own time for the in point where it is known: the computed time drifts
    # past the frame it names, and a seek aimed there starts the head one frame late.
    seek = plan.in_seconds if plan.in_seconds is not None else media.seconds_of(spec.in_frame)
    args = [
        ff.tool("ffmpeg"), *_common_input(),
        "-ss", f"{seek:.6f}", "-i", str(media.path),
        "-frames:v", str(plan.head_frames),
        *_video_map(media),
        "-vf", "setpts=PTS-STARTPTS",
        "-c:v", encoder, "-preset", spec.preset, "-crf", str(spec.crf),
        "-pix_fmt", pix_fmt, "-r", media.grid_rate_text,
        "-video_track_timescale", str(media.timebase.denominator),
        *extra,
        "-an", "-movflags", "+faststart", str(head),
    ]
    first, last = plan.rows()["head"]
    log(f"head        re-encoding rows {first}..{last} from the in point "
        f"with {encoder} (crf {spec.crf}, {spec.preset})")
    ff.run(args, cancel=cancel, log=log, progress=progress,
           expected_seconds=head_seconds)
    return args


def _encode_tail(media: MediaInfo, spec: TrimSpec, plan: TrimPlan, tail: Path,
                 log, cancel, progress=None) -> list[str]:
    """Re-encode the run from the last keyframe before the out point to the out point.

    This is the mirror of :func:`_encode_head`, and it exists because a stream copy cannot
    *stop* on a chosen frame. ``-t`` is a time, and ffmpeg stops on a whole B-frame packet
    group: measured on the reference source the stop moved in steps of four frames, so the
    copy landed two to five frames past the out point no matter how the time was chosen. Two
    consequences, both real: the delivered file ran long, and its sound could stop before its
    picture. Re-encoding this piece settles the frame count on the number that was asked for.

    The picture is seeked on the **input**, like the head, so the encoder sees only the
    frames of the piece; an output seek would hand the encoder frames from before the cut to
    dispose of and the last written frame would depend on the reorder buffer. The count is
    then pinned with ``-frames:v``, which is exact *here* and nowhere else in this module: the
    piece is being re-encoded, so no packet can be dropped by counting, and the plan already
    knows the number wanted. Bounding it by time instead leaves the count at the mercy of how
    many of the source's own frames fall inside the interval -- on a master that averages
    29.999431 while claiming 30, the same arithmetic that is exact elsewhere came back one
    frame long, and the delivered file was one frame over.
    """
    encoder, extra = ENCODERS[media.codec.lower()]
    pix_fmt = media.pix_fmt if media.pix_fmt in PASSTHROUGH_PIX_FMTS else "yuv420p"
    tail_seconds = plan.tail_frames / float(media.grid_rate)
    tail_start_frame = spec.out_frame - plan.tail_frames
    # The tail's first frame is the last keyframe before the out point; the plan carries the
    # container's own time for it, so the seek lands on it rather than beside it.
    start = (plan.tail_keyframe_seconds if plan.tail_keyframe_seconds is not None
             else media.seconds_of(tail_start_frame))
    args = [
        ff.tool("ffmpeg"), *_common_input(),
        "-ss", f"{start:.6f}", "-i", str(media.path),
        "-frames:v", str(plan.tail_frames),
        *_video_map(media),
        "-vf", "setpts=PTS-STARTPTS",
        "-c:v", encoder, "-preset", spec.preset, "-crf", str(spec.crf),
        "-pix_fmt", pix_fmt, "-r", media.grid_rate_text,
        "-video_track_timescale", str(media.timebase.denominator),
        *extra,
        "-an", "-movflags", "+faststart", str(tail),
    ]
    tail_first, tail_last = plan.rows()["tail"]
    log(f"tail        re-encoding rows {tail_first}..{tail_last} with {encoder}, so the "
        f"segment ends on the row the out point names")
    ff.run(args, cancel=cancel, log=log, progress=progress, expected_seconds=tail_seconds)
    return args


def _encode_sound(media: MediaInfo, spec: TrimSpec, sound: Path,
                  log, cancel, progress=None) -> list[str]:
    """Encode the whole segment's audio in one pass.

    One continuous track rather than one piece per video piece. Assembling the sound from
    separately cut pieces is what makes audio clip at a join: each piece has to be a whole
    number of AAC frames, so a join either drops the tail of a piece or repeats part of it,
    and the error lands exactly where the head patch meets the copied body. Encoding the
    segment once removes the joins instead of trying to align them.

    The seek is on the **output** -- after ``-i`` -- which drops whole packets before the
    mark rather than hunting for a sync point, and audio has no GOP, so it lands exactly.
    """
    whole = spec.frames / float(media.grid_rate)
    if media.audio is None:
        return []
    encoder, why = _sound_encoder(media, spec)
    log(f"sound       writing {whole:.3f}s of audio in one piece as {why}, so the joins at the "
        f"head and tail patches are not cut through")
    args = [
        ff.tool("ffmpeg"), *_common_input(),
        "-i", str(media.path),
        "-ss", f"{media.seconds_of(spec.in_frame):.6f}", "-t", f"{whole:.6f}",
        *_audio_map(media),
        "-af", "asetpts=PTS-STARTPTS",
        *encoder,
        "-ar", str(media.audio.sample_rate), "-ac", str(media.audio.channels),
        str(sound),
    ]
    ff.run(args, cancel=cancel, log=log, progress=progress, expected_seconds=whole)
    return args


def _sound_encoder(media: MediaInfo, spec: TrimSpec) -> tuple[list[str], str]:
    """The audio codec to write, and a phrase saying why.

    Chosen for the **container** first, because the container has the final say: an MPEG program
    stream refuses anything but mp1/mp2/mp3/PCM/AC-3/DTS, and an MP4 cannot hold PCM at all. Both
    of those were found by trying it -- "Unsupported audio codec" and "Could not find tag for
    codec pcm_s16le" -- rather than by reading the muxer's rules first.

    Then for the source: uncompressed sound stays uncompressed. A master whose audio is already
    lossless should not collect a generation of AAC because the picture happened to need cutting.
    """
    suffix = (spec.output.suffix or ".mp4").lower()
    source_audio = (media.audio.codec or "").lower() if media.audio is not None else ""
    if suffix in {".mpg", ".mpeg", ".vob", ".ts", ".m2ts"}:
        return ["-c:a", "mp2", "-b:a", "384k"], "mp2, the only kind of sound that container holds"
    if source_audio.startswith("pcm"):
        return ["-c:a", "pcm_s16le"], "uncompressed PCM, as its source is"
    return (["-c:a", "aac", "-b:a", spec.audio_bitrate],
            f"aac at {spec.audio_bitrate}")


def _copy_body(media: MediaInfo, spec: TrimSpec, plan: TrimPlan, body: Path,
               log, cancel, progress=None) -> tuple[list[list[str]], int]:
    """Copy the middle of the segment, from one keyframe to the next, untouched.

    The packets are selected by **frame index** with PyAV, not by asking ffmpeg for a span of
    time. That distinction is the whole of this function, and it was measured before it was
    written:

    * ``ffmpeg -ss ... -t ... -c copy`` stops when the timestamp it is watching passes the
      duration it was given, and its answer moves in coarse steps. On a master whose frames
      are 33.3 ms apart, shortening the request by 5 ms moved the stop by **15 frames**; a
      request for 189 frames gave back 191, and on another master a request for 19250 gave
      back 19252, then 19249, then 19251 as the length was adjusted -- it never lands on the
      number asked for, so no arithmetic can make it. The delivered file was one frame long
      and the seam between the copied run and the re-encoded tail was a frame out.
    * Reading the container's own packets and writing exactly the ones wanted is exact by
      construction: 189 frames in, 189 packets byte-identical, on the same source.

    Picture only. The sound of this piece used to be copied separately and muxed back in,
    because asking for a stream copy of both streams at once makes ffmpeg seek the *audio* to
    its own sync point -- up to four AAC frames before the keyframe -- and
    ``-avoid_negative_ts make_zero`` then rebases the file on that earlier audio packet, so
    the body's picture began 80 ms into its own file and the segment's content landed about
    three frames behind the mark. The sound is now a single track for the whole segment
    (:func:`_encode_sound`), so that failure cannot recur *and* the joins cannot clip.
    """
    want = plan.body_frames
    # `plan.keyframe` is the container's own row number for the keyframe that opens the body, and
    # the copy counts rows from one. The row matching the in point's *time* is `in_frame + 2` on
    # both masters measured, so container row `N` corresponds to frame `N - 2` -- and the body's
    # rows are therefore `keyframe + 1` through `keyframe + body_frames`, which is the same as
    # frames `keyframe - 1` through `keyframe + body_frames - 2`.
    #
    # Measured on the Andy Ross master: the container's keyframe at or after the in point is row
    # 242, the body is 2250 frames, and rows 243..2492 are exactly those frames -- 2250 packets.
    # Taking `keyframe` itself (or `keyframe + 1` as the frame number) leaves the seam a frame
    # out, which showed up as "the re-encoded head sits -3 frames off".
    first = plan.keyframe + 1
    last = first + want - 1
    log(f"body        copying rows {first}..{last} from the original packets")
    # The copy is a packet read rather than a subprocess, so nothing else checks the token for
    # the length of it. A long body is the longest single operation in a trim, and without this
    # Cancel would sit dead for the whole of it.
    if cancel is not None and cancel.cancelled:
        raise ff.Cancelled("cancelled")
    try:
        _copy_packets(media.path, body, first, last, cancel)
    except ff.Cancelled:
        raise
    except Exception as error:                      # noqa: BLE001 - reported, not swallowed
        raise TrimError(
            f"could not copy frames {first}..{last} out of {media.path.name}: {error}"
        ) from error
    measured = ff.inspect(body).frames
    if measured != want:
        # Not fatal: the tail is re-encoded and its own length is pinned, so a body that is a
        # frame short leaves the segment a frame short rather than corrupt. Said plainly so it
        # is visible in the log instead of being absorbed silently.
        log(f"body        WARNING the copy holds {measured} frames of the {want} wanted")
    return [], measured


def _copy_packets(source: Path, body: Path, first: int, last: int, cancel=None) -> None:
    """Write the video packets of frames ``first..last`` inclusive into ``body``.

    The piece is rebased on its own start, because the packets carry the *source's*
    presentation times: a body taken from frame 250 opens at 4.166 s, and the concatenation
    would place it four seconds late. The first dts is the piece's own origin and every packet
    moves by it together, so the spacing between the frames is untouched.

    ``cancel`` is checked while demuxing, not only before it: this is the longest single
    operation in a trim on a long file, and reading it without a look at the token would make
    Cancel appear to do nothing for as long as it took.
    """
    with av.open(str(source)) as src:
        stream = src.streams.video[0]
        collected = _select_body_packets(stream, first, last, cancel)
        if not collected:
            raise ValueError(f"no packets found for frames {first}..{last}")
        # Rebased on the first *decode* timestamp, and written in the order they were read.
        origin = min(packet.dts for packet in collected)
        # No format pinned: PyAV takes it from the extension, which is the output's own. An MP4
        # cannot hold ProRes, and pinning it is how a perfectly legal all-intra source failed
        # with "'mp4' format does not support 'prores' codec".
        with av.open(str(body), "w") as dst:
            out_stream = dst.add_stream_from_template(stream)
            for packet in collected:
                packet.pts -= origin
                packet.dts -= origin
                packet.stream = out_stream
                dst.mux(packet)


def _select_body_packets(stream, first: int, last: int, cancel=None) -> list:
    """The packets that make up frames ``first..last``, in decode order.

    Frame numbers are the packet's own position in decode order -- the one numbering that cannot
    slide, because the container hands over exactly one video packet per frame. Deriving a number
    by dividing a timestamp is what produced every frame-numbering fault in this engine: two
    packets can share a presentation time, a file's frames need not sit evenly on its average
    rate, and a computed time drifts past the frame it names.

    The range runs from the keyframe at or before ``first``, so a decoder can start on it, and
    stops before the keyframe that opens the re-encoded tail, so that keyframe cannot appear in
    the segment twice.

    Counting frames and keeping one packet each was tried and is wrong: the reference master
    carries frames 86 and 87 at one presentation time, the second being the keyframe, and
    collapsing them dropped a real picture -- a 500-frame cut arrived six frames short.
    """
    # The frame number is the packet's **position in decode order**, counted from one. Not a
    # time and not a rounding: the container hands over exactly one video packet per frame, so
    # position is the only numbering that cannot drift. Deriving it as
    # `round(pts * rate) + 1` was still wrong even after all the other fixes -- measured on the
    # TY Gellasch master, a body of frames 48..85 came back with 37 packets, missing frame 49
    # and including frame 86, because the derived numbers do not have to line up with the
    # packets one for one.
    collected: list = []
    found_opening = False
    position = 0
    for seen, packet in enumerate(stream.container.demux(stream)):
        if seen % 2000 == 0 and cancel is not None and cancel.cancelled:
            raise Cancelled("cancelled")
        if packet.pts is None or packet.dts is None:
            # A flush packet carries the decoder's last frames out and holds no picture of its
            # own, so it does not advance the frame count.
            continue
        position += 1
        if not found_opening:
            # Packets in the opening GOP before the in point carry the references the first
            # wanted frames are coded against -- but they belong to the head's re-encode, so
            # they are dropped. The keyframe marks where a copy may begin.
            if packet.is_keyframe and position >= first:
                found_opening = True
                collected.append(packet)
            continue
        if position > last:
            break
        collected.append(packet)
    return collected


def _concat_list(pieces: list[Path], spans: list[Fraction]) -> str:
    """The concat list for the picture pieces, each with its length stated.

    Without the ``duration`` line the demuxer uses whatever duration a piece's container
    reports, and on these sources that is a few frames short of the piece's content -- which
    is exactly how the body ends up repeating the end of the head.

    The length is a ``Fraction`` and is rounded to the microsecond the demuxer is given. Six
    decimals is a microsecond; a master at 186525000/6217501 fps makes 8538 frames
    284.6000338 s, where the float division and the exact fraction can round to different
    microseconds -- and that line is what positions the next piece, so the difference shows up
    as an extra frame in the delivered file. An exact fraction rounds the same way for both.

    The last piece has no ``duration`` line: nothing follows it, so it would only be
    describing a length that the muxer measures for itself.
    """
    lines = ["ffconcat version 1.0"]
    for index, (piece, span) in enumerate(zip(pieces, spans)):
        lines.append(f"file '{piece.as_posix()}'")
        if index < len(pieces) - 1:
            exact = float(span)
            lines.append(f"duration {exact:.6f}")
    return "\n".join(lines) + "\n"


def _join(listing: Path, joined: Path, log, cancel, progress=None,
          expected_seconds: float | None = None) -> list[str]:
    args = [
        ff.tool("ffmpeg"), "-hide_banner", "-v", "error", "-y",
        "-f", "concat", "-safe", "0", "-i", str(listing),
        "-c", "copy", str(joined),
    ]
    log("join        concatenating the re-encoded ends and the copied middle")
    ff.run(args, cancel=cancel, log=log, progress=progress, expected_seconds=expected_seconds)
    return args


def _mux(picture: Path, sound: Path, output: Path, log, cancel, progress=None,
         expected_seconds: float | None = None) -> list[str]:
    """Put the finished picture and the one sound track together.

    A stream copy of both, so this pass cannot change a frame or a sample. ``-shortest`` is
    deliberately *not* used: it would truncate to whichever stream is shorter, and the whole
    point of encoding the sound in one piece is that the two end together on the frame that
    was asked for.
    """
    if not sound.exists():
        log("mux         the source has no audio, so the picture is the deliverable")
        shutil.move(str(picture), str(output))
        return []
    args = [
        ff.tool("ffmpeg"), *_common_input(),
        "-i", str(picture), "-i", str(sound),
        "-map", "0:v:0", "-map", "1:a:0",
        "-c", "copy", "-movflags", "+faststart", str(output),
    ]
    log("mux         joining the picture and the sound")
    ff.run(args, cancel=cancel, log=log, progress=progress, expected_seconds=expected_seconds)
    return args


def _reencode_args(media: MediaInfo, spec: TrimSpec, output: Path) -> list[str]:
    """Built apart from the run so the stream maps can be checked without media.

    They are the part that is easy to get wrong and impossible to get wrong loudly: a pass that
    maps no audio stream is a valid ffmpeg command that writes a silent file, and every other
    check -- frame count, duration, picture -- still passes. It shipped once like that.
    """
    encoder, extra = ENCODERS[media.codec.lower()]
    args = [
        ff.tool("ffmpeg"), *_common_input(),
        "-ss", f"{media.seconds_of(spec.in_frame):.6f}", "-i", str(media.path),
        "-t", f"{spec.frames / float(media.grid_rate):.6f}",
        *_video_map(media), *_audio_map(media),
        "-vf", "setpts=PTS-STARTPTS",
        "-c:v", encoder, "-preset", spec.preset, "-crf", str(spec.crf),
        "-pix_fmt", media.pix_fmt if media.pix_fmt in PASSTHROUGH_PIX_FMTS else "yuv420p",
        "-video_track_timescale", str(media.timebase.denominator), *extra,
    ]
    if media.audio is not None:
        args += ["-af", "asetpts=PTS-STARTPTS",
                 "-c:a", "aac", "-b:a", spec.audio_bitrate,
                 "-ar", str(media.audio.sample_rate), "-ac", str(media.audio.channels)]
    args += ["-movflags", "+faststart", str(output)]
    return args


def _reencode(media: MediaInfo, spec: TrimSpec, output: Path, log, cancel,
              progress=None, expected_seconds: float | None = None) -> list[str]:
    args = _reencode_args(media, spec, output)
    log("encode      no usable keyframe in the segment: re-encoding all of it")
    ff.run(args, cancel=cancel, log=log, progress=progress,
           expected_seconds=expected_seconds)
    return args


def default_output(source: Path, in_frame: int, out_frame: int, rate) -> Path:
    """``clip.mp4`` + a range -> ``clip 00.00.10.00-00.01.00.00.mp4`` (colons are not
    legal in Windows file names, so the timecode is dotted)."""
    start = format_timecode(in_frame, rate).replace(":", ".").replace(";", ".")
    end = format_timecode(out_frame, rate).replace(":", ".").replace(";", ".")
    return source.with_name(f"{source.stem} {start}-{end}{source.suffix}")
