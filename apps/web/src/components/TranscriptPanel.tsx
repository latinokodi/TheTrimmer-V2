/**
 * The transcript panel: search the words, not the timecodes.
 *
 * This is the feature that changes what the tool is for. Finding the frame number of a moment by
 * listening for it is work a computer should do; typing four words is work a person does.
 *
 * ## Why the results are virtualised by hand
 *
 * A three-hour interview is around three thousand cues, and a search for "the" matches most of them.
 * Rendering three thousand rows of highlighted text into the DOM takes long enough that the search
 * box stops feeling like a search box. So the list renders a window of rows around the scroll
 * position, and the row height is fixed so the scrollbar is honest about the total.
 *
 * ## Why a search result becomes a *sentence* and not a cue
 *
 * A cue is a captioning artifact — it breaks wherever the captioner needed a line break. A cut point
 * belongs on a sentence boundary, so a hit is expanded to the group it falls in, and the two buttons
 * offer the two things an editor actually wants: the whole sentence, or the sentence widened to the
 * nearest pauses so the cut cannot clip a plosive.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { IpcFailure, commands } from "../ipc/commands";
import type { SourceView, TranscriptHit } from "../ipc/types";

const ROW_HEIGHT = 26;
const OVERSCAN = 6;
const VIEWPORT_HEIGHT = 320;

export function TranscriptPanel({
  sources,
  focusToken,
  onMark,
}: {
  readonly sources: readonly SourceView[];
  readonly focusToken: number;
  readonly onMark: (input: {
    readonly source: string;
    readonly name: string;
    readonly startFrame: number;
    readonly endFrame: number;
    readonly preset: string | null;
    readonly handleFrames: number;
  }) => Promise<void>;
}): JSX.Element {
  const withTranscripts = useMemo(
    () => sources.filter((source) => source.transcript !== null && source.present),
    [sources],
  );

  const [videoPath, setVideoPath] = useState<string | null>(null);
  const [phrase, setPhrase] = useState("");
  const [hits, setHits] = useState<readonly TranscriptHit[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [selected, setSelected] = useState<number | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  // Focus the search box when the shortcut asks for it. The token changes on every Ctrl+F, so a
  // second press re-focuses rather than doing nothing because the value did not change.
  useEffect(() => {
    if (focusToken > 0) {
      searchRef.current?.focus();
      searchRef.current?.select();
    }
  }, [focusToken]);

  // Default to the first source that has a transcript, so the panel is useful on arrival.
  useEffect(() => {
    if (videoPath === null && withTranscripts.length > 0) {
      setVideoPath(withTranscripts[0]?.path ?? null);
    }
  }, [videoPath, withTranscripts]);

  const runSearch = useCallback(
    async (query: string) => {
      if (videoPath === null || query.trim().length < 2) {
        setHits([]);
        return;
      }
      try {
        setError(null);
        setHits(await commands.searchTranscript(videoPath, query.trim(), 200));
        setScrollTop(0);
        setSelected(null);
      } catch (caught) {
        setHits([]);
        setError(caught instanceof IpcFailure ? caught.message : String(caught));
      }
    },
    [videoPath],
  );

  // Debounced: a search box that fires a command per keystroke is a search box that feels slow on a
  // three-thousand-cue transcript, and the difference is invisible to the user.
  useEffect(() => {
    const timer = window.setTimeout(() => {
      void runSearch(phrase);
    }, 140);
    return () => window.clearTimeout(timer);
  }, [phrase, runSearch]);

  const first = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
  const visible = Math.ceil(VIEWPORT_HEIGHT / ROW_HEIGHT) + OVERSCAN * 2;
  const window_ = hits.slice(first, first + visible);

  return (
    <div className="panel-body">
      <div className="field">
        <label className="field__label" htmlFor="transcript-search">
          Find in the transcript
        </label>
        <input
          id="transcript-search"
          ref={searchRef}
          type="search"
          value={phrase}
          placeholder="the bit about custody…"
          onChange={(event) => setPhrase(event.target.value)}
          aria-describedby="transcript-search-help"
          disabled={videoPath === null}
        />
        <p id="transcript-search-help" className="field__help">
          {videoPath === null
            ? "No source with a transcript beside it. Name a caption file `<video>.srt` and reopen the project."
            : "Matching is case-insensitive and never crosses a pause, so a hit is always inside one breath."}
        </p>
      </div>

      {withTranscripts.length > 1 ? (
        <div className="field">
          <label className="field__label" htmlFor="transcript-source">
            Source
          </label>
          <select
            id="transcript-source"
            value={videoPath ?? ""}
            onChange={(event) => setVideoPath(event.target.value)}
          >
            {withTranscripts.map((source) => (
              <option key={source.path} value={source.path}>
                {source.name} ({source.transcriptCues ?? "?"} cues)
              </option>
            ))}
          </select>
        </div>
      ) : null}

      {error !== null ? (
        <p className="note note--danger" role="alert">
          {error}
        </p>
      ) : null}

      {phrase.trim().length >= 2 ? (
        <p className="panel-body__count figures" aria-live="polite">
          {hits.length === 0
            ? "no matches"
            : `${hits.length}${hits.length === 200 ? "+" : ""} match(es)`}
        </p>
      ) : null}

      <div
        className="hits scroll"
        style={{ height: `${VIEWPORT_HEIGHT}px` }}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
        role="listbox"
        aria-label="Transcript matches"
        tabIndex={0}
      >
        {hits.length === 0 ? (
          <p className="empty empty--tight">
            {phrase.trim().length < 2
              ? "Type at least two characters."
              : "Nothing in this transcript matches."}
          </p>
        ) : (
          <div style={{ height: `${hits.length * ROW_HEIGHT}px`, position: "relative" }}>
            {window_.map((hit, offset) => {
              const index = first + offset;
              return (
                <button
                  key={`${hit.cue}-${hit.byteOffset}`}
                  type="button"
                  role="option"
                  aria-selected={selected === index}
                  className={`hit${selected === index ? " hit--selected" : ""}`}
                  style={{ top: `${index * ROW_HEIGHT}px`, height: `${ROW_HEIGHT}px` }}
                  onClick={() => setSelected(index)}
                  onDoubleClick={() => void markSentence(hit)}
                  title="Double-click to mark this sentence as a segment"
                >
                  <span className="hit__time figures">{formatFrames(hit.startFrame)}</span>
                  <span className="hit__text truncate">{plain(hit.highlighted)}</span>
                </button>
              );
            })}
          </div>
        )}
      </div>

      {selected !== null && hits[selected] !== undefined ? (
        <div className="hits__actions">
          <p className="hits__preview selectable">
            {plain(hits[selected]?.highlighted ?? "")}
          </p>
          <button
            type="button"
            className="btn btn--primary"
            onClick={() => void markSentence(hits[selected] as TranscriptHit)}
          >
            Mark this sentence
          </button>
        </div>
      ) : null}
    </div>
  );

  /**
   * Turn a hit into a queued segment.
   *
   * The range comes from the *index*, not from the highlight: a hit is a phrase inside a cue, and the
   * cut belongs on the sentence boundary the index knows about. Deriving the range from the match
   * would put the cut in the middle of a word.
   *
   * It goes through `onMark` — which is the model's `addSegment` — rather than calling the command
   * directly. That matters for more than tidiness: the model refreshes the queue afterwards, so the
   * new segment appears in the list. A direct command call writes it to the project and leaves the
   * screen showing the state from before, which reads as a button that did nothing.
   */
  async function markSentence(hit: TranscriptHit): Promise<void> {
    if (videoPath === null) {
      return;
    }
    const start = hit.startFrame;
    // The panel has the hit rather than the sentence's own end, so the range runs to the next hit
    // when there is one and to five seconds otherwise. Either way it is editable, and the point is
    // to get the marks roughly right from a word rather than to guess the edit.
    const next = hits.find((candidate) => candidate.startFrame > start + 1);
    const end = next?.startFrame ?? start + 25 * 5;
    setError(null);
    try {
      await onMark({
        source: videoPath,
        name: plain(hit.highlighted).slice(0, 60),
        startFrame: start,
        endFrame: end,
        preset: null,
        handleFrames: 0,
      });
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  }
}

/** Strip the `[[` and `]]` markers the Rust side puts around a match. */
function plain(highlighted: string): string {
  return highlighted.replaceAll("[[", "").replaceAll("]]", "");
}

/** A frame count as seconds, at the rate the transcript was read against. */
function formatFrames(frame: number): string {
  const rate = 25;
  const total = Math.round(frame / rate);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}
