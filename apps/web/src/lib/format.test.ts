/**
 * The interface's own tests.
 *
 * ## What is worth testing here, and what is not
 *
 * There is no DOM test in this file, and that is a decision rather than an omission. A rendered
 * component test would assert that this build of React puts a `<span>` where the last one did, which
 * is a fact about React and not about TheTrimmer. What *is* worth testing is the part of the
 * interface that makes a claim: the formatting. Every number a cutter reads — a length, a size, a
 * cost, a frame rate — comes out of `lib/format`, and a formatter that is wrong is a number a person
 * trusts and should not.
 *
 * The rest of the interface's contract is checked where it can actually fail: `ipc_contract.rs` in
 * the desktop crate drives the real commands through Tauri's real IPC layer, and the Rust suites
 * below it own every decision.
 */

import { describe, expect, it } from "vitest";

import {
  formatBytes,
  formatDuration,
  formatPercent,
  formatRate,
  formatTimestamp,
} from "./format";

describe("formatBytes", () => {
  it("steps through the units at 1024 and never shows a bare fraction of a byte", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(999)).toBe("999 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(1024 * 1024)).toBe("1.0 MB");
    expect(formatBytes(4_200_000_000)).toBe("3.9 GB");
  });

  it("says so rather than showing NaN for a number that is not a size", () => {
    expect(formatBytes(-1)).toBe("—");
    expect(formatBytes(Number.NaN)).toBe("—");
    expect(formatBytes(Number.POSITIVE_INFINITY)).toBe("—");
  });
});

describe("formatDuration", () => {
  it("gives a magnitude below a minute and a clock above it", () => {
    expect(formatDuration(0)).toBe("0.0s");
    expect(formatDuration(8.44)).toBe("8.4s");
    expect(formatDuration(59.9)).toBe("59.9s");
    expect(formatDuration(60)).toBe("1m 00s");
    expect(formatDuration(192)).toBe("3m 12s");
    expect(formatDuration(3720)).toBe("1h 02m");
    expect(formatDuration(7325)).toBe("2h 02m");
  });

  it("refuses a negative or unmeasurable duration instead of printing one", () => {
    expect(formatDuration(-1)).toBe("—");
    expect(formatDuration(Number.NaN)).toBe("—");
  });
});

describe("formatPercent", () => {
  it("does not claim precision the estimate does not have", () => {
    // A re-encode fraction of 0.0004 is not "0.04%"; the plan behind it is a rough one.
    expect(formatPercent(0.0004)).toBe("<1%");
    expect(formatPercent(0)).toBe("0%");
    expect(formatPercent(0.01)).toBe("1%");
    expect(formatPercent(0.333)).toBe("33%");
    expect(formatPercent(1)).toBe("100%");
  });
});

describe("formatRate", () => {
  it("prints the rates an editor says out loud", () => {
    expect(formatRate(25, 1)).toBe("25");
    expect(formatRate(24000, 1001)).toBe("23.976");
    expect(formatRate(30000, 1001)).toBe("29.97");
    expect(formatRate(60000, 1001)).toBe("59.94");
  });

  it("never divides by zero", () => {
    expect(formatRate(25, 0)).toBe("—");
  });
});

describe("formatTimestamp", () => {
  it("refuses an absent timestamp rather than rendering 1970", () => {
    expect(formatTimestamp(0)).toBe("—");
    expect(formatTimestamp(-1)).toBe("—");
  });

  it("renders a real timestamp as a date and a time", () => {
    const text = formatTimestamp(1_700_000_000);
    expect(text).not.toBe("—");
    expect(text).toMatch(/\d{4}/);
  });
});
