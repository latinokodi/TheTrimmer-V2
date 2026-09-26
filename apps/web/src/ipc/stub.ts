/**
 * The bridge, faked, so the interface runs in a plain browser.
 *
 * ## Why this exists
 *
 * The window is a webview loading `dist`, and the only thing it needs from outside is one function:
 * `window.__TAURI__.core.invoke`. So if a browser tab is given that same function, the entire
 * interface — every empty state, the marks, the live frame numbers, the queue, the proof panel —
 * runs with no Rust, no exe, no WebView2 and no installer. `npm run dev` and a refresh is the whole
 * loop, and a test drives the same page in about a second.
 *
 * That is not a mock of the interface. It is the interface, calling the same
 * `apps/web/src/ipc/commands.ts` functions the shipped window calls, with the answers a real project
 * would give. The only thing replaced is the far end of one function.
 *
 * ## What this can and cannot catch
 *
 * It is typed as `Record<CommandName, …>`, so a command added to `COMMAND_NAMES` and not stubbed here
 * is a **compile error** — the failure mode being prevented is a button that works in the window and
 * does nothing in every browser test, which is the sort of gap a test suite never notices.
 *
 * What it cannot catch is a *shape* mismatch: if `add_source` stopped returning `summary`, this stub
 * would keep returning it and the browser tests would keep passing. That half of the contract is
 * `apps/desktop/src-tauri/tests/ipc_contract.rs`, which drives the real commands through Tauri's real
 * invoke handler and asserts on the real JSON. The two are deliberately different tests of different
 * things, and neither is a substitute for the other.
 *
 * ## Why the numbers are realistic
 *
 * Fixtures are a 25 fps master with 3 101 frames, ranges that land on real keyframe boundaries, and a
 * `headPatch` plan when the in point is not one — because a fixture with round numbers hides the
 * arithmetic bugs the interface exists to show. `frameCount` is deliberately not a round number and
 * the timecode arithmetic here is the same arithmetic the domain does.
 */

import type { CommandName } from "./commands";

/** The fixture project, and the one master in it. */
const FIXTURE_FRAMES = 3101;
const FIXTURE_RATE = { num: 25, den: 1 };
const FIXTURE_PATH = "H:\\masters\\reel 2\\A007C012_250312_R1QK.mov";
const FIXTURE_NAME = "A007C012_250312_R1QK.mov";

/** Frames to `HH:MM:SS:FF` at a whole-number frame rate. */
function stamp(frame: number, rate = FIXTURE_RATE.num / FIXTURE_RATE.den): string {
  const fps = Math.round(rate);
  const total = Math.max(0, Math.floor(frame));
  const seconds = Math.floor(total / fps);
  const ff = total % fps;
  const ss = seconds % 60;
  const mm = Math.floor(seconds / 60) % 60;
  const hh = Math.floor(seconds / 3600);
  return [hh, mm, ss, ff].map((part) => String(part).padStart(2, "0")).join(":");
}

/** `HH:MM:SS:FF` back to a frame number. The inverse of {@link stamp}. */
function parse(text: string, rate = FIXTURE_RATE.num / FIXTURE_RATE.den): number {
  const parts = text.trim().split(":").map((part) => Number(part));
  if (parts.length !== 4 || parts.some((part) => !Number.isFinite(part) || part < 0)) {
    throw new Error(`"${text}" is not a timecode. Write it as HH:MM:SS:FF.`);
  }
  const [hh = 0, mm = 0, ss = 0, ff = 0] = parts;
  const fps = Math.round(rate);
  if (ff >= fps) {
    throw new Error(`"${text}" has frame ${ff}, but this source runs at ${fps} fps.`);
  }
  return ((hh * 60 + mm) * 60 + ss) * fps + ff;
}

/** A keyframe every second, which is what the fixture master has. */
const KEYFRAME_EVERY = 25;
function isKeyframe(frame: number): boolean {
  return frame % KEYFRAME_EVERY === 0;
}
function nextKeyframe(frame: number): number | null {
  for (let probe = frame; probe < FIXTURE_FRAMES; probe += 1) {
    if (isKeyframe(probe)) {
      return probe;
    }
  }
  return null;
}

const mockMedia = {
  path: FIXTURE_PATH,
  codec: "prores",
  pixFmt: "yuv422p10le",
  width: 3840,
  height: 2160,
  rate: FIXTURE_RATE,
  averageRate: FIXTURE_RATE,
  timebase: { ticksPerSecond: 25 },
  frameCount: FIXTURE_FRAMES,
  audio: { codec: "pcm_s24le", sampleRate: 48000, channels: 2 },
  sizeBytes: 8_412_663_808,
  startTime: 0,
};

