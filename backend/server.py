"""TheTrimmer's backend: the engine, behind a local HTTP and event-stream API.

## Why there is a server at all

The engine is Python and the interface is a web page, so something has to sit between
them. It is a loopback HTTP server rather than a subprocess protocol because the
progress of a cut is a *stream*: the window needs a bar that moves while ffmpeg works,
and server-sent events are the smallest thing that does that without a socket library on
both sides.

It binds ``127.0.0.1`` only. Nothing here is reachable from another machine, and there is
no authentication because there is nothing to authenticate against — the same single user
who started the window is the only client that can reach the port.

## The shape of a cut

``POST /api/cut`` starts one and returns immediately with a job id; the work happens on a
worker thread because the engine is synchronous and a cut is minutes long. Everything it
says — every stage, every exact command line, every position ffmpeg reports — is pushed
onto ``/api/events`` as it happens, and the run ends with a ``finished`` event carrying
the outcome.

One job at a time. The window marks one range and cuts it; a second concurrent cut would
be two ffmpeg processes fighting for the same disk, which is slower than doing them in
order, and there is nothing in the interface that could ask for one.
"""

from __future__ import annotations

import asyncio
import json
import os
import queue
import re
import sys
import tempfile
import threading
import time
import traceback
from pathlib import Path
from typing import Any

from aiohttp import web

sys.path.insert(0, str(Path(__file__).resolve().parent))

from trimmer import audio as quiet  # noqa: E402
from trimmer import ffmpeg as ff  # noqa: E402
from trimmer import processes  # noqa: E402
from trimmer import proof  # noqa: E402
from trimmer import subtitles as subs  # noqa: E402
from trimmer import trim as cutter  # noqa: E402
from trimmer import verify as verifier  # noqa: E402
from trimmer.timecode import format_timecode, parse_timecode  # noqa: E402

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")

VERSION = "2.0.0"
PORT = int(os.environ.get("PORT", "8765"))
VIDEO_SUFFIXES = [".mp4", ".mov", ".mkv", ".m4v", ".mxf", ".avi", ".webm", ".mts", ".m2ts"]

#: The shell's own PID, handed in by Electron. Zero when the backend was started by hand -- from
#: `start.bat`, a terminal, or a test -- in which case there is no parent to watch for.
PARENT_PID = int(os.environ.get("THE_TRIMMER_PARENT_PID", "0") or 0)

def _data_dir() -> Path:
    """Where this application keeps what is not the user's: proofs, and nothing else yet.

    ``%LOCALAPPDATA%`` rather than the project folder, because the project folder is replaced on
    update and proofs are not the user's files to lose. The fallback is the system temp directory
    for a machine with no profile, which still works and is still outside the install.
    """
    root = os.environ.get("LOCALAPPDATA") or os.environ.get("XDG_CACHE_HOME")
    base = Path(root) if root else Path(tempfile.gettempdir())
    return base / "TheTrimmer"


#: Where a cut's contact sheet is written. Served by :func:`proof_image`, never opened directly.
PROOF_DIR = _data_dir() / "proof"

#: How long a proof is worth keeping. It describes one cut, and the cut it describes is older than
#: this by the time anybody would ask again.
PROOF_DAYS = 7


# ---------------------------------------------------------------------------------------
# Everything the window can be told
# ---------------------------------------------------------------------------------------

