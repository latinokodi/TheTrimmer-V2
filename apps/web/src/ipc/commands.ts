/**
 * The one place the interface talks to Rust.
 *
 * Every call goes through {@link call}, so there is a single function to read when the question is
 * "what can this page actually do" — and the answer is "invoke one of the commands listed in
 * `command`, and nothing else". There is no `fetch`, no `eval`, and no filesystem access anywhere in
 * this application.
 *
 * ## Why errors are normalised
 *
 * A Rust command returns `Result<T, E>`, and Tauri rejects the promise with `E` when it is `Err`.
 * The shape of `E` varies with what failed — a domain refusal, a missing file, a store error — and a
 * component that had to match on that would be coupled to all of it. So {@link call} turns whatever
 * arrives into a single {@link IpcFailure} with a message a person can read and a `kind` a component
 * can branch on. Everything above this file deals in that one shape.
 */

import type {
  BatchOutcomeWire,
  DoctorReportWire,
  IpcError,
  PresetWire,
  ProjectListingWire,
  QueuePreview,
  SegmentView,
  SourceView,
  TranscriptHit,
  WorkspaceSummary,
} from "./types";

/**
 * The Tauri bridge, as the shell injects it.
 *
 * Declared rather than imported so the interface builds and type-checks with no Tauri in the
 * dependency tree — which is what lets `npm run build` work in a plain checkout and what keeps the
 * webview from depending on a package it does not need.
 */
interface TauriGlobal {
  readonly core: {
    invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  };
}

declare global {
  interface Window {
    readonly __TAURI__?: TauriGlobal;
  }
}

/** A failure from a command, in the one shape everything above this file understands. */
export class IpcFailure extends Error {
  readonly kind: IpcError["kind"];

  constructor(message: string, kind: IpcError["kind"] = "internal") {
    super(message);
    this.name = "IpcFailure";
    this.kind = kind;
  }

  /**
   * True when the failure is something the user can fix by changing what they entered — a bad
   * timecode, a range past the end of the file. A component uses this to decide between showing the
   * message beside the field and showing a dialog.
   */
  get isUserFixable(): boolean {
    return this.kind === "domain";
  }
}

/**
 * Turn whatever Tauri rejected with into an {@link IpcFailure}.
 *
 * The interesting case is the string: a command that fails a precondition returns a plain sentence,
 * and a sentence is nearly always about what the user did rather than about a defect, so it is
 * classified as user-fixable. An object is a serialised `CoreError`, which carries its own kind.
 */
function normaliseError(error: unknown): IpcFailure {
  if (error instanceof IpcFailure) {
    return error;
  }
  if (typeof error === "string") {
    return new IpcFailure(error, "domain");
  }
  if (error instanceof Error) {
    return new IpcFailure(error.message, "internal");
  }
  if (typeof error === "object" && error !== null) {
    const record = error as Record<string, unknown>;
    const message =
      typeof record["message"] === "string"
        ? record["message"]
        : typeof record["error"] === "string"
          ? record["error"]
          : JSON.stringify(error);
    return new IpcFailure(message, "domain");
  }
  return new IpcFailure("the command failed for an unknown reason", "internal");
}

/**
 * Every command the interface may call, by name.
 *
 * This list is the interface's declaration of what it needs from Rust, and it exists as data rather
 * than only as the methods below because two other things are checked against it:
 *
 * * `stub.ts` — the browser harness the interface is developed and tested in — is typed as
 *   `Record<CommandName, …>`, so adding a command here and forgetting to stub it is a compile error
 *   rather than a button that does nothing in every browser test.
 * * `apps/desktop/src-tauri/tests/ipc_contract.rs` drives the same names through Tauri's real invoke
 *   handler, so a rename that only lands on one side fails a test.
 *
 * It is `as const` so the element type is the union of the literals rather than `string`.
 */
export const COMMAND_NAMES = [
  "doctor",
  "list_projects",
  "create_project",
  "open_project",
  "delete_project",
  "current_project",
  "save_project",
  "add_source",
  "refresh_sources",
  "remove_source",
  "sources",
  "segments",
  "summary",
  "presets",
  "add_segment",
  "update_segment",
  "remove_segment",
  "reorder_segment",
  "parse_timecode",
  "preview",
  "preview_all",
  "cut_segment",
  "run_batch",
  "cancel_batch",
  "search_transcript",
  "transcript_lines",
  "export_timeline",
  "plan_watch_folder",
  "get_verify_policy",
  "set_verify_policy",
  "reveal",
] as const;

/** One of the command names. */
export type CommandName = (typeof COMMAND_NAMES)[number];

/**
 * Invoke a command.
 *
 * Throws {@link IpcFailure} on failure, so a caller that ignores the result gets a rejected promise
 * rather than an `undefined` it will dereference three lines later.
 */
