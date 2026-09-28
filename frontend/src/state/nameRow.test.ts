/**
 * The segment-name row's three states, and the one that used to be missing.
 *
 * The bug these tests exist for was reported as "I saw a message saying that it could not be used".
 * The message appeared for every name typed before a range was marked, because the row treated
 * "no path yet" as "refused". A test that only covered the good and the refused name would have
 * passed throughout.
 */

import { describe, expect, it } from "vitest";

import { nameRow } from "./nameRow";

const base = { typed: "", output: "", refused: null, hasSource: true };

describe("nameRow", () => {
  it("explains what an empty box does", () => {
    const row = nameRow(base);
    expect(row.text).toBe("leave empty for the range name");
    expect(row.problem).toBe(false);
  });

  it("treats whitespace as an empty box, because the engine does", () => {
    expect(nameRow({ ...base, typed: "   " }).text).toBe("leave empty for the range name");
  });

  it("shows where the segment will go, once the engine has said", () => {
    const row = nameRow({ ...base, typed: "Interview wide", output: "D:/r/Interview wide.mp4" });
    expect(row.text).toBe("D:/r/Interview wide.mp4");
    expect(row.problem).toBe(false);
  });

  it("says nothing has been refused while the answer is still coming", () => {
    // The state that was missing. No path yet, nothing refused — and the row must not invent one.
    const row = nameRow({ ...base, typed: "Interview wide", output: "" });
    expect(row.problem).toBe(false);
    expect(row.text).not.toContain("cannot");
  });

  it("never calls a name unusable just because no range is marked", () => {
    // The reported fault, at its narrowest: a source is loaded, nothing is refused, the plan has
    // not run because there are no marks, and the name is perfectly good.
    const row = nameRow({ ...base, typed: "Interview wide", output: "" });
    expect(row.text).not.toMatch(/cannot|unusable|invalid/i);
    expect(row.problem).toBe(false);
  });

  it("asks for a file when there is none to name segments of", () => {
    const row = nameRow({ ...base, typed: "Interview wide", hasSource: false });
    expect(row.text).toBe("load a file to name its segments");
    expect(row.problem).toBe(false);
  });

  it("shows a refusal in the engine's own words", () => {
    const said = "the segment name contains : which Windows does not allow in a file name";
    const row = nameRow({ ...base, typed: "Take 1:2", refused: said });
    expect(row.text).toBe(said);
    expect(row.problem).toBe(true);
  });

  it("lets a refusal win over a path it was told about earlier", () => {
    // The refusal is newer than the path, so it is the one that is true.
    const row = nameRow({ ...base, typed: "Take 1:2", output: "D:/r/old.mp4", refused: "a colon" });
    expect(row.problem).toBe(true);
    expect(row.text).toBe("a colon");
  });
});