class Hub:
    """The fan-out from the worker thread to whatever is listening on ``/api/events``.

    A plain ``queue.Queue`` per subscriber, and ``put_nowait`` everywhere: a window that
    has stopped reading must never be able to block a cut. Dropping an event for a dead
    subscriber costs nothing; blocking ffmpeg behind a closed browser tab costs the run.
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._subscribers: list[queue.Queue[dict[str, Any]]] = []
        self._history: list[dict[str, Any]] = []
        self._sequence = 0

    def subscribe(self) -> queue.Queue[dict[str, Any]]:
        channel: queue.Queue[dict[str, Any]] = queue.Queue(maxsize=2048)
        with self._lock:
            self._subscribers.append(channel)
            for event in self._history:
                try:
                    channel.put_nowait(event)
                except queue.Full:
                    break
        return channel

    def unsubscribe(self, channel: queue.Queue[dict[str, Any]]) -> None:
        with self._lock:
            if channel in self._subscribers:
                self._subscribers.remove(channel)

    def publish(self, event: dict[str, Any]) -> None:
        self._sequence += 1
        event = {**event, "seq": self._sequence, "at": time.time()}
        with self._lock:
            self._history.append(event)
            # The history is what a window that connects mid-cut is caught up with. It is
            # bounded because a long run says thousands of things and the log only keeps
            # the last few hundred anyway.
            if len(self._history) > 400:
                del self._history[:-400]
            for channel in self._subscribers:
                try:
                    channel.put_nowait(event)
                except queue.Full:
                    pass

    def reset(self) -> None:
        with self._lock:
            self._history.clear()
            self._sequence = 0


HUB = Hub()


# ---------------------------------------------------------------------------------------
# The one running job
# ---------------------------------------------------------------------------------------

class Job:
    """A cut in flight: its cancel token, its thread and its outcome."""

    def __init__(self, spec: cutter.TrimSpec, verify_mode: str) -> None:
        self.spec = spec
        self.verify_mode = verify_mode
        self.cancel = ff.CancelToken()
        self.thread: threading.Thread | None = None
        self.started = time.monotonic()
        self.outcome: dict[str, Any] | None = None
        self.error: str | None = None
        self.done = threading.Event()


CURRENT: Job | None = None
CURRENT_LOCK = threading.Lock()


def _level(text: str) -> str:
    """Which kind of log line this is, from the shape the engine writes.

    The engine's own convention, kept rather than re-derived: a line beginning with ``$``
    is a command, one beginning with ``->`` is a pass ending, indented dots are the
    heartbeat, and the rest are stages. Reading it here is cheaper and more honest than
    having the engine say it twice.
    """
    stripped = text.strip()
    if stripped.startswith("$"):
        return "command"
    if stripped.startswith("->"):
        return "done" if "ok" in stripped else "error"
    if stripped.startswith("..."):
        return "heartbeat"
    if stripped.startswith("FAIL"):
        return "error"
    return "stage"


def _ticks(tick: ff.Ticks) -> dict[str, Any]:
    """One progress reading, as JSON.

    The engine's own dataclass is the source: mapping it through a dictionary in between was
    a second spelling of the same seven fields, and a second spelling is a thing that drifts.
    """
    return {
        "type": "progress",
        "outSeconds": tick.out_seconds,
        "fraction": tick.fraction,
        "speed": tick.speed,
        "remaining": tick.remaining,
        "frame": tick.frame,
        "size": tick.size,
        "expectedSeconds": tick.expected_seconds,
    }


def _describe(media: ff.MediaInfo) -> dict[str, Any]:
    """A probe, as JSON the window reads."""
    audio = media.audio
    return {
        "path": str(media.path),
        "name": media.path.name,
        "codec": media.codec,
        "width": media.width,
        "height": media.height,
        "pixFmt": media.pix_fmt,
        "rate": {"numerator": media.rate.numerator, "denominator": media.rate.denominator},
        "rateText": f"{float(media.rate):g}",
        "frames": media.frames,
        "duration": media.duration,
        "durationText": format_timecode(media.frames, media.rate),
        "sizeBytes": media.size_bytes,
        "timebase": {"ticksPerSecond": media.timebase.denominator},
        "variableRate": media.average_rate != media.rate,
        "audio": (
            None
            if audio is None
            else {
                "codec": audio.codec,
                "sampleRate": audio.sample_rate,
                "channels": audio.channels,
            }
        ),
    }


def _plan_json(spec: cutter.TrimSpec, plan: cutter.TrimPlan, media: ff.MediaInfo) -> dict[str, Any]:
    rate = media.rate
    return {
        "mode": plan.mode,
        "keyframe": plan.keyframe,
        "headFrames": plan.head_frames,
        "bodyFrames": plan.body_frames,
        "requested": plan.requested,
        "reencodeFraction": (plan.head_frames / plan.requested) if plan.requested else 0.0,
        "inFrame": spec.in_frame,
        "endFrame": spec.out_frame,
        "frames": spec.frames,
        "seconds": spec.frames / float(rate) if rate else 0.0,
        "inTimecode": format_timecode(spec.in_frame, rate),
        "outTimecode": format_timecode(spec.out_frame - 1, rate),
        "notes": list(plan.notes),
    }


def _check_json(result: verifier.VerifyResult) -> list[dict[str, Any]]:
    """The verification report, split into rows the proof panel can draw.

    The engine returns prose lines; the interface wants a name and a verdict it can set in
    columns. Splitting on the first gap is the same convention the engine writes with, and
    a line that does not fit keeps its whole text as the name rather than being dropped.
    """
    rows = []
    for line in result.checks:
        name, _, detail = line.partition(" ")
        rows.append({"check": name.rstrip(":"), "status": {"kind": "passed"}, "detail": detail.strip()})
    for line in result.failures:
        name, _, detail = line.partition(" ")
        rows.append({"check": name.rstrip(":"), "status": {"kind": "failed"}, "detail": detail.strip()})
    return rows


def _proof_name(spec: cutter.TrimSpec, side: str) -> str:
    """A file name for one sheet: the segment, the side, and when it was drawn."""
    stem = re.sub(r"[^A-Za-z0-9._-]+", "-", spec.output.stem)[:60].strip("-") or "segment"
    return f"{stem}-{side}-{int(time.time() * 1000)}.png"


def _render_proof(spec: cutter.TrimSpec, report: cutter.TrimReport,
                  media: ff.MediaInfo) -> list[dict[str, Any]]:
    """Draw what the two ends of the cut actually landed on.

    A head-patched cut is four joins -- the in point, the head->body keyframe, the body->tail
    keyframe and the out point -- and the two that matter most sit within a second of a mark.
    So there are two sheets, each anchored on a frame whose time the container itself states
    (`plan.in_seconds`, `plan.tail_keyframe_seconds`), because a time computed from the average
    rate can name the frame beside the one intended and the sheet would then prove the wrong
    frame. The decode is bounded by that anchor and a couple of seconds of span, so this costs
    about a second on a two-hour master rather than the minutes a whole-file selection would.

    A sheet that cannot be drawn is not a failed cut: the list comes back short and the window
    shows the verdicts without a picture.
    """
    plan = report.plan
    rate = float(media.grid_rate)
    last = spec.out_frame - 1
    tail_base = spec.out_frame - plan.tail_frames if plan.tail_frames else last
    sides = [
        ("in", spec.in_frame,
         plan.in_seconds if plan.in_seconds is not None else media.seconds_of(spec.in_frame),
         spec.in_frame + plan.head_frames),
        ("out", tail_base,
         plan.tail_keyframe_seconds if plan.tail_keyframe_seconds is not None
         else (plan.out_seconds if plan.out_seconds is not None
               else media.seconds_of(tail_base)),
         last),
    ]
    images: list[dict[str, Any]] = []
    for side, base, anchor, far in sides:
        wanted = proof.windows([base, far], 4, media.frames)
        relative = sorted({frame - base for frame in wanted if frame >= base})
        if not relative:
            continue
        name = _proof_name(spec, side)
        span = (relative[-1] + 2) / rate
        if proof.render(spec.source, relative, rate, PROOF_DIR / name,
                        seek=anchor, span=span):
            images.append({
                "side": side,
                "name": name,
                "anchor": round(anchor, 3),
                "frames": [base + step for step in relative],
            })
    return images


def _run_job(job: Job) -> None:
    """The worker. Everything it says goes onto the hub as it happens."""
    global CURRENT
    spec = job.spec
    publish = HUB.publish

    def log(text: str) -> None:
        for line in str(text).splitlines() or [""]:
            publish({"type": "log", "level": _level(line), "text": line})

    def progress(tick: ff.Ticks) -> None:
        publish(_ticks(tick))

    try:
        publish({"type": "started", "output": str(spec.output), "source": str(spec.source)})
        media = ff.probe(spec.source)
        publish({"type": "source", "media": _describe(media)})

        # Through the calibration path, which is what both of V1's front ends used and what
        # its command line did by default: the copy is measured against the source and re-cut
        # once if the body landed off the mark. Calling `trim` directly skipped that, so a
        # whole-GOP error in the copied body reached the report unmeasured.
        report, offsets = verifier.trim_with_calibration(
            spec, log=log, cancel=job.cancel, samples=3, progress=progress
        )
        for command in report.commands:
            publish({"type": "command", "args": command})

        outcome: dict[str, Any] = {
            "output": str(report.output),
            "frames": report.frames,
            "duration": report.duration,
            "overshoot": report.overshoot,
            "source": str(spec.source),
            "inFrame": spec.in_frame,
            "outFrame": spec.out_frame,
            "mode": report.plan.mode,
            "plan": _plan_json(spec, report.plan, media),
            "rate": {"numerator": media.rate.numerator, "denominator": media.rate.denominator},
            "checks": [],
            "verified": None,
            "subtitles": None,
            "proof": [],
        }

        if job.verify_mode != "off":
            publish({"type": "stage", "name": "verify", "label": "measuring the result"})
            result = verifier.verify(spec.source, spec, report, media)
            outcome["checks"] = _check_json(result)
            outcome["verified"] = result.ok
            for line in result.report().splitlines():
                log(line)

        if report.subtitles is not None:
            outcome["subtitles"] = {
                "cues": len(report.subtitles.cues),
                "written": None if report.subtitles.written is None
                           else str(report.subtitles.written),
                # A count, not the cues themselves: the window asks "were any clamped" and
                # an array answers a different question.
                "clamped": len(report.subtitles.clamped),
            }

        # Drawn last, after the deliverable exists and after the verdicts: it is evidence about
        # the cut, so a sheet that cannot be drawn must not fail a file that is already correct.
        try:
            outcome["proof"] = _render_proof(spec, report, media)
        except Exception:  # noqa: BLE001
            outcome["proof"] = []

        job.outcome = outcome
        publish({"type": "finished", "ok": outcome["verified"] is not False, "outcome": outcome})
    except ff.Cancelled:
        # A cancel is a decision, not a failure: what was written is kept and the window
        # says so rather than showing an error.
        job.outcome = None
        job.error = None
        publish({"type": "cancelled"})
    except Exception as failure:  # noqa: BLE001 - the worker must never die silently
        job.error = str(failure)
        publish({"type": "log", "level": "error", "text": traceback.format_exc(limit=3)})
        publish({"type": "failed", "message": str(failure)})
    finally:
        job.done.set()
        with CURRENT_LOCK:
            if CURRENT is job:
                CURRENT = None


# ---------------------------------------------------------------------------------------
# Routes
# ---------------------------------------------------------------------------------------

async def health(_request: web.Request) -> web.Response:
    """What the window asks on startup: can this machine cut, and with what.

    The answer is deliberately small. `ffmpeg -encoders` prints about eleven kilobytes of
    codec list, and the only thing the window does with it is decide whether the lamp is
    green — so the decision is made here and the list is not sent.
    """
    answer: dict[str, Any] = {"version": VERSION, "ffmpeg": None, "ffprobe": None, "libx264": False}
    try:
        answer["ffmpeg"] = ff.version("ffmpeg").splitlines()[0]
        answer["ffprobe"] = ff.version("ffprobe").splitlines()[0]
        encoders = ff.encoders()
        answer["libx264"] = "libx264" in encoders
        answer["libx265"] = "libx265" in encoders
    except Exception as failure:  # noqa: BLE001
        answer["error"] = str(failure)
    return web.json_response(answer)


async def probe(request: web.Request) -> web.Response:
    path = Path(request.query.get("path", ""))
    if not path.is_file():
        return web.json_response({"error": f"{path} is not a file"}, status=400)
    try:
        media = await asyncio.to_thread(ff.probe, path)
    except Exception as failure:  # noqa: BLE001
        return web.json_response({"error": str(failure)}, status=400)

    transcript = subs.find_for(path)
    _warm_keyframes(path)
    _warm_silences(path)
    return web.json_response({
        "media": _describe(media),
        "transcript": None if transcript is None else str(transcript),
        "summary": media.summary(),
        "startTimecode": format_timecode(0, media.rate),
        "endTimecode": format_timecode(max(0, media.frames - 1), media.rate),
    })


def _warm_keyframes(path: Path) -> None:
    """Start reading a source's keyframes now, rather than when the first mark is typed.

    Planning needs the whole keyframe list, and on the reference master that read is the one slow
    part: measured at 3.5 s for the first plan of a source and 0.12 s for every plan after it,
    because the list is remembered per file. So the wait lands between typing a mark and being
    allowed to press Trim — which looks exactly like the cut having started, and was asked about
    as "why does the head re-encode occur before I can click Trim?".

    Moved here, the same read happens while the operator is looking at the file they just loaded.
    It is a fire-and-forget task on the same thread pool the plans use, so a plan arriving first
    simply waits for it rather than repeating it. Failure is swallowed on purpose: this is work
    brought forward, and a source whose keyframes cannot be listed has to fail in `plan_trim`,
    where the operator is told why, not here where nothing was asked for.

    The arguments are the ones `plan_trim` will use, taken the same way, because the cache key *is*
    the file's identity, its size and its modification time: warming it with a different key would
    do the whole read and then not be found.
    """
    async def read() -> None:
        try:
            stat = path.stat()
            await asyncio.to_thread(cutter._keyframes_of, str(path), stat.st_size, stat.st_mtime_ns)
        except Exception:  # noqa: BLE001
            pass

    _ = asyncio.get_running_loop().create_task(read())


# ---------------------------------------------------------------------------------------
# Where the sound stops: the map the marks snap into
# ---------------------------------------------------------------------------------------

#: Silence per source, keyed the way every cache in this engine is keyed: the path, its size and
#: its modification time, because those are what change when a file is replaced. A weak key here
#: would serve the previous file's gaps against the new file's footage, which is worse than no map.
_SILENCES: dict[tuple[str, int, int], list] = {}
_SILENCES_LOCK = threading.Lock()


def _silences_of(path: Path) -> list:
    """The cached gaps for a source, or an empty list. Never measures; see :func:`_warm_silences`."""
    try:
        stat = path.stat()
    except OSError:
        return []
    with _SILENCES_LOCK:
        return _SILENCES.get((str(path), stat.st_size, stat.st_mtime_ns), [])


def _warm_silences(path: Path) -> None:
    """Measure where a source's audio goes quiet, in the background, once.

    Brought forward for the same reason the keyframe list is: the operator should not wait for a
    measurement they did not ask for at the moment they ask for something else. The difference is
    that this one is *reported* rather than merely cached -- the window draws the gaps as bands
    and uses them to snap a mark into a pause, so the answer has to arrive as an event, not as a
    page load that happens to be slower.

    Silence is the audio equivalent of a keyframe: a place a cut costs nothing. A cut inside a
    word clips a consonant the operator cannot see in a frame-accurate picture, and the pause
    either side of it is the only place worth landing on.
    """
    async def measure() -> None:
        try:
            stat = path.stat()
            key = (str(path), stat.st_size, stat.st_mtime_ns)
            with _SILENCES_LOCK:
                if key in _SILENCES:
                    spans = _SILENCES[key]
                else:
                    spans = None
            if spans is None:
                spans = await asyncio.to_thread(quiet.silences, path)
                with _SILENCES_LOCK:
                    _SILENCES[key] = spans
            duration = (await asyncio.to_thread(ff.probe, path)).duration
            HUB.publish({
                "type": "silence",
                "source": str(path),
                "spans": [[round(span.start, 3), round(span.end, 3)] for span in spans],
                "speech": [[round(span.start, 3), round(span.end, 3)]
                           for span in quiet.speech(spans, duration)],
                "duration": duration,
            })
        except Exception:  # noqa: BLE001 - a map that cannot be drawn is not a failed load
            pass

    _ = asyncio.get_running_loop().create_task(measure())


async def silence(request: web.Request) -> web.Response:
    """The gaps for the loaded source, if they have been measured already.

    Asked on a reload, when the event that carried them has been and gone. An empty list means
    "not measured yet", which the window shows as no bands rather than as a file with no pauses.
    """
    path = Path(request.query.get("path", ""))
    if not path.is_file():
        return web.json_response({"error": f"{path} is not a file"}, status=400)
    return web.json_response({
        "spans": [[round(span.start, 3), round(span.end, 3)] for span in _silences_of(path)],
    })


async def snap(request: web.Request) -> web.Response:
    """Move a frame into the nearest pause, if there is one within reach.

    The rule lives in the engine (`trimmer.audio.quietest_frame`) rather than in the window, so
    that "the nearest pause" means one thing: a second implementation in TypeScript would drift
    from this one the first time either was tuned.
    """
    body = await request.json()
    source = Path(body.get("source", ""))
    if not source.is_file():
        return web.json_response({"error": "no source is loaded"}, status=400)
    try:
        frame = int(body["frame"])
    except (KeyError, TypeError, ValueError):
        return web.json_response({"error": "a frame number is required"}, status=400)
    media = await asyncio.to_thread(ff.probe, source)
    if frame < 0 or frame >= media.frames:
        return web.json_response({"error": f"frame {frame} is outside this file"}, status=400)
    spans = _silences_of(source)
    moved = quiet.quietest_frame(spans, frame, float(media.grid_rate))
    # The timecode comes back from here rather than being formatted in the window: drop-frame is
    # arithmetic, and a second copy of it in TypeScript is how two fields come to show the same
    # frame at different timecodes.
    return web.json_response({
        "frame": moved,
        "from": frame,
        "moved": moved != frame,
        "timecode": format_timecode(moved, media.rate),
    })


async def proof_image(request: web.Request) -> web.Response:
    """Serve one contact sheet by name, from the one folder they are written to.

    The name is checked against a pattern and the resolved path against the folder rather than
    the request against anything: a route that serves files by name is the shortest way to read
    somebody else's disk, and this one must be unable to leave its own directory even if the
    caller sends a traversal it thinks is clever.
    """
    name = request.query.get("name", "")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*\.png", name):
        return web.json_response({"error": "not a proof name"}, status=400)
    target = (PROOF_DIR / name).resolve()
    if PROOF_DIR.resolve() not in target.parents or not target.is_file():
        return web.json_response({"error": "no such proof"}, status=404)
    return web.FileResponse(target, headers={"Cache-Control": "no-store, must-revalidate"})


async def parse(request: web.Request) -> web.Response:
    """A timecode as a frame number, so a field can show its frame as it is typed."""
    body = await request.json()
    source = Path(body.get("source", ""))
    if not source.is_file():
        return web.json_response({"error": "no source is loaded"}, status=400)
    try:
        media = await asyncio.to_thread(ff.probe, source)
        frame = parse_timecode(body.get("text", ""), media.rate)
    except Exception as failure:  # noqa: BLE001
        return web.json_response({"error": str(failure)}, status=400)
    if frame >= media.frames:
        return web.json_response(
            {"error": f"{body.get('text')} is past the end of the source, which is "
                      f"{format_timecode(max(0, media.frames - 1), media.rate)}. "},
            status=400,
        )
    return web.json_response({"frame": frame, "timecode": format_timecode(frame, media.rate)})


def _spec_from(body: dict[str, Any]) -> cutter.TrimSpec:
    """A request body as the engine's own request type."""
    source = Path(body["source"])
    in_frame = int(body["inFrame"])
    end_frame = int(body["endFrame"])
    rate = float(body.get("rate", 25))
    # An explicit path wins, because that is what the API has always accepted and what the tests
    # pass. Otherwise the name decides: the typed one when there is one, and the range name when
    # there is not -- which is what the output has been called since the beginning.
    if body.get("output"):
        output = Path(body["output"])
    else:
        output = cutter.output_for(
            source, in_frame, end_frame - 1, rate,
            name=body.get("name"), in_folder=bool(body.get("inFolder")),
        )
    transcript = subs.find_for(source)
    return cutter.TrimSpec(
        source=source,
        output=output,
        in_frame=in_frame,
        out_frame=end_frame,
        crf=int(body.get("crf", 18)),
        preset=str(body.get("preset", "veryfast")),
        concat_offset=int(body.get("concatOffset", 0)),
        audio_bitrate=str(body.get("audioBitrate", "192k")),
        subtitles=None if body.get("subtitles") is False else transcript,
        # Not `body.get("overwrite", True)`. Permission to replace a file is the one default that
        # must not be generous: a request that does not say gets no, the engine refuses rather
        # than overwrites, and the window turns that refusal into a question. The reverse default
        # is silent, irreversible, and destroys somebody's previous cut before they see it.
        overwrite=body.get("overwrite") is True,
    )


