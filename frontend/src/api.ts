/**
 * Talking to the engine.
 *
 * ## What changed in the migration, and what did not
 *
 * This file is the whole of the difference between the old shell and this one. The engine
 * is the same Python that has always done the cutting; the interface is the same React and
 * the same stylesheet. What was Tauri's `invoke` is now `fetch` against the backend, and
 * what was a Tauri event is now a server-sent event stream.
 *
 * That is deliberate. The window is a web page either way, so the seam was always HTTP-
 * shaped — the previous shell just hid it behind a bridge that had to be compiled.
 *
 * ## Why events and not polling
 *
 * A cut is minutes of ffmpeg with nothing to look at. The bar has to move *while* it
 * works, so the backend pushes: every stage, every exact command line, every position
 * ffmpeg reports. Polling would either be late or be a metronome, and neither is progress.
 */

/**
 * Where the engine is.
 *
 * Three cases, in order of how much is known:
 *
 * * the window started the engine and says which port it got — that is the answer;
 * * the page was served *by* the engine, so its own origin is the engine;
 * * neither, which means the Vite dev server, whose origin is not the engine and which
 *   therefore has to be told where the engine is.
 */
const FALLBACK_BASE = "http://127.0.0.1:8765";

function locate(): string {
  if (import.meta.env.DEV) {
    return FALLBACK_BASE;
  }
  if (window.location.protocol === "http:" || window.location.protocol === "https:") {
    return window.location.origin;
  }
  return FALLBACK_BASE;
}

let resolved: Promise<string> | null = null;

function base(): Promise<string> {
  if (resolved === null) {
    resolved = Promise.resolve(desktop.backendUrl?.() ?? locate())
      .then((found) => (found === "" ? locate() : found))
      .catch(() => locate());
  }
  return resolved;
}

/** A file's facts, as the probe reports them. */
export interface MediaView {
  readonly path: string;
  readonly name: string;
  readonly codec: string;
  readonly width: number;
  readonly height: number;
  readonly pixFmt: string;
  readonly rate: { readonly numerator: number; readonly denominator: number };
  readonly rateText: string;
  readonly frames: number;
  readonly duration: number;
  readonly durationText: string;
  readonly sizeBytes: number;
  readonly variableRate: boolean;
  readonly audio: {
    readonly codec: string;
    readonly sampleRate: number;
    readonly channels: number;
  } | null;
}

/** One verification row, as the proof panel draws it. */
export interface CheckView {
  readonly check: string;
  readonly status: { readonly kind: string };
  readonly detail: string;
}

/** What a range will do, before anything is written. */
export interface PlanView {
  readonly mode: "copy" | "headpatch" | "reencode";
  readonly keyframe: number;
  readonly headFrames: number;
  readonly bodyFrames: number;
  readonly requested: number;
  readonly reencodeFraction: number;
  readonly inFrame: number;
  readonly endFrame: number;
  readonly frames: number;
  readonly seconds: number;
  readonly inTimecode: string;
  readonly outTimecode: string;
  readonly notes: readonly string[];
}

/** The result of a cut, and everything measured against it. */
export interface OutcomeView {
  readonly output: string;
  readonly frames: number;
  readonly duration: number;
  readonly overshoot: number;
  readonly source: string;
  readonly inFrame: number;
  readonly outFrame: number;
  readonly mode: string;
  readonly plan: PlanView;
  readonly checks: readonly CheckView[];
  readonly verified: boolean | null;
  readonly subtitles: {
    readonly cues: number;
    readonly written: string | null;
    readonly clamped: number;
  } | null;
}

/** One reading of where a pass has got to. */
export interface TickView {
  readonly outSeconds: number;
  readonly fraction: number | null;
  readonly speed: number | null;
  readonly remaining: number | null;
  readonly frame: number | null;
  readonly size: number | null;
  readonly expectedSeconds: number | null;
}

/** Anything the engine says while it works. */
export type EngineEvent =
  | { readonly type: "started"; readonly output: string; readonly source: string }
  | { readonly type: "source"; readonly media: MediaView }
  | { readonly type: "stage"; readonly name: string; readonly label: string }
  | { readonly type: "log"; readonly level: string; readonly text: string }
  | { readonly type: "command"; readonly args: readonly string[] }
  | ({ readonly type: "progress" } & TickView)
  | { readonly type: "finished"; readonly ok: boolean; readonly outcome: OutcomeView }
  | { readonly type: "failed"; readonly message: string }
  | { readonly type: "cancelled" };

/**
 * Raised with the backend's own sentence, so the window never invents a reason.
 *
 * `reason` is the engine's tag for *which* refusal this is. It exists for the one case the window
 * has to place differently: a refused segment name belongs beside the field it was typed into, and
 * telling that apart from the engine's other refusals by matching on prose would break the moment
 * the prose was improved.
 */
export class ApiFailure extends Error {
  readonly reason: string | null;

  constructor(message: string, reason: string | null = null) {
    super(message);
    this.name = "ApiFailure";
    this.reason = reason;
  }
}

