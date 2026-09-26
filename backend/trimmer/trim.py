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

import shutil
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

from . import ffmpeg as ff
from . import subtitles as subs
from .ffmpeg import CancelToken, MediaInfo
from .subtitles import RetimeResult
from .timecode import format_timecode

#: Source codec -> the encoder that can join it, and anything the muxer needs.
ENCODERS = {
    "h264": ("libx264", []),
    "hevc": ("libx265", ["-tag:v", "hvc1"]),
    "h265": ("libx265", ["-tag:v", "hvc1"]),
}
#: Pixel formats libx264/libx265 will accept straight through. Anything else (a 12-bit
#: or exotic source) is converted to the 8-bit 4:2:0 its codec family expects.
PASSTHROUGH_PIX_FMTS = {
    "yuv420p", "yuv422p", "yuv444p", "yuv420p10le", "yuv422p10le", "yuv444p10le",
}


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
    head_audio_copy: bool = False        # copy the head's audio instead of re-encoding
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
    #: The chosen keyframe's own presentation time, and the next keyframe's. They are here
    #: because a stream copy cannot be aimed at a *frame number*: ffmpeg seeks by time, and
    #: where it lands depends on the values below. See `body_seek`.
    keyframe_seconds: float | None = None
    next_keyframe_seconds: float | None = None

    @property
    def requested(self) -> int:
        return self.head_frames + self.body_frames

    def describe(self, rate) -> str:
        lines = [f"mode        {self.mode}"]
        if self.mode == "headpatch":
            lines.append(
                f"head        frames {self.head_frames} "
                f"({self.head_frames / float(rate):.3f}s) re-encoded from the in point "
                f"to keyframe {self.keyframe}"
            )
            lines.append(f"body        frames {self.body_frames} copied untouched")
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


