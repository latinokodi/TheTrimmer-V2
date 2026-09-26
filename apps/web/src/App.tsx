/**
 * The application shell.
 *
 * ## The whole window
 *
 * ```
 * ┌ TheTrimmer ────────────────────────── frame-exact, lossless ──── ● ffmpeg ready ─┐
 * ├──────────────────────────────────────────────┬──────────────────────────────────┤
 * │ VIDEO  A007C012_250312_R1QK.mov     [Change…]│ QUEUE                             │
 * │ 3840×2160 · prores · 25 fps · 3,101 frames   │ #  NAME  IN  OUT  FRAMES  PLAN    │
 * ├──────────────────────────────────────────────┤ 1  cold open  00:00:01:00 …       │
 * │ IN [00:00:01:00]           OUT [00:00:02:00] │                                   │
 * │    frame 25                  frame 50         │                                   │
 * │ DELIVERY [master ▾]  HANDLES [0]  VERIFY [▾] │                                   │
 * │ 26 frames · 1.04 s · lossless copy   [Trim]  │                                   │
 * ├──────────────────────────────────────────────┴───────────────────────────────────┤
 * │ TRANSCRIPT [ search… ]  3 hits   │  the first cue, and the next two                │
 * ├──────────────────────────────────────────────────────────────────────────────────┤
 * │ ● ffmpeg ready · 0 of 2 runnable · what just happened                     dark    │
 * └──────────────────────────────────────────────────────────────────────────────────┘
 * ```
 *
 * There is no project step, no rail of masters, no inspector tabs and nothing folded away. A
 * session *is* the project: the window keeps one, names it after the day, and opens it on launch —
 * so the first thing on screen is a video to choose and two boxes to type a range into, rather than a
 * dialog asking the user to name something before they are allowed to do anything.
 *
 * The store is still there, and that is deliberate. A batch of nine marked segments that a restart
 * forgets is a worse defect than the dialog it replaced, and the audit trail, the export and the
 * `watch` folder all read the same project. What was removed is the *ceremony*, not the record.
 *
 * ## Density
 *
 * The earlier shell was three columns with a rail, a table and a tabbed inspector, and on a 1440 px
 * window it left a column of empty space down the middle while five settings sat behind three
 * collapsed sections. Everything is now on the main window: the marks, the delivery preset, the
 * handles and the verification policy on one row each, with their current values visible. Empty space
 * in a tool is not calm, it is something to scroll past.
 *
 * ## Keyboard
 *
 * `Ctrl+Enter` trims, `Ctrl+Q` queues the marked range, `Ctrl+O` chooses a video, `Ctrl+E` exports,
 * `Ctrl+F` finds in the transcript, `Ctrl+D` opens the delivery settings.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { CutTable } from "./components/CutTable";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { ExportDialog } from "./components/ExportDialog";
import { ProofPanel } from "./components/ProofPanel";
import { SourceDialog } from "./components/SourceDialog";
import { StatusBar } from "./components/StatusBar";
import { Toolbar } from "./components/Toolbar";
import { TranscriptPanel } from "./components/TranscriptPanel";
import { TrimPanel } from "./components/TrimPanel";
import { useAppModel } from "./state/useAppModel";
import { useSegmentDraft } from "./state/useSegmentDraft";
import { useTheme } from "./state/useTheme";

export function App(): JSX.Element {
  const model = useAppModel();
  const { theme, toggle: toggleTheme } = useTheme();
  const draft = useSegmentDraft(model.sources);
  const [sourceOpen, setSourceOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [searchFocusToken, setSearchFocusToken] = useState(0);
  const [ran, setRan] = useState(false);

  const queued = model.segments.length;
  const runnable = model.summary?.runnable ?? 0;

  /**
   * What the plan says about the range currently marked.
   *
   * Matched on the frame numbers rather than on an id, because a range that has been marked but not
   * queued has no id — `preview_all` reports it as an entry whose start and end are the real ones.
   */
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
   * Plan as the marks are typed.
   *
   * The product's one claim is that only the frames a cut cannot avoid are re-encoded, and the
   * difference between a lossless copy and a full re-encode is a decision the length line makes for
   * you. Behind a button, that answer arrives after the decision has been made; asked here, on a delay
   * that keeps it to about one call per pause in typing, it arrives while the marks are still being
   * changed.
   *
   * The function is held in a ref because the model object is rebuilt on every render — deliberately,
   * since it is a cache of the last answer and not a store. Naming it in the dependency list would
   * re-run this on every render; the effect sets previews, previews re-render, and the page spins.
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
    // A marked range that is not queued yet is what the plain "Trim" means: "cut this". Queueing it
    // first and then running the batch is one code path rather than two, so a single trim and a batch
    // of nine go through exactly the same queue. When something is already queued the button says
    // `Trim n` and this leaves the marks alone — the fields are for the *next* range, and a person who
    // has typed one and not queued it should not have it swept into the run by accident.
    const request = queued === 0 ? draft.request() : null;
    if (request !== null) {
      await model.addSegment(request);
      draft.reset();
    }
    await model.runBatch({ stopOnError: false, skipVerification: false, label: "trim" });
    setRan(true);
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
        case "d":
          event.preventDefault();
          document.getElementById("trim-preset")?.focus();
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
      <div className="app">
        <a className="skip-link" href="#trim-in">
          Skip to the In point
        </a>

        <Toolbar theme={theme} busy={model.busy !== null} onToggleTheme={toggleTheme} />

        <div className="app__body">
          <main className="app__main">
            <TrimPanel
              draft={draft}
              sources={model.sources}
              presets={model.presets}
              preview={markedPlan}
              policy={model.verifyPolicy}
              queued={queued}
              busy={model.busy !== null}
              onQueue={() => void queue()}
              onTrimNow={() => void trimNow()}
              onChooseMaster={() => setSourceOpen(true)}
              onPolicy={(value) => void model.setVerifyPolicy(value)}
            />

            <section className="transcript-strip" aria-label="Transcript">
              <TranscriptPanel
                sources={model.sources}
                focusToken={searchFocusToken}
                onMark={async (input) => {
                  await model.addSegment(input);
                }}
              />
            </section>
          </main>

          <aside className="app__side" aria-label="Queue">
            <div className="side__head">
              <span className="field__label">Queue</span>
              <span className="side__count figures">{queued > 0 ? queued : ""}</span>
              <span className="spacer" />
              <span className="side__hint figures">
                {queued === 0 ? "nothing marked yet" : `${runnable} of ${queued} ready`}
              </span>
            </div>

            <div className="side__body scroll">
              <CutTable
                segments={model.segments}
                selected={model.selectedSegment}
                onSelect={model.selectSegment}
                onToggleEnabled={(id, enabled) => void model.updateSegment({ id, enabled })}
                onRemove={(id) => void model.removeSegment(id)}
                previews={model.previews}
                busy={model.busy !== null}
              />
            </div>

            {/*
              The proof appears where the queue was, once there is one. A finished run is the answer to
              the question the queue was asking, so it belongs in the same place rather than behind a
              tab — and a panel that is empty until it has something to say is a panel that never
              wastes a pixel.
            */}
            {ran || model.lastOutcome !== null ? (
              <div className="side__proof">
                <ProofPanel outcome={model.lastOutcome} model={model} />
              </div>
            ) : null}
          </aside>
        </div>

        <StatusBar model={model} theme={theme} />

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
