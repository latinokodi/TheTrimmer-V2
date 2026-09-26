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
 * This is the listener. It renders the same two things the original application rendered: a progress
 * strip, and a log you can read afterwards. That is what makes a long cut legible rather than
 * mysterious.
 *
 * ## The two event vocabularies
 *
 * `trimmer-media` speaks `Progress` about one ffmpeg process; `trimmer-app` speaks `QueueEvent` about a
 * batch. Both arrive on the same event under the same name, discriminated on `kind`, because that is
 * what the shell emits. Handling both in one place is what lets a single cut and a batch of nine look
 * the same here.
 */

import { useEffect, useRef, useState } from "react";

/** One line of the log. */
export interface LogLine {
  readonly at: number;
  readonly text: string;
  readonly tone: "plain" | "step" | "command" | "error";
}

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
  readonly kind: "step" | "command" | "elapsed" | "finished" | "message";
  readonly label?: string;
  readonly text?: string;
  readonly seconds?: number;
  readonly ok?: boolean;
}

/** A `trimmer-app` batch message. */
interface QueueProgress {
  readonly kind: "started" | "state" | "jobProgress" | "finished" | "completed";
  readonly total?: number;
  readonly name?: string;
  readonly state?: string;
  readonly succeeded?: number;
  readonly failed?: number;
  readonly skipped?: number;
  readonly unverified?: number;
  readonly progress?: MediaProgress;
}

/** How many lines to keep. A log that grows without bound is a memory leak with a scrollbar. */
const MAX_LINES = 400;

export function useCutLog(listening: boolean): {
  readonly lines: readonly LogLine[];
  readonly step: string | null;
  readonly clear: () => void;
} {
  const [lines, setLines] = useState<readonly LogLine[]>([]);
  const [step, setStep] = useState<string | null>(null);
  const counter = useRef(0);

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

    const push = (text: string, tone: LogLine["tone"]): void => {
      counter.current += 1;
      const line = { at: counter.current, text, tone };
      setLines((current) => {
        const next = [...current, line];
        return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
      });
    };

    const onMedia = (progress: MediaProgress): void => {
      switch (progress.kind) {
        case "step":
          if (progress.label !== undefined) {
            setStep(progress.label);
            push(progress.label, "step");
          }
          break;
        case "command":
          // The exact command line, which is the one thing that makes a slow step explicable.
          if (progress.text !== undefined) {
            push(progress.text, "command");
          }
          break;
        case "finished":
          if (progress.label !== undefined) {
            push(
              `${progress.label} — ${progress.ok === true ? "ok" : "FAILED"} (${(progress.seconds ?? 0).toFixed(1)}s)`,
              progress.ok === true ? "plain" : "error",
            );
          }
          break;
        case "message":
          if (progress.text !== undefined) {
            push(progress.text, "error");
          }
          break;
        case "elapsed":
          break;
      }
    };

    const onQueue = (event: QueueProgress): void => {
      switch (event.kind) {
        case "started":
          push(`batch of ${event.total ?? 0}`, "step");
          break;
        case "state":
          push(`${event.name ?? "segment"}: ${event.state ?? ""}`, "step");
          break;
        case "jobProgress":
          if (event.progress !== undefined) {
            onMedia(event.progress);
          }
          break;
        case "finished":
          push(`${event.name ?? "segment"} finished`, "plain");
          break;
        case "completed":
          push(
            `done: ${event.succeeded ?? 0} written, ${event.unverified ?? 0} uncertified, ` +
              `${event.failed ?? 0} failed, ${event.skipped ?? 0} skipped`,
            (event.failed ?? 0) > 0 ? "error" : "plain",
          );
          setStep(null);
          break;
      }
    };

    void bridge.event
      .listen<MediaProgress | QueueProgress>("cut-progress", (event) => {
        const payload = event.payload;
        if (payload.kind === "started" || payload.kind === "completed" || payload.kind === "jobProgress") {
          onQueue(payload as QueueProgress);
        } else if (payload.kind === "state" || payload.kind === "finished") {
          onQueue(payload as QueueProgress);
        } else {
          onMedia(payload as MediaProgress);
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

  return { lines, step, clear: () => setLines([]) };
}