def _refusal(
    failure: Exception,
    status: int = 400,
    reason: str | None = None,
    exists: list[str] | None = None,
) -> web.Response:
    """A refusal, with a tag when the window has to place it differently from the others.

    Two tags so far, and both exist for the same reason: the window cannot tell from the sentence
    alone whether a refusal belongs beside the name field, or in a dialog asking whether to replace
    a file. A stable tag can be relied on; matching on prose cannot, because the prose is meant to
    be rewritten whenever it can be said better.

    ``exists`` travels with the replace refusal so that the dialog names the same file the plan
    would have named. The sentence carries the names too, and a window that dug them back out of
    prose would be parsing the one thing this project keeps free to reword -- so the structured
    answer and the sentence are both sent, and the window reads the structured one.
    """
    payload: dict[str, Any] = {"error": str(failure)}
    if isinstance(failure, cutter.NameRefused):
        payload["reason"] = "name"
    elif reason is not None:
        payload["reason"] = reason
    if exists:
        payload["exists"] = exists
    return web.json_response(payload, status=status)


async def name_output(request: web.Request) -> web.Response:
    """Where a named segment will be written, without reading the source.

    The output path of a named segment does not depend on the marks -- it is the source's folder
    and the name -- so this answers before a range exists and without a probe, which is what makes
    it usable while somebody is typing. That matters more than it looks: the first version of the
    window worked this out from the *plan*, and the plan does not run until both marks are set, so
    typing a name before marking a range appeared to refuse every name.

    The rule still lives in one place, `cutter.output_for`; this only reaches it earlier.
    """
    body = await request.json()
    name = body.get("name")
    if name is None or not str(name).strip():
        # No name is not a refusal, and there is nothing to preview: the range name needs the marks.
        return web.json_response({"output": None})
    source = Path(body["source"])
    try:
        output = cutter.output_for(
            source,
            int(body.get("inFrame") or 0),
            int(body.get("endFrame") or 1) - 1,
            float(body.get("rate") or 25),
            name=str(name),
            in_folder=bool(body.get("inFolder")),
        )
    except Exception as failure:  # noqa: BLE001
        return _refusal(failure)
    return web.json_response({"output": str(output)})


