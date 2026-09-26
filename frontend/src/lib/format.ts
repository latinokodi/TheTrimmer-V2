/**
 * Turning numbers into words a person reads.
 *
 * The interface shows sizes, positions and estimates. Each of these is a small formatting
 * decision that is wrong in a different way if it is written twice, so each has exactly one
 * home — and a formatter with no caller does not have one, so there are three here and not
 * seven.
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

/**
 * Seconds as a **fixed-width clock**: `0:08`, `3:12`, `1:02:03`.
 *
 * Every duration in this interface is read by scanning a column — off a log's left edge, or
 * down a countdown — and a column whose width changes as it counts is a column the eye has to
 * find again on every line. That is the whole reason there is no `8.4s` form here.
 */
export function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) {
    return "—";
  }
  const total = Math.floor(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  const tail = `${minutes}:${String(secs).padStart(2, "0")}`;
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${String(secs).padStart(2, "0")}` : tail;
}
