"""Where the speech and the dead air are, measured from the master's own sound.

An hour-long master is mostly not speech, and the first question asked of it is where the takes
are and where the dead air is. ``silencedetect`` answers that from one audio decode -- no video
decode, no transcript -- so it works on music and screen recordings too, and on a long file it
is the cheap half of anything that would otherwise decode the picture.

Every value here is seconds on the **source** timeline, rounded to milliseconds. That is not a
presentation detail: the strip, the snap and the log all do arithmetic on these numbers, and an
unrounded value prints as ``0.999958`` and compares unpredictably with the mark it is supposed to
contain. A millisecond is still finer than a frame at 120 fps, so nothing visible is lost.
"""

from __future__ import annotations

import math
import re
from dataclasses import dataclass
from pathlib import Path

from . import ffmpeg as ff

#: A silent run narrower than this is not drawn. On an hour-long master the strip is a few hundred
#: pixels wide, so 20 ms is under a pixel: drawn honestly it aliases into a hairline seam through
#: the speech fill, which reads as a rendering fault rather than as a pause. The measurement keeps
#: such a run; only :func:`speech` joins across it.
HAIRLINE = 0.020

#: ``[silencedetect @ 0x...] silence_start: 1.234``. The prefix is deliberately not matched: the
#: address changes every run, and a saved log may have had it stripped.
_START = re.compile(r"silence_start:\s*(-?\d+(?:\.\d+)?)")
#: The end line carries its own length, which is what makes an out-of-order log pairable at all.
_END = re.compile(
    r"silence_end:\s*(-?\d+(?:\.\d+)?)"
    r"(?:\s*\|\s*silence_duration:\s*(-?\d+(?:\.\d+)?))?"
)
#: ``Duration: 01:04:03.20`` from the input header ffmpeg prints before it decodes anything.
_LOGGED_DURATION = re.compile(r"Duration:\s*(\d+):(\d{2}):(\d{2}(?:\.\d+)?)")

#: What ffmpeg says when the source has no stream for the output it was asked for -- which for
#: ``-vn -af`` means no audio at all. It is a fact about the source, not a failed measurement.
_NO_AUDIO = ("does not contain any stream", "matches no streams")


@dataclass(frozen=True)
class Span:
    """A run of time on the source's clock, in seconds."""

    start: float
    end: float

    @property
    def duration(self) -> float:
        """How long the run lasts.

        Never negative: an inverted span is malformed input, and a negative length would quietly
        subtract from a total that is reported as a percentage of the file.
        """
        return max(0.0, self.end - self.start)


def _number(text: str | None) -> float | None:
    """A captured number, or ``None`` when it is not one. Never raises."""
    if text is None:
        return None
    try:
        value = float(text)
    except ValueError:
        return None
    return value if math.isfinite(value) else None


