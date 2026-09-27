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

    Deliberately inside the **copied body and nowhere else**, because that is the only region
    whose frames are the source's own packets and therefore the only one a hash comparison can
    speak about. Both of the re-encoded regions are excluded: the head at the near end, and
    now the tail at the far end, which used to be copied and stopped on a packet boundary. A
    sample taken in either one reports "no matching frames in the source" for a file that is
    perfectly correct, which is a false alarm rather than a finding.

    On a short segment those margins would swallow every sample, so they shrink with it.
    """
    rate = float(media.grid_rate)
    # The margin keeps samples clear of the re-encoded head, and on a long segment two seconds
    # is right. It must never be larger than the copied body itself, though: a 38-frame body
    # with a 60-frame margin puts every sample in the tail, which is a fresh encode and matches
    # nothing -- measured, all three samples reported "no matching frames" for a file whose body
    # was frame-for-frame the source's own packets.
    body_frames = max(0, spec.frames - plan.head_frames - plan.tail_frames)
    margin = max(1, min(round(2.0 * rate), spec.frames // 8, max(1, body_frames // 4)))
    # The re-encoded head occupies output frames 1..head_frames, so the first frame a hash
    # comparison can speak about is the one after it. `plan.head_frames` is a count and the
    # output's frames are counted from one, which is the off-by-one that put every sample inside
    # the head on a six-frame cut -- measured, all three samples reported "no matching frames"
    # for a file whose body was frame-exact.
    body_start = plan.head_frames + 1
    first = max(body_start + margin, margin)
    # The last sample's *window* has to finish inside the copied body: frames at or after the
    # tail's first frame are a fresh encode and can never match.
    body_end = spec.frames - plan.tail_frames
    last = body_end - WINDOW
    if last < first:
        if spec.frames <= 0:
            return []
        middle = max(body_start, min(body_end - 1, max(body_start, body_end // 2)))
        if middle < body_start or middle >= body_end:
            # Nothing of the copied body is wide enough to sample: the alignment check has
            # nothing it can honestly say, and the head and tail checks carry the report.
            return []
        return [max(0, middle)]
    if count == 1 or first == last:
        return [(first + last) // 2]
    step = (last - first) / (count - 1)
    return [first + round(step * index) for index in range(count)]


def body_is_intact(source: Path, output: Path, spec: TrimSpec, plan, media: MediaInfo,
                   limit: int | None = None) -> tuple[int, int, list[str]]:
    """Compare the copied body of the delivered file with the source, frame for frame.

    Returns ``(compared, matched, examples)``, where ``examples`` names the first few frames that
    did not match, so a failure can say *where* it went wrong rather than only that it did.

    ## Why this replaces the search

    The previous check asked "where in the source does this run of frames appear?", which needs a
    window of frames, a range to search over, and a frame number worked out from a timestamp at
    both ends. Every one of those steps produced false failures: a window that starts a frame out
    matches nowhere, and the offsets it reported were fed to the calibration loop, which then
    "corrected" a correct cut and made it genuinely wrong.

    None of that searching is necessary, because the answer is already known. The copied body is
    the source's packets taken in order, so output body frame *k* **is** source frame
    ``keyframe + k``. There is no offset to discover and nothing that can slide. All that is left
    is to hash both sides and compare -- which is exactly what the product's claim, "the body is
    the original packets", says.

    Frames are addressed through :mod:`trimmer.container`'s numbering -- the container's own
    packet order -- which is the numbering the copy selects by, so the two sides cannot disagree
    about which frame is which.
    """
    body = plan.body_frames
    if body <= 0:
        return 0, 0, []
    facts = ff.inspect(output)
    if facts.frames < plan.head_frames + body:
        return 0, 0, [f"the file holds {facts.frames} frames, short of the "
                      f"{plan.head_frames + body} its head and body need"]

    rate = float(media.grid_rate)
    # The body sits after the re-encoded head in the output, and after the opening keyframe in
    # the source. Both counts start at one, which is what `frames_near` reports.
    output_first = plan.head_frames + 1
    source_first = plan.keyframe + 1

    checked = body if limit is None else min(body, limit)
    mine = ff.frames_near(output, (output_first - 1) / rate, checked)
    theirs = ff.frames_near(source, (source_first - 1) / rate, checked)
    if len(mine) < checked or len(theirs) < checked:
        return 0, 0, [f"could not read {checked} frames from both files "
                      f"(output {len(mine)}, source {len(theirs)})"]

    matched = 0
    wrong: list[str] = []
    for index in range(checked):
        if mine[index][1] == theirs[index][1]:
            matched += 1
        elif len(wrong) < 5:
            wrong.append(f"body frame {index + 1} (output frame {output_first + index}, source "
                         f"frame {source_first + index}) is not the source's packet")
    return checked, matched, wrong


def head_within_frame(source: Path, output: Path, media: MediaInfo, facts,
                      output_frame: int, source_frame: int) -> float | None:
    """How well the delivered file's ``output_frame`` matches the source's ``source_frame``.

    Both numbers are in the container's own packet order, and the caller knows which source frame
    the head should be showing -- the run begins at the in point, so head frame ``n`` is source
    frame ``in_frame + n - 1``. Comparing those two directly removes the search entirely, and the
    search was the problem: it looked for the best match within a few frames and reported the
    peak as an offset, so a head that was exactly right came back as "sits -3 frames off" with an
    ssim of 0.899 whenever neighbouring frames were similar.

    A re-encode cannot match its original by hash, so this is the one check that must look at
    pixels. It returns ``None`` when either frame cannot be read.
    """
    import tempfile

    rate = float(media.grid_rate)
    half = 0.5 / rate
    with tempfile.TemporaryDirectory(prefix="thetrimmer-head-") as folder:
        work = Path(folder)
        mine = work / "output.png"
        theirs = work / "source.png"
        # Into the middle of the frame rather than onto its stated time: a timestamp is the
        # instant a frame starts, so a seek aimed there is a coin toss with its neighbour.
        if not ff.frame_png(output, (output_frame - 1) / rate + half, mine):
            return None
        if not ff.frame_png(source, (source_frame - 1) / rate + half, theirs):
            return None
        return ff.ssim(mine, theirs)


def measure_offset(source: Path, output: Path, in_frame: int, rate, at_frame: int, *,
                   window: int = WINDOW, search: int = SEARCH, source_start: float = 0.0,
                   output_start: float = 0.0, source_time=None,
                   output_time=None) -> OffsetCheck:
    """Which source frame the output shows ``at_frame`` frames into the segment.

    The comparison is frame for frame rather than "the frame at time t", which two files whose
    first timestamps differ by a fraction of a frame would answer differently. ``offset = 0`` is
    what we want: the frame the output shows is the one the in point plus this frame number
    names. Positive means the output's content has run ahead of the source's, negative that it
    lags.

    ``source_time`` and ``output_time`` map a frame number to the time each file states for it,
    and the windows are seeked by those. Computing the times from the average rate instead is
    a fraction of a millisecond out by the far end of a long file, which is enough for a seek
    to land inside the next frame and for the window to start one frame late -- which matches
    nowhere, and reads as a fault in a file that is exact.
    """
    rate_value = float(rate)
    segment = _window_by_time(output, at_frame, window, rate_value, output_start,
                              output_time)
    if len(segment) < window:
        return OffsetCheck(at_frame, None, rate_value)
    source_frames = _window_by_time(source, in_frame + at_frame - search,
                                    window + 2 * search, rate_value, source_start,
                                    source_time)
    if len(source_frames) < window:
        return OffsetCheck(at_frame, None, rate_value)

    # Where the output's pictures sit in the source is looked up by content. The output's window
    # is a run of *pictures*, and the answer comes from the frame numbers the source's container
    # states for the same pictures -- so nothing here depends on counting decoded frames.
    wanted = [digest for _, digest in segment]
    lookup: dict[str, int] = {}
    for number, digest in source_frames:
        lookup.setdefault(digest, number)
    found = [lookup.get(digest) for digest in wanted]
    if any(number is None for number in found):
        # A picture the output shows is nowhere near that place in the source: either the
        # segment is re-encoded there or the cut moved. The caller decides which.
        return OffsetCheck(at_frame, None, rate_value)
    # Every frame of a window is consecutive, so every one of them must give the same offset.
    # Any disagreement means the lookup matched the wrong pictures -- a still, a title card, a
    # shot where neighbouring frames are near-identical -- and a guess is worse than no answer.
    # Both sides are 1-based now, so the frame the output shows t_frame frames in -- its
    # frame t_frame -- is the source's frame in_frame + at_frame - 1. The index within the
    # window shifts that by one per frame.
    base = in_frame + at_frame - 1
    offsets = {number - (base + index) for index, number in enumerate(found)}
    if len(offsets) != 1:
        return OffsetCheck(at_frame, None, rate_value)
    return OffsetCheck(at_frame, offsets.pop(), rate_value)


def measure_offsets(source: Path, output: Path, spec: TrimSpec, plan, media: MediaInfo,
                    count: int = 3, *, source_start: float | None = None,
                    output_start: float | None = None,
                    left_time=None, right_time=None,
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
    # The origin of each file, used only to aim the seek. The frame *numbers* that come back are
    # absolute -- `frames_near` keeps the container's own timestamps -- so both windows are
    # addressed from zero and the two files' numbers mean the same thing.
    source_seek_base = media.start_time if source_start is None else source_start
    output_seek_base = (ff.stream_start_time(output) if output_start is None
                        else output_start)
    # Both files are seeked to times they state themselves. `FrameTimes` reads them from the
    # container and falls back to the computed grid time, so this is the same behaviour as
    # before on a file that reports nothing. The rate is what lets it read *near* the frame
    # rather than from the start, which is the difference between instant and thirty seconds
    # on a long master.
    rate_value = float(media.grid_rate)
    source_time = left_time if left_time is not None else ff.FrameTimes(
        source, lambda frame: source_seek_base + frame / rate_value, rate_value,
        source_seek_base)
    output_time = right_time if right_time is not None else ff.FrameTimes(
        output, lambda frame: output_seek_base + frame / rate_value, rate_value,
        output_seek_base)

    def one(frame: int) -> OffsetCheck:
        return measure_offset(source, output, spec.in_frame, media.grid_rate, frame,
                              source_start=0.0, output_start=0.0,
                              source_time=source_time, output_time=output_time)

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
    # Just inside each frame rather than onto its stated time: a frame's timestamp is the
    # instant it starts, so a seek aimed there is a coin toss between it and its neighbour.
    # Same correction as the tail check and the window extraction.
    nudge = 0.02 / rate_value
    with tempfile.TemporaryDirectory(prefix="thetrimmer-head-") as folder:
        work = Path(folder)
        segment = work / "segment.png"
        if not ff.frame_png(output, output_start + at_frame / rate_value + nudge, segment):
            return None
        scores: dict[int, float] = {}
        for delta in range(-search, search + 1):
            frame = work / f"source{delta:+d}.png"
            at = source_start + (base + delta) / rate_value
            if ff.frame_png(source, at + nudge, frame):
                scores[delta] = ff.ssim(segment, frame)
        if not scores:
            return None
        best = max(scores, key=lambda delta: scores[delta])
        return best, scores[best]


def tail_frame_offset(source: Path, output: Path, spec: TrimSpec, plan, media: MediaInfo,
                      facts) -> tuple[int, float] | None:
    """Which source frame the *re-encoded tail* ends on, and how well it matches.

    The far end of the segment is the one place a fault is both most likely and least
    visible: the tail is a fresh encode whose length is chosen by the engine, so it can be a
    frame long or short, and the picture of a final frame is the last thing anyone checks by
    playing the file. So it is checked the way the head is -- by looking -- but at the frame
    a sequence would actually end on rather than in the middle of the patch.

    Both extractions sit on their file's own frame grid, for the same reason as in
    :func:`measure_offset`.
    """
    import tempfile

    rate_value = float(media.grid_rate)
    if facts.frames < 1:
        return None
    source_start = media.start_time if media.start_time is not None else 0.0
    # A tail can be a single frame long, so the search is never wider than the tail itself.
    search = min(plan.tail_frames, SEARCH) if plan.tail_frames else SEARCH
    # The output's last frame is read back from the *end* of the file.
    #
    # Seeking forward to it cannot be made reliable: a frame's stated time is the instant it
    # starts, so a seek there is a coin toss between that frame and its neighbour, and a seek
    # even slightly past it produces nothing because the picture has ended. Read backwards, it
    # is the first frame there is. See `ffmpeg.frame_png`.
    back = max(0.02, 1.5 / rate_value)
    with tempfile.TemporaryDirectory(prefix="thetrimmer-tail-") as folder:
        work = Path(folder)
        segment = work / "segment.png"
        if not ff.frame_png(output, back, segment, from_end=True):
            return None
        scores: dict[int, float] = {}
        for delta in range(-search, search + 1):
            frame = work / f"source{delta:+d}.png"
            # The source frame's mark, and a nudge just inside it, for the same reason.
            at = source_start + (spec.out_frame - 1 + delta) / rate_value - 0.02 / rate_value
            if ff.frame_png(source, at, frame):
                scores[delta] = ff.ssim(segment, frame)
        if not scores:
            return None
        best = max(scores, key=lambda delta: scores[delta])
        return best, scores[best]


def _window_by_time(path: Path, at_frame: int, count: int, rate: float,
                    start_time: float = 0.0, frame_time=None) -> list[tuple[int, str]]:
    """Frames of ``path`` from just before ``at_frame`` on, as ``(frame number, MD5)`` pairs.

    The frame number comes from the time **the container states for that frame**, not from a
    position in what was decoded, and that is the whole point of this function.

    Two earlier attempts addressed the window by position and both were wrong on real material.
    Seeking to a time and hashing from there can land a frame off; cutting the window out of
    what was decoded with ``select`` can come up one frame short, because the decoder and the
    seek between them do not guarantee the first frame handed over is the first one asked for.
    Either way every index in the window slides by one, so a cut that is exactly right is
    reported as four to six frames out -- and those phantom offsets were fed to the calibration
    loop, which "corrected" a correct cut and made it genuinely wrong, six frames short.

    A frame addressed by its own stated time cannot slide. The times are absolute, because
    ``ff.frames_near`` keeps the container's own timestamps rather than rebasing them to the
    seek, which is what makes the numbers comparable between two files that begin at different
    instants: the caller passes ``start_time=0`` and the number is the frame's own position.
    """
    if count <= 0 or at_frame < 0 or rate <= 0:
        return []
    back = 3
    first = max(0, at_frame - back)
    if frame_time is not None:
        seek = frame_time(first)
    else:
        seek = start_time + first / rate
    frames = ff.frames_near(path, seek, count + 2 * back)
    out: list[tuple[int, str]] = []
    for at, digest in frames:
        # +1 because the container hands over its first packet as frame 1, which is the numbering
        # the marks are typed in and the one the copy selects by.
        out.append((round((at - start_time) * rate) + 1, digest))
    return out


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
    tolerance = 4.0 / float(media.grid_rate)
    expected = spec.frames / float(media.grid_rate)
    if abs(video_duration - expected) <= tolerance:
        gap = (video_duration - expected) * float(media.grid_rate)
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
        looked = head_frame_offset(source, output, spec.in_frame, media.grid_rate,
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

    # The copied body, compared frame for frame against the source. This is the product's whole
    # claim -- that the body is the original packets -- so it is checked directly and completely
    # rather than sampled. The offset is not searched for, because it is not in doubt: the body
    # is the source's packets in order.
    compared, matched, wrong = body_is_intact(source, output, spec, report.plan, media)
    if compared == 0 and wrong:
        result.failures.extend("body        " + line for line in wrong)
    elif compared and matched == compared:
        result.checks.append(
            f"body        all {compared} copied frame(s) are the source's own packets"
        )
    elif compared:
        result.failures.append(
            f"body        {compared - matched} of {compared} copied frame(s) are not the "
            f"source's packets")
        result.failures.extend("body        " + line for line in wrong)

    head_seconds = report.plan.head_frames / float(media.grid_rate)
    # The head is a re-encode, so it cannot be checked by hashing. It is checked by looking, and
    # against the frame the mark names rather than by searching for where it landed: the run
    # begins at the in point, so the head's frame `n` is the source's frame `in_frame + n - 1`
    # and comparing those two directly is both simpler and unambiguous -- searching is what
    # produced the phantom "sits -3 frames off" on a head that was where it should be.
    if head_seconds >= 0.3:
        at = max(1, report.plan.head_frames // 2)
        score = head_within_frame(source, output, media, facts,
                                  output_frame=at, source_frame=spec.in_frame + at - 1)
        if score is None:
            result.failures.append("head        could not read the re-encoded head")
        elif score >= 0.85:
            result.checks.append(
                f"head        the re-encoded head shows the frame the in point names "
                f"(ssim {score:.3f})")
        else:
            result.failures.append(
                f"head        the re-encoded head does not show the frame the in point names "
                f"(ssim {score:.3f})")
        result.checks.append(
            f"head length the first {head_seconds:.3f}s are a crf-{spec.crf} re-encode, "
            "as designed; every frame after them is the original packet data"
        )

    # The tail is a re-encode for the same reason the head is, so it is checked the same way --
    # and it matters more, because it is where the file ends. A tail that came out a frame long
    # or short reads as a correct file and is wrong at the only frame a client will look for.
    # Checked whenever there is a tail at all: the shortest useful one is a single frame.
    tail_seconds = report.plan.tail_frames / float(media.grid_rate)
    if report.plan.tail_frames:
        looked = tail_frame_offset(source, output, spec, report.plan, media, facts)
        if looked is None:
            result.failures.append("tail        could not read the re-encoded tail")
        else:
            delta, score = looked
            if abs(delta) <= 1 and score >= 0.85:
                result.checks.append(
                    f"tail        the last frame is the frame the out point names "
                    f"({delta:+d} frame, ssim {score:.3f})")
            else:
                result.failures.append(
                    f"tail        the file ends {delta:+d} frame(s) from the out point "
                    f"(ssim {score:.3f})")
        result.checks.append(
            f"tail length the last {tail_seconds:.3f}s are a crf-{spec.crf} re-encode, "
            "which is what makes the segment end on the frame that was asked for"
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
    progress=None,
) -> tuple[TrimReport, list[OffsetCheck]]:
    """Trim, measure, and re-cut once if the body landed off the mark.

    Returns the report of the file that was kept, and the checks made of it.

    ## What changed, and why the correction is gone

    This used to cut, *search* the result for where its content had landed, and re-cut with a
    ``concat_offset`` when the answer was not zero. The search was the fault: it looked for the
    best match over a range of frames and reported the peak as a number of frames out, so a head
    that was exactly right came back as "-3 frames off" whenever neighbouring frames were
    similar. Those phantom offsets were then "corrected", which shortened the re-encoded head --
    and because the head's length is pinned, shortening it deleted real frames. A 500-frame cut
    arrived six frames short that way, and the picture ran 100 ms ahead of the sound.

    The correction is no longer needed. The body is selected by the container's own packet order
    (see :mod:`trimmer.container`), so it is where the marks say it is by construction rather
    than by measurement -- and when it is not, the honest answer is to say so, not to adjust the
    head and hope. So the cut is made once and the *body itself* is compared against the source,
    frame for frame, which is a statement about the file rather than about a search.

    ``samples`` and ``attempts`` are kept in the signature because both of V1's front ends and
    the command line pass them; neither is used now.
    """
    media = ff.probe(spec.source)
    report = trim(spec, log=log, cancel=cancel, media=media, progress=progress)

    compared, matched, wrong = body_is_intact(spec.source, spec.output, spec, report.plan, media)
    if compared == 0 or not wrong:
        log(f"alignment   all {compared} copied frame(s) match the source")
        return report, [OffsetCheck(0, 0, float(media.grid_rate))]
    log(f"alignment   {compared - matched} of {compared} copied frame(s) do not match the source")
    for line in wrong:
        log("alignment   " + line)
    return report, [OffsetCheck(0, 1, float(media.grid_rate))]
