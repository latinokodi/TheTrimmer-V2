/**
 * The application: a fixed instrument panel.
 *
 * ## What this is
 *
 * A grid of labelled zones that fills the window. It opens fullscreen, it has a minimum size, and
 * **nothing in it scrolls except the three zones that can hold an unbounded number of rows** — the
 * queue, the transcript hits, and the log. The frame itself never moves.
 *
 * ## The frame
 *
 * ```
 * ┌ TheTrimmer ─ frame-exact, lossless ───────────────────────────────────────────────────┐
 * ├──────────────────────────────┬────────────────────────────────────────────────────────┤
 * │ VIDEO                        │ QUEUE                                                  │
 * │ TRANSCRIPT                   │  [x] # Name          In      Out     Frames Plan    ×  │
 * │ RANGE · HH:MM:SS:FF          │                                                        │
 * │ OPTIONS                      │ ── WHAT CAME OUT ──                                    │
 * │ ── [Queue] [Export] [Trim] ──│                                                        │
 * ├──────────────────────────────┴────────────────────────────────────────────────────────┤
 * │ TRANSCRIPT · find + hits                                                              │
 * ├───────────────────────────────────────────────────────────────────────────────────────┤
 * │ PROGRESS · bar · status · the log                                                     │
 * ├───────────────────────────────────────────────────────────────────────────────────────┤
 * │ ● ffmpeg ready · 2/2 runnable · notice                                      dark      │
 * └───────────────────────────────────────────────────────────────────────────────────────┘
 * ```
 *
 * ## What it is, in design terms
 *
 * *Tactical telemetry*: a rack panel. Zones are separated by solid hairlines and a change of substrate
 * rather than by cards on a background, every corner is 90 degrees, the data face is monospace, and
 * there is **one** accent — hazard red — used for the focus ring, the selected row's edge and a
 * failure. One signal colour, terminal green, appears on exactly one element: the ffmpeg lamp in the
 * footer. A single instrument that reads as live; used anywhere else it would stop meaning anything.
 *
 * Three earlier versions of this shell were a three-column dashboard, a two-column workbench and a
 * column of cards. Each tried to arrange a form that did not need arranging, and each made a simple
 * tool look like a complicated one. This is the form, with the compartments a broadcast panel would
 * have, and the new features — the queue, the transcript search, the log — as compartments of their own.
 *
 * ## Density
 *
 * A 26 px row. Small on purpose: the window sits beside a video monitor and the operator is comparing
 * two timecodes, three column values and a plan badge at once.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { CutTable } from "./components/CutTable";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { ExportDialog } from "./components/ExportDialog";
import { ProgressLog } from "./components/ProgressLog";
import { ProofPanel } from "./components/ProofPanel";
import { SourceDialog } from "./components/SourceDialog";
import { TranscriptPanel } from "./components/TranscriptPanel";
import { useAppModel } from "./state/useAppModel";
import { useCutLog } from "./state/useCutLog";
import { useSegmentDraft } from "./state/useSegmentDraft";
import { useTheme } from "./state/useTheme";

export function App(): JSX.Element {
  const model = useAppModel();
  const { theme, toggle: toggleTheme } = useTheme();
  const draft = useSegmentDraft(model.sources);
  const running = model.busy !== null;
  const log = useCutLog(running);

  const [sourceOpen, setSourceOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [searchFocusToken, setSearchFocusToken] = useState(0);

  const queued = model.segments.length;
  const runnable = model.summary?.runnable ?? 0;
  const chosen = model.sources.find((source) => source.path === draft.source) ?? model.sources[0];
  const hasTranscript = chosen?.transcript != null;

  /** What the plan says about the range currently marked, matched on the frame numbers. */
  const markedPlan = useMemo(() => {
    if (draft.inFrame === null || draft.endFrame === null) {
      return null;
    }
    return (
      model.previews.find(
        (preview) =>
          preview.plan?.startFrame === draft.inFrame && preview.plan?.endFrame === draft.endFrame,
      ) ?? null
    );
  }, [draft.endFrame, draft.inFrame, model.previews]);

  /**
   * Plan as the marks are typed, so the plan line says what will happen before the decision is made
   * rather than after. The function is held in a ref because the model object is rebuilt on every
   * render; naming it in the dependency list would run this on every render and spin.
   */
  const planRef = useRef(model.planQuietly);
  planRef.current = model.planQuietly;

  useEffect(() => {
    if (!draft.ready || draft.inFrame === null || draft.endFrame === null) {
      return;
    }
    const marked = {
      startFrame: draft.inFrame,
      endFrame: draft.endFrame,
      preset: draft.preset === "" ? null : draft.preset,
      handleFrames: draft.handles,
    };
    const timer = setTimeout(() => {
      void planRef.current(marked);
    }, 300);
    return () => clearTimeout(timer);
  }, [draft.endFrame, draft.handles, draft.inFrame, draft.preset, draft.ready]);

  const queue = useCallback(async () => {
    const request = draft.request();
    if (request === null) {
      return;
    }
    await model.addSegment(request);
    draft.reset();
  }, [draft, model]);

  const trimNow = useCallback(async () => {
    // A marked range that is not queued is what the plain "Trim" means. With something already queued
    // the button says `Trim n` and the marks are left alone — they are for the next range, and a person
    // who has typed one and not queued it should not have it swept into the run by accident.
    const request = queued === 0 ? draft.request() : null;
    if (request !== null) {
      await model.addSegment(request);
      draft.reset();
    }
    await model.runBatch({ stopOnError: false, skipVerification: false, label: "trim" });
  }, [draft, model, queued]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!event.ctrlKey && !event.metaKey) {
        return;
      }
      switch (event.key.toLowerCase()) {
        case "enter":
          event.preventDefault();
          void trimNow();
          break;
        case "q":
          event.preventDefault();
          void queue();
          break;
        case "o":
        case "m":
          event.preventDefault();
          setSourceOpen(true);
          break;
        case "e":
          event.preventDefault();
          setExportOpen(true);
          break;
        case "f":
          event.preventDefault();
          setSearchFocusToken((token) => token + 1);
          document.getElementById("transcript-search")?.focus();
          break;
        default:
          break;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [queue, trimNow]);

  return (
    <ErrorBoundary>
      <div className="app">
        <a className="skip-link" href="#trim-in">
          Skip to the In point
        </a>

        {/* ---- title bar ------------------------------------------------------------------ */}
        <header className="titlebar">
          <span className="titlebar__mark" aria-hidden="true">
            T
          </span>
          <h1 className="titlebar__product">TheTrimmer</h1>
          <span className="titlebar__rule" aria-hidden="true" />
          <p className="titlebar__tagline">
            Frame-exact, lossless segment cutting &nbsp;·&nbsp; only the keyframe head is re-encoded
          </p>
          <span className="spacer" />
          <button
            type="button"
            className="btn btn--ghost btn--small"
            onClick={toggleTheme}
            disabled={running}
            aria-label={`Switch to the ${theme === "dark" ? "light" : "dark"} theme`}
            title={`Switch to the ${theme === "dark" ? "light" : "dark"} theme`}
          >
            {theme === "dark" ? "Light" : "Dark"}
          </button>
        </header>

        {/* ---- the two columns ------------------------------------------------------------ */}
        <div className="app__body">
          <div className="app__col app__col--form">
            {/* ---- VIDEO ---------------------------------------------------------------- */}
            <section className="zone">
              <header className="zone__head">
                <h2 className="zone__title">Video</h2>
                <span className="spacer" />
                <span className="zone__note">
                  {chosen === undefined
                    ? "no file"
                    : chosen.media === null
                      ? "not on disk"
                      : `${chosen.media.width}×${chosen.media.height} · ${chosen.media.codec} · ${chosen.media.rate.num / chosen.media.rate.den} fps`}
                </span>
              </header>
              <div className="zone__body zone__body--tight">
                <div className="row">
                  <input
                    id="video-path"
                    key={chosen?.path ?? "none"}
                    className="path-field"
                    type="text"
                    defaultValue={chosen?.path ?? ""}
                    placeholder="path to the video to trim"
                    spellCheck={false}
                    aria-label="Video file"
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        void model.addSource(event.currentTarget.value.trim());
                      }
                    }}
                  />
                  <button
                    type="button"
                    className="btn"
                    onClick={() => setSourceOpen(true)}
                    disabled={running}
                  >
                    Browse
                  </button>
                </div>
                <p className="zone__note">
                  {chosen === undefined
                    ? "Pick a video to see its frame rate and length."
                    : chosen.media === null
                      ? chosen.summary
                      : `${chosen.media.frameCount.toLocaleString()} frames · ${chosen.media.pixFmt} · ${chosen.media.audio === null ? "no audio" : `${chosen.media.audio.codec} ${chosen.media.audio.sampleRate} Hz ${chosen.media.audio.channels}ch`}`}
                </p>
              </div>
            </section>

            {/* ---- TRANSCRIPT: the file, not the search --------------------------------- */}
            <section className="zone">
              <header className="zone__head">
                <h2 className="zone__title">Caption file</h2>
                <span className="spacer" />
                <span className="zone__note">
                  {hasTranscript
                    ? `${chosen?.transcriptCues ?? "?"} cues, retimed with every segment`
                    : "none found beside the video"}
                </span>
              </header>
              <div className="zone__body zone__body--tight">
                <div className="row">
                  <input
                    id="subtitle-path"
                    className="path-field"
                    type="text"
                    value={chosen?.transcript ?? ""}
                    placeholder="no caption file beside this video"
                    spellCheck={false}
                    aria-label="Transcript file"
                    readOnly
                  />
                  <button
                    type="button"
                    className="btn"
                    onClick={() => {
                      setSearchFocusToken((token) => token + 1);
                      document.getElementById("transcript-search")?.focus();
                    }}
                    disabled={running || !hasTranscript}
                    title={
                      hasTranscript
                        ? "Search the transcript and mark a range from a sentence"
                        : "No caption file was found beside this video"
                    }
                  >
                    Find
                  </button>
                </div>
              </div>
            </section>

            {/* ---- RANGE ---------------------------------------------------------------- */}
            <section className="zone zone--grow">
              <header className="zone__head">
                <h2 className="zone__title">Range</h2>
                <span className="spacer" />
                <span className="zone__note">Premiere timecode HH:MM:SS:FF</span>
              </header>
              <div className="zone__body">
                <div className="range">
                  <div className="range__row">
                    <label className="field-row__label" htmlFor="trim-in">
                      In point
                    </label>
                    <input
                      id="trim-in"
                      className="timecode-field"
                      type="text"
                      inputMode="numeric"
                      autoComplete="off"
                      spellCheck={false}
                      value={draft.inText}
                      placeholder="00:00:00:00"
                      aria-describedby="trim-in-help"
                      aria-invalid={draft.inText !== "" && draft.inFrame === null}
                      onChange={(event) => draft.setInText(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          document.getElementById("trim-out")?.focus();
                        }
                      }}
                    />
                    <span className="field-row__note" id="trim-in-help">
                      {draft.inFrame === null
                        ? "first frame kept"
                        : `frame ${draft.inFrame.toLocaleString()}`}
                    </span>
                  </div>

                  <div className="range__row">
                    <label className="field-row__label" htmlFor="trim-out">
                      Out point
                    </label>
                    <input
                      id="trim-out"
                      className="timecode-field"
                      type="text"
                      inputMode="numeric"
                      autoComplete="off"
                      spellCheck={false}
                      value={draft.outText}
                      placeholder="00:00:10:00"
                      aria-describedby="trim-out-help"
                      aria-invalid={draft.outText !== "" && draft.outFrame === null}
                      onChange={(event) => draft.setOutText(event.target.value)}
                    />
                    <span className="field-row__note" id="trim-out-help">
                      {draft.outFrame === null
                        ? "last frame kept, as Premiere's Out point works"
                        : `frame ${draft.outFrame.toLocaleString()}, the last one kept`}
                    </span>
                  </div>
                </div>

                <p className="range__plan" aria-live="polite">
                  {draft.error !== null ? (
                    <span className="danger">{draft.error}</span>
                  ) : draft.frames === null ? (
                    "Type both timecodes; the frame numbers appear here as you type."
                  ) : (
                    <>
                      <span>
                        frames {draft.inFrame} … {draft.outFrame}
                      </span>
                      <span className="range__sep" aria-hidden="true" />
                      <span>{draft.frames.toLocaleString()} frames</span>
                      {markedPlan?.plan != null ? (
                        <>
                          <span className="range__sep" aria-hidden="true" />
                          <span className={markedPlan.plan.mode === "copy" ? "ok" : "warn"}>
                            {markedPlan.plan.mode === "copy"
                              ? "lossless copy — every frame is the original"
                              : markedPlan.plan.mode === "headPatch"
                                ? `head patch — ${markedPlan.plan.headFrames} re-encoded, ${markedPlan.plan.bodyFrames} copied`
                                : "full re-encode — no keyframe inside this range"}
                          </span>
                        </>
                      ) : null}
                    </>
                  )}
                </p>
              </div>
            </section>

            {/* ---- OPTIONS -------------------------------------------------------------- */}
            <section className="zone">
              <header className="zone__head">
                <h2 className="zone__title">Options</h2>
              </header>
              <div className="zone__body zone__body--tight">
                <div className="field-row">
                  <label className="field-row__label" htmlFor="trim-preset">
                    Delivery
                  </label>
                  <div className="field-row__value">
                    <select
                      id="trim-preset"
                      value={draft.preset}
                      onChange={(event) => draft.setPreset(event.target.value)}
                    >
                      <option value="">the project default — lossless, original packets</option>
                      {model.presets.map((item) => (
                        <option key={item.name} value={item.name}>
                          {item.name} — {item.description}
                        </option>
                      ))}
                    </select>
                  </div>
                </div>

                <div className="field-row">
                  <label className="field-row__label" htmlFor="trim-handles">
                    Handles
                  </label>
                  <div className="field-row__value">
                    <input
                      id="trim-handles"
                      type="number"
                      min={0}
                      step={1}
                      value={draft.handles}
                      style={{ width: "72px" }}
                      onChange={(event) =>
                        draft.setHandles(Math.max(0, Number(event.target.value) || 0))
                      }
                    />
                    <span className="field-row__note">
                      extra frames kept either side, for a crossfade
                    </span>
                  </div>
                </div>

                <div className="field-row">
                  <label className="field-row__label" htmlFor="trim-policy">
                    Verify
                  </label>
                  <div className="field-row__value">
                    <select
                      id="trim-policy"
                      value={model.verifyPolicy}
                      onChange={(event) => void model.setVerifyPolicy(event.target.value)}
                    >
                      <option value="off">Off — do not measure the result</option>
                      <option value="standard">
                        Standard — frame count, duration, audio alignment
                      </option>
                      <option value="strict">Strict — also hash decoded frames at sample points</option>
                      <option value="forensic">
                        Forensic — also compare the head pixel-wise, and every caption cue
                      </option>
                    </select>
                  </div>
                </div>
              </div>
            </section>

            {/* ---- actions -------------------------------------------------------------- */}
            <div className="actions">
              <button
                type="button"
                className="btn"
                onClick={() => void queue()}
                disabled={!draft.ready || running}
                title="Add this range to the queue without trimming it yet (Ctrl+Q)"
              >
                Queue
              </button>
              <button
                type="button"
                className="btn btn--ghost"
                onClick={() => setExportOpen(true)}
                disabled={running || queued === 0}
                title={
                  queued === 0
                    ? "Mark a segment first: there is no timeline to export"
                    : "Write the timeline as a Premiere XML, FCPXML, EDL or CSV (Ctrl+E)"
                }
              >
                Export
              </button>
              <button
                type="button"
                className="btn btn--ghost"
                onClick={() => void model.cancelBatch()}
                disabled={!running}
                title="Stop after the segment in flight"
              >
                Cancel
              </button>
              <span className="spacer" />
              <button
                type="button"
                className="btn btn--primary btn--big"
                onClick={() => void trimNow()}
                disabled={running || (queued === 0 && !draft.ready)}
                title={
                  queued > 0
                    ? `Cut the ${queued} queued segment(s), then check every finished file (Ctrl+Enter)`
                    : "Cut the marked range, then check the finished file (Ctrl+Enter)"
                }
              >
                {running ? "Working" : queued > 0 ? `Trim ${queued}` : "Trim"}
              </button>
            </div>
          </div>

          {/* ---- the queue column ------------------------------------------------------- */}
          <div className="app__col app__col--queue">
            <section className="zone zone--grow">
              <header className="zone__head">
                <h2 className="zone__title">Queue</h2>
                {queued > 0 ? <span className="zone__count figures">{queued}</span> : null}
                <span className="spacer" />
                <span className="zone__note">
                  {queued === 0 ? "nothing marked yet" : `${runnable} of ${queued} ready`}
                </span>
              </header>
              <div className="zone__body zone__body--flush">
                <CutTable
                  segments={model.segments}
                  selected={model.selectedSegment}
                  onSelect={model.selectSegment}
                  onToggleEnabled={(id, enabled) => void model.updateSegment({ id, enabled })}
                  onRemove={(id) => void model.removeSegment(id)}
                  previews={model.previews}
                  busy={running}
                />
              </div>
            </section>

            {model.lastOutcome !== null ? (
              <section className="zone zone--grow">
                <header className="zone__head">
                  <h2 className="zone__title">What came out</h2>
                  <span className="spacer" />
                  <span className="zone__note">{model.lastOutcome.jobs.length} job(s)</span>
                </header>
                <div className="zone__body zone__body--flush">
                  <ProofPanel outcome={model.lastOutcome} model={model} />
                </div>
              </section>
            ) : null}
          </div>
        </div>

        {/* ---- the transcript search ------------------------------------------------------ */}
        <section className="zone">
          <header className="zone__head">
            <h2 className="zone__title">Find in the transcript</h2>
            <span className="spacer" />
            <span className="zone__note">search the words, then mark the sentence (Ctrl+F)</span>
          </header>
          <div className="zone__body zone__body--tight">
            <TranscriptPanel
              sources={model.sources}
              focusToken={searchFocusToken}
              onMark={async (input) => {
                await model.addSegment(input);
              }}
            />
          </div>
        </section>

        {/* ---- progress ------------------------------------------------------------------ */}
        <section className="zone">
          <header className="zone__head">
            <h2 className="zone__title">Progress</h2>
            <span className="spacer" />
            {log.lines.length > 0 ? (
              <button type="button" className="btn btn--ghost btn--small" onClick={log.clear}>
                Clear
              </button>
            ) : null}
          </header>
          <ProgressLog lines={log.lines} step={log.step} running={running} />
        </section>

        {/* ---- footer -------------------------------------------------------------------- */}
        <footer className="footerline">
          {/*
            The one signal-green element in the application: a live instrument. `doctor` runs once at
            startup, so a green lamp here means ffmpeg resolved and the build has libx264 — the two
            facts that decide whether anything else in this window can work.
          */}
          <span className="footerline__item">
            <span
              className={`status ${
                model.doctor === null
                  ? "status--idle"
                  : model.doctor.libx264
                    ? "status--ok"
                    : "status--danger"
              }`}
              title={model.doctor?.ffmpeg ?? "checking ffmpeg"}
            >
              {model.doctor === null
                ? "ffmpeg"
                : model.doctor.libx264
                  ? "ffmpeg ready"
                  : "no H.264"}
            </span>
            {model.doctor !== null ? (
              <span className="footerline__detail">{model.doctor.ffmpeg}</span>
            ) : null}
          </span>

          <span className="footerline__sep" aria-hidden="true" />

          <span className="footerline__item figures">
            {model.summary === null
              ? "no project"
              : `${model.summary.runnable}/${model.summary.segments} runnable`}
            {(model.summary?.missingSources ?? 0) > 0
              ? ` · ${model.summary?.missingSources} source(s) missing`
              : ""}
          </span>

          <span className="spacer" />

          {model.notice !== null ? (
            <span className="footerline__notice" role="status">
              {model.notice}
            </span>
          ) : null}

          {model.error !== null ? (
            <button
              type="button"
              className="btn btn--danger btn--small"
              onClick={model.clearMessages}
              title={model.error.message}
            >
              {model.error.message.split("\n")[0]?.slice(0, 70) ?? "error"}
            </button>
          ) : null}

          <span className="footerline__sep" aria-hidden="true" />
          <span className="footerline__item">{theme}</span>
        </footer>

        {sourceOpen ? (
          <SourceDialog
            sources={model.sources}
            onClose={() => setSourceOpen(false)}
            onAdd={(path) => model.addSource(path)}
          />
        ) : null}
        {exportOpen ? (
          <ExportDialog
            projectName={model.sessionName}
            onClose={() => setExportOpen(false)}
            onExport={(input) => model.exportTimeline(input)}
          />
        ) : null}
      </div>
    </ErrorBoundary>
  );
}