async def plan(request: web.Request) -> web.Response:
    """What a range will do, without writing anything. Called as the marks are typed."""
    body = await request.json()
    try:
        spec = _spec_from(body)
        media = await asyncio.to_thread(ff.probe, spec.source)
        worked = await asyncio.to_thread(cutter.plan_trim, spec, media)
    except Exception as failure:  # noqa: BLE001
        return _refusal(failure)
    # Whether this cut would replace something, answered here because this is the one request the
    # window already sends while the marks are being typed and before Trim is pressed. Knowing it
    # in advance is what lets the question be asked *instead of* the cut, rather than as a refusal
    # that arrives once the operator has already committed to the work.
    return web.json_response({
        "plan": _plan_json(spec, worked, media),
        "output": str(spec.output),
        "transcript": None if spec.subtitles is None else str(spec.subtitles),
        "exists": [piece.name for piece in cutter.existing_pieces(spec)],
    })


async def cut(request: web.Request) -> web.Response:
    """Start a cut. Returns at once; the work is reported on ``/api/events``."""
    global CURRENT
    body = await request.json()
    try:
        spec = _spec_from(body)
    except Exception as failure:  # noqa: BLE001
        return _refusal(failure)

    # Asked here, before a worker exists and before anything is written, so that a window which
    # never saw the plan -- one that raced ahead of it, or was reopened on an old range -- is still
    # asked rather than quietly obeyed. The engine refuses the same cut on its own; this only makes
    # the refusal cheap, and tagged, so it can be turned into a question.
    already_there = cutter.existing_pieces(spec)
    if already_there and not spec.overwrite:
        return _refusal(
            cutter.TrimError(cutter.replace_refusal(already_there)),
            status=409,
            reason="exists",
            exists=[piece.name for piece in already_there],
        )

    with CURRENT_LOCK:
        if CURRENT is not None:
            return web.json_response(
                {"error": "a cut is already running; cancel it first"}, status=409
            )
        HUB.reset()
        job = Job(spec, str(body.get("verify", "standard")))
        CURRENT = job

    job.thread = threading.Thread(target=_run_job, args=(job,), daemon=True, name="trim")
    job.thread.start()
    return web.json_response({"started": True, "output": str(spec.output)})


