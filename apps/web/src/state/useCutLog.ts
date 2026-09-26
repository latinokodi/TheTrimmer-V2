/**
 * Live progress, and the log.
 *
 * ## Why this exists
 *
 * The window has always *emitted* progress — every step, every ffmpeg command line, every failure —
 * onto a `cut-progress` event, and nothing was listening. The status bar showed a label and the rest
 * was discarded. So a cut that took four minutes looked like a window that had stopped, and the one
 * thing that would have explained why was being thrown away.
 *
 * ## The three numbers, and why only one of them is a bar
 *
 * A cut is a **batch** of segments, each of which is a **sequence of ffmpeg passes**, each of which
 * reports **how far through itself** it is. Three levels, and they are not interchangeable:
 *
 * | Level | Comes from | Honest as a fraction? |
 * |---|---|---|
 * | `finished / total` segments | the queue's `Started` and `Finished` | yes, and very coarse — it moves five times in a nine-segment batch |
 * | steps done within a segment | the media layer's `Step` events | no: the head encode is most of the wall time and the join is none of it, so counting steps would sit at 20 % for four minutes |
 * | position within the current step | ffmpeg's own `-progress` output | **yes** — `out_time_us` against the length the step was built for |
 *
 * So the bar is the third one, always: it is the only fraction that is both fine-grained and true. The
 * first is shown as `segment k of n`, which is exactly what it is. The second is not shown as progress
 * at all, because it would be a lie told with real numbers.
 *
 * A step that does not know its own length reports ticks with no expectation, and the bar falls back to
 * the indeterminate sweep rather than inventing a denominator.
 */

import { useEffect, useRef, useState } from "react";

/** One line of the log. */
export interface LogLine {
  readonly at: number;
  /** Seconds since the run began, for a reader working out what took the time. */
  readonly offset: number;
  readonly text: string;
  readonly tone: "plain" | "step" | "command" | "error";
}

/** Where the run has got to. Every field is `null` until something reports it. */
export interface RunProgress {
  /** Segments finished, and how many the batch will run. */
  readonly finished: number;
  readonly total: number | null;
  /** The segment being worked on, by name. */
  readonly segment: string | null;
  /** The pass inside the segment: `head encode, frames 250..375`. */
  readonly step: string | null;
  /** Seconds of output ffmpeg has written in this pass, and how long the pass should be. */
  readonly doneSeconds: number | null;
  readonly totalSeconds: number | null;
  /** ffmpeg's own throughput as a multiple of real time, as it measures it. */
  readonly speed: number | null;
  readonly frame: number | null;
  readonly bytes: number | null;
  /** Seconds since the run began. */
  readonly elapsedSeconds: number;
}

const NOTHING: RunProgress = {
  finished: 0,
  total: null,
  segment: null,
  step: null,
  doneSeconds: null,
  totalSeconds: null,
  speed: null,
  frame: null,
  bytes: null,
  elapsedSeconds: 0,
};

/** What the shell's bridge offers for events. */
interface TauriEventBridge {
  readonly event?: {
    listen<T>(
      name: string,
      handler: (event: { readonly payload: T }) => void,
    ): Promise<() => void>;
  };
}

/** A `trimmer-media` progress message. */
interface MediaProgress {
  readonly kind: "step" | "command" | "elapsed" | "finished" | "message" | "ticks";
  readonly label?: string;
  readonly text?: string;
  readonly seconds?: number;
  readonly ok?: boolean;
  readonly ticks?: {
    readonly outSeconds?: number;
    readonly frame?: number;
    readonly speed?: number;
    readonly bytes?: number;
    readonly expectedSeconds?: number | null;
  };
}

/** A `trimmer-app` batch message. */
interface QueueProgress {
  readonly kind: "started" | "state" | "jobProgress" | "finished" | "completed";
  /** Which job. Every queue event but `started` and `completed` carries one, which is how they are told
   *  apart from the media layer's — see {@link isQueueEvent}. */
  readonly job?: number | string;
  readonly total?: number;
  readonly name?: string;
  readonly state?: string;
  readonly succeeded?: number;
  readonly failed?: number;
  readonly skipped?: number;
  readonly unverified?: number;
  readonly progress?: MediaProgress;
}

