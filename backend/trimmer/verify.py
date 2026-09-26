"""Checking the finished file, and cancelling the one thing that can still be off.

A head-patch cut is easy to get subtly wrong in a way nothing complains about: the file
plays, the duration looks right, and yet the content sits a few frames away from where
it was asked to be, because the concat demuxer started the copied body early. So the
app measures the result against the source instead of trusting the arithmetic.

The measurement uses ``framemd5``: the body is a stream copy, so its decoded frames are
identical to the source's, and a window of frames either matches the source exactly or
it does not. That gives the offset in whole frames with no tolerance to argue about and
no third-party libraries.
"""

from __future__ import annotations

import re
from collections.abc import Iterable
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path

from . import ffmpeg as ff
from . import subtitles as subs
from .ffmpeg import CancelToken, MediaInfo
from .timecode import format_seconds
from .trim import TrimReport, TrimSpec, trim

#: Frames compared when looking for a match, and frames searched either side.
WINDOW = 12
SEARCH = 6
#: Frames the file may run past the out point. A stream copy stops on a packet boundary
#: and the muxer's reorder buffer holds a frame or two, so a correct cut is usually one
#: to three frames long at the end. Counting packets to force exactness would drop a
#: frame that was asked for instead -- see trim._copy.
MAX_OVERSHOOT = 8
#: Samples measured at once. Each is an independent pair of ffmpeg calls waiting on the
#: disk, and three concurrent readers are past the point where more of them help.
MAX_WORKERS = 3


@dataclass
class OffsetCheck:
    """Where a window of the output's frames was found in the source."""

    at_frame: int
    offset: int | None
    rate: float = 0.0

    @property
    def ok(self) -> bool:
        return self.offset == 0

    @property
    def at_seconds(self) -> float:
        return self.at_frame / self.rate if self.rate else 0.0

    def describe(self) -> str:
        at = format_seconds(self.at_seconds)
        if self.offset is None:
            return f"at +{at}: no matching frames in the source (re-encoded region?)"
        if self.offset == 0:
            return f"at +{at}: frame-exact"
        direction = "later" if self.offset > 0 else "earlier"
        milliseconds = abs(self.offset) / self.rate * 1000 if self.rate else 0.0
        return (f"at +{at}: content sits {abs(self.offset)} frame(s) {direction} than "
                f"asked ({milliseconds:.0f} ms)")


@dataclass
class VerifyResult:
    checks: list[str] = field(default_factory=list)
    failures: list[str] = field(default_factory=list)
    offsets: list[OffsetCheck] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return not self.failures

    def report(self) -> str:
        lines = [f"  {check}" for check in self.checks]
        lines += [f"  FAIL {failure}" for failure in self.failures]
        lines.append("PASS" if self.ok else f"{len(self.failures)} check(s) failed")
        return "\n".join(lines)