export async function call<T>(command: CommandName, args?: Record<string, unknown>): Promise<T> {
  const bridge = window.__TAURI__;
  if (bridge === undefined) {
    throw new IpcFailure(
      "this page is not running inside TheTrimmer, so it cannot read or cut anything",
      "internal",
    );
  }
  try {
    return await bridge.core.invoke<T>(command, args ?? {});
  } catch (error) {
    throw normaliseError(error);
  }
}

/**
 * The commands, one function each.
 *
 * Named after the Rust commands exactly, so a rename is a single grep. Every function is `async` and
 * every one can fail; none of them swallow a failure.
 */
export const commands = {
  /** The environment report for the doctor panel. */
  doctor: () => call<DoctorReportWire>("doctor"),


  /** The projects in the store. */
  listProjects: () => call<readonly ProjectListingWire[]>("list_projects"),

  /** Create a project and open it. */
  createProject: (name: string, createdBy: string) =>
    call<{ readonly id: string }>("create_project", { name, createdBy }),

  /** Open a project, probing what it needs. */
  openProject: (id: string) => call<void>("open_project", { id }),

  /** Delete a project. */
  deleteProject: (id: string) => call<void>("delete_project", { id }),

  /** Add a source to the open project. */
  addSource: (path: string) => call<SourceView>("add_source", { path }),

  /** Re-probe every source. */
  refreshSources: () => call<void>("refresh_sources"),

  /** Remove a source and everything that cut it. */
  removeSource: (path: string) => call<void>("remove_source", { path }),

  /** The sources, as the list shows them. */
  sources: () => call<readonly SourceView[]>("sources"),

  /** The segments. */
  segments: () => call<readonly SegmentView[]>("segments"),

  /** The summary line above the run button. */
  summary: () => call<WorkspaceSummary>("summary"),

  /** The delivery presets this project offers. */
  presets: () => call<readonly PresetWire[]>("presets"),

  /** Add a segment. */
  addSegment: (input: {
    readonly source: string;
    readonly name: string;
    readonly startFrame: number;
    readonly endFrame: number | null;
    readonly preset: string | null;
    readonly handleFrames: number;
  }) => call<{ readonly id: string }>("add_segment", input),

  /** Update a segment in place. */
  updateSegment: (input: {
    readonly id: string;
    readonly name?: string;
    readonly startFrame?: number;
    readonly endFrame?: number | null;
    readonly preset?: string | null;
    readonly handleFrames?: number;
    readonly enabled?: boolean;
    readonly note?: string | null;
  }) => call<void>("update_segment", input),

  /** Remove a segment. */
  removeSegment: (id: string) => call<void>("remove_segment", { id }),

  /** Move a segment in the running order. */
  reorderSegment: (from: number, to: number) => call<void>("reorder_segment", { from, to }),

  /** What one segment will do and cost. */
  preview: (id: string) => call<QueuePreview>("preview", { id }),

  /** What the whole batch will do and cost, and the marked range when there is one. */
  previewAll: (marked?: {
    readonly startFrame: number;
    readonly endFrame: number;
    readonly preset: string | null;
    readonly handleFrames: number;
  }) => call<readonly QueuePreview[]>("preview_all", marked ?? {}),

  /** Parse a timecode into a frame number, so a field can show its frame live. */
  parseTimecode: (text: string, source: string) =>
    call<{ readonly frame: number; readonly timecode: string }>("parse_timecode", { text, source }),

  /** Search a transcript. */
  searchTranscript: (video: string, phrase: string, limit: number) =>
    call<readonly TranscriptHit[]>("search_transcript", { video, phrase, limit }),

  /** Cut one segment, with progress streamed on the `cut-progress` event. */
  cutSegment: (id: string) => call<BatchOutcomeWire>("cut_segment", { id }),

  /** Run the whole batch. */
  runBatch: (options: {
    readonly stopOnError: boolean;
    readonly skipVerification: boolean;
    readonly label: string;
  }) => call<BatchOutcomeWire>("run_batch", options),

  /** Ask the running batch to stop after the segment in flight. */
  cancelBatch: () => call<void>("cancel_batch"),

  /** Export the project's timeline. */
  exportTimeline: (input: {
    readonly format: "premiere" | "fcpxml" | "edl" | "csv";
    readonly path: string;
    readonly sequenceName: string;
  }) => call<{ readonly warnings: readonly string[]; readonly clips: number }>("export_timeline", input),

  /** Reveal a file in Explorer. */
  reveal: (path: string) => call<void>("reveal", { path }),

  /**
   * The verification policy.
   *
   * A property of the project rather than of the window, so it is read after the project is open and
   * written when the select moves. Both are here rather than in a settings dialog because there is no
   * settings dialog.
   */
  getVerifyPolicy: () => call<string>("get_verify_policy"),

  setVerifyPolicy: (policy: string) => call<void>("set_verify_policy", { policy }),
} as const;

export type Commands = typeof commands;