/**
 * Which vocabulary a payload belongs to.
 *
 * The two share an event name and, in one case, a `kind`: **both** have a `finished`. `trimmer-media`
 * means "this ffmpeg pass ended" and carries the pass's label, its seconds and whether it exited zero;
 * `trimmer-app` means "this segment ended" and carries the job it belongs to.
 *
 * The original listener sent every `finished` to the queue handler, so the log never showed a single
 * step's timing — every pass was announced as `segment finished`, and the number of segments that had
 * finished was in fact the number of ffmpeg passes that had. Nothing caught it because nothing in a
 * browser could produce these events at all.
 *
 * A queue `finished` is the one that names its job, and that is the test.
 */
function isQueueEvent(payload: MediaProgress | QueueProgress): payload is QueueProgress {
  switch (payload.kind) {
    case "started":
    case "completed":
    case "jobProgress":
    case "state":
      return true;
    case "finished":
      return "job" in payload;
    default:
      return false;
  }
}

/** How many lines to keep. A log that grows without bound is a memory leak with a scrollbar. */
const MAX_LINES = 400;

export function useCutLog(listening: boolean): {
  readonly lines: readonly LogLine[];
  readonly progress: RunProgress;
  readonly clear: () => void;
} {
  const [lines, setLines] = useState<readonly LogLine[]>([]);
  const [progress, setProgress] = useState<RunProgress>(NOTHING);
  const counter = useRef(0);
  const startedAt = useRef<number | null>(null);
  /**
   * `finished` is counted from the events rather than read off the progress state, because the log and
   * the progress update in different places and a stale closure would drop a segment.
   */
  const finished = useRef(0);

  // A clock, so the elapsed readout keeps counting between events. Without it a step that reports every
  // half-second looks live and a step that reports nothing looks frozen — and the one thing a waiting
  // operator needs is to see that time is still passing.
  useEffect(() => {
    if (!listening) {
      return;
    }
    const timer = window.setInterval(() => {
      setProgress((current) =>
        startedAt.current === null
          ? current
          : { ...current, elapsedSeconds: (Date.now() - startedAt.current) / 1000 },
      );
    }, 250);
    return () => window.clearInterval(timer);
  }, [listening]);

  useEffect(() => {
    if (!listening) {
      return;
    }
    const bridge = (window as unknown as { readonly __TAURI__?: TauriEventBridge }).__TAURI__;
    if (bridge?.event === undefined) {
      // The browser harness has no event bridge, so a cut's progress is simply not shown there. The
      // interface must not fail because of it — the log is a courtesy, not a control.
      return;
    }

    let stop: (() => void) | null = null;
    let cancelled = false;

    const offset = (): number =>
      startedAt.current === null ? 0 : (Date.now() - startedAt.current) / 1000;

    const push = (text: string, tone: LogLine["tone"]): void => {
      counter.current += 1;
      const line = { at: counter.current, offset: offset(), text, tone };
      setLines((current) => {
        const next = [...current, line];
        return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
      });
    };

    const begin = (): void => {
      if (startedAt.current === null) {
        startedAt.current = Date.now();
      }
    };

    const onMedia = (media: MediaProgress): void => {
      switch (media.kind) {
        case "step":
          if (media.label !== undefined) {
            // A new pass: the position it reports next belongs to *this* pass, not the last one. Clearing
            // the fraction here is what stops the bar showing the previous step's number for a moment.
            setProgress((current) => ({
              ...current,
              step: media.label ?? null,
              doneSeconds: null,
              totalSeconds: null,
              speed: null,
              frame: null,
              bytes: null,
            }));
            push(media.label, "step");
          }
          break;
        case "command":
          // The exact command line, which is the one thing that makes a slow step explicable.
          if (media.text !== undefined) {
            push(media.text, "command");
          }
          break;
        case "finished":
          if (media.label !== undefined) {
            push(
              `${media.label} — ${media.ok === true ? "ok" : "FAILED"} (${(media.seconds ?? 0).toFixed(1)}s)`,
              media.ok === true ? "plain" : "error",
            );
          }
          break;
        case "message":
          if (media.text !== undefined) {
            push(media.text, "error");
          }
          break;
        case "ticks": {
          const ticks = media.ticks;
          if (ticks === undefined) {
            break;
          }
          setProgress((current) => ({
            ...current,
            doneSeconds: ticks.outSeconds ?? current.doneSeconds,
            // An expectation of `null` means the step does not know its own length, which must clear the
            // denominator rather than keep the previous step's — otherwise the bar would measure this
            // pass against the last one's length.
            totalSeconds:
              ticks.expectedSeconds === undefined ? current.totalSeconds : ticks.expectedSeconds,
            speed: ticks.speed ?? current.speed,
            frame: ticks.frame ?? current.frame,
            bytes: ticks.bytes ?? current.bytes,
          }));
          break;
        }
        case "elapsed":
          // Superseded by the clock and by the ticks; the heartbeat only ever said "still alive".
          break;
      }
    };

    const onQueue = (event: QueueProgress): void => {
      switch (event.kind) {
        case "started":
          begin();
          finished.current = 0;
          setProgress((current) => ({ ...current, finished: 0, total: event.total ?? null }));
          push(`batch of ${event.total ?? 0} segment(s)`, "step");
          break;
        case "state":
          setProgress((current) => ({ ...current, segment: event.name ?? current.segment }));
          push(`${event.name ?? "segment"}: ${event.state ?? ""}`, "step");
          break;
        case "jobProgress":
          if (event.progress !== undefined) {
            onMedia(event.progress);
          }
          break;
        case "finished":
          finished.current += 1;
          setProgress((current) => ({
            ...current,
            finished: finished.current,
            step: null,
            doneSeconds: null,
            totalSeconds: null,
          }));
          push(`${event.name ?? "segment"} finished`, "plain");
          break;
        case "completed":
          push(
            `done: ${event.succeeded ?? 0} written, ${event.unverified ?? 0} uncertified, ` +
              `${event.failed ?? 0} failed, ${event.skipped ?? 0} skipped`,
            (event.failed ?? 0) > 0 ? "error" : "plain",
          );
          setProgress((current) => ({
            ...current,
            step: null,
            doneSeconds: null,
            totalSeconds: null,
            speed: null,
          }));
          break;
      }
    };

    void bridge.event
      .listen<MediaProgress | QueueProgress>("cut-progress", (event) => {
        const payload = event.payload;
        if (isQueueEvent(payload)) {
          onQueue(payload);
        } else {
          onMedia(payload);
        }
      })
      .then((unlisten) => {
        if (cancelled) {
          unlisten();
        } else {
          stop = unlisten;
        }
      });

    return () => {
      cancelled = true;
      if (stop !== null) {
        stop();
      }
    };
  }, [listening]);

  // A new run starts from zero rather than from the last one's numbers.
  useEffect(() => {
    if (listening) {
      startedAt.current = Date.now();
      finished.current = 0;
      setProgress({ ...NOTHING, elapsedSeconds: 0 });
    }
  }, [listening]);

  return { lines, progress, clear: () => setLines([]) };
}

/** The fraction of the current step that is done, or `null` when the step does not know its length. */
export function stepFraction(progress: RunProgress): number | null {
  const { doneSeconds, totalSeconds } = progress;
  if (doneSeconds === null || totalSeconds === null || totalSeconds <= 0) {
    return null;
  }
  return Math.min(1, Math.max(0, doneSeconds / totalSeconds));
}

/**
 * Seconds left in the current step, from ffmpeg's own throughput.
 *
 * `speed` is output seconds produced per second of wall clock, measured by the encoder — so
 * `remaining_output / speed` is an estimate the program doing the work produced, not one extrapolated
 * from how long we have been waiting. It is `null` until there is both a rate and a position, because a
 * guess early in a step is worse than no number.
 */
export function remainingSeconds(progress: RunProgress): number | null {
  const { doneSeconds, totalSeconds, speed } = progress;
  if (doneSeconds === null || totalSeconds === null || speed === null || speed <= 0) {
    return null;
  }
  const left = totalSeconds - doneSeconds;
  return left <= 0 ? 0 : left / speed;
}