interface StubSegment {
  id: string;
  source: string;
  name: string;
  startFrame: number;
  endFrame: number | null;
  preset: string | null;
  handleFrames: number;
  enabled: boolean;
}

/** The state a stub session accumulates, so a workflow can be driven from start to finish. */
interface StubState {
  projects: { id: string; name: string; updatedAt: number }[];
  openProject: string | null;
  /** The masters in this session, in the order they were added. A project holds as many as it is given. */
  sources: StubSource[];
  segments: StubSegment[];
  runs: number;
  /** The last path `reveal` was asked for. A browser cannot open Explorer; this records the intent. */
  revealed: string | null;
  /** What `setFullscreen` was last asked for. A browser tab cannot go fullscreen; see the bridge. */
  fullscreen: boolean;
}

/**
 * The event bridge, faked so a test can drive the progress display.
 *
 * ## Why this is test scaffolding rather than a simulation
 *
 * In the window, progress arrives from Rust: the shell emits `cut-progress` as the engine works, and the
 * interface renders a bar and a log from it. There is no Rust in a browser, so without this the entire
 * progress display — the bar, the fraction, the rate, the estimate, the log's shape — could only ever be
 * checked by running a real cut in the window. That is the gap that hid the last two faults: a fixture
 * that cannot represent the state leaves the code that handles it untested (see ADR-021).
 *
 * What this deliberately does **not** do is invent the events. `emit` hands the caller's payload to the
 * listeners verbatim; it is a wire, not a producer. The *shape* of the payload is asserted against the
 * real Rust serialisation in `apps/desktop/src-tauri/tests/ipc_contract.rs`, which is what keeps a test
 * that drives this bridge from passing on a shape Rust does not send.
 */
class StubEvents {
  private readonly listeners = new Map<string, ((event: { payload: unknown }) => void)[]>();

  async listen<T>(
    name: string,
    handler: (event: { readonly payload: T }) => void,
  ): Promise<() => void> {
    const existing = this.listeners.get(name) ?? [];
    existing.push(handler as (event: { payload: unknown }) => void);
    this.listeners.set(name, existing);
    // Tauri's `listen` resolves to the function that stops listening, and the interface relies on that.
    return () => {
      const current = this.listeners.get(name) ?? [];
      this.listeners.set(
        name,
        current.filter((candidate) => candidate !== handler),
      );
    };
  }

  /** Deliver a payload to whatever is listening. The test driver; the window gets this from Rust. */
  emit(name: string, payload: unknown): void {
    for (const handler of this.listeners.get(name) ?? []) {
      handler({ payload });
    }
  }
}

const stubEvents = new StubEvents();

/** Wait, so a narrated run takes a moment rather than a tick. */
function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function freshState(): StubState {
  return {
    projects: [],
    openProject: null,
    sources: [],
    segments: [],
    runs: 0,
    revealed: null,
    fullscreen: false,
  };
}

/**
 * A refusal, as a command would give it.
 *
 * An `Error` rather than a bare string so the rejection carries a stack in the browser's console,
 * which is where a developer reads it. The interface turns either into the same `IpcFailure`.
 */
function refuse(message: string): never {
  throw new Error(message);
}

let state = freshState();

/** The fixture project's path and name, for a test that wants to type or find them. */
export const FIXTURE = {
  path: FIXTURE_PATH,
  name: FIXTURE_NAME,
} as const;

/**
 * Reset the fixture project to empty.
 *
 * Exported for a test that drives two workflows in one page without a reload. Nothing in this
 * repository calls it yet, and it is kept deliberately: an export with no call site is a warning
 * sign, and this one has a specific one — the moment a test needs two scenarios in one `page.goto`,
 * the alternative is a reload, which is slower and hides state bugs.
 */
export function resetStub(): void {
  state = freshState();
}

let sequence = 0;
function id(): string {
  sequence += 1;
  // A real segment id is a uuid and the interface only ever displays it, so the shape is what
  // matters: 36 characters, five groups, hex where the real one is hex.
  const tail = String(sequence).padStart(12, "0");
  return `01a0de58-609a-7018-99bb-${tail}`;
}

