/**
 * The two numbers the progress bar is allowed to show.
 *
 * `stepFraction` is the bar's value and `remainingSeconds` is its estimate, and both return `null`
 * rather than a guess when the pass has not reported enough to work from. That is the whole of the
 * contract: a bar that fills to a number nobody measured is worse than a bar that sweeps.
 */

import { describe, expect, it } from "vitest";

import { type RunProgress, remainingSeconds, stepFraction } from "./useRunLog";

function progress(overrides: Partial<RunProgress> = {}): RunProgress {
  return {
    stage: null,
    doneSeconds: null,
    totalSeconds: null,
    speed: null,
    frame: null,
    size: null,
    ...overrides,
  };
}

describe("stepFraction", () => {
  it("is the share of this pass that is done", () => {
    expect(stepFraction(progress({ doneSeconds: 5, totalSeconds: 10 }))).toBe(0.5);
  });

  it("is null until the pass knows its own length", () => {
    // A position without a denominator is a position, not a fraction.
    expect(stepFraction(progress({ doneSeconds: 5 }))).toBeNull();
    expect(stepFraction(progress({ totalSeconds: 10 }))).toBeNull();
    expect(stepFraction(progress())).toBeNull();
  });

  it("will not divide by a length of zero", () => {
    expect(stepFraction(progress({ doneSeconds: 0, totalSeconds: 0 }))).toBeNull();
  });

  it("is clamped, because a pass can report a position past its own estimate", () => {
    expect(stepFraction(progress({ doneSeconds: 12, totalSeconds: 10 }))).toBe(1);
    expect(stepFraction(progress({ doneSeconds: -1, totalSeconds: 10 }))).toBe(0);
  });
});

describe("remainingSeconds", () => {
  it("divides what is left by the rate ffmpeg itself measured", () => {
    expect(remainingSeconds(progress({ doneSeconds: 2, totalSeconds: 10, speed: 2 }))).toBe(4);
  });

  it("is null without a rate, because an extrapolation is not a measurement", () => {
    expect(remainingSeconds(progress({ doneSeconds: 2, totalSeconds: 10 }))).toBeNull();
    expect(remainingSeconds(progress({ doneSeconds: 2, totalSeconds: 10, speed: 0 }))).toBeNull();
  });

  it("is zero rather than negative once the estimate is passed", () => {
    expect(remainingSeconds(progress({ doneSeconds: 11, totalSeconds: 10, speed: 1 }))).toBe(0);
  });
});