def parse_silences(stderr: str, *, duration: float | None = None) -> list[Span]:
    """Read ``silencedetect``'s log lines into spans, sorted and clamped to the file.

    The log is written in order, but a captured stderr -- a saved log, a test fixture, two runs
    spliced together -- may not be. Pairing starts and ends by line order would then attach an end
    to the wrong start and shade speech as silence, so every ``silence_end`` is matched to the
    start that its own ``silence_duration`` implies; the start lines only supply the values to
    match against. A duplicate end therefore finds its start already claimed and is dropped,
    rather than producing a second, overlapping silence.

    A final ``silence_start`` with no end is a pause that runs to the end of the file. It needs
    ``duration``: without one, "silence to the end" is indistinguishable from a log that stopped
    early, and an invented end would shade over speech, so it is dropped rather than guessed.

    Values are rounded to milliseconds and the limit to the same grid before the clamp, because
    the clamp has to agree with the numbers the UI draws. An end a fraction of a millisecond past
    the file would otherwise make the report claim more silence than the file has time for, and a
    start at ``-0.0004`` would shift every band by a fraction of a pixel. Nothing here raises on a
    malformed line: a log is read to find what it does say, not to be validated.
    """
    starts: list[float] = []
    ends: list[tuple[float, float | None]] = []
    for line in (stderr or "").splitlines():
        found = _START.search(line)
        if found is not None:
            start = _number(found.group(1))
            if start is not None:
                starts.append(round(start, 3))
            continue
        found = _END.search(line)
        if found is None:
            continue
        end = _number(found.group(1))
        if end is None:
            continue
        ends.append((round(end, 3), _number(found.group(2))))

    unique: list[float] = []
    seen: set[float] = set()
    for value in starts:
        if value not in seen:
            seen.add(value)
            unique.append(value)

    pairs: list[tuple[float, float]] = []
    claimed: set[float] = set()
    for end, length in ends:
        if length is not None:
            implied = round(end - length, 3)
            if implied in seen:
                start = implied
            else:
                nearest = min(unique, key=lambda value: abs(value - implied), default=None)
                # Print rounding can put an end and its start either side of a millisecond
                # boundary. Five milliseconds is far below the shortest pause reported here and
                # far above the error in two three-decimal values.
                near = nearest is not None and abs(nearest - implied) <= 0.005
                start = nearest if near else implied
        else:
            earlier = [value for value in unique if value <= end and value not in claimed]
            if not earlier:
                continue
            start = max(earlier)
        if start in claimed:
            continue
        claimed.add(start)
        pairs.append((start, end))

    limit = round(duration, 3) if duration is not None else None
    if limit is not None:
        unclaimed = [value for value in unique if value not in claimed]
        finished = max((end for _, end in pairs), default=float("-inf"))
        if unclaimed and max(unclaimed) >= finished:
            # Only the latest start can be the one running to the end, and only when it begins
            # after every finished pause. An earlier unmatched start would have to run *through*
            # a known pause to reach the end, which would shade speech; a log ends with at most
            # one open start, so the rest are stale lines rather than pauses.
            pairs.append((max(unclaimed), limit))

    spans: list[Span] = []
    for start, end in pairs:
        start = round(max(0.0, start), 3)
        end = round(max(0.0, end), 3)
        if limit is not None:
            start = min(start, limit)
            end = min(end, limit)
        if end > start:
            spans.append(Span(start, end))
    spans.sort(key=lambda span: (span.start, span.end))
    return spans


def _logged_duration(stderr: str) -> float | None:
    """The ``Duration:`` line from the input header, or ``None`` when it says ``N/A``.

    This is the length ffmpeg has already printed, and a pause running to the end needs one. A
    separate ffprobe launch would cost another process on every load for a number that is sitting
    in the log being parsed.
    """
    found = _LOGGED_DURATION.search(stderr or "")
    if found is None:
        return None
    hours, minutes, seconds = found.groups()
    return int(hours) * 3600 + int(minutes) * 60 + float(seconds)


def silences(path: Path, *, noise_db: float = -35.0, minimum: float = 0.25,
             cancel=None, timeout: float = 900.0) -> list[Span]:
    """Measure where the source is silent, by decoding its audio once.

    ``noise_db`` is the threshold and ``minimum`` the shortest run worth reporting. The defaults
    are the ones the strip uses: a quarter of a second is about the shortest pause that reads as
    a pause rather than as the gap between two syllables, and -35 dB catches a room with a fan in
    it without catching a quiet breath.

    A source with no audio track is **not** an error: ffmpeg refuses the empty output with
    "Output file does not contain any stream", which is a fact about the file, so the answer is an
    empty list. An empty list is also what a file with no silence returns, and this function
    cannot tell the caller which it found; a caller that needs that distinction has to ask
    ffprobe for the streams. Any other failure is ffmpeg failing, and is raised with its own
    stderr.

    A cancelled token returns an empty list *before* a process is started -- if the user has
    already cancelled, the strip is not worth a decode. A cancellation during the run raises
    :class:`trimmer.ffmpeg.Cancelled`, so it travels the caller's normal cancel path instead of
    being mistaken for "no silences" and cached as an answer.

    The trailing pause is recovered from the ``Duration:`` line ffmpeg printed in the same log.
    Without it, a master that ends on dead air -- which is most of them -- would lose exactly the
    silence at its end.
    """
    if cancel is not None and getattr(cancel, "cancelled", False):
        return []

    command = [
        ff.tool("ffmpeg"),
        "-hide_banner", "-nostats",
        "-i", str(path),
        "-vn",
        "-af", f"silencedetect=noise={noise_db:g}dB:d={minimum:g}",
        "-f", "null", "-",
    ]
    try:
        stderr = ff.run(command, cancel=cancel, timeout=timeout)
    except ff.FFmpegError as failure:
        if any(text in (failure.stderr or "") for text in _NO_AUDIO):
            return []
        raise
    return parse_silences(stderr, duration=_logged_duration(stderr))