/**
 * One master in the fixture session.
 *
 * ## Why the session holds a list rather than a single source
 *
 * It held one, behind a `hasSource` boolean, and that is why a real defect shipped: the window's Browse
 * button added a second master to the project and the panel went on showing the first, with no way to
 * switch. In a browser the same sequence produced *one* source — because the stub could not represent
 * two — so the panel displaying "the" source looked perfectly correct. A fixture that cannot express the
 * state a bug lives in will not find that bug.
 *
 * Every fixture source reports the same media, differing in path, name and caption file, which is
 * exactly enough to tell them apart and to drive the source switcher.
 */
interface StubSource {
  path: string;
  name: string;
  transcript: string | null;
  transcriptCues: number;
}

/** The master the picker hands back, and the shape every fixture source is built from. */
const FIXTURE_TRANSCRIPT = "H:\\masters\\reel 2\\A007C012_250312_R1QK.srt";
const FIXTURE_CUES = 214;

/** The base name of a Windows path, for a second source added by a test. */
function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}

const sourceView = (source: StubSource) => ({
  path: source.path,
  name: source.name,
  present: true,
  media: mockMedia,
  summary: `3840x2160 prores yuv422p10le, 25 fps, ${FIXTURE_FRAMES} frames, pcm_s24le 48000 Hz 2ch`,
  transcript: source.transcript,
  transcriptCues: source.transcript === null ? null : source.transcriptCues,
  label: null,
  variableRate: false,
});

function segmentView(segment: StubSegment): unknown {
  const end = segment.endFrame ?? FIXTURE_FRAMES;
  const frames = Math.max(0, end - segment.startFrame);
  return {
    id: segment.id,
    name: segment.name,
    sourceName: baseName(segment.source),
    inTimecode: stamp(segment.startFrame),
    outTimecode: stamp(Math.max(segment.startFrame, end - 1)),
    endFrame: segment.endFrame,
    frames,
    seconds: frames / (FIXTURE_RATE.num / FIXTURE_RATE.den),
    handleFrames: segment.handleFrames,
    enabled: segment.enabled,
    problems: [],
    notes: [],
  };
}

/**
 * The plan for a range.
 *
 * The mode is computed the way the domain computes it — a keyframe at the in point is a pure copy, a
 * keyframe further in is a head patch, none at all is a full re-encode — because the interface's job
 * here is to show which of the three is about to happen, and a stub that always said `copy` would
 * make that untestable.
 */
function planFor(segment: StubSegment): unknown {
  const start = segment.startFrame;
  const end = segment.endFrame ?? FIXTURE_FRAMES;
  const frames = Math.max(0, end - start);
  const keyframe = nextKeyframe(start);

  if (isKeyframe(start)) {
    return {
      mode: "copy",
      startFrame: start,
      endFrame: end,
      keyframe: start,
      headFrames: 0,
      bodyFrames: frames,
      concatOffset: 0,
      videoTimescale: 25,
      headCodec: "prores",
      headEncoder: "prores_ks",
      rateNumerator: FIXTURE_RATE.num,
      rateDenominator: FIXTURE_RATE.den,
      hasAudio: true,
      notes: [],
      invariants: [],
    };
  }
  if (keyframe !== null && keyframe < end) {
    return {
      mode: "headPatch",
      startFrame: start,
      endFrame: end,
      keyframe,
      headFrames: keyframe - start,
      bodyFrames: end - keyframe,
      concatOffset: 0,
      videoTimescale: 25,
      headCodec: "prores",
      headEncoder: "prores_ks",
      rateNumerator: FIXTURE_RATE.num,
      rateDenominator: FIXTURE_RATE.den,
      hasAudio: true,
      notes: [],
      invariants: [],
    };
  }
  return {
    mode: "reencode",
    startFrame: start,
    endFrame: end,
    keyframe: null,
    headFrames: 0,
    bodyFrames: frames,
    concatOffset: 0,
    videoTimescale: 25,
    headCodec: "prores",
    headEncoder: "prores_ks",
    rateNumerator: FIXTURE_RATE.num,
    rateDenominator: FIXTURE_RATE.den,
    hasAudio: true,
    notes: [],
    invariants: [],
  };
}