async function send<T>(path: string, body?: unknown, method = "POST"): Promise<T> {
  const response = await fetch(`${await base()}${path}`, {
    method: body === undefined && method === "POST" ? "GET" : method,
    ...(body === undefined ? {} : { headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }),
  });
  const payload: unknown = await response.json().catch(() => ({}));
  if (!response.ok) {
    const shaped = typeof payload === "object" && payload !== null ? (payload as Record<string, unknown>) : {};
    const message = "error" in shaped ? String(shaped["error"]) : `${path} failed (${response.status})`;
    const reason = "reason" in shaped ? String(shaped["reason"]) : null;
    throw new ApiFailure(message, reason);
  }
  return payload as T;
}

export interface Health {
  readonly version: string;
  readonly ffmpeg: string | null;
  readonly ffprobe: string | null;
  readonly libx264: boolean;
  readonly libx265?: boolean;
  readonly error?: string;
}

export const api = {
  health: () => send<Health>("/api/health", undefined, "GET"),

  probe: (path: string) =>
    send<{ media: MediaView; summary: string; transcript: string | null; endTimecode: string }>(
      `/api/probe?path=${encodeURIComponent(path)}`,
      undefined,
      "GET",
    ),

  /** A timecode as a frame number. Throws {@link ApiFailure} with the engine's own sentence. */
  parse: (source: string, text: string) =>
    send<{ frame: number; timecode: string }>("/api/parse", { source, text }),

  plan: (input: {
    readonly source: string;
    readonly inFrame: number;
    readonly endFrame: number;
    readonly rate: number;
    readonly crf: number;
    readonly preset: string;
    /** The segment's own name. Empty leaves the output called after its range. */
    readonly name?: string;
    /** Put the segment and its transcript in a folder of that name. */
    readonly inFolder?: boolean;
  }) => send<{ plan: PlanView; output: string; transcript: string | null }>("/api/plan", input),

  /**
   * Where a named segment will be written — answered without reading the source.
   *
   * Separate from `plan` because a plan needs both marks and this does not: the output path of a
   * named segment is the source's folder and the name, so it can be resolved while somebody is
   * still typing, and a refused name can be reported before there is a range to plan.
   * `output` is null when no name was given, which is not an error.
   */
  name: (input: {
    readonly source: string;
    readonly name: string;
    readonly inFolder: boolean;
    readonly inFrame?: number;
    readonly endFrame?: number;
    readonly rate?: number;
  }) => send<{ output: string | null }>("/api/name", input),

  cut: (input: {
    readonly source: string;
    readonly inFrame: number;
    readonly endFrame: number;
    readonly rate: number;
    readonly crf: number;
    readonly preset: string;
    readonly verify: string;
    readonly output?: string;
    readonly name?: string;
    readonly inFolder?: boolean;
  }) => send<{ started: boolean; output: string }>("/api/cut", input),

  cancel: () => send<{ cancelled: boolean }>("/api/cancel", {}),
};

/**
 * Listen to a run.
 *
 * Returns the function that stops listening, so the caller can drop the stream when the
 * window closes or the component unmounts. `EventSource` reconnects on its own, which is
 * why the backend replays its recent history to a subscriber that arrives late: a window
 * that was busy for a second catches up rather than showing a gap.
 */
export function listen(onEvent: (event: EngineEvent) => void): () => void {
  let close: (() => void) | null = null;
  let stopped = false;
  void base().then((root) => {
    if (stopped) {
      return;
    }
    const source = new EventSource(`${root}/api/events`);
    source.onmessage = (message: MessageEvent<string>) => {
      try {
        onEvent(JSON.parse(message.data) as EngineEvent);
      } catch {
        // A frame that will not parse is not a reason to tear down the stream.
      }
    };
    close = () => source.close();
  });
  return () => {
    stopped = true;
    close?.();
  };
}

/** The file dialog and Explorer, which belong to the window rather than to the page. */
interface ElectronBridge {
  readonly openVideo?: () => Promise<string | null>;
  readonly reveal?: (path: string) => Promise<void>;
  readonly toggleFullscreen?: () => Promise<boolean>;
  readonly isFullscreen?: () => Promise<boolean>;
  readonly backendUrl?: () => Promise<string>;
}

export const desktop = (window as unknown as { readonly electronAPI?: ElectronBridge }).electronAPI ?? {};

/** Ask for a video. `null` when the operator cancels, which is a decision and not a fault. */
export async function openVideo(): Promise<string | null> {
  return (await desktop.openVideo?.()) ?? null;
}

/**
 * Show a finished file in Explorer.
 *
 * This is the window's job and not the engine's: a file manager is a thing the operating
 * system owns, and a loopback HTTP route that shells out to `explorer.exe` on any path a
 * request names is a route that did not need to exist.
 */
export async function reveal(path: string): Promise<void> {
  if (desktop.reveal === undefined) {
    throw new ApiFailure("Showing a file in Explorer needs the desktop window; there is none.");
  }
  await desktop.reveal(path);
}