async def cancel(_request: web.Request) -> web.Response:
    with CURRENT_LOCK:
        job = CURRENT
    if job is None:
        return web.json_response({"cancelled": False, "reason": "nothing is running"})
    job.cancel.cancel()
    return web.json_response({"cancelled": True})


async def events(request: web.Request) -> web.StreamResponse:
    """The stream: everything a run says, as it says it.

    ## A client that goes away is not an error

    Windows reports a closed loopback connection as `ConnectionResetError` — and on a socket
    that is being written to, `ClientConnectionResetError` — from `prepare` as well as from
    `write`. None of that is a fault: the window was reloaded, or closed, or a subscriber
    simply finished. Letting it reach aiohttp's handler produces a full traceback per event,
    which buries whatever the engine was actually saying. So every one of them is caught, and
    the only thing that happens is the subscriber leaving the list.
    """
    response = web.StreamResponse(
        status=200,
        headers={
            "Content-Type": "text/event-stream",
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
        },
    )
    channel: queue.Queue[dict[str, Any]] = queue.Queue(maxsize=2048)
    try:
        await response.prepare(request)
        channel = HUB.subscribe()
        while True:
            try:
                event = await asyncio.to_thread(channel.get, True, 0.5)
            except queue.Empty:
                # A comment frame, so a proxy or a sleeping window keeps the pipe open and
                # the browser does not decide the stream has died.
                await response.write(b": keep-alive\n\n")
                continue
            payload = json.dumps(event, default=str)
            await response.write(f"data: {payload}\n\n".encode())
    except (ConnectionResetError, ConnectionError, asyncio.CancelledError):
        pass
    finally:
        HUB.unsubscribe(channel)
    return response


