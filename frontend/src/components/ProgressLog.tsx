/**
 * The progress strip and the log.
 *
 * A cut is a sequence of ffmpeg passes that take seconds to minutes, and the two things a
 * person waiting wants are "how far along" and "what is it doing". A label in a status bar
 * answers neither, which is why this is a zone of its own with a bar, a readout and the log
 * underneath.
 *
 * ## The bar means one thing, and says which
 *
 * It is the fraction of **the pass ffmpeg is running right now** — its own `out_time_us`
 * against the length the pass was built for. That is the only fraction in the system that is
 * both fine-grained and true: counting passes would sit near the start for most of a run,
 * because the head encode is most of the wall time and the join is none of it.
 *
 * When a pass does not know its own length there is no fraction, and the bar is not a
 * `<progress>` at all — it is a plain track with a travelling segment. One element per
 * meaning, and no number invented.
 *
 * ## The log
 *
 * Every pass with its timing, the exact command line that ran, and whatever the engine said.
 * Each line is stamped with the seconds since the run began, because "which of these took
 * the four minutes" is the question a log exists to answer.
 */

import { useEffect, useRef } from "react";

import { type LogLine, type RunProgress, newestFirst, remainingSeconds, stepFraction } from "../state/useRunLog";
import { formatBytes, formatClock } from "../lib/format";

export function ProgressLog({
  lines,
  progress,
  running,
  elapsed,
  onCancel,
}: {
  readonly lines: readonly LogLine[];
  readonly progress: RunProgress;
  readonly running: boolean;
  readonly elapsed: number;
  readonly onCancel: () => void;
}): JSX.Element {
  const log = useRef<HTMLDivElement>(null);

  // The newest line is at the top, and the top is where a reader looks, so the log stays there. A
  // log that had to be scrolled to see what just happened would be hiding the thing it exists to
  // show — which is what it did when it appended downwards.
  useEffect(() => {
    const element = log.current;
    if (element === null) {
      return;
    }
    element.scrollTop = 0;
  }, [lines]);

  const fraction = stepFraction(progress);
  const remaining = remainingSeconds(progress);

  return (
    <>
      <div className="progress-strip">
        {running && fraction !== null ? (
          <progress
            className="progress-bar"
            value={fraction}
            max={1}
            aria-label="Progress of the current pass"
          />
        ) : (
          <div
            className={`progress-bar${running ? " progress-bar--running" : ""}`}
            aria-hidden="true"
          />
        )}
        {fraction !== null ? (
          <span className="progress-percent figures">{Math.round(fraction * 100)}%</span>
        ) : null}

        <p className="progress-status" role="status" aria-live="polite">
          {running ? (progress.stage ?? "working…") : "ready"}
        </p>

        {running ? (
          <button
            type="button"
            className="btn btn--small"
            onClick={onCancel}
            title="Stop after the file in flight. What is already written is kept."
          >
            Cancel
          </button>
        ) : null}

        <span className="spacer" />

        {running ? (
          <>
            {/*
              Two clocks, and they answer different questions, so they are labelled. The left
              one is where the pass has got to in *output seconds*; this one is how long the
              run has been going in wall seconds. Without the second, a pass that reports
              nothing looks frozen when it is merely quiet.
            */}
            <span className="progress-figure figures" title="since this run began">
              {formatClock(elapsed)}
            </span>
            <span className="progress-figure figures" title="seconds of output written by this pass">
              out {formatClock(progress.doneSeconds ?? 0)}
            </span>
          </>
        ) : null}
        {running && progress.speed !== null ? (
          <span className="progress-figure figures" title="ffmpeg's own throughput">
            {progress.speed.toFixed(2)}×
          </span>
        ) : null}
        {running && progress.frame !== null ? (
          <span className="progress-figure figures">{progress.frame.toLocaleString()} f</span>
        ) : null}
        {running && progress.size !== null ? (
          <span className="progress-figure figures">{formatBytes(progress.size)}</span>
        ) : null}
        {running && remaining !== null ? (
          <span className="progress-figure figures" title="estimated from ffmpeg's own rate">
            {/* Sub-second rather than `0:00`, which reads as "done" when it is least true. */}
            ~{remaining < 1 ? "<1s" : formatClock(remaining)} left
          </span>
        ) : null}

        <span className="progress-figure figures">
          {lines.length === 0 ? "no output yet" : `${lines.length} line(s)`}
        </span>
      </div>

      <div className="log" ref={log} role="log" aria-label="Cut log">
        {lines.length === 0 ? (
          <p className="log__empty">
            Nothing has run yet. Every pass, its timing, and the exact ffmpeg command line behind
            it appear here, newest first — including the frames that were copied rather than
            re-encoded.
          </p>
        ) : (
          newestFirst(lines).map((line) => (
            <p key={line.at} className={`log__line log__line--${line.tone}`}>
              <span className="log__at figures">{formatClock(line.offset)}</span>
              <span className="log__text">{line.text}</span>
            </p>
          ))
        )}
      </div>
    </>
  );
}
