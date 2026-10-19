/**
 * A stretch of the source's timeline, in seconds.
 *
 * Declared here rather than imported from `api.ts` so this module stays free of the browser: the
 * feature scenarios run in node, and `api.ts` reads `window` as it loads.
 */
export interface Span {
  readonly start: number;
  readonly end: number;
}

/** One shaded stretch of the strip, as percentages of the whole file. */
export interface Bar {
  readonly left: number;
  readonly width: number;
}

/**
 * The pauses as a strip under the marks.
 *
 * Percentages rather than pixels because the strip is as wide as the zone and the file is not:
 * the drawing has to survive a resize without measuring anything. A span that runs past the end
 * of the file — a truncated measurement, or a duration that rounds — is clipped rather than
 * allowed to paint past the strip, and a span with no length is dropped because a zero-width
 * band is an invisible artifact rather than information.
 */
export function toBars(spans: readonly Span[], duration: number): readonly Bar[] {
  if (!(duration > 0)) return [];
  const bars: Bar[] = [];
  for (const span of spans) {
    const start = Math.max(0, Math.min(duration, span.start));
    const end = Math.max(0, Math.min(duration, span.end));
    if (end <= start) continue;
    bars.push({ left: (start / duration) * 100, width: ((end - start) / duration) * 100 });
  }
  return bars;
}

/**
 * One line for the zone's note: how many pauses there are, and how much of the file is quiet.
 *
 * `null` when there is nothing to say — a file with no sound, or one whose gaps have not been
 * measured yet, both arrive as an empty list and neither deserves a sentence claiming it is
 * silent throughout.
 */
export function pauseNote(spans: readonly Span[], duration: number): string | null {
  if (!(duration > 0) || spans.length === 0) return null;
  const quiet = spans.reduce((total, span) => total + Math.max(0, span.end - span.start), 0);
  const share = Math.round((quiet / duration) * 100);
  const gaps = spans.length === 1 ? "1 pause" : `${spans.length} pauses`;
  return `${gaps} · ${clock(quiet)} quiet of ${clock(duration)} (${share}%)`;
}

function clock(seconds: number): string {
  const total = Math.max(0, Math.round(seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const rest = total % 60;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${String(rest).padStart(2, "0")}s`;
  return `${rest}s`;
}