async def index(_request: web.Request) -> web.Response:
    return web.json_response({
        "version": VERSION,
        "routes": ["health", "probe", "parse", "plan", "cut", "cancel", "events"],
    })


# ---------------------------------------------------------------------------------------
# The window itself
# ---------------------------------------------------------------------------------------

#: Where the built interface lives, relative to this file: `<root>/frontend/dist`.
INTERFACE = Path(__file__).resolve().parent.parent / "frontend" / "dist"


async def interface(request: web.Request) -> web.Response:
    """Serve the built page, so the window and the engine are one origin.

    ## Why the engine serves the interface and not Electron

    It was `loadFile` first, and it cannot work. A Vite build is an ES module, and Chromium
    refuses a module script loaded from `file://` — the origin is opaque, so the fetch fails
    CORS before a line of the app runs. Even had it loaded, every call to the engine would
    have been `file://` -> `http://127.0.0.1:8765`, which is cross-origin too, so the page
    would have had to be given CORS headers to talk to its own backend.

    Serving the two from one origin deletes both problems rather than working around them,
    and it is one handler. The alternative — an IIFE build plus `Access-Control-Allow-Origin`
    on every response — is more moving parts to arrive at the same place.
    """
    wanted = request.match_info.get("path", "")
    # The API is registered before this and matched first; anything under `api/` that reached
    # here does not exist, and answering it with the interface would be a lie.
    if wanted == "api" or wanted.startswith("api/"):
        raise web.HTTPNotFound()

    target = (INTERFACE / wanted).resolve() if wanted else INTERFACE / "index.html"
    # `..` in a request is not a path, it is an attempt. Refuse anything outside the build.
    if target != INTERFACE and INTERFACE not in target.parents:
        raise web.HTTPNotFound()
    if not target.is_file():
        # A single-page app has one document; an unknown path is a route the page handles.
        target = INTERFACE / "index.html"
    if not target.is_file():
        return web.json_response(
            {"error": f"the interface has not been built yet ({INTERFACE})"}, status=503
        )
    response = web.FileResponse(target)
    # The bundle's files keep their names across builds -- `app.js` is `app.js` -- so a copy held in
    # the window's cache is indistinguishable from the one that was just built, and Chromium will
    # reuse it on the strength of a `Last-Modified` alone. That is a window showing an interface
    # that no longer exists, which is indistinguishable from a change that never took effect.
    #
    # Nothing is lost by refusing to cache: this is a file on the same disk as the window that reads
    # it, read once at startup. `no-store` rather than `no-cache`, because there is no version worth
    # revalidating -- the point is that the next launch reads what is there.
    response.headers["Cache-Control"] = "no-store, must-revalidate"
    return response


