/**
 * The application: a column of cards, in the order the work happens.
 *
 * ## Why this shape, and what it is copying
 *
 * This is the original TheTrimmer window's architecture: a title, then a stack of labelled cards down
 * a single centred column — Video, Subtitles, Range, Options — then the actions, then a progress strip
 * and a log. Two versions of this shell went somewhere else, a three-column dashboard and then a
 * two-column workbench, and both were worse for the same reason: they rearranged a form that did not
 * need rearranging and made a simple tool look like a complicated one.
 *
 * The form is the product. So it is back to the original's structure, and the improvements are *added
 * to it* rather than built around it:
 *
 * * **A queue.** The original trimmed one range and stopped. A card under the actions holds every range
 *   you have marked, and `Trim` cuts them all — because the queue is the thing the engine was always
 *   able to do and the window never exposed.
 * * **A transcript you can search.** The original had a subtitle *path* box. This has the path box and a
 *   search over the same file, because "the bit where he says the thing about custody" is not a
 *   timecode anybody knows by heart.
 * * **A progress strip and a log that actually work.** The original had both. The listener for the
 *   events the shell emits was missing, so a four-minute cut looked like a hung window.
 * * **A delivery preset with a description** rather than an encoder name and a CRF, because "which
 *   encoder" is a question an editor should not have to answer.
 *
 * ## What each card is for
 *
 * ```
 * TheTrimmer                     frame-exact, lossless segment cutting
 * ┌ Video ──────────────────────────────────────────────────────────────┐
 * │ [ path                                        ] [ Browse… ]          │
 * │ 3840×2160 · prores · 25 fps · 3,101 frames · 214 caption cues        │
 * └─────────────────────────────────────────────────────────────────────┘
 * ┌ Subtitles ──────────────────────────────────────────────────────────┐
 * │ [ path to the caption file                    ] [ Browse… ]          │
 * │ [x] Cut the transcript with the segment, as a .srt named after it    │
 * └─────────────────────────────────────────────────────────────────────┘
 * ┌ Range · Premiere timecode HH:MM:SS:FF ──────────────────────────────┐
 * │ In point   [ 00:00:01:00 ]                                           │
 * │ Out point  [ 00:00:02:00 ]                                           │
 * │            frame 50, the last one kept                               │
 * │ 26 frames · 1.04 s · lossless copy — every frame is the original     │
 * └─────────────────────────────────────────────────────────────────────┘
 * ┌ Options ────────────────────────────────────────────────────────────┐
 * │ Delivery  [ master — Original packets, MP4, audio copied        ▾ ]  │
 * │ Handles   [ 0 ]   Verify  [ Strict — hash decoded frames        ▾ ]  │
 * └─────────────────────────────────────────────────────────────────────┘
 * ┌ Queue · 2 ──────────────────────────────────────────────────────────┐
 * │ [x] # Name          In           Out          Frames  Plan    ×      │
 * └─────────────────────────────────────────────────────────────────────┘
 *                     [ Queue it ]  [ Trim 2 ]  [ Export… ]
 * ┌ Progress ───────────────────────────────────────────────────────────┐
 * │ ▬▬▬▬▬▬▬▬                                                             │
 * │ head encode — ok (0.2s)                                              │
 * │ ffmpeg -ss 1.48 -i master.mp4 -t 0.52 … -c:v libx264 …               │
 * └─────────────────────────────────────────────────────────────────────┘
 * ```
 *
 * `Ctrl+Enter` trims, `Ctrl+Q` queues the marked range, `Ctrl+O` chooses a video, `Ctrl+E` exports,
 * `Ctrl+F` finds in the transcript.
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
   * Plan as the marks are typed, so the length line says what will happen before the decision is made
   * rather than after. The function is held in a ref because the model object is rebuilt on every
   * render; naming it in the dependency list would make this run on every render and spin.
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
    }, 350);
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
    // A marked range that is not queued yet is what the plain "Trim" means. When something is already
    // queued the button says `Trim n` and the marks are left alone — they are for the next range, and a
    // person who has typed one and not queued it should not have it swept into the run by accident.
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
      <div className="app" data-theme={theme}>
        <div className="app__scroll scroll">
          <div className="app__column">
            <header className="masthead">
              <div>
                <h1 className="masthead__product">TheTrimmer</h1>
                <p className="masthead__tagline">
                  Frame-exact, lossless segment trimming &nbsp;·&nbsp; only the keyframe head is
                  re-encoded
                </p>
              </div>
              <div className="spacer" />
              <button
                type="button"
                className="btn btn--ghost btn--icon"
                onClick={toggleTheme}
                disabled={running}
                aria-label={`Switch to the ${theme === "dark" ? "light" : "dark"} theme`}
                title={`Switch to the ${theme === "dark" ? "light" : "dark"} theme`}
              >
                {theme === "dark" ? "☾" : "☀"}
              </button>
            </header>

            {/* ---- Video ---------------------------------------------------------------- */}
            <section className="card">
              <header className="card__head">
                <h2 className="card__title">
                  Video
                </h2>
              </header>
              <div className="card__body">
                <div className="row">
                  <input
                    id="video-path"
                    className="path-field"
                    type="text"
                    value={chosen?.path ?? ""}
                    placeholder="path to the video to trim"
                    spellCheck={false}
                    aria-label="Video file"
                    onChange={(event) => {
                      // Typing a path does not probe on every keystroke; Enter or Browse does. A probe
                      // is a process launch.
                      void event.target.value;
                    }}
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
                    Browse…
                  </button>
                </div>
                <p className="card__facts figures">
                  {chosen === undefined
                    ? "Pick a video to see its frame rate and length."
                    : chosen.media === null
                      ? chosen.summary
                      : `${chosen.media.width}×${chosen.media.height}  ·  ${chosen.media.codec} ` +
                        `${chosen.media.pixFmt}  ·  ${chosen.media.rate.num / chosen.media.rate.den} fps  ·  ` +
                        `${chosen.media.frameCount.toLocaleString()} frames  ·  ` +
                        `${chosen.transcript === null ? "no transcript beside it" : `${chosen.transcriptCues ?? "?"} caption cues`}`}
                </p>
              </div>
            </section>

            {/* ---- Subtitles ------------------------------------------------------------ */}
            <section className="card">
              <header className="card__head">
                <h2 className="card__title">
                  Subtitles
                </h2>
              </header>
              <div className="card__body">
                <div className="row">
                  <input
                    id="subtitle-path"
                    className="path-field"
                    type="text"
                    value={chosen?.transcript ?? ""}
                    placeholder="no transcript found beside this video"
                    spellCheck={false}
                    aria-label="Transcript"
                    readOnly
                  />
                  <button
                    type="button"
                    className="btn"
                    onClick={() => {
                      setSearchFocusToken((token) => token + 1);
                      document.getElementById("card-transcript")?.scrollIntoView({ block: "start" });
                    }}
                    disabled={running || chosen?.transcript == null}
                    title={
                      chosen?.transcript == null
                        ? "No caption file was found beside this video"
                        : "Search the transcript and mark a range from a sentence"
                    }
                  >
                    Search…
                  </button>
                </div>
                <p className="card__facts">
                  {chosen?.transcript == null
                    ? "A caption file named after the video (<video>.srt) is picked up automatically."
                    : `${chosen.transcript} is retimed with every segment, as a .srt named after the output.`}
                </p>
              </div>
            </section>

            {/* ---- Range ---------------------------------------------------------------- */}
            <section className="card">
              <header className="card__head">
                <h2 className="card__title">
                  Range
                  <span className="card__note">Premiere timecode HH:MM:SS:FF</span>
                </h2>
              </header>
              <div className="card__body">
                <div className="range">
                  <div className="range__row">
                    <label className="range__label" htmlFor="trim-in">
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
                    <span className="range__help figures" id="trim-in-help">
                      {draft.inFrame === null ? "first frame kept" : `frame ${draft.inFrame.toLocaleString()}`}
                    </span>
                  </div>

                  <div className="range__row">
                    <label className="range__label" htmlFor="trim-out">
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
                    <span className="range__help figures" id="trim-out-help">
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
                      <span className="figures">
                        frames {draft.inFrame} … {draft.outFrame}
                      </span>
                      <span className="range__sep" aria-hidden="true" />
                      <span className="figures">
                        {draft.frames.toLocaleString()} frames
                      </span>
                      {markedPlan?.plan != null ? (
                        <>
                          <span className="range__sep" aria-hidden="true" />
                          <span className={markedPlan.plan.mode === "copy" ? "ok" : "warn"}>
                            {markedPlan.plan.mode === "copy"
                              ? "lossless copy — every frame is the original"
                              : markedPlan.plan.mode === "headPatch"
                                ? `head patch — ${markedPlan.plan.headFrames} frames re-encoded, ${markedPlan.plan.bodyFrames} copied`
                                : "full re-encode — no keyframe inside this range"}
                          </span>
                        </>
                      ) : null}
                    </>
                  )}
                </p>
              </div>
            </section>

            {/* ---- Options -------------------------------------------------------------- */}
            <section className="card">
              <header className="card__head">
                <h2 className="card__title">
                  Options
                </h2>
              </header>
              <div className="card__body">
                <div className="options">
                  <div className="options__row">
                    <label className="options__label" htmlFor="trim-preset">
                      Delivery
                    </label>
                    <select
                      id="trim-preset"
                      className="options__wide"
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

                  <div className="options__row">
                    <label className="options__label" htmlFor="trim-handles">
                      Handles
                    </label>
                    <input
                      id="trim-handles"
                      className="options__narrow"
                      type="number"
                      min={0}
                      step={1}
                      value={draft.handles}
                      onChange={(event) =>
                        draft.setHandles(Math.max(0, Number(event.target.value) || 0))
                      }
                    />
                    <span className="options__note">
                      extra frames kept either side, for a crossfade
                    </span>
                  </div>

                  <div className="options__row">
                    <label className="options__label" htmlFor="trim-policy">
                      Verify
                    </label>
                    <select
                      id="trim-policy"
                      className="options__wide"
                      value={model.verifyPolicy}
                      onChange={(event) => void model.setVerifyPolicy(event.target.value)}
                    >
                      <option value="off">Off — do not measure the result</option>
                      <option value="standard">Standard — frame count, duration, audio alignment</option>
                      <option value="strict">
                        Strict — also hash decoded frames at sample points
                      </option>
                      <option value="forensic">
                        Forensic — also compare the head pixel-wise, and every caption cue
                      </option>
                    </select>
                  </div>
                </div>
              </div>
            </section>

            {/* ---- Queue ---------------------------------------------------------------- */}
            <section className="card">
              <header className="card__head">
                <h2 className="card__title">
                  Queue
                  {queued > 0 ? <span className="card__count figures">{queued}</span> : null}
                </h2>
                <span className="spacer" />
                <span className="card__note figures">
                  {queued === 0 ? "nothing marked yet" : `${runnable} of ${queued} ready`}
                </span>
              </header>
              <div className="card__body card__body--flush">
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

            {/* ---- Actions -------------------------------------------------------------- */}
            <div className="actions">
              <button
                type="button"
                className="btn"
                onClick={() => void queue()}
                disabled={!draft.ready || running}
                title="Add this range to the queue without trimming it yet (Ctrl+Q)"
              >
                Queue it
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
                Export…
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
              <div className="spacer" />
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
                {running ? "Working…" : queued > 0 ? `Trim ${queued}` : "Trim"}
              </button>
            </div>

            {/* ---- The proof, once there is one ----------------------------------------- */}
            {model.lastOutcome !== null ? (
              <section className="card">
                <header className="card__head">
                  <h2 className="card__title">
                    What came out
                  </h2>
                </header>
                <div className="card__body">
                  <ProofPanel outcome={model.lastOutcome} model={model} />
                </div>
              </section>
            ) : null}

            {/* ---- Transcript search, when it is asked for ------------------------------ */}
            <section className="card">
              <header className="card__head">
                <h2 className="card__title">
                  Transcript
                </h2>
                <span className="spacer" />
                <span className="card__note">
                  search the words, then mark the sentence (Ctrl+F)
                </span>
              </header>
              <div className="card__body">
                <TranscriptPanel
                  sources={model.sources}
                  focusToken={searchFocusToken}
                  onMark={async (input) => {
                    await model.addSegment(input);
                  }}
                />
              </div>
            </section>

            {/* ---- Progress ------------------------------------------------------------- */}
            <ProgressLog
              lines={log.lines}
              step={log.step}
              running={running}
              onClear={log.clear}
            />

            <footer className="footerline">
              <span className="footerline__item">
                <span
                  className={`status ${model.doctor?.libx264 === true ? "status--ok" : "status--danger"}`}
                  title={model.doctor?.ffmpeg ?? ""}
                >
                  {model.doctor === null
                    ? "checking ffmpeg…"
                    : model.doctor.libx264
                      ? "ffmpeg ready"
                      : "no H.264 encoder"}
                </span>
                {model.doctor !== null ? (
                  <span className="footerline__detail truncate figures">{model.doctor.ffmpeg}</span>
                ) : null}
              </span>
              <span className="spacer" />
              {model.notice !== null ? (
                <span className="footerline__notice truncate" role="status">
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
                  {model.error.message.split("\n")[0]?.slice(0, 80) ?? "error"}
                </button>
              ) : null}
            </footer>
          </div>
        </div>

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
