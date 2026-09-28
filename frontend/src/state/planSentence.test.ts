/**
 * The two sentences that describe a run without being part of it.
 *
 * Both exist because the window was stating things it had not done yet, or had already finished, as
 * though they were happening now. The plan line is a forecast and the progress panel is a record,
 * and each has to say so. The tests below are mostly about tense, which is unusual and is the point:
 * the fault being fixed was a sentence in the wrong one.
 */

import { describe, expect, it } from "vitest";

import { planSentence, progressLabel } from "./planSentence";

const patch = { mode: "headpatch" as const, frames: 900, headFrames: 37, bodyFrames: 835 };

describe("planSentence", () => {
  it("promises a head patch in the future tense", () => {
    // "37 frame(s) re-encoded" was read as an account of something happening. Nothing has happened:
    // this line appears while the marks are being typed, before Trim can be pressed.
    const said = planSentence(patch);
    expect(said.text).toBe("head patch — will re-encode 37, copy 835");
    expect(said.text).toContain("will");
    expect(said.tone).toBe("warn");
  });

  it("does not describe a copy as done either", () => {
    const said = planSentence({ ...patch, mode: "copy" });
    expect(said.text).toBe("lossless copy — will copy all 900 frames untouched");
    expect(said.tone).toBe("ok");
  });

  it("says how much of a full re-encode is coming", () => {
    const said = planSentence({ ...patch, mode: "reencode" });
    expect(said.text).toContain("will be re-encoded");
    expect(said.text).toContain("900");
    expect(said.text).toContain("no keyframe");
    expect(said.tone).toBe("danger");
  });

  it("never states a re-encode as a thing that has occurred", () => {
    // The regression this guards, stated as plainly as it can be: anything said about a re-encode
    // is said in the future tense. Phrased this way round rather than by forbidding the word,
    // because "will be re-encoded" ends a sentence perfectly well.
    for (const mode of ["copy", "headpatch", "reencode"] as const) {
      const said = planSentence({ ...patch, mode });
      expect(said.text).not.toMatch(/\bhas been\b/);
      expect(said.text).not.toMatch(/\bis being\b/);
      if (said.text.includes("re-encode")) {
        expect(said.text).toContain("will");
      }
    }
  });
});

describe("progressLabel", () => {
  it("says the lines are a record once the run has ended", () => {
    expect(progressLabel(false, 12)).toBe("from the last run");
  });

  it("says nothing while the run is live, because then they are not a record", () => {
    expect(progressLabel(true, 12)).toBeNull();
  });

  it("says nothing when there is nothing to label", () => {
    expect(progressLabel(false, 0)).toBeNull();
  });
});