def plan_trim(spec: TrimSpec, media: MediaInfo) -> TrimPlan:
    """Work out the split point, and refuse clearly when the source cannot be cut.

    Called before the trim so the CLI can show the plan and the GUI can react to an
    impossible request without starting ffmpeg.
    """
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
            f"the source is {media.codec}; the head-patch method needs the head to be the "
            "same codec as the body, and only H.264 and HEVC sources are supported. "
            "Re-encode the source first, or trim it with a plain ffmpeg re-encode."
        )

    notes: list[str] = []
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
    in_seconds = media.seconds_of(spec.in_frame)
    out_seconds = media.seconds_of(spec.out_frame)
    # Look a little before the in point (a keyframe exactly on it counts) and up to 30s
    # after: a GOP longer than that is not something the head-patch method should
    # silently re-encode its way through.
    window_end = min(out_seconds, in_seconds + 30.0)
    marks = ff.keyframes(media.path, max(0.0, in_seconds - 1.0), window_end)
    after = [t for t in marks if t >= in_seconds - 1e-6]

    if not after or round((after[0] - media.start_time) * float(rate)) >= spec.out_frame:
        notes.append(
            "no keyframe inside the segment: the whole segment is re-encoded, because a "
            "stream-copied body has to begin on a keyframe"
        )
        return TrimPlan(media, "reencode", -1, spec.frames, 0, notes)

    # The keyframe that opens the copied body, and the one after it. The gap between them is
    # the GOP, which is how far inside the keyframe a stream copy has to be aimed.
    opens = after[0]
    following = after[1] if len(after) > 1 else None

    # The frame a keyframe holds is measured from the source's **own** first timestamp, not
    # from zero. A master that starts at 0.021 s -- 0.63 of a frame at 30 fps -- puts every
    # keyframe a frame later than `pts * rate` says, so this read 13493 for a keyframe that is
    # really 13492, and the copied body began a frame early. `verify` has always measured from
    # `media.start_time`, so the two disagreed by exactly that frame and the alignment check
    # reported it on every sample.
    keyframe = round((opens - media.start_time) * float(rate))
    if keyframe == spec.in_frame:
        return TrimPlan(media, "copy", keyframe, 0, spec.frames, notes,
                        keyframe_seconds=opens, next_keyframe_seconds=following)

    offset = spec.concat_offset
    head_frames = keyframe - spec.in_frame - offset
    if head_frames <= 0:
        clamped = max(0, keyframe - spec.in_frame - 1)
        notes.append(
            f"the concat offset of {offset} frames leaves an empty head; clamped to "
            f"{clamped}"
        )
        head_frames = max(1, clamped)
        if head_frames == 0:
            return TrimPlan(media, "copy", keyframe, 0, spec.frames, notes,
                            keyframe_seconds=opens, next_keyframe_seconds=following)
    return TrimPlan(media, "headpatch", keyframe, head_frames, spec.out_frame - keyframe,
                    notes, keyframe_seconds=opens, next_keyframe_seconds=following)


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
    plan = plan_trim(spec, media)
    rate = media.grid_rate
    whole = spec.frames / float(rate)

    if spec.output.exists() and not spec.overwrite:
        raise TrimError(f"{spec.output.name} already exists (overwrite is off)")

    commands: list[list[str]] = []
    work = Path(keep_temp) if keep_temp else Path(tempfile.mkdtemp(prefix="thetrimmer-"))
    work.mkdir(parents=True, exist_ok=True)
    head = work / "head.mp4"
    body = work / "body.mp4"
    joined = work / "joined.mp4"
    try:
        log(f"source      {media.summary()}")
        log(plan.describe(rate))

        if plan.mode == "reencode":
            commands.append(_reencode(media, spec, spec.output, log, cancel, progress, whole))
        elif plan.mode == "copy":
            commands.append(_copy(media, spec, plan, spec.output, log, cancel, progress, whole))
        else:
            commands.append(_encode_head(media, spec, plan, head, log, cancel, progress))
            commands += _copy_body(media, spec, plan, body, log, cancel, progress)
            listing = work / "concat.txt"
            listing.write_text(_concat_list(head, body, plan.head_frames / float(rate)),
                               encoding="utf-8")
            commands.append(_join(listing, joined, log, cancel, progress, whole))
            shutil.move(str(joined), str(spec.output))
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
        log(f"tail        the copy stopped {report.overshoot} frame(s) past the out point; "
            f"a sequence's out point trims them")
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
    mapping = ["-map", "0:v:0"]
    if media.audio is not None:
        mapping += ["-map", "0:a:0"]
    return mapping


def _encode_head(media: MediaInfo, spec: TrimSpec, plan: TrimPlan, head: Path,
                 log, cancel, progress=None) -> list[str]:
    encoder, extra = ENCODERS[media.codec.lower()]
    pix_fmt = media.pix_fmt if media.pix_fmt in PASSTHROUGH_PIX_FMTS else "yuv420p"
    head_seconds = plan.head_frames / float(media.grid_rate)
    args = [
        ff.tool("ffmpeg"), *_common_input(),
        "-ss", f"{media.seconds_of(spec.in_frame):.6f}", "-i", str(media.path),
        "-t", f"{head_seconds:.6f}",
        *_video_map(media),
        "-vf", "setpts=PTS-STARTPTS",
        "-c:v", encoder, "-preset", spec.preset, "-crf", str(spec.crf),
        "-pix_fmt", pix_fmt, "-r", media.grid_rate_text,
        "-video_track_timescale", str(media.timebase.denominator),
        *extra,
    ]
    if media.audio is not None:
        if spec.head_audio_copy:
            args += ["-c:a", "copy"]
        else:
            args += ["-af", "asetpts=PTS-STARTPTS",
                     "-c:a", "aac", "-b:a", spec.audio_bitrate,
                     "-ar", str(media.audio.sample_rate), "-ac", str(media.audio.channels)]
    args += ["-movflags", "+faststart", str(head)]
    log(f"head        re-encoding frames {spec.in_frame}..{plan.keyframe - 1} "
        f"with {encoder} (crf {spec.crf}, {spec.preset})")
    ff.run(args, cancel=cancel, log=log, progress=progress,
           expected_seconds=head_seconds)
    return args