function previewFor(segment: StubSegment): unknown {
  const plan = planFor(segment) as { mode: string; headFrames: number; bodyFrames: number };
  const frames = plan.headFrames + plan.bodyFrames;
  return {
    segment: segment.id,
    plan,
    preset: segment.preset ?? "master",
    forcesFullEncode: plan.mode === "reencode",
    reencodeFraction: frames === 0 ? 0 : plan.headFrames / frames,
    estimatedBytes: Math.round(frames * 1_900_000),
    commands: [
      {
        label:
          plan.mode === "copy"
            ? "lossless copy, no re-encode"
            : "head encode, then the original packets",
        args: [
          "-hide_banner",
          "-v",
          "error",
          "-ss",
          String(segment.startFrame / 25),
          "-i",
          FIXTURE_PATH,
          "-c",
          plan.mode === "copy" ? "copy" : "prores_ks",
        ],
      },
    ],
    problems: [],
    notes: [],
  };
}

function summary(): unknown {
  const runnable = state.segments.filter((segment) => segment.enabled).length;
  const totalFrames = state.segments
    .filter((segment) => segment.enabled)
    .reduce((sum, segment) => sum + Math.max(0, (segment.endFrame ?? FIXTURE_FRAMES) - segment.startFrame), 0);
  return {
    segments: state.segments.length,
    runnable,
    skipped: state.segments.length - runnable,
    fullEncodes: 0,
    totalFrames,
    totalSeconds: totalFrames / 25,
    // Nothing in the fixture is ever missing, so this is a real zero rather than a placeholder.
    missingSources: 0,
  };
}

function checksFor(segment: StubSegment): unknown[] {
  const end = segment.endFrame ?? FIXTURE_FRAMES;
  const frames = Math.max(0, end - segment.startFrame);
  return [
    {
      check: "Frames",
      status: { kind: "passed" },
      measured: frames,
      expected: frames,
    },
    {
      check: "Duration",
      status: { kind: "passed" },
      measured: frames / 25,
      expected: frames / 25,
    },
    {
      check: "AudioAlignment",
      status: { kind: "passed" },
      measured: 0,
      expected: 0,
    },
    {
      check: "FrameAlignment",
      status: { kind: "passed" },
      measured: 0,
      expected: 0,
    },
    { check: "CodecMatch", status: { kind: "passed" }, measured: null, expected: null },
    {
      check: "HeadFidelity",
      status: {
        kind: "skipped",
        reason: "only the forensic policy compares the head against the source pixel-wise",
      },
      measured: null,
      expected: null,
    },
    {
      check: "Captions",
      status: { kind: "skipped", reason: "no captions were written for this segment" },
      measured: null,
      expected: null,
    },
    {
      check: "TimescalePreserved",
      status: { kind: "passed" },
      measured: null,
      expected: null,
    },
    {
      check: "Overshoot",
      status: {
        kind: "warning",
        detail:
          "the cut is 1 frame(s) longer than the 26 that were asked for. Every frame that was asked for is still there: a stream copy stops on a packet boundary rather than on the mark, so it typically runs one to three frames long, and this is that. It is a warning, not a failure.",
      },
      measured: 1,
      expected: 0,
    },
  ];
}

function outputPath(segment: StubSegment): string {
  const name = segment.name.replace(/[^\w -]/g, "").trim() || "segment";
  return `H:\\masters\\reel 2\\${FIXTURE_NAME.replace(/\.mov$/, "")} ${name} ${stamp(segment.startFrame).replace(/:/g, ".")}.mov`;
}

