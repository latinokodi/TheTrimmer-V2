/**
 * The segment being marked.
 *
 * ## Why this is a hook and not a dialog
 *
 * The window's whole job is: choose a video, type an in point and an out point, trim. In the first
 * version of this interface the marks lived in a modal behind a button, so the two most important
 * fields in the product were invisible until you found the right control — and a person who had just
 * opened the application saw a table of nothing and a status bar.
 *
 * Here the draft is application state, not dialog state. It exists from the moment a master is
 * chosen, the fields are on screen beside the video they refer to, and the frame numbers under them
 * update as they are typed. Marking a *second* segment resets the draft rather than opening a form.
 *
 * ## Why the parsing goes through Rust
 *
 * `parse_timecode` is the domain's own arithmetic — including drop-frame, which is where a second
 * implementation would silently disagree by a frame. So the hook calls the command and holds the
 * result, rather than reimplementing `HH:MM:SS:FF` in TypeScript. That is also why each parse is
 * cancelled on the next keystroke: a stale reply must not overwrite a newer one.
 */

import { useCallback, useEffect, useMemo, useState } from "react";

import { IpcFailure, commands } from "../ipc/commands";
import type { SourceView } from "../ipc/types";

/** What the marks currently say, in the two forms the interface needs. */
export interface SegmentDraft {
  readonly source: string;
  readonly name: string;
  readonly inText: string;
  readonly outText: string;
  readonly preset: string;
  readonly handles: number;
  /** The first frame kept, from the in point. `null` while the field does not parse. */
  readonly inFrame: number | null;
  /** The last frame kept, from the out point. `null` while the field does not parse. */
  readonly outFrame: number | null;
  /** One past `outFrame` — what the domain stores. */
  readonly endFrame: number | null;
  /** How many frames the range holds, when both marks parse. `null` until then. */
  readonly frames: number | null;
  readonly error: string | null;
  /** True when the marks are a range that could be trimmed. */
  readonly ready: boolean;
  readonly setSource: (path: string) => void;
  readonly setName: (value: string) => void;
  readonly setInText: (value: string) => void;
  readonly setOutText: (value: string) => void;
  readonly setPreset: (value: string) => void;
  readonly setHandles: (value: number) => void;
  /** The payload `add_segment` takes, or `null` when the marks do not parse. */
  readonly request: () => {
    readonly source: string;
    readonly name: string;
    readonly startFrame: number;
    readonly endFrame: number;
    readonly preset: string | null;
    readonly handleFrames: number;
  } | null;
  /** Clear the marks, keeping the master. Used after a segment is queued. */
  readonly reset: () => void;
}

/** Parse one timecode against a source, cancelling the previous reply. */
function useParsedFrame(
  text: string,
  source: string,
  onError: (message: string) => void,
): number | null {
  const [frame, setFrame] = useState<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    if (source === "" || text.trim() === "") {
      setFrame(null);
      return;
    }
    void commands
      .parseTimecode(text, source)
      .then((parsed) => {
        if (!cancelled) {
          setFrame(parsed.frame);
        }
      })
      .catch((caught: unknown) => {
        if (!cancelled) {
          setFrame(null);
          onError(caught instanceof IpcFailure ? caught.message : String(caught));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [text, source, onError]);

  return frame;
}

export function useSegmentDraft(sources: readonly SourceView[]): SegmentDraft {
  const usable = useMemo(() => sources.filter((source) => source.present), [sources]);
  const [source, setSource] = useState("");
  const [name, setName] = useState("");
  const [inText, setInText] = useState("");
  const [outText, setOutText] = useState("");
  const [preset, setPreset] = useState("");
  const [handles, setHandles] = useState(0);
  const [error, setError] = useState<string | null>(null);

  // A stable callback, because the parse effect depends on it: an inline arrow would re-run the
  // effect on every render and turn one keystroke into a request per repaint.
  const onError = useCallback((message: string) => setError(message), []);

  // Follow the sources: pick the first present one, and drop a selection whose file has gone.
  useEffect(() => {
    if (usable.length === 0) {
      if (source !== "") {
        setSource("");
      }
      return;
    }
    if (!usable.some((item) => item.path === source)) {
      setSource(usable[0]?.path ?? "");
    }
  }, [usable, source]);

  const inFrame = useParsedFrame(inText, source, onError);
  const parsedOut = useParsedFrame(outText, source, onError);

  // The out field is the **last frame kept**, the way Premiere's Out point works, and the domain
  // stores one past it. The conversion happens here, once; the command stores what it is given.
  const endFrame = parsedOut === null ? null : parsedOut + 1;
  const outFrame = parsedOut;

  const frames = inFrame !== null && endFrame !== null ? Math.max(0, endFrame - inFrame) : null;
  const inRange =
    inFrame !== null &&
    outFrame !== null &&
    source !== "" &&
    (frames ?? 0) > 0 &&
    usable.some((item) => item.path === source);

  // Clear a stale parse error as soon as the marks become valid again, so a corrected field does not
  // leave a red sentence underneath it.
  useEffect(() => {
    if (inRange) {
      setError(null);
    }
  }, [inRange]);

  const request = useCallback(() => {
    if (inFrame === null || endFrame === null || !inRange) {
      return null;
    }
    return {
      source,
      name: name.trim() === "" ? `Segment at ${inText}` : name.trim(),
      startFrame: inFrame,
      endFrame,
      preset: preset === "" ? null : preset,
      handleFrames: handles,
    };
  }, [endFrame, handles, inFrame, inRange, inText, name, preset, source]);

  const reset = useCallback(() => {
    setInText("");
    setOutText("");
    setName("");
    setHandles(0);
  }, []);

  return {
    source,
    name,
    inText,
    outText,
    preset,
    handles,
    inFrame,
    outFrame,
    endFrame,
    frames,
    error,
    ready: inRange,
    setSource: setSource,
    setName,
    setInText,
    setOutText,
    setPreset,
    setHandles,
    request,
    reset,
  };
}
