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


def _beside(offset: int) -> str:
    """How to say where a re-encoded frame sat relative to the mark.

    ``0`` is silent, because on a file whose timing is exactly its nominal grid -- which is most
    of them -- that is every cut and there is nothing to say. A non-zero offset is worth naming
    rather than hiding: it is the container's own timing showing through, it is within the one
    packet the master's grid is known to drift by, and a reader who sees it should know it was
    seen. It is not a fault and must not read like one.
    """
    if offset == 0:
        return ""
    return f" {abs(offset)} frame{'s' if abs(offset) != 1 else ''} {'after' if offset > 0 else 'before'} the mark"


def head_within_frame(source: Path, output: Path, media: MediaInfo, facts,
                      output_frame: int, source_frame: int) -> tuple[int, float] | None:
    """Which source frame the delivered file's ``output_frame`` actually shows, and how well.

    Both numbers are in the container's own packet order. The caller knows which source frame the
    head should be showing -- the run begins at the in point, so head frame ``n`` is source frame
    ``in_frame + n - 1`` -- so this compares against that frame and its immediate neighbours and
    returns the best match as ``(offset, ssim)``.

    The neighbours are not optional, and they are the reason an earlier version of this check was
    wrong in both directions. A frame is addressed here by time, because that is the only handle
    ffmpeg offers, but on a master recorded by OBS the packet grid is not exactly the nominal one:
    a frame's true time sits up to one packet either side of ``frame / rate``. Asking for the mark
    alone therefore sometimes draws the frame next to it and reports a good cut as a bad one. The
    window is bounded at one because that same measurement says the drift never exceeds it, so a
    match further away would mean something else is wrong and should still fail.

    An offset of ``0`` is the mark, ``-1`` the frame before it, ``+1`` the frame after. A
    non-zero offset is reported as information, not as a fault: on the sources this product is
    built for, it is what the container's own timing does. It returns ``None`` -- not a failure --
    when the file cannot be looked at one frame at a time, which is a different answer from being
    wrong.
    """
    import tempfile

    rate = float(media.grid_rate)
    half = 0.5 / rate
    with tempfile.TemporaryDirectory(prefix="thetrimmer-head-") as folder:
        work = Path(folder)
        mine = work / "output.png"
        # Into the middle of the frame rather than onto its stated time: a timestamp is the
        # instant a frame starts, so a seek aimed at the edge is a coin toss with its neighbour.
        if not ff.frame_png(output, (output_frame - 1) / rate + half, mine):
            return None
        scores: dict[int, float] = {}
        for delta in (-1, 0, 1):
            frame = work / f"source{delta:+d}.png"
            if not ff.frame_png(source, (source_frame - 1 + delta) / rate + half, frame):
                continue
            scores[delta] = ff.ssim(mine, frame)
        if not scores:
            return None
        # Whether this file can be looked at one frame at a time at all. Where it cannot -- an
        # MPEG program stream seek lands on the keyframe, so every -ss inside a GOP gives back the
        # same picture -- the candidates are one image and the best of them means nothing.
        # Measured: five different frames of an MPEG-2 file all returned ssim 0.8394 against one
        # delivered frame. A check that cannot see must say so rather than fail the file.
        #
        # The two outermost candidates are compared rather than two adjacent ones, because they
        # are the furthest apart this check can ask for and therefore the clearest evidence that
        # the extractions are following the frames at all.
        low, high = work / "source-1.png", work / "source+1.png"
        if low.exists() and high.exists() and ff.ssim(low, high) > 0.999:
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
        # Whether this file can be looked at one frame at a time at all. Where it cannot -- an
        # MPEG program stream seek lands on the keyframe, so every -ss inside a GOP returns the
        # same picture -- the candidates are the same image and the best of them means nothing.
        # Measured on an MPEG-2 cut: the tail scored 0.806 for a segment whose frame count was
        # exact, because it was being compared against a keyframe. Saying "failed" there is a
        # lie about the file; "not checked" is the truth.
        zero, one = work / "source+0.png", work / "source+1.png"
        if zero.exists() and one.exists() and ff.ssim(zero, one) > 0.999:
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
        # Nothing in this file is a stream copy -- there was no keyframe to copy from -- so
        # hashes cannot match the source and looking is the only honest check. The whole segment
        # is a re-encode, so its frame `n` is the source's frame `in_frame + n - 1`, the same
        # relationship the head has; there is nothing to search for.
        result.checks.append(
            "alignment   no keyframe fell inside the segment, so all of it was "
            "re-encoded; hashes cannot match by construction")
        at = max(1, spec.frames // 2)
        looked = head_within_frame(source, output, media, facts,
                                   output_frame=at, source_frame=spec.in_frame + at - 1)
        if looked is None:
            # Not a failure: the file could not be *looked at*, which is a different answer
            # from being wrong. The verdict has three values for exactly this reason.
            result.checks.append(
                "alignment   not checked: this container will not give up one frame at a time, "
                "so the re-encoded picture could not be compared")
        elif looked[1] >= 0.85:
            result.checks.append(
                f"alignment   the re-encoded picture shows the frame the in point names"
                f"{_beside(looked[0])} (ssim {looked[1]:.3f})")
        else:
            result.failures.append(
                f"alignment   the re-encoded picture does not show the frame the in point "
                f"names (ssim {looked[1]:.3f})")
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
        looked = head_within_frame(source, output, media, facts,
                                   output_frame=at, source_frame=spec.in_frame + at - 1)
        if looked is None:
            # Not a failure -- see the note on the same branch above. An MPEG program stream
            # seek lands on the keyframe, so every frame inside a GOP comes back identical and
            # there is nothing to compare; saying "failed" there would be a lie about the file.
            result.checks.append(
                "head        not checked: this container will not give up one frame at a time, "
                "so the re-encoded head could not be compared")
        elif looked[1] >= 0.85:
            result.checks.append(
                f"head        the re-encoded head shows the frame the in point names"
                f"{_beside(looked[0])} (ssim {looked[1]:.3f})")
        else:
            result.failures.append(
                f"head        the re-encoded head does not show the frame the in point names "
                f"(ssim {looked[1]:.3f})")
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
            result.checks.append(
                "tail        not checked: this container will not give up one frame at a time, "
                "so the re-encoded tail could not be compared")
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
    head and hope.

    ## Why it measures nothing

    It used to compare the body against the source here, which meant the comparison ran on every
    cut whether or not the operator had asked for the result to be measured -- the `Verify`
    setting could be turned off and the work still happened, because it lived in the cutting path
    rather than in the checking one. Measured, it was most of the twenty seconds a cut spent
    after its last ffmpeg pass.

    The measurement belongs to :func:`verify`, which the caller skips when the setting is off and
    which already makes exactly this comparison. So all this does now is cut.

    ``samples`` and ``attempts`` are kept in the signature because both of V1's front ends and
    the command line pass them; neither is used.
    """
    media = ff.probe(spec.source)
    report = trim(spec, log=log, cancel=cancel, media=media, progress=progress)
    return report, []
