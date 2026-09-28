"""ffmpeg and ffprobe, wrapped thinly enough that nothing is hidden.

Every call the app makes goes through :func:`run`, so the log shows the exact command
line that produced a file. Nothing here uses a shell, so paths with spaces, brackets
and unicode are safe: arguments are passed as a list.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import threading
import time
from dataclasses import dataclass
from fractions import Fraction
from pathlib import Path

import av

from .timecode import format_rate, parse_rate

#: How long to wait between checks for a cancelled job. Long enough to be free, short
#: enough that the Cancel button feels instant.
POLL_SECONDS = 0.15
#: How often a running command says it is still running. A stream copy of a long body
#: prints nothing for minutes, and silence should not look like a hang.
HEARTBEAT_SECONDS = 15.0
#: Windows gives every console child of a *windowed* process its own console window, and
#: the GUI runs under pythonw. Without this flag a trim -- a dozen ffmpeg and ffprobe
#: calls -- looks like the app opening and closing command windows instead of working.
#: The children's output is read through pipes either way, so there is nothing to show.
#: The flag does not exist off Windows, where this is 0 and subprocess ignores it.
NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)
#: How often ffmpeg is asked to report where it has got to, in seconds. Twice a second is
#: often enough to look live and rare enough to cost nothing: a ten-minute copy is about
#: twelve hundred readings.
PROGRESS_PERIOD = 0.5


@dataclass
class Ticks:
    """Where a running command has got to, as **ffmpeg itself** reported it.

    ``ffmpeg -progress pipe:1`` writes a block of ``key=value`` lines every
    :data:`PROGRESS_PERIOD` seconds. ``out_time_us`` is microseconds of output written,
    which against a length the caller already knows is a real fraction rather than an
    animation, and ``speed`` is ffmpeg's own throughput -- so the estimate of what is
    left is measured by the program doing the work.
    """

    out_seconds: float
    frame: int | None = None
    speed: float | None = None
    size: int | None = None
    expected_seconds: float | None = None

    @property
    def fraction(self) -> float | None:
        """How far through, or ``None`` when the length was not known in advance."""
        if not self.expected_seconds or self.expected_seconds <= 0:
            return None
        return max(0.0, min(1.0, self.out_seconds / self.expected_seconds))

    @property
    def remaining(self) -> float | None:
        """Seconds left, at ffmpeg's own rate. ``None`` until there is a rate to use."""
        if not self.speed or self.speed <= 0 or not self.expected_seconds:
            return None
        left = self.expected_seconds - self.out_seconds
        return max(0.0, left / self.speed) if left > 0 else 0.0


def _progress_flags() -> list[str]:
    """The flags that make ffmpeg report, placed straight after the program name."""
    return ["-nostats", "-progress", "pipe:1", "-stats_period", f"{PROGRESS_PERIOD:.2f}"]


class _TickReader:
    """Turn ffmpeg's progress stream into :class:`Ticks`, one per block.

    Incremental by necessity: the stream arrives in whatever chunks the pipe delivers, so
    a ``key=value`` pair can be split across two reads. Lines are buffered until a newline
    arrives and a block is emitted when ``progress=`` ends it.

    A block with no position is **dropped** rather than read as zero. ffmpeg's first block
    carries ``out_time_us=N/A``, and reporting that as ``0.0`` would put the bar back to
    the start a moment after it had moved.
    """

    def __init__(self) -> None:
        self._partial = ""
        self._block: dict[str, str] = {}

    def feed(self, chunk: str) -> list[dict[str, str]]:
        """Take a chunk of output, and return whatever complete blocks it produced."""
        self._partial += chunk
        *lines, self._partial = self._partial.split("\n")
        blocks: list[dict[str, str]] = []
        for line in lines:
            key, sep, value = line.partition("=")
            if not sep:
                continue
            key = key.strip()
            value = value.strip()
            if key == "progress":
                # Only a block that says *where* the job has got to is worth reporting.
                if self._block.get("out_time_us", "N/A") not in ("N/A", ""):
                    blocks.append(self._block)
                self._block = {}
            else:
                self._block[key] = value
        return blocks


def _ticks(block: dict[str, str], expected: float | None) -> Ticks | None:
    """One parsed block as :class:`Ticks`, or ``None`` when it carries no position."""
    raw = block.get("out_time_us") or block.get("out_time_ms")
    if raw is None or raw == "N/A":
        return None
    try:
        out_seconds = int(raw) / 1_000_000
    except ValueError:
        return None

    def number(key: str, cast):
        value = block.get(key, "N/A")
        if value in ("N/A", ""):
            return None
        try:
            return cast(float(value.rstrip("x")))
        except ValueError:
            return None

    return Ticks(
        out_seconds=out_seconds,
        frame=number("frame", int),
        speed=number("speed", float),
        size=number("total_size", int),
        expected_seconds=expected,
    )