/** The handlers, one per command. Typed against the name union so a gap is a compile error. */
const handlers: Record<CommandName, (args: Record<string, unknown>) => unknown> = {
  doctor: () => ({
    version: "2.0.0",
    ffmpeg: "ffmpeg version 8.0.1-essentials_build-www.gyan.dev",
    ffprobe: "ffmpeg version 8.0.1-essentials_build-www.gyan.dev",
    capabilities: "ffmpeg      ffmpeg version 8.0.1\nlibx264     yes  (H.264 sources)",
    storePath: "C:\\Users\\you\\AppData\\Local\\TheTrimmer\\projects.db",
    libx264: true,
    libx265: true,
  }),

  list_projects: () => state.projects,

  create_project: (args) => {
    const created = {
      id: id(),
      name: String(args["name"] ?? "untitled"),
      updatedAt: Math.floor(Date.now() / 1000),
    };
    state.projects = [...state.projects, created];
    state.openProject = created.id;
    return { id: created.id };
  },

  open_project: (args) => {
    const wanted = String(args["id"] ?? "");
    if (!state.projects.some((project) => project.id === wanted)) {
      refuse(`there is no project ${wanted}`);
    }
    state.openProject = wanted;
    return null;
  },

  delete_project: (args) => {
    const wanted = String(args["id"] ?? "");
    state.projects = state.projects.filter((project) => project.id !== wanted);
    if (state.openProject === wanted) {
      state.openProject = null;
      state.sources = [];
      state.segments = [];
    }
    return null;
  },

  current_project: () =>
    state.openProject === null
      ? null
      : (state.projects.find((project) => project.id === state.openProject) ?? null),

  save_project: () => null,

  /**
   * Add a master to the session.
   *
   * An **upsert**, which is what the domain does: a path already in the project is re-probed rather than
   * duplicated. The caption file is attached only to the fixture master, so a test that adds a second
   * video can tell "this one has a transcript" from "this one does not" — which is how the transcript
   * panel's own source select is exercised.
   */
  add_source: (args) => {
    const path = String(args["path"] ?? "");
    if (path.trim() === "") {
      refuse("a source needs a path");
    }
    if (!path.toLowerCase().endsWith(".mov") && !path.toLowerCase().endsWith(".mp4")) {
      refuse(`${path} is not a video TheTrimmer recognises`);
    }
    const isFixture = path === FIXTURE_PATH;
    const source: StubSource = {
      path,
      name: baseName(path),
      transcript: isFixture ? FIXTURE_TRANSCRIPT : null,
      transcriptCues: isFixture ? FIXTURE_CUES : 0,
    };
    const existing = state.sources.findIndex((item) => item.path === path);
    if (existing === -1) {
      state.sources.push(source);
    } else {
      state.sources[existing] = source;
    }
    return sourceView(state.sources[state.sources.length - 1] as StubSource);
  },

  refresh_sources: () => null,

  /**
   * Remove a master, and the segments that were marked against it.
   *
   * The segments go with it because a segment whose source is not in the project cannot be planned or
   * cut; leaving them would be a queue of rows that can only fail.
   */
  remove_source: (args) => {
    const path = String(args["path"] ?? "");
    state.sources = state.sources.filter((source) => source.path !== path);
    state.segments = state.segments.filter((segment) => segment.source !== path);
    return null;
  },

  sources: () => state.sources.map(sourceView),

  segments: () => state.segments.map(segmentView),

  summary: () => {
    if (state.openProject === null) {
      refuse("no project is open");
    }
    return summary();
  },

  presets: () => [
    {
      name: "master",
      description: "keeps the source codec and the original packets",
      preservesPicture: true,
      container: "mov",
    },
    {
      name: "h264-review",
      description: "a small H.264 file for review",
      preservesPicture: false,
      container: "mp4",
    },
    {
      name: "vertical",
      description: "crops to 9:16 and re-encodes the picture",
      preservesPicture: false,
      container: "mp4",
    },
  ],

  add_segment: (args) => {
    if (state.openProject === null) {
      refuse("no project is open");
    }
    const startFrame = Number(args["startFrame"] ?? 0);
    const endFrame = args["endFrame"] === null || args["endFrame"] === undefined ? null : Number(args["endFrame"]);
    if (endFrame !== null && endFrame <= startFrame) {
      refuse(`the out point (frame ${endFrame}) is not after the in point (frame ${startFrame})`);
    }
    const segment: StubSegment = {
      id: id(),
      source: String(args["source"] ?? FIXTURE_PATH),
      name: String(args["name"] ?? "segment"),
      startFrame,
      endFrame,
      preset: args["preset"] === null || args["preset"] === undefined ? null : String(args["preset"]),
      handleFrames: Number(args["handleFrames"] ?? 0),
      enabled: true,
    };
    state.segments = [...state.segments, segment];
    return { id: segment.id };
  },

  update_segment: (args) => {
    const wanted = String(args["id"] ?? "");
    const found = state.segments.find((segment) => segment.id === wanted);
    if (found === undefined) {
      refuse(`segment ${wanted} is not in this project`);
    }
    if (args["name"] !== undefined) {
      found.name = String(args["name"]);
    }
    if (args["enabled"] !== undefined) {
      found.enabled = Boolean(args["enabled"]);
    }
    if (args["handleFrames"] !== undefined) {
      found.handleFrames = Number(args["handleFrames"]);
    }
    return null;
  },

  remove_segment: (args) => {
    const wanted = String(args["id"] ?? "");
    state.segments = state.segments.filter((segment) => segment.id !== wanted);
    return null;
  },

  reorder_segment: (args) => {
    const from = Number(args["from"] ?? 0);
    const to = Number(args["to"] ?? 0);
    const moved = state.segments.splice(from, 1)[0];
    if (moved !== undefined) {
      state.segments.splice(to, 0, moved);
    }
    return null;
  },

  parse_timecode: (args) => {
    const text = String(args["text"] ?? "");
    const frame = parse(text);
    if (frame >= FIXTURE_FRAMES) {
      refuse(`${text} is past the end of the source, which is ${stamp(FIXTURE_FRAMES - 1)}.`);
    }
    return { frame, timecode: stamp(frame) };
  },

  preview: (args) => {
    const wanted = String(args["id"] ?? "");
    const found = state.segments.find((segment) => segment.id === wanted);
    if (found === undefined) {
      refuse(`segment ${wanted} is not in this project`);
    }
    return previewFor(found);
  },

  /**
   * Every queued segment, **plus** the range currently marked.
   *
   * The range under the fields is planned continuously so the length line can say what will happen
   * before it is queued, and it has no segment id to ask about — so it arrives as an entry with the
   * id `marked` and its real `startFrame` and `endFrame`. The interface matches on those two numbers,
   * which makes the extra entry indistinguishable from a queued one to everything that reads it.
   *
   * A range that is backwards is left out rather than refused: the interface has already decided it
   * cannot be cut, and an error would put a sentence on screen for a state it does not consider an
   * error.
   */
  preview_all: (args) => {
    const planned = state.segments.map(previewFor);
    const start = args["startFrame"];
    const end = args["endFrame"];
    if (typeof start === "number" && typeof end === "number" && end > start) {
      planned.push(
        previewFor({
          id: "marked",
          source: FIXTURE_PATH,
          name: "the marked range",
          startFrame: start,
          endFrame: end,
          preset: typeof args["preset"] === "string" ? args["preset"] : null,
          handleFrames: typeof args["handleFrames"] === "number" ? args["handleFrames"] : 0,
          enabled: true,
        }),
      );
    }
    return planned;
  },

  cut_segment: () => {
    refuse("cutting one segment at a time is not wired in the browser harness; use the CLI or the window");
  },

  run_batch: async (args) => {
    if (state.openProject === null) {
      refuse("no project is open");
    }
    const runnable = state.segments.filter((segment) => segment.enabled);
    if (runnable.length === 0) {
      refuse("nothing to run: no segment is enabled");
    }
    state.runs += 1;

    /*
     * The run, narrated.
     *
     * A cut in the window emits `cut-progress` throughout — a step boundary per ffmpeg pass, its command
     * line, a position every half-second, the pass's own verdict, then the segment's. The stub used to
     * answer `run_batch` with a value and say nothing, which meant **a browser could not produce a single
     * progress event**: the bar, the fraction, the rate, the estimate and the shape of the log could only
     * be checked by running a real cut in the window, and were therefore not checked at all. That is the
     * fixture lesson from ADR-021, and this is the fix for it.
     *
     * The payloads are the shape Rust sends, which
     * `apps/desktop/src-tauri/tests/ipc_contract.rs` asserts key by key — a narration that drifted from
     * the real one would make these tests pass on a display the window never produces.
     *
     * Compressed: under a second per segment rather than minutes. The fractions, the order and the
     * relationships are real; only the clock is not. The tempo is chosen so that a test can *observe* a
     * run in progress — the readout only exists while working, so a narration that finished in
     * milliseconds would leave the whole display unassertable.
     */
    const PASSES = [
      { label: "head encode, frames 25..50", seconds: 1.0 },
      { label: "body picture copy, frames 50..", seconds: 2.0 },
      { label: "body sound copy, cut on an output seek", seconds: 2.0 },
      { label: "body mux, picture and sound", seconds: 2.0 },
      { label: "join head and body", seconds: 3.0 },
    ];
    const TICKS_PER_PASS = 4;
    const TICK_MS = 45;

    stubEvents.emit("cut-progress", { kind: "started", total: runnable.length });

    for (const [index, segment] of runnable.entries()) {
      const frames = Math.max(0, (segment.endFrame ?? FIXTURE_FRAMES) - segment.startFrame) + 1;
      stubEvents.emit("cut-progress", {
        kind: "state",
        job: index,
        name: segment.name,
        state: "cutting",
      });

      for (const pass of PASSES) {
        stubEvents.emit("cut-progress", { kind: "step", label: pass.label });
        stubEvents.emit("cut-progress", {
          kind: "command",
          text: `ffmpeg -hide_banner -nostdin -v error -y -progress pipe:1 ${pass.label}`,
          args: ["-hide_banner", "-nostdin", "-v", "error", "-y"],
        });
        for (let tick = 1; tick <= TICKS_PER_PASS; tick += 1) {
          const share = tick / TICKS_PER_PASS;
          stubEvents.emit("cut-progress", {
            kind: "ticks",
            ticks: {
              outSeconds: pass.seconds * share,
              frame: Math.round((frames / PASSES.length) * share),
              speed: 4.5,
              bytes: Math.round(1_200_000 * share),
              expectedSeconds: pass.seconds,
            },
          });
          await delay(TICK_MS);
        }
        stubEvents.emit("cut-progress", {
          kind: "finished",
          label: pass.label,
          seconds: pass.seconds / 4,
          ok: true,
        });
      }

      stubEvents.emit("cut-progress", {
        kind: "finished",
        job: index,
        name: segment.name,
        status: {
          kind: "succeeded",
          output: outputPath(segment),
          frames,
          overshoot: 1,
          seconds: 0.42 + index * 0.31,
          steps: PASSES.length,
          checks: checksFor(segment),
        },
      });
    }

    const jobs = runnable.map((segment, index) => ({
      job: index,
      segment: segment.id,
      name: segment.name,
      status: {
        kind: "succeeded",
        output: outputPath(segment),
        frames: Math.max(0, (segment.endFrame ?? FIXTURE_FRAMES) - segment.startFrame) + 1,
        overshoot: 1,
        seconds: 0.42 + index * 0.31,
        steps: 1,
        checks: checksFor(segment),
      },
    }));

    stubEvents.emit("cut-progress", {
      kind: "completed",
      succeeded: jobs.length,
      unverified: 0,
      failed: 0,
      skipped: state.segments.length - runnable.length,
    });

    const deliveredFrames = jobs.reduce((sum, job) => sum + job.status.frames, 0);
    return {
      jobs,
      deliveredFrames,
      deliveredSeconds: deliveredFrames / 25,
      elapsedSeconds: jobs.reduce((sum, job) => sum + job.status.seconds, 0),
      cancelled: false,
      // A real run signature: 64 hex characters, so nothing that reads it can be fooled into
      // treating a short string as one.
      digest: "9f2c4a1e7b3d8056af1c92e4d7b03a5c8e6f1942a7c3b5d8e0f2a4c6b8d1e3f5",
    };
  },

  cancel_batch: () => null,

  search_transcript: (args) => {
    const phrase = String(args["phrase"] ?? "").toLowerCase();
    const cues = [
      { at: 0.2, text: "So the whole point of the roll is that nothing is cut twice." },
      { at: 12.4, text: "We shot the second take on the long lens, and it is the one." },
      { at: 48.9, text: "That is the moment the light goes, so that is our out point." },
      { at: 96.1, text: "Pick it up from the second slate and keep rolling." },
    ];
    const hits = cues
      .filter((cue) => phrase === "" || cue.text.toLowerCase().includes(phrase))
      .map((cue, index) => {
        const start = Math.round(cue.at * 25);
        const marker = cue.text.toLowerCase().indexOf(phrase);
        const highlighted =
          phrase === "" || marker < 0
            ? cue.text
            : `${cue.text.slice(0, marker)}[[${cue.text.slice(marker, marker + phrase.length)}]]${cue.text.slice(marker + phrase.length)}`;
        return {
          cue: index,
          sentence: index,
          byteOffset: marker < 0 ? 0 : marker,
          byteLen: phrase.length,
          highlighted,
          startFrame: start,
          startSeconds: cue.at,
        };
      });
    const limit = Number(args["limit"] ?? hits.length);
    return hits.slice(0, limit);
  },

  transcript_lines: (args) => {
    const limit = Number(args["limit"] ?? 200);
    return Array.from({ length: Math.min(limit, 6) }, (_, index) => ({
      index,
      startFrame: index * 625,
      endFrame: (index + 1) * 625,
      text: `Cue ${index + 1}: a line of the transcript, long enough to wrap in the panel.`,
    }));
  },

  export_timeline: (args) => {
    const path = String(args["path"] ?? "");
    if (path.trim() === "") {
      refuse("an export needs somewhere to write");
    }
    if (state.segments.length === 0) {
      refuse("there is nothing to export: no segment is marked");
    }
    return { warnings: [], clips: state.segments.length };
  },

  plan_watch_folder: () => ({ planned: 0, skipped: 0 }),

  get_verify_policy: () => "strict",

  set_verify_policy: (args) => {
    const policy = String(args["policy"] ?? "");
    if (!["off", "standard", "strict", "forensic"].includes(policy)) {
      refuse(`${policy} is not a verification policy`);
    }
    return null;
  },

  reveal: (args) => {
    const path = String(args["path"] ?? "");
    // The browser cannot open Explorer, and pretending otherwise would make a test pass on a claim
    // that is not true. It records the intent instead.
    state.revealed = path;
    return null;
  },
};

