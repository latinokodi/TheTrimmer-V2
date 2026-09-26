import { describe, expect, it } from "vitest";

import { type RunProgress, remainingSeconds, stepFraction } from "./useCutLog";

/**
 * The arithmetic behind the bar.
 *
 * These are the two numbers the interface shows a person waiting for a cut, and both are the kind of
 * figure that is trusted precisely because nobody checks it. A wrong percentage is not a cosmetic bug:
 * it is a promise about how long the job will take, and an operator who stops watching the window
 * because it said `99%` will be watching a window that stopped.
 *
 * Pure functions, so every awkward case is a table entry rather than a race against a running process.
 */

/** Nothing known yet, which is where every run starts. */
const nothing: RunProgress = {
  finished: 0,
  total: null,
  segment: null,
  step: null,
  doneSeconds: null,
  totalSeconds: null,
  speed: null,
  frame: null,
  bytes: null,
  elapsedSeconds: 0,
};

const withTicks = (doneSeconds: number | null, totalSeconds: number | null, speed: number | null) => ({
  ...nothing,
  doneSeconds,
  totalSeconds,
  speed,
});

describe("the fraction of the current step", () => {
  it("is the position over the length the step was built for", () => {
    expect(stepFraction(withTicks(2.2, 4.4, null))).toBeCloseTo(0.5, 10);
    expect(stepFraction(withTicks(1.0, 4.0, null))).toBeCloseTo(0.25, 10);
  });

  it("is null until both the position and the length are known", () => {
    expect(stepFraction(nothing)).toBeNull();
    expect(stepFraction(withTicks(2.2, null, null))).toBeNull();
    expect(stepFraction(withTicks(null, 4.4, null))).toBeNull();
  });

  it("is null for a step of no length, rather than dividing by zero", () => {
    // `expected_seconds` is zero for a degenerate plan — a segment of no frames — and a bar that filled
    // to `Infinity %` would be worse than one that admitted it had nothing to show.
    expect(stepFraction(withTicks(1.0, 0, null))).toBeNull();
    expect(stepFraction(withTicks(1.0, -1, null))).toBeNull();
  });

  it("never leaves the track", () => {
    // ffmpeg's `out_time_us` can pass the expected length: a copy stops on its own packet boundary, and
    // the overshoot is a documented property of the tool rather than an error. A bar that overflowed its
    // own track would look broken at exactly the moment the cut is finishing normally.
    expect(stepFraction(withTicks(5.0, 4.0, null))).toBe(1);
    expect(stepFraction(withTicks(-1.0, 4.0, null))).toBe(0);
  });
});

describe("the estimate of what is left", () => {
  it("is the remaining output divided by the rate ffmpeg measured", () => {
    // 2.2s of 4.4s done at 4.27x: 2.2 seconds of output left, which takes 2.2/4.27 of a second.
    expect(remainingSeconds(withTicks(2.2, 4.4, 4.27))).toBeCloseTo(2.2 / 4.27, 10);
  });

  it("is null without a rate, because a guess is worse than nothing", () => {
    expect(remainingSeconds(withTicks(2.2, 4.4, null))).toBeNull();
    expect(remainingSeconds(nothing)).toBeNull();
    expect(remainingSeconds(withTicks(2.2, null, 4.0))).toBeNull();
  });

  it("is a real zero when there is nothing left, not a small negative", () => {
    // Overshoot again: the arithmetic would give a negative number of seconds, and `-0.1s left` in a
    // readout is a defect a person would notice and not forgive.
    expect(remainingSeconds(withTicks(4.4, 4.4, 2.0))).toBe(0);
    expect(remainingSeconds(withTicks(5.0, 4.0, 2.0))).toBe(0);
  });

  it("is null rather than infinite for a rate of zero", () => {
    expect(remainingSeconds(withTicks(1.0, 4.0, 0))).toBeNull();
  });
});