def _prune_proofs() -> None:
    """Forget proofs older than a few days. They describe cuts that are already delivered."""
    try:
        cutoff = time.time() - PROOF_DAYS * 86400
        for stale in PROOF_DIR.glob("*.png"):
            if stale.stat().st_mtime < cutoff:
                stale.unlink(missing_ok=True)
    except OSError:
        pass


async def _watch_parent(_app: web.Application) -> None:
    """End the engine when the window that started it is gone.

    Windows does not kill a process's children when the parent dies, and Electron quits by
    killing *this* process. Without this, a closed window leaves ffmpeg encoding a master with
    the file locked and the job in no state at all -- so the engine watches the shell and takes
    its children with it. A quit that goes through the shell first is still the ordinary path;
    this is what covers the window being ended by something else: the task manager, a crash, a
    session ending.

    Two seconds of latency is the trade: a watcher that polled hard would wake the CPU through
    an idle evening for nothing, and an orphan that lives two seconds longer than it should is
    not the failure this exists to prevent.
    """
    if PARENT_PID <= 0:
        return
    while True:
        await asyncio.sleep(2.0)
        if processes.parent_alive(PARENT_PID):
            continue
        with CURRENT_LOCK:
            job = CURRENT
        if job is not None:
            job.cancel.cancel()
        processes.terminate_all()
        print("[thetrimmer] the window is gone; stopping with it", flush=True)
        # Nothing left to flush: the deliverable is either finished or was never published, and
        # the pieces are worthless without the job that knew what they were.
        os._exit(0)


