/**
 * The progress strip and the log.
 *
 * The original application put these at the bottom of the window, and it was right to: a cut is a
 * sequence of ffmpeg processes that take seconds to minutes, and the two things a person waiting wants
 * are "how far along" and "what is it doing". A label in a status bar answers neither.
 *
 * The log is monospace, scrolls, and holds four hundred lines. It is not a debug console that only a
 * developer reads — it is where the exact command line, the step timings and any failure appear, and it
 * is the reason a cut that went wrong can be explained afterwards without re-running it.
 */

import { useEffect, useRef } from "react";

import type { LogLine } from "../state/useCutLog";

export function ProgressLog({
  lines,
  step,
  running,
  onClear,
}: {
  readonly lines: readonly LogLine[];
  readonly step: string | null;
  readonly running: boolean;
  readonly onClear: () => void;
}): JSX.Element {
  const log = useRef<HTMLDivElement>(null);

  // Follow the tail, the way a terminal does. Only when it is already at the bottom, so scrolling up to
  // read something is not undone by the next line arriving.
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
    <section className="card card--log" aria-label="Progress">
      <header className="card__head">
        <h2 className="card__title">Progress</h2>
        <span className="spacer" />
        {lines.length > 0 ? (
          <button type="button" className="btn btn--ghost btn--small" onClick={onClear}>
            Clear
          </button>
        ) : null}
      </header>

      <div className="card__body">
        <div className="progress-strip">
          <div className={`progress-bar${running ? " progress-bar--running" : ""}`} aria-hidden="true" />
          <p className="progress-status" role="status" aria-live="polite">
            {running ? (step ?? "working…") : "ready"}
          </p>
        </div>

        <div className="log scroll" ref={log} role="log" aria-label="Cut log">
          {lines.length === 0 ? (
            <p className="log__empty">
              Nothing has run yet. Every step, and the exact ffmpeg command line behind it, appears
              here — including the frames that were copied rather than re-encoded.
            </p>
          ) : (
            lines.map((line) => (
              <p key={line.at} className={`log__line log__line--${line.tone}`}>
                {line.text}
              </p>
            ))
          )}
        </div>
      </div>
    </section>
  );
}
