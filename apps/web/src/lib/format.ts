/**
 * Turning numbers into words a person reads.
 *
 * The interface shows sizes, durations, fractions and frame rates. Each of these is a small
 * formatting decision that is wrong in a different way if it is written twice, so each has exactly
 * one home.
 */

/** Bytes as a human-readable size. Used for an estimate, so one decimal is plenty. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) {
    return "—";
  }
  const units = ["B", "KB", "MB", "GB", "TB"] as const;
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 ? 0 : 1;
  return `${value.toFixed(digits)} ${units[unit] ?? "B"}`;
}

/** Seconds as `1h 02m` or `3m 12s` or `8.4s`. An editor reads a magnitude, not a precision. */
export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) {
    return "—";
  }
  if (seconds < 60) {
    return `${seconds.toFixed(1)}s`;
  }
  const total = Math.round(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  if (hours > 0) {
    return `${hours}h ${String(minutes).padStart(2, "0")}m`;
  }
  return `${minutes}m ${String(secs).padStart(2, "0")}s`;
}

/**
 * A fraction as a percentage, at a resolution that does not lie.
 *
 * Below one percent the useful statement is "under 1%", not "0.04%": the estimate behind the number
 * is a rough one, and printing four significant figures of a rough number is false precision.
 */
export function formatPercent(fraction: number): string {
  if (!Number.isFinite(fraction) || fraction < 0) {
    return "—";
  }
  const percent = fraction * 100;
  if (percent > 0 && percent < 1) {
    return "<1%";
  }
  return `${Math.round(percent)}%`;
}

/** A frame rate as a person says it: `29.97`, `25`, `23.976`. */
export function formatRate(num: number, den: number): string {
  if (den === 0) {
    return "—";
  }
  const value = num / den;
  if (Number.isInteger(value)) {
    return String(value);
  }
  // Three decimals distinguishes 23.976 from 24 and 29.97 from 30, which is the whole point.
  return value.toFixed(3).replace(/0+$/, "").replace(/\.$/, "");
}

/** A unix timestamp as a local date and time, for a project listing. */
export function formatTimestamp(unixSeconds: number): string {
  if (!Number.isFinite(unixSeconds) || unixSeconds <= 0) {
    return "—";
  }
  const date = new Date(unixSeconds * 1000);
  return date.toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}