/**
 * Install the stub as the window's bridge.
 *
 * The shape is the real one — `{ core: { invoke }, dialog: { open, save } }` — because the interface
 * reaches for that shape and nothing else.
 *
 * ## This never runs in the shipped window, and that is enforced twice
 *
 * `main.tsx` calls it only under `import.meta.env.DEV`, so a production build has no call to it and
 * Rollup removes the whole module — `tools/check-bundle.mjs` then fails the build if any trace of it
 * survives into `dist`. That is the load-bearing guarantee.
 *
 * ## The guard that failed, and why it looked at the wrong thing
 *
 * The second line of defence is the check below, and it used to test the wrong property. It asked
 * whether `window.__TAURI__` was already defined — the *convenience* global that `withGlobalTauri`
 * injects. It is injected late, and the stub therefore won the race: the shipped window ran on fixtures,
 * `doctor` reported a hard-coded ffmpeg build string, and the file picker returned
 * `H:\masters\reel 2\A007C012_250312_R1QK.mov` without opening anything.
 *
 * The real signal is `window.__TAURI_INTERNALS__`: the IPC bridge itself, which is present from the
 * first script. That is what this now checks, and what `tools/smoke-window.ps1` asserts after the
 * window is up. The marker it leaves behind is deliberate — when the stub declines, it says *why*, so a
 * future reader does not have to re-derive this.
 */
