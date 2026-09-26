"""Retiming a transcript so it plays against a cut segment.

A transcript is a sidecar file: ``clip.mp4`` and ``clip.srt``. When a segment is cut out
of the video, the captions have to move with it, and the rules that matter are these:

* **A mark is a frame, a cue is a span.** The in and out points come from Premiere
  timecodes, which land wherever the editor clicked, so a cue can straddle a mark.
  Dropping such a cue would leave the audio it still covers uncaptioned; keeping it whole
  would put text on screen for words that were cut away. So it is *clamped* to the window
  when at least :data:`MIN_OVERLAP` of it survives, and dropped when less does -- a
  caption that flashes for forty milliseconds is worse than no caption at all.
* **Everything shifts to zero.** The segment starts at 00:00:00,000, and cues are
  renumbered from 1, because a caption track with gaps in its numbering is needless risk
  in an importer.
* **The file keeps its own shape.** BOM, line endings and the decimal separator the
  source used are preserved: these files get diffed and re-imported, and a gratuitous
  reformat shows up as a whole-file change.

Nothing here talks to ffmpeg. It is text in, text out, which is also why cutting captions
alongside a video costs no measurable time.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

#: A straddling cue with less than this much of itself left inside the segment is
#: dropped rather than clamped. A quarter of a second is about the shortest a line can
#: be on screen and still be read.
MIN_OVERLAP = 0.25

#: ``00:00:01,250`` -- and ``00:00:01.250``, which some tools write.
STAMP = re.compile(r"(\d{1,2}):(\d{2}):(\d{2})[,.](\d{1,3})")


@dataclass
class Cue:
    """One caption: seconds from zero, and the text as it was written."""

    start: float
    end: float
    text: str

    @property
    def duration(self) -> float:
        return self.end - self.start


@dataclass
class Transcript:
    """A parsed subtitle file, plus the shape it was written in."""

    path: Path | None
    cues: list[Cue]
    newline: str = "\n"
    bom: bool = False


@dataclass
class RetimeResult:
    """What a retime kept, trimmed and threw away."""

    cues: list[Cue] = field(default_factory=list)
    clamped: list[Cue] = field(default_factory=list)
    dropped: list[Cue] = field(default_factory=list)
    outside: int = 0

    def __len__(self) -> int:
        return len(self.cues)


def parse_stamp(text: str) -> float:
    """``00:01:02,500`` -> 62.5 seconds."""
    match = STAMP.search(text)
    if not match:
        raise ValueError(f"not a subtitle timestamp: {text!r}")
    hours, minutes, seconds, fraction = match.groups()
    milliseconds = int(fraction.ljust(3, "0"))
    return int(hours) * 3600 + int(minutes) * 60 + int(seconds) + milliseconds / 1000


def format_stamp(seconds: float) -> str:
    """Seconds -> ``HH:MM:SS,mmm``, rounded rather than truncated."""
    total = max(0, round(seconds * 1000))
    hours, rest = divmod(total, 3_600_000)
    minutes, rest = divmod(rest, 60_000)
    secs, millis = divmod(rest, 1000)
    return f"{hours:02d}:{minutes:02d}:{secs:02d},{millis:03d}"


def parse(text: str) -> list[Cue]:
    """An SRT body -> cues, tolerating BOM, CRLF and blank-line quirks.

    The numbering line is not trusted for anything: blocks are found by their timing
    line, which is the only part every tool agrees on.
    """
    text = text.replace("\r\n", "\n").replace("\r", "\n").lstrip("\ufeff")
    cues: list[Cue] = []
    for block in re.split(r"\n\s*\n", text):
        lines = block.strip("\n").split("\n")
        timing = next((index for index, line in enumerate(lines) if "-->" in line), None)
        if timing is None:
            continue
        begin, _, finish = lines[timing].partition("-->")
        try:
            start, end = parse_stamp(begin), parse_stamp(finish)
        except ValueError:
            continue
        body = "\n".join(lines[timing + 1:]).strip("\n")
        cues.append(Cue(start, end, body))
    cues.sort(key=lambda cue: (cue.start, cue.end))
    return cues


def read(path: Path) -> Transcript:
    """Read a subtitle file, remembering how it was written."""
    raw = path.read_bytes()
    bom = raw.startswith(b"\xef\xbb\xbf")
    text = raw.decode("utf-8-sig", errors="replace")
    newline = "\r\n" if "\r\n" in text else "\n"
    return Transcript(path=path, cues=parse(text), newline=newline, bom=bom)


def render(cues: list[Cue], newline: str = "\n", bom: bool = False) -> str:
    """Cues -> the text of an SRT file, numbered from 1."""
    blocks = [f"{index}\n{format_stamp(cue.start)} --> {format_stamp(cue.end)}\n{cue.text}"
              for index, cue in enumerate(cues, start=1)]
    body = (newline + newline).join(blocks)
    text = body + newline if body else ""
    return ("\ufeff" if bom else "") + text


def write(path: Path, cues: list[Cue], newline: str = "\n", bom: bool = False) -> None:
    path.write_text(render(cues, newline, bom), encoding="utf-8", newline="")


def retime(cues: list[Cue], start: float, end: float,
           min_overlap: float = MIN_OVERLAP) -> RetimeResult:
    """Move cues onto a segment that runs from ``start`` to ``end`` in the source.

    Cues outside the window are left out; cues that cross a mark are clamped when enough
    of them survives and dropped when not. The result is sorted and starts at zero.
    """
    if end <= start:
        raise ValueError("the segment ends before it starts")
    result = RetimeResult()
    for cue in cues:
        if cue.end <= start or cue.start >= end:
            result.outside += 1
            continue
        head, tail = max(cue.start, start), min(cue.end, end)
        straddles = cue.start < start - 1e-6 or cue.end > end + 1e-6
        if straddles and tail - head < min_overlap:
            result.dropped.append(cue)
            continue
        moved = Cue(round(head - start, 3), round(tail - start, 3), cue.text)
        result.cues.append(moved)
        if straddles:
            result.clamped.append(moved)
    result.cues.sort(key=lambda cue: (cue.start, cue.end))
    return result


def find_for(video: Path) -> Path | None:
    """The transcript that belongs to a video: ``clip.srt``, or ``clip.<lang>.srt``.

    Matching the stem exactly is the common case (the editor exported the transcript
    beside the video). The dotted form covers ``clip.en.srt``, which is what transcription
    tools write. Anything else has to be pointed at, because guessing between several
    subtitle files would be worse than asking.
    """
    folder = video.parent
    exact = folder / f"{video.stem}.srt"
    if exact.is_file():
        return exact
    prefix = f"{video.stem.lower()}."
    for candidate in sorted(folder.glob("*.srt")):
        if candidate.name.lower().startswith(prefix):
            return candidate
    return None


def summary(result: RetimeResult, path: Path | None, rate_note: str = "") -> str:
    """One line for the log: what was written, and what was cut off at the marks."""
    if path is None:
        return ("subtitles   nothing fell inside the segment: no .srt written. If this "
                "transcript is already relative to the segment, copy it beside the video "
                "instead of retiming it")
    parts = [f"subtitles   {path.name}: {len(result)} cues"]
    if result.clamped:
        parts.append(f"{len(result.clamped)} clamped at the marks")
    if result.dropped:
        parts.append(f"{len(result.dropped)} dropped (under {MIN_OVERLAP:g}s inside)")
    if not result.clamped and not result.dropped:
        parts.append("no cue crossed a mark")
    if rate_note:
        parts.append(rate_note)
    return "   ·   ".join(parts)


def retime_file(source_srt: Path, output_srt: Path, start: float, end: float,
                min_overlap: float = MIN_OVERLAP) -> tuple[RetimeResult, Path | None]:
    """Read a transcript, move it onto the segment, write it beside the video.

    Returns the result and the file written -- which is ``None`` when the transcript held
    nothing inside the window, so that pointing the app at an already-trimmed transcript
    cannot quietly produce an empty caption file.
    """
    transcript = read(source_srt)
    result = retime(transcript.cues, start, end, min_overlap)
    if not result.cues:
        return result, None
    write(output_srt, result.cues, transcript.newline, transcript.bom)
    return result, output_srt
