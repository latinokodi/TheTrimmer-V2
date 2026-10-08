/**
 * The decision to ask, and the one state that is easy to lose: "no".
 *
 * A question that reappears the moment it is dismissed is not a question. That is the failure
 * these tests exist for: the plan keeps describing the same occupied path, so without a memory of
 * the answer the dialog would reopen on every render and the operator would be stuck behind it.
 */

import { describe, expect, it } from "vitest";

import { overwriteAsk, overwriteReason, overwriteTitle } from "./overwriteAsk";

const none = new Set<string>();

const base = {
  output: "D:/r/Interview wide.mp4",
  files: ["Interview wide.mp4"],
  refusedOutput: null,
  refusedReason: null,
  dismissed: none,
};

describe("overwriteAsk", () => {
  it("asks when the plan says the output is occupied", () => {
    const ask = overwriteAsk(base);
    expect(ask).not.toBeNull();
    expect(ask?.output).toBe("D:/r/Interview wide.mp4");
    expect(ask?.files).toEqual(["Interview wide.mp4"]);
    expect(ask?.source).toBe("planned");
  });

  it("says nothing when nothing is in the way", () => {
    expect(overwriteAsk({ ...base, files: [] })).toBeNull();
  });

  it("asks about the captions too, because a cut writes them too", () => {
    const ask = overwriteAsk({ ...base, files: ["Interview wide.mp4", "Interview wide.srt"] });
    expect(ask?.files).toHaveLength(2);
  });

  it("does not ask again about a file the operator has already declined", () => {
    // The whole point of remembering. The plan is unchanged, so without this the question would
    // come straight back.
    const dismissed = new Set(["D:/r/Interview wide.mp4"]);
    expect(overwriteAsk({ ...base, dismissed })).toBeNull();
  });

  it("still asks about a different file after one was declined", () => {
    // Declining one segment is not declining the next one.
    const dismissed = new Set(["D:/r/Something else.mp4"]);
    expect(overwriteAsk({ ...base, dismissed })).not.toBeNull();
  });

  it("asks again, at the moment of the cut, when the engine refuses one", () => {
    // Newer than the plan and about the cut that was actually attempted: a file can appear, or be
    // replaced, between the plan and the button. The names come with the refusal, so the dialog
    // says the same thing it would have said before the button was pressed.
    const ask = overwriteAsk({
      ...base,
      refusedOutput: "D:/r/Interview wide.mp4",
      refusedReason: "exists",
    });
    expect(ask).not.toBeNull();
    expect(ask?.source).toBe("refused");
    expect(ask?.files).toEqual(["Interview wide.mp4"]);
  });

  it("treats another kind of refusal as another kind of refusal", () => {
    // A run already in flight is also a 409. It is not a question about a file, and asking
    // "replace this?" about it would be a lie.
    const ask = overwriteAsk({
      ...base,
      files: [],
      refusedOutput: "D:/r/Interview wide.mp4",
      refusedReason: "name",
    });
    expect(ask).toBeNull();
  });

  it("never asks about a file the engine refused without naming", () => {
    // Nothing to put in the sentence, so there is no sentence to put.
    const ask = overwriteAsk({
      ...base,
      files: [],
      refusedOutput: "D:/r/x.mp4",
      refusedReason: "exists",
    });
    expect(ask).toBeNull();
  });

  it("does not ask again after a refusal for a file that was declined", () => {
    const dismissed = new Set(["D:/r/Interview wide.mp4"]);
    const ask = overwriteAsk({
      ...base,
      refusedOutput: "D:/r/Interview wide.mp4",
      refusedReason: "exists",
      dismissed,
    });
    expect(ask).toBeNull();
  });
});

describe("the words", () => {
  it("counts the files rather than saying 'file(s)'", () => {
    expect(overwriteTitle(["a.mp4"])).toBe("Replace this segment?");
    expect(overwriteTitle(["a.mp4", "a.srt"])).toBe("Replace these 2 files?");
  });

  it("says which moment it is, because the two are not the same moment", () => {
    expect(overwriteReason("planned")).toContain("already there");
    expect(overwriteReason("refused")).toContain("still there");
  });
});
