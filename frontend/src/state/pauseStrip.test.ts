import { describe, expect, it } from "vitest";

import { pauseNote, toBars } from "./pauseStrip";

describe("the pause strip", () => {
  it("places a pause as a percentage band", () => {
    expect(toBars([{ start: 25, end: 50 }], 100)).toEqual([{ left: 25, width: 25 }]);
  });

  it("clips a span that runs past the end rather than painting past the strip", () => {
    expect(toBars([{ start: 90, end: 140 }], 100)).toEqual([{ left: 90, width: 10 }]);
  });

  it("drops a span with no length, because a zero-width band says nothing", () => {
    expect(toBars([{ start: 10, end: 10 }], 100)).toEqual([]);
  });

  it("draws nothing when the duration is not a number a strip can be scaled from", () => {
    expect(toBars([{ start: 0, end: 1 }], 0)).toEqual([]);
  });

  it("says nothing about a file whose gaps are unknown", () => {
    expect(pauseNote([], 100)).toBeNull();
    expect(pauseNote([{ start: 0, end: 1 }], 0)).toBeNull();
  });

  it("counts the pauses and the share of the file that is quiet", () => {
    expect(pauseNote([{ start: 0, end: 30 }, { start: 60, end: 90 }], 300)).toBe(
      "2 pauses · 1m 00s quiet of 5m 00s (20%)",
    );
  });

  it("speaks of one pause as one", () => {
    expect(pauseNote([{ start: 0, end: 2 }], 10)).toBe("1 pause · 2s quiet of 10s (20%)");
  });
});