def body_seek(plan: TrimPlan, rate, frames: int) -> tuple[str, float]:
    """Where to aim a stream copy of the body, and how long to let it run.

    ## The defect this exists to fix

    With ``-ss`` before ``-i`` and ``-c copy``, ffmpeg begins the copy at the keyframe at or
    before the target and counts ``-t`` from the **target**. Aimed at a keyframe's own
    timestamp it takes the keyframe *before* it, so the copy comes out a whole GOP too long
    and its content sits a whole GOP early. On a 30 fps master with a 250-frame GOP that is
    8.33 seconds: the picture ends up 8.33 s ahead of the sound, and the alignment check
    finds none of its samples in the source because every one of them is 250 frames from
    where the marks say it is. Measured on real material, both halves of that.

    Aimed *inside* the GOP, ffmpeg takes the keyframe that opens it, and the preroll it
    writes is then exactly ``target - keyframe`` — a known quantity, which comes off ``-t``.
    The middle of the GOP is far from both the keyframe and the next one, so the choice is
    not near a boundary, and the measurements hold from a ninth of a GOP to two thirds of
    one.

    The result is one frame long rather than 250, which is the packet-boundary overshoot the
    product already reports and never fails on.
    """
    duration = frames / float(rate)
    opens, following = plan.keyframe_seconds, plan.next_keyframe_seconds
    if opens is None or following is None or following <= opens:
        # No GOP to aim inside, so the arithmetic the engine has always used is kept.
        return f"{plan.keyframe / float(rate):.6f}", duration
    preroll = (following - opens) / 2.0
    return f"{opens + preroll:.6f}", duration - preroll


def _copy_body(media: MediaInfo, spec: TrimSpec, plan: TrimPlan, body: Path,
               log, cancel, progress=None) -> list[list[str]]:
    """Copy the segment's tail from the keyframe to the out point.

    Two copies, not one, and the reason was found the hard way. Asking for a stream
    copy of both streams at once makes ffmpeg seek the *audio* to its own sync point,
    up to four AAC frames before the keyframe, and ``-avoid_negative_ts make_zero``
    then rebases the file on that earlier audio packet: the body's video begins 80 ms
    into its own file, and since the concatenation puts the body straight after the
    head, the whole segment's content ends up about three frames behind the mark.
    Copying the picture and the sound separately fixes it -- the picture starts on the
    keyframe, and the sound is cut with an *output* seek, which drops the packets
    before the mark instead of hunting for a sync point -- and only then are they muxed
    back together.
    """
    body_frames = spec.out_frame + 1 - plan.keyframe
    whole = body_frames / float(media.grid_rate)
    # The picture seeks inside the GOP and gives the preroll back, so the copy begins on the
    # keyframe that opens the body. The sound keeps the engine's output seek at the mark
    # itself: audio has no GOP, so it lands exactly and needs none of that care.
    start, duration = body_seek(plan, media.grid_rate, body_frames)
    # The sound begins at the keyframe's own presentation time, which is where the picture
    # begins, so the two are cut from the same instant rather than a frame apart.
    audio_start = (f"{plan.keyframe_seconds:.6f}" if plan.keyframe_seconds is not None
                   else f"{media.seconds_of(plan.keyframe):.6f}")
    commands: list[list[str]] = []

    video = body.with_name("body-picture.mp4")
    picture = [
        ff.tool("ffmpeg"), *_common_input(),
        "-ss", start, "-i", str(media.path), "-t", f"{duration:.6f}",
        "-map", "0:v:0", "-c", "copy", "-avoid_negative_ts", "make_zero", str(video),
    ]
    commands.append(picture)
    if media.audio is None:
        log(f"body        copying frames {plan.keyframe}.. from the original packets")
        ff.run(picture, cancel=cancel, log=log, progress=progress,
               expected_seconds=duration)
        shutil.move(str(video), str(body))
        return commands

    sound = body.with_name("body-sound.m4a")
    audio = [
        ff.tool("ffmpeg"), *_common_input(),
        "-i", str(media.path), "-ss", audio_start, "-t", f"{whole:.6f}",
        "-map", "0:a:0", "-c", "copy", str(sound),
    ]
    mux = [
        ff.tool("ffmpeg"), *_common_input(),
        "-i", str(video), "-i", str(sound),
        "-map", "0:v:0", "-map", "1:a:0",
        "-c", "copy", "-avoid_negative_ts", "make_zero", str(body),
    ]
    log(f"body        copying frames {plan.keyframe}.. from the original packets "
        f"(picture and sound separately)")
    for number, (step, span) in enumerate(
        ((picture, duration), (audio, whole), (mux, whole)), start=1
    ):
        # Each pass says how long it should take, so the bar measures the pass it is drawing.
        log(f"body        step {number} of 3")
        ff.run(step, cancel=cancel, log=log, progress=progress, expected_seconds=span)
    commands += [audio, mux]
    return commands