async def _start_engine(app: web.Application) -> None:
    _prune_proofs()
    _ = asyncio.get_running_loop().create_task(_watch_parent(app))


async def _stop_engine(_app: web.Application) -> None:
    """Every exit path through aiohttp ends the children, not just the watched one."""
    processes.terminate_all()


def build_app() -> web.Application:
    app = web.Application()
    app.add_routes([
        web.get("/api", index),
        web.get("/api/health", health),
        web.get("/api/probe", probe),
        web.get("/api/silence", silence),
        web.post("/api/snap", snap),
        web.post("/api/parse", parse),
        web.post("/api/plan", plan),
        web.post("/api/name", name_output),
        web.post("/api/cut", cut),
        web.post("/api/cancel", cancel),
        web.get("/api/events", events),
        web.get("/api/proof", proof_image),
    ])
    app.on_startup.append(_start_engine)
    app.on_cleanup.append(_stop_engine)
    # Last, and a catch-all on purpose: `/api/health` was registered first and wins, so the
    # only requests that reach this are ones for the page or for a file the page needs.
    app.add_routes([web.get("/", interface), web.get("/{path:.*}", interface)])
    return app


if __name__ == "__main__":
    print(f"TheTrimmer backend {VERSION} on http://127.0.0.1:{PORT}", flush=True)
    # No access log: every event-stream reconnect would be a line, and the console belongs to
    # what the engine is saying about the work, not to a list of requests from one window.
    web.run_app(build_app(), host="127.0.0.1", port=PORT, print=None, access_log=None)
