/**
 * The run's state: where it has got to, and what it has said.
 *
 * ## The one number the bar is allowed to use
 *
 * The engine reports, for each ffmpeg pass, how many seconds of output it has written and
 * how long that pass should be. The fraction of those two is the only progress figure in the
 * system that is both fine-grained and true — so it is the one the bar draws. Everything
 * else here is a measurement passed through: ffmpeg's own throughput, which is where the
 * estimate comes from, and the frames and bytes it has written.
 *
 * When a pass does not know its own length the fraction is `null` and the bar goes back to
 * the indeterminate sweep, rather than filling to a number nobody measured.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { TickView } from "../api";

/** One line of the log. */
export interface LogLine {
  readonly at: number;
  /** Seconds since the run began, for a reader working out what took the time. */
  readonly offset: number;
  readonly text: string;
  readonly tone: "plain" | "stage" | "command" | "done" | "error" | "heartbeat";
}

/** Where the run has got to. Every field is `null` until something reports it. */
export interface RunProgress {
  readonly stage: string | null;
  readonly doneSeconds: number | null;
  readonly totalSeconds: number | null;
  readonly speed: number | null;
  readonly frame: number | null;
  readonly size: number | null;
}

const NOTHING: RunProgress = {
  stage: null,
  doneSeconds: null,
  totalSeconds: null,
  speed: null,
  frame: null,
  size: null,
};

/** How many lines to keep. A log that grows without bound is a memory leak with a scrollbar. */
const MAX_LINES = 400;

export interface RunLog {
  readonly lines: readonly LogLine[];
  readonly progress: RunProgress;
  readonly running: boolean;
  readonly elapsed: number;
  /** A run has begun: clear the last one and start the clock. */
  readonly begin: () => void;
  readonly tick: (event: TickView) => void;
  readonly line: (text: string, level: string) => void;
  readonly command: (args: readonly string[]) => void;
  readonly stage: (label: string) => void;
  readonly finish: (ok: boolean, summary: string) => void;
  readonly cancelled: () => void;
  readonly clear: () => void;
}

export function useRunLog(): RunLog {
  const [lines, setLines] = useState<readonly LogLine[]>([]);
  const [progress, setProgress] = useState<RunProgress>(NOTHING);
  const [running, setRunning] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const counter = useRef(0);
  const startedAt = useRef<number | null>(null);

  const offset = useCallback(
    (): number => (startedAt.current === null ? 0 : (Date.now() - startedAt.current) / 1000),
    [],
  );

  const push = useCallback(
    (text: string, tone: LogLine["tone"]) => {
      const trimmed = text.trimEnd();
      if (trimmed === "") {
        return;
      }
      counter.current += 1;
      const entry = { at: counter.current, offset: offset(), text: trimmed, tone };
      setLines((current) => {
        const next = [...current, entry];
        return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
      });
    },
    [offset],
  );

  // A clock, so the elapsed readout keeps counting between events. Without it a pass that
  // reports every half-second looks live and one that reports nothing looks frozen — and
  // seeing that time is still passing is the whole point of an elapsed figure.
  useEffect(() => {
    if (!running) {
      return;
    }
    const timer = window.setInterval(() => setElapsed(offset()), 250);
    return () => window.clearInterval(timer);
  }, [offset, running]);

  const begin = useCallback(() => {
    startedAt.current = Date.now();
    counter.current = 0;
    setElapsed(0);
    setLines([]);
    setProgress(NOTHING);
    setRunning(true);
  }, []);

  const tick = useCallback((event: TickView) => {
    setProgress((current) => ({
      ...current,
      doneSeconds: event.outSeconds,
      // A pass that does not know its length clears the denominator rather than keeping the
      // last pass's: measuring this one against that would be a made-up percentage.
      totalSeconds: event.expectedSeconds,
      speed: event.speed ?? current.speed,
      frame: event.frame ?? current.frame,
      size: event.size ?? current.size,
    }));
  }, []);

  const line = useCallback(
    (text: string, level: string) => {
      const tone: LogLine["tone"] =
        level === "command"
          ? "command"
          : level === "error"
            ? "error"
            : level === "done"
              ? "done"
              : level === "heartbeat"
                ? "heartbeat"
                : "stage";
      push(text, tone);
    },
    [push],
  );

  const command = useCallback((args: readonly string[]) => push(args.join(" "), "command"), [push]);

  const stage = useCallback(
    (label: string) => {
      // A new stage: the position it reports next belongs to *this* pass, not the last one.
      setProgress((current) => ({
        ...current,
        stage: label,
        doneSeconds: null,
        totalSeconds: null,
      }));
      push(label, "stage");
    },
    [push],
  );

  const finish = useCallback(
    (ok: boolean, summary: string) => {
      setRunning(false);
      setProgress((current) => ({ ...current, stage: null, doneSeconds: null, totalSeconds: null }));
      push(summary, ok ? "done" : "error");
    },
    [push],
  );

  const cancelled = useCallback(() => {
    setRunning(false);
    setProgress(NOTHING);
    push("cancelled. What was already written is kept, and the marks are still here.", "plain");
  }, [push]);

  const clear = useCallback(() => setLines([]), []);

  /**
   * The same object between renders unless something in it changed.
   *
   * This is not a micro-optimisation, it is load-bearing. The window subscribes to the run
   * once per value of this object, so a fresh literal on every render tore the event stream
   * down and rebuilt it several times a second — and each new subscriber is replayed the
   * recent history, so the log filled with its own past and the backend was left holding
   * hundreds of half-closed sockets. A stable identity is what makes "subscribe to the run"
   * mean subscribe once.
   */
  return useMemo(
    () => ({
      lines,
      progress,
      running,
      elapsed,
      begin,
      tick,
      line,
      command,
      stage,
      finish,
      cancelled,
      clear,
    }),
    [lines, progress, running, elapsed, begin, tick, line, command, stage, finish, cancelled, clear],
  );
}

/** The fraction of the current pass that is done, or `null` when its length is not known. */
export function stepFraction(progress: RunProgress): number | null {
  const { doneSeconds, totalSeconds } = progress;
  if (doneSeconds === null || totalSeconds === null || totalSeconds <= 0) {
    return null;
  }
  return Math.min(1, Math.max(0, doneSeconds / totalSeconds));
}

/**
 * Seconds left in the current pass, from ffmpeg's own throughput.
 *
 * `speed` is output seconds produced per second of wall clock, measured by the encoder — so
 * `remaining_output / speed` is an estimate the program doing the work produced, not one
 * extrapolated from how long we have been waiting. It is `null` until there is both a rate
 * and a position, because a guess early in a pass is worse than no number.
 */
export function remainingSeconds(progress: RunProgress): number | null {
  const { doneSeconds, totalSeconds, speed } = progress;
  if (doneSeconds === null || totalSeconds === null || speed === null || speed <= 0) {
    return null;
  }
  const left = totalSeconds - doneSeconds;
  return left <= 0 ? 0 : left / speed;
}
