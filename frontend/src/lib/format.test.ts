/**
 * The formatters, checked where being wrong is invisible.
 *
 * All three exist to stop a number being printed twice, two ways. `formatClock` is the one that
 * matters: every duration in the window is read by scanning a column, so a width that changes as it
 * counts is a column the eye has to find again on every line.
 */

import { describe, expect, it } from "vitest";

import { formatBytes, formatClock } from "./format";

describe("formatClock", () => {
  it("keeps a fixed width as the seconds count up", () => {
    // The point of the formatter: these are all the same width when drawn.
    for (const seconds of [0, 5, 59, 60, 605, 3599]) {
      expect(formatClock(seconds)).toMatch(/^\d+:\d{2}$/);
    }
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(5)).toBe("0:05");
    expect(formatClock(65)).toBe("1:05");
    expect(formatClock(605)).toBe("10:05");
  });

  it("grows to hours rather than showing minutes past sixty", () => {
    expect(formatClock(3600)).toBe("1:00:00");
    expect(formatClock(3725)).toBe("1:02:05");
  });

  it("refuses to invent a figure for a number that is not one", () => {
    expect(formatClock(Number.NaN)).toBe("—");
    expect(formatClock(Number.POSITIVE_INFINITY)).toBe("—");
    expect(formatClock(-1)).toBe("—");
  });
});

describe("formatBytes", () => {
  it("reads as a magnitude with one decimal past the first unit", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
  });

  it("refuses to invent a figure for a number that is not one", () => {
    expect(formatBytes(Number.NaN)).toBe("—");
    expect(formatBytes(-1)).toBe("—");
  });
});
