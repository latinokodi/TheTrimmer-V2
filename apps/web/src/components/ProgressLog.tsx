/**
 * The progress strip and the log.
 *
 * A cut is a sequence of ffmpeg processes that take seconds to minutes, and the two things a person
 * waiting wants are "how far along" and "what is it doing". A label in a status bar answers neither,
 * which is why this is a zone of its own with a bar, a readout and the log underneath.
 *
 * ## The bar means one thing, and says which
 *
 * It is the fraction of **the pass ffmpeg is running right now** — `out_time_us` against the length the
 * pass was built for. That is the only fraction in the system that is both fine-grained and true: the
 * batch's own fraction moves five times in a nine-segment batch, and counting ffmpeg passes would sit at
 * 20 % for four minutes because the head encode is most of the work and the join is none of it.
 *
 * When a pass does not know its own length there is no fraction, and the bar goes back to the
 * indeterminate sweep rather than filling to a number nobody measured. The batch's own position is not
 * lost — it is the `segment 2 of 5` in the readout, which is exactly what it is.
 *
 * The log is the reason a cut that went wrong can be explained afterwards without running it again: it
 * carries every pass with its timing, the **exact ffmpeg command line** that ran, and whatever the child
 * said. Each line is stamped with the seconds since the run began, because "which of these took the
 * four minutes" is the question a log exists to answer.
 *
 * It scrolls inside itself and the frame does not move — which is the distinction that matters in a
 * fixed panel: an unbounded list gets its own well, and the application around it stays put.
 */

import { useEffect, useRef } from "react";

import { type LogLine, type RunProgress, remainingSeconds, stepFraction } from "../state/useCutLog";
import { formatBytes, formatClock } from "../lib/format";

export function ProgressLog({
  lines,
  progress,
  running,
}: {
  readonly lines: readonly LogLine[];
  readonly progress: RunProgress;
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

  const fraction = stepFraction(progress);
  const remaining = remainingSeconds(progress);

  return (
    <>
      <div className="progress-strip">
        {/*
          A real `<progress>` element when there is a fraction — a screen reader announces its value, and
          the number beside it is the same figure for everyone else.

          When there is no fraction it is **not** a `<progress>` at all, and that is deliberate rather
          than a styling accident. An indeterminate `<progress>` is a replaced element with its own
          user-agent layout, and its `::after` is laid out against that rather than against the 6 px
          track — the sweep overflowed its own box by 12 px, which the clipping probe reported the moment
          this was written. A step of unknown length has no value to announce, so it gets a plain
          `aria-hidden` element with the sweep on it and the `role="status"` beside it says what is
          happening. One element per meaning, and no fight with the platform's own layout.
        */}
        {running && fraction !== null ? (
          <progress
            className="progress-bar"
            value={fraction}
            max={1}
            aria-label="Progress of the current step"
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
          {running ? (progress.step ?? progress.segment ?? "working…") : "ready"}
        </p>

        <span className="spacer" />

        {/*
          The numbers, in the order a person reads them: where I am in the batch, how long it has been,
          how fast it is going, and how much longer. Each is left out when it is unknown rather than
          shown as a zero, because a readout that says `0:00 left` at the start of a nine-minute job is
          worse than one that says nothing.
        */}
        {running && progress.total !== null ? (
          <span className="progress-figure figures">
            segment {Math.min(progress.finished + 1, progress.total)} of {progress.total}
          </span>
        ) : null}
        {running ? (
          <span className="progress-figure figures">{formatClock(progress.elapsedSeconds)}</span>
        ) : null}
        {running && progress.speed !== null ? (
          <span className="progress-figure figures" title="ffmpeg's own throughput">
            {progress.speed.toFixed(2)}×
          </span>
        ) : null}
        {running && progress.bytes !== null ? (
          <span className="progress-figure figures">{formatBytes(progress.bytes)}</span>
        ) : null}
        {running && remaining !== null ? (
          <span className="progress-figure figures" title="estimated from ffmpeg's own rate">
            {/* Sub-second rather than `0:00`, which reads as "done" at the moment it is least true. */}
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
            Nothing has run yet. Every pass, its timing, and the exact ffmpeg command line behind it
            appear here — including the frames that were copied rather than re-encoded.
          </p>
        ) : (
          lines.map((line) => (
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
