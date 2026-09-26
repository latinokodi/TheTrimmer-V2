/**
 * The wire types: exactly what the Rust commands send and receive.
 *
 * These are written by hand to mirror the `serde` shapes in the Rust crates, and every one is
 * `readonly` because a value that came off the wire is not the interface's to mutate — the state
 * lives in Rust and changes only through a command. Mutating a local copy would produce a view that
 * disagrees with the project, which is the class of bug this whole architecture exists to prevent.
 *
 * ## The one thing that must not drift
 *
 * If a Rust field is renamed, these types will not notice, and the failure will be `undefined` at
 * runtime rather than an error at build time. The mitigation is that the commands are thin and
 * named one-to-one with these types, and `ipc.ts` is the single place they are called: a rename is
 * therefore a small, greppable change. A generated binding would be better still and is noted in
 * the README as unfinished work rather than pretended away.
 */

/** A frame rate, as `num/den`. */
export interface FrameRateWire {
  readonly num: number;
  readonly den: number;
}

/** One source file. */
export interface SourceView {
  readonly path: string;
  readonly name: string;
  readonly present: boolean;
  readonly media: MediaInfoWire | null;
  readonly summary: string;
  readonly transcript: string | null;
  readonly transcriptCues: number | null;
  readonly label: string | null;
  readonly variableRate: boolean;
}

/** What was probed about a source. */
export interface MediaInfoWire {
  readonly path: string;
  readonly codec: string;
  readonly pixFmt: string;
  readonly width: number;
  readonly height: number;
  readonly rate: FrameRateWire;
  readonly averageRate: FrameRateWire | null;
  readonly timebase: { readonly ticksPerSecond: number };
  readonly frameCount: number;
  readonly audio: { readonly codec: string; readonly sampleRate: number; readonly channels: number } | null;
  readonly sizeBytes: number;
  readonly startTime: number;
}

/** One cut. */
export interface SegmentView {
  readonly id: string;
  readonly name: string;
  readonly sourceName: string;
  readonly inTimecode: string;
  readonly outTimecode: string;
  readonly endFrame: number | null;
  readonly frames: number | null;
  readonly seconds: number | null;
  readonly handleFrames: number;
  readonly enabled: boolean;
  readonly problems: readonly string[];
  readonly notes: readonly string[];
}

/** What one segment will cost and do. */
export interface QueuePreview {
  readonly segment: string;
  readonly plan: CutPlanWire | null;
  readonly preset: string;
  readonly forcesFullEncode: boolean;
  readonly reencodeFraction: number;
  readonly estimatedBytes: number | null;
  readonly commands: readonly PreparedWire[];
  readonly problems: readonly string[];
  readonly notes: readonly string[];
}

/** One ffmpeg invocation, as it will run. */
export interface PreparedWire {
  readonly label: string;
  readonly args: readonly string[];
}

/** The cut decision. */
export interface CutPlanWire {
  readonly mode: "copy" | "headPatch" | "reencode";
  readonly startFrame: number;
  readonly endFrame: number;
  readonly keyframe: number | null;
  readonly headFrames: number;
  readonly bodyFrames: number;
  readonly concatOffset: number;
  readonly videoTimescale: number;
  readonly headCodec: string;
  readonly headEncoder: string;
  readonly rateNumerator: number;
  readonly rateDenominator: number;
  readonly hasAudio: boolean;
  readonly notes: readonly string[];
  readonly invariants: readonly string[];
}

/** The summary line above the run button. */
export interface WorkspaceSummary {
  readonly segments: number;
  readonly runnable: number;
  readonly skipped: number;
  readonly fullEncodes: number;
  readonly totalFrames: number;
  readonly totalSeconds: number;
  readonly missingSources: number;
}

/** A search result in a transcript. */
export interface TranscriptHit {
  readonly cue: number;
  readonly sentence: number | null;
  readonly byteOffset: number;
  readonly byteLen: number;
  readonly highlighted: string;
  readonly startFrame: number;
  readonly startSeconds: number;
}

/** What a job did. */
export type JobStatus =
  | {
      readonly kind: "succeeded";
      readonly output: string;
      readonly frames: number;
      readonly overshoot: number;
      readonly seconds: number;
      readonly steps: number;
      readonly checks: readonly CheckResultWire[];
    }
  | {
      readonly kind: "unverified";
      readonly output: string;
      readonly frames: number;
      readonly seconds: number;
      readonly checks: readonly CheckResultWire[];
    }
  | { readonly kind: "failed"; readonly reason: string; readonly cancelled: boolean }
  | { readonly kind: "skipped"; readonly reason: string };

/** One check's verdict. */
export interface CheckResultWire {
  readonly check: string;
  readonly status:
    | { readonly kind: "passed" }
    | { readonly kind: "failed"; readonly detail: string }
    | { readonly kind: "warning"; readonly detail: string }
    | { readonly kind: "skipped"; readonly reason: string };
  readonly measured: number | null;
  readonly expected: number | null;
}

/** One job in a finished batch. */
export interface BatchJobWire {
  readonly job: number;
  readonly segment: string;
  readonly name: string;
  readonly status: JobStatus;
}

/** The result of a batch. */
export interface BatchOutcomeWire {
  readonly jobs: readonly BatchJobWire[];
  readonly deliveredFrames: number;
  readonly deliveredSeconds: number;
  readonly elapsedSeconds: number;
  readonly cancelled: boolean;
  readonly digest: string;
}

/** A project in the store's listing. */
export interface ProjectListingWire {
  readonly id: string;
  readonly name: string;
  readonly updatedAt: number;
}

/** What the licence allows. */
export interface LicenceStatusWire {
  readonly present: boolean;
  readonly licensee: string | null;
  readonly edition: string | null;
  readonly seats: number | null;
  readonly expiresAt: number | null;
  readonly daysRemaining: number | null;
  readonly features: readonly string[];
  readonly machineId: string;
  readonly error: string | null;
}

/** The doctor panel. */
export interface DoctorReportWire {
  readonly version: string;
  readonly ffmpeg: string;
  readonly ffprobe: string;
  readonly capabilities: string;
  readonly storePath: string;
  readonly libx264: boolean;
  readonly libx265: boolean;
}

/** Which delivery presets a project offers. */
export interface PresetWire {
  readonly name: string;
  readonly description: string;
  readonly container: string;
  readonly preservesPicture: boolean;
  readonly batchSafe: boolean;
}

/** The shape every command returns on failure. */
export interface IpcError {
  readonly message: string;
  readonly kind: "domain" | "media" | "store" | "internal";
}