def _concat_list(head: Path, body: Path, head_seconds: float) -> str:
    """The concat list, with the head's duration stated rather than inferred.

    Without the ``duration`` line the demuxer uses whatever duration the head's
    container reports, and on these sources that is a few frames short of the head's
    content -- which is exactly how the body ends up repeating the end of the head.
    """
    return (
        "ffconcat version 1.0\n"
        f"file '{head.as_posix()}'\n"
        f"duration {head_seconds:.6f}\n"
        f"file '{body.as_posix()}'\n"
    )


def _join(listing: Path, joined: Path, log, cancel, progress=None,
          expected_seconds: float | None = None) -> list[str]:
    args = [
        ff.tool("ffmpeg"), "-hide_banner", "-v", "error", "-y",
        "-f", "concat", "-safe", "0", "-i", str(listing),
        "-c", "copy", "-movflags", "+faststart", str(joined),
    ]
    log("join        concatenating head and body")
    ff.run(args, cancel=cancel, log=log, progress=progress, expected_seconds=expected_seconds)
    return args


def _copy(media: MediaInfo, spec: TrimSpec, plan: TrimPlan, output: Path, log, cancel,
          progress=None, expected_seconds: float | None = None) -> list[str]:
    """The in point is a keyframe, so nothing needs re-encoding at all.

    The cut is bounded by *time*, one frame past the out point. Pinning the count with
    ``-frames:v`` looks tempting -- an MP4 holds one packet per video frame -- but with
    B-frames ``-frames:v`` counts packets in **decode** order, so it drops a frame near
    the end that has not been handed over yet and keeps one that comes after the out
    point. Measured on a 91-frame cut: the last frame kept was the source's 151st while
    the 150th that was asked for had gone. Time-based stops cannot do that.

    Aimed through :func:`body_seek`, because this is the same input-seek-and-copy that used
    to start a whole GOP early whenever the target was a keyframe's own timestamp.
    """
    start, duration = body_seek(plan, media.grid_rate, spec.frames + 1)
    args = [
        ff.tool("ffmpeg"), *_common_input(),
        "-ss", start, "-i", str(media.path),
        *_video_map(media), "-c", "copy",
        "-t", f"{duration:.6f}",
        "-movflags", "+faststart", str(output),
    ]
    log("copy        the in point is a keyframe: no re-encode at all")
    ff.run(args, cancel=cancel, log=log, progress=progress,
           expected_seconds=expected_seconds)
    return args


def _reencode(media: MediaInfo, spec: TrimSpec, output: Path, log, cancel,
              progress=None, expected_seconds: float | None = None) -> list[str]:
    encoder, extra = ENCODERS[media.codec.lower()]
    args = [
        ff.tool("ffmpeg"), *_common_input(),
        "-ss", f"{media.seconds_of(spec.in_frame):.6f}", "-i", str(media.path),
        "-t", f"{spec.frames / float(media.grid_rate):.6f}",
        *_video_map(media),
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