def sample_frames(spec: TrimSpec, plan, media: MediaInfo, count: int = 3) -> list[int]:
    """Frames of the output at which to check alignment, counted from its start.

    Deliberately away from both ends: the first frames are the re-encoded head, whose
    pixels cannot match the source exactly, and the last may hold the body's tail
    overshoot before the exact-frame trim runs. On a short segment those margins would
    swallow every sample, so they shrink with it: a three-second cut still gets one.
    """
    rate = float(media.rate)
    margin = max(1, min(round(2.0 * rate), spec.frames // 8))
    first = max(plan.head_frames + margin, margin)
    # The last sample's window has to sit inside what was asked for: the frames past the
    # out point belong to the copy's packet boundary, not to the request, so matching
    # them against the source would fail for a file that is perfectly correct.
    last = spec.frames - WINDOW
    if last < first:
        if spec.frames <= 0:
            return []
        middle = max(0, min(spec.frames - 1, max(plan.head_frames, spec.frames // 2)))
        return [middle]
    if count == 1 or first == last:
        return [(first + last) // 2]
    step = (last - first) / (count - 1)
    return [first + round(step * index) for index in range(count)]


def measure_offset(source: Path, output: Path, in_frame: int, rate, at_frame: int, *,
                   window: int = WINDOW, search: int = SEARCH, source_start: float = 0.0,
                   output_start: float = 0.0) -> OffsetCheck:
    """Which source frame the output shows ``at_frame`` frames into the segment.

    Both windows are extracted on their file's own grid -- ``start + n / rate`` -- so
    the comparison is frame for frame rather than "the frame at time t", which two
    files whose first timestamps differ by a fraction of a frame would answer
    differently. ``offset = 0`` is what we want: the frame the output shows is the one
    the in point plus this frame number names. Positive means the output's content has
    run ahead of the source's, negative that it lags.
    """
    rate_value = float(rate)
    segment_start = output_start + at_frame / rate_value
    source_start_time = source_start + (in_frame + at_frame - search) / rate_value
    segment = ff.frame_md5s(output, segment_start, window)
    if len(segment) < window:
        return OffsetCheck(at_frame, None, rate_value)
    source_frames = ff.frame_md5s(source, source_start_time, window + 2 * search)
    for index in range(len(source_frames) - window + 1):
        if source_frames[index:index + window] == segment:
            return OffsetCheck(at_frame, index - search, rate_value)
    return OffsetCheck(at_frame, None, rate_value)


def measure_offsets(source: Path, output: Path, spec: TrimSpec, plan, media: MediaInfo,
                    count: int = 3, *, source_start: float | None = None,
                    output_start: float | None = None,
                    workers: int = MAX_WORKERS) -> list[OffsetCheck]:
    """Measure every sample, in parallel.

    Each sample is two ffmpeg calls waiting on a disk that is doing seeks, which is the
    one part of this app that is genuinely wait-bound, so they run together. They are
    independent -- each opens the files, hashes a window and exits -- and the results
    come back in sample order. With a single sample, or with ``workers=1``, the calls
    stay on this thread.
    """
    frames = sample_frames(spec, plan, media, count)
    if not frames:
        return []
    if source_start is None:
        source_start = media.start_time
    if output_start is None:
        output_start = ff.stream_start_time(output)

    def one(frame: int) -> OffsetCheck:
        return measure_offset(source, output, spec.in_frame, media.rate, frame,
                              source_start=source_start, output_start=output_start)

    if len(frames) == 1 or workers <= 1:
        return [one(frame) for frame in frames]
    with ThreadPoolExecutor(max_workers=min(workers, len(frames))) as pool:
        return list(pool.map(one, frames))


def head_frame_offset(source: Path, output: Path, in_frame: int, rate, at_frame: int,
                      search: int = 3, *, source_start: float | None = None,
                      output_start: float | None = None) -> tuple[int, float] | None:
    """Which source frame the *re-encoded* head shows, and how well it matches.

    The head cannot be compared by hash -- it is a fresh encode -- so this scores the
    output's frame against the source's frames either side of the mark and takes the
    best. Neighbouring frames of a moving picture score clearly lower, which is what
    makes the peak meaningful. Both extractions sit on their file's own frame grid, for
    the same reason as in :func:`measure_offset`.
    """
    import tempfile

    rate_value = float(rate)
    base = in_frame + at_frame
    if source_start is None:
        source_start = ff.stream_start_time(source)
    if output_start is None:
        output_start = ff.stream_start_time(output)
    with tempfile.TemporaryDirectory(prefix="thetrimmer-head-") as folder:
        work = Path(folder)
        segment = work / "segment.png"
        if not ff.frame_png(output, output_start + at_frame / rate_value, segment):
            return None
        scores: dict[int, float] = {}
        for delta in range(-search, search + 1):
            frame = work / f"source{delta:+d}.png"
            at = source_start + (base + delta) / rate_value
            if ff.frame_png(source, at, frame):
                scores[delta] = ff.ssim(segment, frame)
        if not scores:
            return None
        best = max(scores, key=lambda delta: scores[delta])
        return best, scores[best]


def check_subtitles(source_srt: Path, output_srt: Path, start: float,
                    end: float) -> tuple[list[str], list[str]]:
    """Check the retimed transcript against the one it came from.

    Captions fail quietly -- they play, they are simply in the wrong place -- so the
    written file is read back and compared cue by cue with the source shifted by the in
    point. Numbering is checked on the file's own text, because ``parse`` deliberately
    ignores it.
    """
    checks: list[str] = []
    failures: list[str] = []
    expected = subs.retime(subs.read(source_srt).cues, start, end)

    if not output_srt.exists():
        if expected.cues:
            failures.append(f"subtitles   {output_srt.name} is missing "
                            f"({len(expected.cues)} cues were expected)")
        else:
            checks.append("subtitles   no cue fell inside the segment, and none was "
                          "written")
        return checks, failures

    written = subs.read(output_srt)
    if len(written.cues) != len(expected.cues):
        failures.append(f"subtitles   {output_srt.name} has {len(written.cues)} cues, "
                        f"the segment holds {len(expected.cues)}")
    else:
        for index, (got, want) in enumerate(zip(written.cues, expected.cues, strict=True),
                                             start=1):
            if got.text != want.text:
                failures.append(f"subtitles   cue {index} reads {got.text[:40]!r}, "
                                f"expected {want.text[:40]!r}")
            elif abs(got.start - want.start) > 0.001 or abs(got.end - want.end) > 0.001:
                failures.append(
                    f"subtitles   cue {index} sits at "
                    f"{subs.format_stamp(got.start)}..{subs.format_stamp(got.end)}, "
                    f"expected {subs.format_stamp(want.start)}.."
                    f"{subs.format_stamp(want.end)}")

    length = end - start
    outside = [cue for cue in written.cues if cue.start < -0.001 or cue.end > length + 0.001]
    if outside:
        failures.append(f"subtitles   {len(outside)} cue(s) run past the segment "
                        f"(it is {length:.3f}s long)")
    if written.cues and written.cues[-1].end > length + 0.001:
        failures.append("subtitles   the last cue ends after the picture does")

    numbering = [block.splitlines()[0].strip() if block.splitlines() else ""
                 for block in re.split(r"\n\s*\n",
                                       output_srt.read_text(encoding="utf-8-sig",
                                                            errors="replace").strip())]
    if numbering != [str(index) for index in range(1, len(numbering) + 1)]:
        failures.append("subtitles   cues are not numbered 1..N without gaps")

    if not failures and written.cues:
        checks.append(
            f"subtitles   {len(written.cues)} cues from {source_srt.name}, shifted by "
            f"exactly {subs.format_stamp(start)}, first at "
            f"{subs.format_stamp(written.cues[0].start)}, last ends at "
            f"{subs.format_stamp(written.cues[-1].end)}")
    return checks, failures


def consensus(offsets: Iterable[OffsetCheck]) -> int | None:
    """The offset the checks agree on, or None when they are not usable.

    Several checks all reporting the same non-zero number is the concat drift; checks
    that disagree mean the timeline is not simply shifted, and nothing automated should
    paper over that.
    """
    values = [check.offset for check in offsets if check.offset is not None]
    if not values:
        return None
    if len(set(values)) > 1:
        return None
    return values[0]


def verify(source: Path, spec: TrimSpec, report: TrimReport, media: MediaInfo) -> VerifyResult:
    """Check the finished file: structure, length, and frame-for-frame alignment."""
    result = VerifyResult()
    output = report.output
    # One ffprobe call for frames, both durations and the picture's start time: the
    # checks below need all of them, and they used to be a launch each.
    facts = ff.inspect(output)
    frames = facts.frames
    video_duration = facts.video_duration
    audio_duration = facts.audio_duration

    if spec.frames <= frames <= spec.frames + MAX_OVERSHOOT:
        if frames == spec.frames:
            result.checks.append(f"frames      {frames}, exactly as asked")
        else:
            result.checks.append(
                f"frames      {frames}, {frames - spec.frames} past the out point "
                "(a stream copy stops on a packet boundary, and counting packets with "
                "-frames:v would drop a frame instead)")
    else:
        result.failures.append(
            f"frames      {frames}, expected {spec.frames}..{spec.frames + MAX_OVERSHOOT}")
    tolerance = 4.0 / float(media.rate)
    expected = spec.frames / float(media.rate)
    if abs(video_duration - expected) <= tolerance:
        gap = (video_duration - expected) * float(media.rate)
        result.checks.append(
            f"duration    video {video_duration:.3f}s of {expected:.3f}s requested "
            f"({gap:+.2f} frame(s) of trailing hold)")
    else:
        result.failures.append(
            f"duration    video {video_duration:.3f}s, expected {expected:.3f}s")
    if media.audio is not None:
        if audio_duration < 0:
            result.failures.append("audio       the segment has no audio stream")
        elif abs(audio_duration - video_duration) <= 0.25:
            result.checks.append(f"audio       {audio_duration:.3f}s, level with the "
                                 f"picture")
        else:
            result.failures.append(
                f"audio       {audio_duration:.3f}s against {video_duration:.3f}s of "
                "picture: sound and picture end apart")
    else:
        result.checks.append("audio       the source has none, and neither does the cut")

    if report.plan.mode == "reencode":
        # Nothing in this file is a stream copy -- there was no keyframe to copy from --
        # so hashes cannot match the source and looking is the only honest check.
        looked = head_frame_offset(source, output, spec.in_frame, media.rate,
                                   at_frame=spec.frames // 2,
                                   source_start=media.start_time,
                                   output_start=facts.video_start)
        result.checks.append(
            "alignment   no keyframe fell inside the segment, so all of it was "
            "re-encoded; hashes cannot match by construction")
        if looked is None:
            result.failures.append("alignment   could not read the re-encoded segment")
        else:
            delta, score = looked
            if abs(delta) <= 1 and score >= 0.85:
                result.checks.append(
                    f"alignment   the re-encoded picture is on the mark "
                    f"({delta:+d} frame, ssim {score:.3f})")
            else:
                result.failures.append(
                    f"alignment   the re-encoded picture sits {delta:+d} frame(s) off "
                    f"(ssim {score:.3f})")
        _check_subtitles(result, spec, output, media)
        return result

    result.offsets = measure_offsets(source, output, spec, report.plan, media,
                                     source_start=media.start_time,
                                     output_start=facts.video_start)
    for check in result.offsets:
        if check.ok:
            result.checks.append("alignment   " + check.describe())
        else:
            result.failures.append("alignment   " + check.describe())
    head_seconds = report.plan.head_frames / float(media.rate)
    # The head is a re-encode, so it is checked by looking, not by hashing: the frame
    # the segment shows a little way in must be the frame the in point plus that time
    # names, within one frame.
    if head_seconds >= 0.3:
        looked = head_frame_offset(source, output, spec.in_frame, media.rate,
                                   at_frame=max(1, report.plan.head_frames // 2),
                                   source_start=media.start_time,
                                   output_start=facts.video_start)
        if looked is None:
            result.failures.append("head        could not read the re-encoded head")
        else:
            delta, score = looked
            if abs(delta) <= 1 and score >= 0.85:
                result.checks.append(
                    f"head        the re-encoded head is on the mark "
                    f"({delta:+d} frame, ssim {score:.3f})")
            else:
                result.failures.append(
                    f"head        the re-encoded head sits {delta:+d} frame(s) off "
                    f"(ssim {score:.3f})")
        result.checks.append(
            f"head length the first {head_seconds:.3f}s are a crf-{spec.crf} re-encode, "
            "as designed; every frame after them is the original packet data"
        )

    if spec.subtitles is not None:
        _check_subtitles(result, spec, output, media)
    return result


def _check_subtitles(result: VerifyResult, spec: TrimSpec, output: Path,
                     media: MediaInfo) -> None:
    """Fold the transcript's checks into the report, whichever way the cut was made."""
    source_srt = spec.subtitles
    if source_srt is None:
        return
    checks, failures = check_subtitles(
        source_srt,
        output.with_suffix(".srt"),
        media.seconds_of(spec.in_frame),
        media.seconds_of(spec.out_frame),
    )
    result.checks += checks
    result.failures += failures


def trim_with_calibration(
    spec: TrimSpec,
    *,
    log=print,
    cancel: CancelToken | None = None,
    samples: int = 2,
    attempts: int = 2,
) -> tuple[TrimReport, list[OffsetCheck]]:
    """Trim, measure, and re-cut once if the body landed off the mark.

    Returns the report of the file that was kept, and the alignment checks for it. The
    correction is applied to ``concat_offset``: the measurement says how far ahead or
    behind the content is, and the head absorbs exactly that many frames.
    """
    media = ff.probe(spec.source)
    last_offsets: list[OffsetCheck] = []
    for attempt in range(1, attempts + 1):
        report = trim(spec, log=log, cancel=cancel, media=media)
        # The output's timestamps are read inside the measurement, once per attempt: the
        # file has just been rewritten. The source's come from the probe above.
        last_offsets = measure_offsets(spec.source, spec.output, spec, report.plan, media,
                                       count=samples, source_start=media.start_time)
        for check in last_offsets:
            log("alignment   " + check.describe())
        measured = consensus(last_offsets)
        if measured is None:
            return report, last_offsets
        if measured == 0:
            return report, last_offsets
        if attempt == attempts:
            log(f"alignment   still {measured:+d} frame(s) out after {attempt} attempts; "
                "keeping the last cut")
            return report, last_offsets
        # The content is `measured` frames ahead of the mark, so the head has to give
        # that many frames back: a shorter head pushes the copied body later.
        correction = -measured
        log(f"calibration content is {measured:+d} frame(s) off; re-cutting with "
            f"concat offset {spec.concat_offset} -> {spec.concat_offset + correction}")
        spec = TrimSpec(**{**spec.__dict__, "concat_offset": spec.concat_offset + correction})
    return report, last_offsets
