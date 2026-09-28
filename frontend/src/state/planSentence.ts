/**
 * What the plan line says, as a function rather than as a template inside the component.
 *
 * The line appears as soon as a range is marked, because the engine plans while the marks are being
 * typed — before anything is cut. It used to read:
 *
 *     head patch — 25 frame(s) re-encoded, 500 copied
 *
 * which is a statement of fact about something that has not happened. It was read, reasonably, as
 * the cut already being under way, and asked about as *"why does the head re-encode occur before I
 * can click Trim?"*.
 *
 * A sentence about the future has to be written in the future tense, or it is a false report. That
 * is the whole of the rule here, and it is why this is a function with tests: a template string in
 * a component has nowhere to say what it is promising.
 */

export interface PlanSentence {
  /** How it should be drawn: `ok` a lossless copy, `warn` a patch, `danger` a full re-encode. */
  readonly tone: "ok" | "warn" | "danger";
  readonly text: string;
}

/** The fields this needs from a plan. A structural type keeps it independent of the wire shape. */
export interface Planned {
  readonly mode: "copy" | "headpatch" | "reencode";
  readonly frames: number;
  readonly headFrames: number;
  readonly bodyFrames: number;
}

export function planSentence(plan: Planned): PlanSentence {
  if (plan.mode === "copy") {
    return { tone: "ok", text: `lossless copy — will copy all ${plan.frames} frames untouched` };
  }
  if (plan.mode === "headpatch") {
    return {
      tone: "warn",
      text: `head patch — will re-encode ${plan.headFrames}, copy ${plan.bodyFrames}`,
    };
  }
  return {
    tone: "danger",
    text: `full re-encode — no keyframe in this range, so all ${plan.frames} will be re-encoded`,
  };
}

/**
 * What the Progress panel says about the lines it is holding.
 *
 * The lines stay after a run ends, which is wanted — they are the record of it. What was missing is
 * any sign that they are a record rather than live output: the previous cut's *"head re-encoding
 * rows 0..24"* sat there while the next range was being marked and read as something happening now.
 * That is the second half of the same report, so it is the second half of the same rule: the panel
 * says which run it is showing.
 */
export function progressLabel(running: boolean, lineCount: number): string | null {
  if (running || lineCount === 0) {
    return null;
  }
  return "from the last run";
}
