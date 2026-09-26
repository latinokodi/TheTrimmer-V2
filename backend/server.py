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
import sys
import threading
import time
import traceback
from dataclasses import asdict, is_dataclass
from fractions import Fraction
from pathlib import Path
from typing import Any

from aiohttp import web

sys.path.insert(0, str(Path(__file__).resolve().parent))

from trimmer import ffmpeg as ff  # noqa: E402
from trimmer import subtitles as subs  # noqa: E402
from trimmer import trim as cutter  # noqa: E402
from trimmer import verify as verifier  # noqa: E402
from trimmer.timecode import format_timecode, parse_timecode  # noqa: E402

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")

VERSION = "2.0.0"
PORT = int(os.environ.get("PORT", "8765"))
VIDEO_SUFFIXES = [".mp4", ".mov", ".mkv", ".m4v", ".mxf", ".avi", ".webm", ".mts", ".m2ts"]


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

        report = cutter.trim(spec, log=log, cancel=job.cancel, progress=progress, media=media)
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
                "written": str(report.subtitles.path) if report.subtitles.path else None,
                "clamped": report.subtitles.clamped,
            }

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
    return web.json_response({
        "media": _describe(media),
        "transcript": None if transcript is None else str(transcript),
        "summary": media.summary(),
        "startTimecode": format_timecode(0, media.rate),
        "endTimecode": format_timecode(max(0, media.frames - 1), media.rate),
    })


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
    output = Path(body["output"]) if body.get("output") else cutter.default_output(
        source, in_frame, end_frame - 1, float(body.get("rate", 25))
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
        head_audio_copy=bool(body.get("headAudioCopy", False)),
        audio_bitrate=str(body.get("audioBitrate", "192k")),
        subtitles=None if body.get("subtitles") is False else transcript,
    )


async def plan(request: web.Request) -> web.Response:
    """What a range will do, without writing anything. Called as the marks are typed."""
    body = await request.json()
    try:
        spec = _spec_from(body)
        media = await asyncio.to_thread(ff.probe, spec.source)
        worked = await asyncio.to_thread(cutter.plan_trim, spec, media)
    except Exception as failure:  # noqa: BLE001
        return web.json_response({"error": str(failure)}, status=400)
    return web.json_response({
        "plan": _plan_json(spec, worked, media),
        "output": str(spec.output),
        "transcript": None if spec.subtitles is None else str(spec.subtitles),
    })


async def cut(request: web.Request) -> web.Response:
    """Start a cut. Returns at once; the work is reported on ``/api/events``."""
    global CURRENT
    body = await request.json()
    try:
        spec = _spec_from(body)
    except Exception as failure:  # noqa: BLE001
        return web.json_response({"error": str(failure)}, status=400)

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
    """The stream: everything a run says, as it says it."""
    response = web.StreamResponse(
        status=200,
        headers={
            "Content-Type": "text/event-stream",
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
        },
    )
    await response.prepare(request)
    channel = HUB.subscribe()
    try:
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
    except (ConnectionResetError, asyncio.CancelledError):
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
    return web.FileResponse(target)


def build_app() -> web.Application:
    app = web.Application()
    app.add_routes([
        web.get("/api", index),
        web.get("/api/health", health),
        web.get("/api/probe", probe),
        web.post("/api/parse", parse),
        web.post("/api/plan", plan),
        web.post("/api/cut", cut),
        web.post("/api/cancel", cancel),
        web.get("/api/events", events),
    ])
    # Last, and a catch-all on purpose: `/api/health` was registered first and wins, so the
    # only requests that reach this are ones for the page or for a file the page needs.
    app.add_routes([web.get("/", interface), web.get("/{path:.*}", interface)])
    return app


if __name__ == "__main__":
    print(f"TheTrimmer backend {VERSION} on http://127.0.0.1:{PORT}", flush=True)
    web.run_app(build_app(), host="127.0.0.1", port=PORT, print=None)
