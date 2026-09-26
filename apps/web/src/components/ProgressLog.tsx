/**
 * The progress strip and the log.
 *
 * A cut is a sequence of ffmpeg processes that take seconds to minutes, and the two things a person
 * waiting wants are "how far along" and "what is it doing". A label in a status bar answers neither,
 * which is why this is a zone of its own with a bar, a status line and the log underneath.
 *
 * The log is the reason a cut that went wrong can be explained afterwards without running it again: it
 * carries every step, its timing, and the **exact ffmpeg command line** that ran. The shell has always
 * emitted all of it; until now nothing was listening.
 *
 * It scrolls inside itself and the frame does not move — which is the distinction that matters in a
 * fixed panel: an unbounded list gets its own well, and the application around it stays put.
 */

import { useEffect, useRef } from "react";

import type { LogLine } from "../state/useCutLog";

export function ProgressLog({
  lines,
  step,
  running,
}: {
  readonly lines: readonly LogLine[];
  readonly step: string | null;
  readonly running: boolean;
}): JSX.Element {
  const log = useRef<HTMLDivElement>(null);

  // Follow the tail, the way a terminal does — but only when it is already at the bottom, so scrolling
  // up to read something is not undone by the next line arriving.
  useEffect(() => {
    const element = log.current;
    if (element === null) {
      return;
    }
    const atBottom = element.scrollHeight - element.scrollTop - element.clientHeight < 40;
    if (atBottom) {
      element.scrollTop = element.scrollHeight;
    }
  }, [lines]);

  return (
    <>
      <div className="progress-strip">
        <span
          className={`progress-bar${running ? " progress-bar--running" : ""}`}
          aria-hidden="true"
        />
        <p className="progress-status" role="status" aria-live="polite">
          {running ? (step ?? "working…") : "ready"}
        </p>
        <span className="spacer" />
        <span className="progress-status">
          {lines.length === 0 ? "no output yet" : `${lines.length} line(s)`}
        </span>
      </div>

      <div className="log" ref={log} role="log" aria-label="Cut log">
        {lines.length === 0 ? (
          <p className="log__empty">
            Nothing has run yet. Every step, and the exact ffmpeg command line behind it, appears here —
            including the frames that were copied rather than re-encoded.
          </p>
        ) : (
          lines.map((line) => (
            <p key={line.at} className={`log__line log__line--${line.tone}`}>
              {line.text}
            </p>
          ))
        )}
      </div>
    </>
  );
}