def _runs(spans: list[Span], duration: float) -> list[tuple[float, float]]:
    """Clamped, merged, ordered runs -- the one interpretation of the list every reader shares.

    Merging is not cosmetic: two overlapping spans would otherwise be counted twice by the
    summary, and a percentage over 100 is a report the operator cannot trust.
    """
    limit = max(0.0, duration)
    runs: list[tuple[float, float]] = []
    for span in sorted(spans, key=lambda one: (one.start, one.end)):
        start = min(max(span.start, 0.0), limit)
        end = min(max(span.end, 0.0), limit)
        if end <= start:
            continue
        if runs and start <= runs[-1][1]:
            runs[-1] = (runs[-1][0], max(runs[-1][1], end))
        else:
            runs.append((start, end))
    return runs


def speech(spans: list[Span], duration: float) -> list[Span]:
    """The complement of the silence list: what is not silent, for drawing the strip.

    Overlapping silences are merged first, so the answer is a set of disjoint runs rather than
    double-counted time. A silent run narrower than :data:`HAIRLINE` is then filled in: at strip
    scale it is narrower than a pixel, so drawing it honestly would leave a hairline seam through
    a block of speech, which reads as a rendering fault rather than as a pause. Filling it joins
    the speech on either side, which is what the eye would do anyway.

    A file that is silent from end to end has no speech, so the answer is empty -- not a
    zero-length span, which a canvas would draw as a hairline at the very start.
    """
    if duration <= 0:
        return []
    runs = [(start, end) for start, end in _runs(spans, duration)
            if end - start >= HAIRLINE]
    pieces: list[Span] = []
    cursor = 0.0
    for start, end in runs:
        if start > cursor:
            pieces.append(Span(cursor, start))
        cursor = max(cursor, end)
    if cursor < duration:
        pieces.append(Span(cursor, duration))
    return pieces


def quietest_frame(spans: list[Span], frame: int, rate: float, *,
                   limit: float = 2.0) -> int:
    """The frame nearest ``frame`` that sits in a pause, or ``frame`` when there is none.

    The target is the **middle** of the pause, not its nearest edge. A cut in the middle is the
    furthest a cut can be from a word on either side of the pause, and that margin is what absorbs
    the 30-200 ms of drift between a transcript, a container and the sound. Aiming at the edge
    would put the cut exactly where the next word starts.

    Only pauses whose start or end is within ``limit`` seconds of the mark are considered: a pause
    ten seconds away is not a snap, it is a different edit. Either end counts because a mark can
    sit before a pause (drawn to its start) or after one (drawn to its end).

    A pause can be shorter than a frame, and rounding its middle can then land on the frame after
    it -- which is speech, not silence. The answer is therefore clamped to the frames whose own
    time is inside the pause, and a pause with no such frame is skipped.
    """
    if frame < 0 or rate <= 0 or limit < 0:
        return frame
    at = frame / rate
    best: int | None = None
    for span in sorted(spans, key=lambda one: (one.start, one.end)):
        if span.end <= span.start:
            continue
        if abs(span.start - at) > limit and abs(span.end - at) > limit:
            continue
        first = math.ceil(span.start * rate - 1e-9)
        last = math.ceil(span.end * rate - 1e-9) - 1
        if last < first:
            continue
        middle = round((span.start + span.end) / 2.0 * rate)
        candidate = min(max(middle, first), last)
        if best is None or abs(candidate - frame) < abs(best - frame):
            best = candidate
    return frame if best is None else int(best)


def _clock(seconds: float) -> str:
    """A length as the log writes it: ``42s``, ``6m 12s``, ``1h 4m``."""
    total = max(0, round(seconds))
    hours, rest = divmod(total, 3600)
    minutes, secs = divmod(rest, 60)
    if hours:
        return f"{hours}h {minutes}m"
    if minutes:
        return f"{minutes}m {secs}s"
    return f"{secs}s"


def summary(spans: list[Span], duration: float) -> str:
    """One line for the log: how many pauses, how long they add up to, and what share.

    One line because the log is a record of what was done, and a paragraph for a measurement the
    operator did not ask for pushes the next decision off the screen. The total is the union of
    the spans, not their sum, so overlapping input cannot report more silence than the file has.
    Above an hour, minutes are the useful unit and seconds are noise; below it, seconds are.
    """
    runs = _runs(spans, duration)
    silent = sum(end - start for start, end in runs)
    share = 100.0 * silent / duration if duration > 0 else 0.0
    count = len(runs)
    return (f"{'silence':<12}{count} gap{'' if count == 1 else 's'}, "
            f"{_clock(silent)} silent of {_clock(duration)} ({share:.1f}%)")