def _run_watched(
    args: list[str],
    *,
    cancel: CancelToken | None,
    log,
    progress,
    heartbeat: float | None,
    expected_seconds: float | None,
    timeout: float | None = None,
) -> str:
    """``run`` for a command that was asked to report its position.

    Separate from the plain path on purpose: reading ffmpeg's progress means owning
    standard output, and ``communicate`` -- which is what makes the plain path's
    cancellation and timeout simple -- cannot be used once a reader thread holds the pipe.
    """
    if timeout is None:
        timeout = max(180.0, (expected_seconds or 0) * 10 + 60.0)
    if log:
        log("  $ " + " ".join(str(a) for a in args))
    started = time.monotonic()
    process = subprocess.Popen(
        [str(a) for a in args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        errors="replace",
        bufsize=1,
        creationflags=NO_WINDOW,
    )

    stderr_parts: list[str] = []
    stop = threading.Event()

    def drain_stderr() -> None:
        for line in process.stderr or ():
            stderr_parts.append(line)

    def read_progress() -> None:
        reader = _TickReader()
        for chunk in process.stdout or ():
            for block in reader.feed(chunk):
                tick = _ticks(block, expected_seconds)
                if tick is not None and progress is not None:
                    progress(tick)

    readers = [threading.Thread(target=drain_stderr, daemon=True)]
    if progress is not None:
        readers.append(threading.Thread(target=read_progress, daemon=True))
    for reader in readers:
        reader.start()

    if log and heartbeat:
        def ticker() -> None:
            while not stop.wait(heartbeat):
                log(f"    ... {time.monotonic() - started:.0f}s")

        threading.Thread(target=ticker, daemon=True).start()

    try:
        while process.poll() is None:
            if cancel is not None and cancel.cancelled:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                raise Cancelled("cancelled") from None
            if timeout is not None and time.monotonic() - started > timeout:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                raise FFmpegError(
                    f"command timed out after {timeout:.0f}s",
                    [str(a) for a in args],
                    "".join(stderr_parts),
                )
            time.sleep(POLL_SECONDS)
    finally:
        stop.set()
        for reader in readers:
            reader.join(timeout=5)

    stderr = "".join(stderr_parts)
    if log:
        state = "ok" if process.returncode == 0 else f"exit {process.returncode}"
        log(f"  -> {state} in {time.monotonic() - started:.1f}s")
    if process.returncode != 0:
        raise FFmpegError(
            f"command failed (exit {process.returncode})",
            [str(a) for a in args],
            stderr or "",
        )
    return stderr


def _run_quiet(args: list[str], timeout: float = 60.0, **kwargs) -> subprocess.CompletedProcess:
    """``subprocess.run`` with no console window for the child and a default timeout."""
    try:
        return subprocess.run(args, creationflags=NO_WINDOW, timeout=timeout, **kwargs)
    except subprocess.TimeoutExpired as exc:
        raise FFmpegError(f"command timed out after {timeout}s", [str(a) for a in args]) from exc


class FFmpegError(RuntimeError):
    """ffmpeg or ffprobe exited non-zero, or could not be found."""

    def __init__(self, message: str, command: list[str] | None = None, stderr: str = ""):
        self.command = command or []
        self.stderr = stderr
        detail = f"\n  $ {' '.join(self.command)}" if self.command else ""
        tail = "\n".join(stderr.strip().splitlines()[-12:])
        super().__init__(f"{message}{detail}" + (f"\n{tail}" if tail else ""))


def tool(name: str) -> str:
    """Locate ``ffmpeg``/``ffprobe``, honouring an override for a bundled build.

    Three places, in order, and the order is the point:

    1. ``THE_TRIMMER_FFMPEG`` / ``THE_TRIMMER_FFPROBE``, which is what ``start.bat`` sets to the
       binaries it has just verified. This wins so that the application uses exactly the build
       that was checked rather than whichever copy happens to come first in ``PATH``.
    2. ``.tools/ffmpeg/bin`` beside the application, where ``start.bat`` unpacks a portable build
       when the machine has none. This is the second rather than the first so that an override
       still overrides it, and it is here at all so the engine works when it is started directly
       -- from the packaged app, or from a developer's own command line -- and not only through
       the script that provisioned it.
    3. ``PATH``, which is every machine that already had ffmpeg.
    """
    override = os.environ.get(f"THE_TRIMMER_{name.upper()}")
    if override:
        if Path(override).exists():
            return override
        raise FFmpegError(f"THE_TRIMMER_{name.upper()} points at {override}, which is not there")

    # `backend/trimmer/ffmpeg.py` -> the application's own folder. Both layouts matter: the
    # project as it is checked out, and the packaged app, where the engine sits beside `resources`.
    here = Path(__file__).resolve()
    for root in (here.parents[2], here.parents[1]):
        local = root / ".tools" / "ffmpeg" / "bin" / f"{name}.exe"
        if local.exists():
            return str(local)

    found = shutil.which(name)
    if not found:
        raise FFmpegError(
            f"{name} was not found on PATH, and there is no portable copy beside the "
            f"application. Run start.bat, which installs one, or set "
            f"THE_TRIMMER_{name.upper()} to its full path."
        )
    return found


def version(name: str = "ffmpeg") -> str:
    """The first line of ``ffmpeg -version``, for the report."""
    try:
        result = _run_quiet([tool(name), "-version"], capture_output=True, text=True)
        return result.stdout.splitlines()[0] if result.stdout else "unknown"
    except (FFmpegError, OSError):
        return "not found"


def encoders() -> str:
    """ffmpeg's list of what it can encode.

    A separate call from :func:`run` on purpose: ``run`` hands back stderr, because that
    is where an encode reports itself, while ``-encoders`` prints its list to stdout.
    Reading the wrong one silently says "no encoders at all".
    """
    result = _run_quiet([tool("ffmpeg"), "-hide_banner", "-encoders"],
                        capture_output=True, text=True)
    return result.stdout or ""


def run(
    args: list[str],
    *,
    cancel: CancelToken | None = None,
    log=None,
    progress=None,
    expected_seconds: float | None = None,
    heartbeat: float | None = HEARTBEAT_SECONDS,
    timeout: float | None = None,
) -> str:
    """Run a command, returning its stderr (where ffmpeg says everything useful).

    With a cancel token the child is terminated as soon as the token is set, which is
    what makes the GUI's Cancel button work on a trim that is already running. A
    heartbeat logs how long the job has been going, because a long stream copy prints
    nothing at all and silence is indistinguishable from a hang; when the command ends,
    how long it took and how it exited are logged too, so the log reads as a record of
    the work rather than a list of intentions.

    With ``progress`` the command is asked to report where it has got to and
    ``progress(tick)`` is called with a :class:`Ticks` for each reading.
    ``expected_seconds`` is how long the command should produce, which is what turns a
    position into a fraction -- and it is passed in rather than guessed, because a bar
    with an invented denominator is worse than no bar.
    """
    if timeout is None:
        timeout = max(180.0, (expected_seconds or 0) * 10 + 60.0)
    if progress is not None:
        return _run_watched(
            list(args[:1]) + _progress_flags() + list(args[1:]),
            cancel=cancel,
            log=log,
            progress=progress,
            heartbeat=heartbeat,
            expected_seconds=expected_seconds,
            timeout=timeout,
        )
    if log:
        log("  $ " + " ".join(str(a) for a in args))
    started = time.monotonic()
    process = subprocess.Popen(
        [str(a) for a in args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        errors="replace",
        creationflags=NO_WINDOW,
    )
    stop_heartbeat = threading.Event()
    if log and heartbeat:
        def tick() -> None:
            while not stop_heartbeat.wait(heartbeat):
                log(f"    ... {time.monotonic() - started:.0f}s")

        threading.Thread(target=tick, daemon=True).start()
    try:
        while True:
            try:
                _, stderr = process.communicate(timeout=POLL_SECONDS)
                break
            except subprocess.TimeoutExpired:
                if cancel is not None and cancel.cancelled:
                    process.terminate()
                    try:
                        process.communicate(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                    raise Cancelled("cancelled") from None
                if timeout is not None and time.monotonic() - started > timeout:
                    process.terminate()
                    try:
                        process.communicate(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                    raise FFmpegError(
                        f"command timed out after {timeout:.0f}s",
                        [str(a) for a in args],
                        "",
                    )
    except Cancelled:
        raise
    finally:
        stop_heartbeat.set()
    if log:
        state = "ok" if process.returncode == 0 else f"exit {process.returncode}"
        log(f"  -> {state} in {time.monotonic() - started:.1f}s")
    if process.returncode != 0:
        raise FFmpegError(f"command failed (exit {process.returncode})", [str(a) for a in args],
                          stderr or "")
    return stderr or ""


class Cancelled(RuntimeError):
    """Raised when the user cancels a running trim."""


class CancelToken:
    """A one-way flag shared with the worker thread."""

    def __init__(self) -> None:
        self._flag = False

    def cancel(self) -> None:
        self._flag = True

    @property
    def cancelled(self) -> bool:
        return self._flag


@dataclass
class AudioStream:
    codec: str
    sample_rate: int
    channels: int


@dataclass
class FileFacts:
    """What one ffprobe call can say about a finished file.

    Reading frames, durations and the picture's start time one field at a time costs a
    process launch each -- around 100 ms on a multi-gigabyte file -- and the app asks for
    all of them after every cut and again for every check. They come from a single call.
    """

    path: Path
    frames: int
    rate: Fraction | None
    video_duration: float
    video_start: float
    audio_duration: float


@dataclass
class MediaInfo:
    """What ffprobe says about the source, in the units the trim needs."""

    path: Path
    codec: str
    width: int
    height: int
    pix_fmt: str
    rate: Fraction
    average_rate: Fraction | None
    timebase: Fraction
    frames: int
    duration: float
    audio: AudioStream | None
    size_bytes: int
    #: The first timestamp the picture reports. A fresh encode can leave a file starting
    #: a fraction of a frame late, and the alignment measurement has to know that to
    #: compare frame for frame (see verify.measure_offset).
    start_time: float = 0.0

    @property
    def nominal_rate(self) -> int:
        from .timecode import nominal_rate

        return nominal_rate(self.rate)

    @property
    def grid_drift(self) -> float:
        """How far the file's timestamps have wandered from the grid its rate describes.

        In frames, at the end of the file. A file of 52210 frames that claims 30 fps puts
        frame 52209 at 1740.300 s; if its container says 1740.366 s, its timestamps are a
        whole frame away from that grid by the end -- one frame in some places and none in
        others, because the two grids cross.
        """
        if self.frames <= 0 or self.rate <= 0 or self.duration <= 0:
            return 0.0
        on_the_grid = self.frames / float(self.rate)
        return abs(self.duration - on_the_grid) * float(self.rate)

    @property
    def variable(self) -> bool:
        """True when the file's real frame grid is not the one its rate claims.

        Two tests, and the second is the one that earns its place.

        A relative rate difference catches a genuinely variable-rate recording. It does not
        catch the commoner case: this project's reference master reports ``30/1`` and averages
        ``156630000/5221099`` -- a difference of 0.0019%, under any sensible threshold -- yet
        across 52210 frames it accumulates to *exactly one frame*. That is enough for a mark to
        land a frame out in one part of the file and exactly right in another, which is a
        frame-exactness claim the file cannot support, so it has to be said.

        The test is therefore the accumulated drift and not the instantaneous ratio: half a
        frame anywhere in the file is the point at which "the mark is frame-exact" stops being
        true.
        """
        if self.rate <= 0:
            return False
        if self.average_rate is not None and self.average_rate != 0:
            if abs(float(self.rate) - float(self.average_rate)) / float(self.rate) > 0.001:
                return True
        return self.grid_drift > 0.5

    @property
    def rate_text(self) -> str:
        return format_rate(self.rate)

    @property
    def grid_rate(self) -> Fraction:
        """The frame rate this file's frames are **actually** on.

        Not the same thing as the rate it *claims*, and the difference is the whole of the
        frame-exactness problem on real masters. The reference master here reports
        ``r_frame_rate 30/1`` and averages ``156630000/5221099`` -- 29.99943. Counting frames
        at 30 puts frame 52209 at 1740.300 s while the file says 1740.366 s: the two grids are
        a whole frame apart by the end, having crossed somewhere in the middle. Every mark is
        then a frame out in one part of the file and exactly right in another, which is not a
        grid any check can be exact against.

        So a frame's time is measured on this rate, not on the nominal one. A constant-rate
        file has the two identical and nothing changes; a drifting one is converted on the grid
        it really has, and the accumulated error goes to zero.

        ``avg_frame_rate`` is the exact rational the container reports, so it is used when it
        is there. When it is not, the rate is derived from the frame count over the file's own
        span, which is the same quantity computed rather than reported.
        """
        if self.average_rate is not None and self.average_rate > 0:
            return self.average_rate
        if self.frames > 0 and self.duration > self.start_time:
            # A wide bound on purpose. `parse_rate`'s usual limit of 1001 is right for reading
            # a spelling like "29.97" and wrong here: it would turn 29.999431 into 29999/1000,
            # a thousandth of a frame per second out, which is three quarters of a second of
            # accumulated error over a twenty-nine minute file -- the very fault this is here
            # to remove.
            return Fraction(self.frames / (self.duration - self.start_time)).limit_denominator(
                1_000_000
            )
        return self.rate

    @property
    def grid_rate_text(self) -> str:
        """The grid rate as ffmpeg wants it, for ``-r``."""
        return format_rate(self.grid_rate)

    def seconds_of(self, frame: int) -> float:
        """Frame number -> **when that frame is shown**, on the file's own clock.

        Two corrections, and both were the cause of a one-frame error that showed up as a
        failed alignment check on real material.

        A frame's presentation time is ``start_time + frame / rate``, not ``frame / rate``: a
        master whose picture begins at 0.021 s -- 0.63 of a frame at 30 fps -- has every frame
        a fraction of a frame later than counting from zero says, and a seek aimed with the
        bare division lands on the frame *before* the one meant.

        And the rate is the file's own grid rate, not the nominal one, so the frames are not
        counted on a grid the file is not on.

        This is a *timestamp*, and every caller wants one: seek targets, the keyframe search
        window, and the caption shift. Durations are a different quantity and are computed as
        ``frames / rate`` where they are needed -- see `_encode_head` and `_concat_list`, which
        must not take this in, because a duration does not move with the file's start.
        """
        return self.start_time + frame / float(self.grid_rate)

    def frame_of(self, seconds: float) -> int:
        """A time on the file's clock -> the frame shown then. The inverse of `seconds_of`."""
        return round((seconds - self.start_time) * float(self.grid_rate))

    def summary(self) -> str:
        audio = (f"{self.audio.codec} {self.audio.sample_rate} Hz "
                 f"{self.audio.channels}ch") if self.audio else "none"
        return (f"{self.path.name}: {self.width}x{self.height} {self.codec}/{self.pix_fmt} "
                f"{self.rate_text} fps, {self.frames} frames, "
                f"{self.duration:.3f}s, audio {audio}")


def _fraction(value: str | None) -> Fraction | None:
    if not value or value in {"0/0", "N/A"}:
        return None
    try:
        return parse_rate(value)
    except (ValueError, ZeroDivisionError):
        return None


def _seconds(mapping, key: str, fallback: float) -> float:
    try:
        return float(mapping.get(key))
    except (TypeError, ValueError, AttributeError):
        return fallback


def _json_streams(path: Path) -> dict:
    """One ffprobe call, JSON out: the shape both probe() and inspect() need."""
    result = _run_quiet(
        [tool("ffprobe"), "-v", "error", "-print_format", "json",
         "-show_streams", "-show_format", str(path)],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise FFmpegError(f"ffprobe could not read {path.name}", ["ffprobe", str(path)],
                          result.stderr)
    return json.loads(result.stdout or "{}")


def inspect(path: Path) -> FileFacts:
    """Frames, durations and the picture's start time for a finished file.

    This is the app's hot path: every trim and every check asks for all of these, and
    each one on its own used to be its own ffprobe launch.
    """
    data = _json_streams(path)
    streams = data.get("streams", [])
    video = next((s for s in streams if s.get("codec_type") == "video"), None)
    audio = next((s for s in streams if s.get("codec_type") == "audio"), None)
    duration = _seconds(video or {}, "duration",
                        _seconds(data.get("format", {}), "duration", -1.0))
    rate = _fraction((video or {}).get("r_frame_rate"))

    frames_text = (video or {}).get("nb_frames")
    frames = int(frames_text) if frames_text and str(frames_text).isdigit() else -1
    if frames <= 0 and rate is not None and duration > 0:
        frames = round(duration * float(rate))

    return FileFacts(
        path=path,
        frames=frames,
        rate=rate,
        video_duration=duration,
        video_start=_seconds(video or {}, "start_time", 0.0),
        audio_duration=_seconds(audio or {}, "duration", -1.0) if audio else -1.0,
    )


def probe(path: Path) -> MediaInfo:
    """Read the source's rate, frame count, timebase and audio layout."""
    data = _json_streams(path)
    streams = data.get("streams", [])
    video = next((s for s in streams if s.get("codec_type") == "video"), None)
    if video is None:
        raise FFmpegError(f"{path.name} has no video stream")
    audio_stream = next((s for s in streams if s.get("codec_type") == "audio"), None)

    rate = _fraction(video.get("r_frame_rate")) or Fraction(30)
    average = _fraction(video.get("avg_frame_rate"))
    timebase = _fraction(video.get("time_base")) or Fraction(1, 90000)
    duration = float(video.get("duration") or data.get("format", {}).get("duration") or 0.0)

    frames_text = video.get("nb_frames")
    frames = int(frames_text) if frames_text and str(frames_text).isdigit() else 0
    if frames <= 0:
        frames = round(duration * float(rate))

    audio = None
    if audio_stream is not None:
        audio = AudioStream(
            codec=audio_stream.get("codec_name", "?"),
            sample_rate=int(audio_stream.get("sample_rate") or 48000),
            channels=int(audio_stream.get("channels") or 2),
        )

    return MediaInfo(
        path=path,
        codec=video.get("codec_name", "?"),
        width=int(video.get("width") or 0),
        height=int(video.get("height") or 0),
        pix_fmt=video.get("pix_fmt", "yuv420p"),
        rate=rate,
        average_rate=average,
        timebase=timebase,
        frames=frames,
        duration=duration,
        audio=audio,
        size_bytes=int(data.get("format", {}).get("size") or 0),
        start_time=_seconds(video, "start_time", 0.0),
    )


def keyframes(path: Path, start_seconds: float, end_seconds: float) -> list[float]:
    """Timestamps of the keyframes inside a window, in ascending order.

    Read from the container's **packet flags**, which is the same information ``ffprobe
    -skip_frame nokey`` produces and measured 2.7x faster on a 35-minute master: 2.6 s against
    7.0 s for an identical list. The difference matters because the planner calls this while the
    operator is still typing, to decide whether the Trim button is enabled, so the whole delay
    between a keystroke and the button is this function plus a probe.

    ``ffprobe`` stays as the fallback for a file PyAV cannot open.
    """
    try:
        return _keyframes_by_packet(path, start_seconds, end_seconds)
    except Exception:                             # noqa: BLE001 - fall back, do not fail
        return _keyframes_by_probe(path, start_seconds, end_seconds)


def _keyframes_by_packet(path: Path, start_seconds: float,
                         end_seconds: float) -> list[float]:
    """The same list, from the demuxer's own keyframe flags."""
    with av.open(str(path)) as container:
        stream = container.streams.video[0]
        time_base = float(stream.time_base)
        times = []
        for packet in container.demux(stream):
            if packet.pts is None or not packet.is_keyframe:
                continue
            # In the stream's timebase, then shifted onto the same clock as the caller's
            # window. `stream.start_time` is that clock's zero.
            at = packet.pts * time_base
            if at < start_seconds:
                continue
            if at > end_seconds:
                # Presentation times run ahead of decode order by a GOP at most, so nothing
                # wanted is behind a packet that has already passed the window's end.
                break
            times.append(at)
    return sorted(times)


def _keyframes_by_probe(path: Path, start_seconds: float,
                        end_seconds: float) -> list[float]:
    """The same list, from ffprobe. Kept for a container PyAV will not open."""
    result = _run_quiet(
        [tool("ffprobe"), "-v", "error", "-select_streams", "v:0", "-skip_frame", "nokey",
         "-show_entries", "frame=pts_time", "-of", "csv=p=0",
         "-read_intervals", f"{start_seconds:.6f}%{end_seconds:.6f}", str(path)],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise FFmpegError("could not list keyframes", ["ffprobe", str(path)], result.stderr)
    times = []
    for token in result.stdout.split():
        # csv=p=0 still writes the record separator, so every token but the last carries a
        # trailing comma. Without stripping it `float` raises and the token is dropped by the
        # handler below -- which silently lost the file's *first* keyframe, and with it the
        # knowledge that the opening GOP is copyable at all.
        token = token.strip().rstrip(",")
        if not token:
            continue
        try:
            times.append(float(token))
        except ValueError:
            continue
    return sorted(times)


def frame_pts(path: Path, count: int) -> list[float]:
    """The presentation time of each of the first ``count`` video frames, in seconds.

    This is the container's *own* statement of when each frame is shown, and it is the only
    trustworthy answer for seeking. A frame's time can be computed as ``start_time +
    frame / grid_rate`` instead, which is what this module does everywhere else, but that
    assumes the frames sit on the average-rate grid -- and on a real master they do not: the
    reference source's frames step by 0.0333 s while its average rate, stretched over
    1872.633 s, implies a step very slightly larger. By frame 8596 the computed time is
    0.28 ms *past* the frame it names, so ``-ss`` aimed there lands inside the next frame and
    the delivered segment starts one frame late. The error grows with the frame number, which
    is why a short clip is exact and a long one is not.

    Read in one probe pass. ``pts_time`` is already in seconds and is written to six decimal
    places, which is finer than a frame at any rate this product handles.
    """
    if count <= 0:
        return []
    result = _run_quiet(
        [tool("ffprobe"), "-v", "error", "-select_streams", "v:0",
         "-show_entries", "frame=pts_time", "-of", "csv=p=0",
         "-read_intervals", f"%+#{count}", str(path)],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        return []
    times: list[float] = []
    for token in result.stdout.split():
        token = token.strip().rstrip(",")
        if not token:
            continue
        try:
            times.append(float(token))
        except ValueError:
            continue
    return times


def frame_pts_near(path: Path, frame: int, rate: float, start_time: float = 0.0,
                   window: int = 480) -> float | None:
    """The presentation time of one frame, read from *near* it rather than from the start.

    :func:`frame_pts` reads frames from the file's beginning up to the one wanted, which is
    fine for a low frame number and ruinous for a high one: on a 2.6 GB 34-minute master,
    asking for frame 10048 spent over 27 s reading ten thousand frames to learn a single
    timestamp -- and it ran before the first log line, so the window showed a started run and
    then nothing at all, looking hung.

    So the read is aimed at the frame. Two things about that are easy to get wrong, and this
    function got both of them wrong for a long time:

    ``-read_intervals`` **seeks, and a seek lands on the keyframe at or before the time asked
    for** -- not on the time. The window therefore has to reach past the longest gap between
    keyframes, not a few frames. Asking for 32 frames got the 32 frames after the *keyframe*:
    measured on the reference master, whose keyframes are 219 frames apart, a request for frame
    162241 returned a time 7.3 s and 222 frames early, one for frame 162900 came back 131
    frames early and one for frame 8596 came back 42 frames early.

    And a frame is **not** the one whose time is nearest the computed guess. On a master whose
    frames do not sit on its average-rate grid the nearest time belongs to a different frame --
    a B-frame one position along has a later presentation time -- so a request for frame 162000
    came back with the time of row 161998. Returning that as an answer is worse than returning
    nothing, because the caller cannot tell: it was used as the seek target for the re-encoded
    ends, and the head of a cut started in the wrong place.

    So the answer is validated against the numbering the rest of the engine uses -- the frame
    whose time is ``round((at - start_time) * rate)`` -- and ``None`` is returned when the
    window holds no such frame. The caller then falls back to the computed time, which is
    exactly what it did when the probe failed before, so nothing is lost and a wrong answer can
    no longer be passed off as a right one.
    """
    if frame < 0 or rate <= 0:
        return None
    guess = start_time + frame / rate
    # Far enough back that the frame wanted is inside the window even if the seek lands a whole
    # keyframe early, and far enough forward to cover a seek that lands late.
    lead = window / 2.0 / rate
    begin = max(0.0, guess - lead)
    span = window / rate
    result = _run_quiet(
        [tool("ffprobe"), "-v", "error", "-select_streams", "v:0",
         "-show_entries", "frame=pts_time", "-of", "csv=p=0",
         "-read_intervals", f"{begin:.6f}%+{span:.6f}", str(path)],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        return None
    for token in result.stdout.split():
        token = token.strip().rstrip(",")
        if not token:
            continue
        try:
            at = float(token)
        except ValueError:
            continue
        if round((at - start_time) * rate) == frame:
            return at
    return None


class FrameTimes:
    """The exact presentation time of a frame, read from the container on demand.

    Built because a seek aimed at a *computed* time lands inside the wrong frame on a long
    master, and only a long one: see :func:`frame_pts` for the measurement. Reads near the
    frame rather than from the start -- see :func:`frame_pts_near` for why that matters -- and
    keeps every answer, so the head, the body and the tail questions of a plan together cost
    one or two launches rather than three.
    """

    #: Frames read beyond the one wanted, so the next question about a nearby frame is free.
    LOOKAHEAD = 8

    def __init__(self, path: Path, fallback, rate: float | None = None,
                 start_time: float = 0.0) -> None:
        self.path = path
        self.fallback = fallback
        self.rate = rate
        self.start_time = start_time
        self._known: dict[int, float] = {}

    def __call__(self, frame: int) -> float:
        if frame < 0:
            return self.fallback(frame)
        if frame in self._known:
            return self._known[frame]
        exact = None
        if self.rate:
            exact = frame_pts_near(self.path, frame, self.rate, self.start_time)
        if exact is None:
            # The container did not report a time for this frame -- a damaged index, or a format
            # that omits them. The computed time is the fallback: exact for any file whose frames
            # really are on its average-rate grid.
            exact = self.fallback(frame)
        self._known[frame] = exact
        return exact

    def row_of(self, frame: int) -> int | None:
        """Which container row carries the time this frame's mark names.

        Container row and frame number are **not** a fixed distance apart: measured on three
        masters the offset is +1 on one and +2 on the other two, because the row whose stated
        time equals a frame's time depends on where the file's frames actually sit. So the row is
        found by asking the container's own index, never by computing it.

        Returns ``None`` when the container cannot say, and the caller keeps the arithmetic it
        used before.
        """
        from . import container as packets

        try:
            index = packets.read(self.path)
        except (OSError, ValueError):
            return None
        want = self(frame)
        best: tuple[float, int] | None = None
        for row in range(1, min(index.count, frame + 64) + 1):
            distance = abs(index.time_of(row) - want)
            if best is None or distance < best[0]:
                best = (distance, row)
                if distance == 0.0:
                    break
        if best is None or not self.rate:
            return best[1] if best else None
        # A row a whole frame away is not the row meant.
        return best[1] if best[0] <= 0.25 / self.rate else None


def frames_near(path: Path, at_seconds: float, count: int) -> list[tuple[float, str]]:
    """Frames decoded from ``at_seconds`` onward: each one's time and its MD5.

    The times are the container's **own**, not rebased to the seek. ``-copyts`` is what keeps
    them: without it ffmpeg rewrites the first frame found to zero, which loses the very thing
    the caller needs -- where in the file the frame actually is. Measured on the reference
    master, a seek to 300 s gives pts 9000 with the flag and pts 0 without it, and 9000 ticks at
    1/90000 is exactly the 300 s that was asked for.

    The time travels with the hash so a caller can identify a frame by when the container says
    it is shown rather than by counting positions in what came back. An earlier check addressed
    the frames in a window by index, and a seek that lands a frame off slides every index in it:
    on a 25 fps fixture whose cut was provably exact that reported four to six frames of drift,
    and those phantom offsets were fed to the calibration loop, which "corrected" a correct cut
    and made it genuinely wrong.
    """
    if count <= 0:
        return []
    result = _run_quiet(
        [tool("ffmpeg"), "-hide_banner", "-v", "error", "-ss", f"{at_seconds:.6f}",
         "-copyts", "-i", str(path), "-map", "0:v:0", "-an", "-frames:v", str(count),
         "-f", "framemd5", "-"],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise FFmpegError("could not read frames", ["ffmpeg", str(path)], result.stderr)
    frames: list[tuple[float, str]] = []
    for line in result.stdout.splitlines():
        if line.startswith("#") or not line.strip():
            continue
        fields = [field.strip() for field in line.split(",")]
        if len(fields) < 6:
            continue
        try:
            frames.append((float(fields[2]), fields[5]))
        except ValueError:
            continue
    return frames


def frame_md5s(path: Path, start_seconds: float, count: int) -> list[str]:
    """The MD5 of each decoded video frame in a window, as ffmpeg reports it.

    This is the app's ground truth for "is this the same picture". Two files that hold
    the same packets decode to identical frames, so their MD5s match exactly; a
    re-encoded frame never does. That is what lets the trim be checked for both
    losslessness and alignment without a single third-party dependency.

    ``-map 0:v:0 -an`` is not optional: without it the muxer hashes the audio frames
    too, and one audio frame per 21 ms quietly pads every list.
    """
    if count <= 0:
        return []
    result = _run_quiet(
        [tool("ffmpeg"), "-hide_banner", "-v", "error", "-ss", f"{start_seconds:.6f}",
         "-i", str(path), "-map", "0:v:0", "-an", "-frames:v", str(count),
         "-f", "framemd5", "-"],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise FFmpegError("could not hash frames", ["ffmpeg", str(path)], result.stderr)
    digests = []
    for line in result.stdout.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        fields = [field.strip() for field in line.split(",")]
        if len(fields) >= 6:
            digests.append(fields[-1])
    return digests


def frame_png(path: Path, at_seconds: float, out: Path, *, from_end: bool = False) -> bool:
    """Write one frame to a PNG, for comparisons that need pixels rather than hashes.

    ``from_end`` reads the last frame by counting back from the end of the file instead of
    forward from the start. It exists because the *last* frame cannot be reached reliably by
    seeking forward to a time: a frame's timestamp is the instant it starts, so a seek aimed at
    it is a coin toss, and a seek even slightly past it yields nothing at all because the
    file's picture has ended. Measured on an eight-frame tail, every forward seek into it came
    back empty while reading back from the end produced the frame every time.
    """
    out.parent.mkdir(parents=True, exist_ok=True)
    if from_end:
        place = ["-sseof", f"-{max(at_seconds, 0.001):.6f}"]
    else:
        place = ["-ss", f"{at_seconds:.6f}"]
    result = _run_quiet(
        [tool("ffmpeg"), "-hide_banner", "-v", "error", "-y", *place,
         "-i", str(path), "-map", "0:v:0", "-frames:v", "1", "-update", "1", str(out)],
        capture_output=True, text=True,
    )
    return result.returncode == 0 and out.exists() and out.stat().st_size > 0


def ssim(a: Path, b: Path) -> float:
    """Structural similarity between two still images, or -1 when unmeasurable.

    Frame hashes cannot compare a re-encoded frame with its original -- every pixel
    moves a little by construction -- so the head of a cut is checked this way instead:
    the score peaks on the right frame and falls away on its neighbours.
    """
    result = _run_quiet(
        [tool("ffmpeg"), "-hide_banner", "-nostats", "-i", str(a), "-i", str(b),
         "-lavfi", "ssim", "-f", "null", "-"],
        capture_output=True, text=True,
    )
    match = re.search(r"All:([0-9.]+)", result.stderr or "")
    return float(match.group(1)) if match else -1.0


def count_frames(path: Path) -> int:
    """Frame count from the container, without decoding the file.

    ``nb_frames`` is written by ffmpeg's MP4 muxer, so this is instant; counting by
    decoding a two-hour body instead costs minutes for a number the header already has.
    """
    result = _run_quiet(
        [tool("ffprobe"), "-v", "error", "-select_streams", "v:0",
         "-show_entries", "stream=nb_frames", "-of", "csv=p=0", str(path)],
        capture_output=True, text=True,
    )
    text = result.stdout.strip()
    return int(text) if text.isdigit() else -1


def stream_start_time(path: Path, stream: str = "v:0") -> float:
    """The first timestamp a stream reports, in seconds.

    A freshly encoded head can leave the joined file's video starting a fraction of a
    frame late (AAC priming is the usual reason). That fraction matters when frames are
    compared by extracting "the frame at time t" from two files: without knowing each
    file's own start, the two extractions can land one frame apart and every alignment
    measurement flickers by a frame. Callers align their windows with this.
    """
    result = _run_quiet(
        [tool("ffprobe"), "-v", "error", "-select_streams", stream,
         "-show_entries", "stream=start_time", "-of", "csv=p=0", str(path)],
        capture_output=True, text=True,
    )
    try:
        return float(result.stdout.strip())
    except ValueError:
        return 0.0


def stream_duration(path: Path, stream: str = "v:0") -> float:
    """A stream's duration in seconds, or -1 when ffprobe will not say."""
    result = _run_quiet(
        [tool("ffprobe"), "-v", "error", "-select_streams", stream,
         "-show_entries", "stream=duration", "-of", "csv=p=0", str(path)],
        capture_output=True, text=True,
    )
    try:
        return float(result.stdout.strip())
    except ValueError:
        return -1.0