export function installStub(): boolean {
  if (typeof window === "undefined") {
    return false;
  }

  const globals = window as unknown as {
    __TAURI__?: unknown;
    __TAURI_INTERNALS__?: unknown;
    __TAURI_STUB__?: string;
  };

  if (globals.__TAURI_INTERNALS__ !== undefined) {
    globals.__TAURI_STUB__ = "declined: window.__TAURI_INTERNALS__ is present, so this is the real window";
    return false;
  }
  if (globals.__TAURI__ !== undefined && (globals.__TAURI__ as { mocks?: unknown }).mocks === undefined) {
    globals.__TAURI_STUB__ = "declined: a real window.__TAURI__ is already installed";
    return false;
  }

  const invoke = async (command: string, args: Record<string, unknown> = {}): Promise<unknown> => {
    const handler = handlers[command as CommandName];
    if (handler === undefined) {
      refuse(`the browser harness has no ${command}; add it to src/ipc/stub.ts`);
    }
    // A tick of latency, because the interface must not depend on a command resolving synchronously:
    // in the window every one of these crosses a process boundary.
    await new Promise((resolve) => setTimeout(resolve, 12));
    return handler(args);
  };

  globals.__TAURI__ = {
    mocks: true,
    core: { invoke },
    event: stubEvents,
    /*
     * The window handle, faked the same way everything else is.
     *
     * A browser tab has no window to put in fullscreen, so this tracks the value and reports it back —
     * which is exactly enough for the interface to render its button and behave, and no more. It is
     * deliberately not a real fullscreen request: a browser test that made the page go fullscreen would
     * be testing the browser, not the product.
     */
    window: {
      getCurrentWindow: () => ({
        isFullscreen: async (): Promise<boolean> => state.fullscreen,
        setFullscreen: async (value: boolean): Promise<void> => {
          state.fullscreen = value;
        },
      }),
    },
    dialog: {
      async open(): Promise<string | null> {
        // There is no file picker in a browser. Returning the fixture is the useful behaviour for
        // development: "Add a master…" produces a master instead of nothing.
        return FIXTURE_PATH;
      },
      async save(options: { defaultPath?: string }): Promise<string | null> {
        return `H:\\masters\\reel 2\\${options.defaultPath ?? "timeline.xml"}`;
      },
      async message(): Promise<void> {},
      async confirm(): Promise<boolean> {
        return true;
      },
      async ask(): Promise<boolean> {
        return true;
      },
    },
  };
  globals.__TAURI_STUB__ = "installed: there is no real bridge, so this is a browser";
  return true;
}

