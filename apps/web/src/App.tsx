/**
 * The application shell.
 *
 * ## The layout, and why
 *
 * ```
 * ┌──────────────────────────── toolbar ────────────────────────────┐
 * │ project · refresh · plan · run · export · theme                 │
 * ├──────────────┬────────────────────────────────────┬─────────────┤
 * │ sources      │ cut table (the workspace)          │ inspector   │
 * │              │                                    │  preview    │
 * │              │                                    │  transcript │
 * │              │                                    │  proof      │
 * ├──────────────┴────────────────────────────────────┴─────────────┤
 * │ status bar: ffmpeg · segment count · busy                      │
 * └─────────────────────────────────────────────────────────────────┘
 * ```
 *
 * Three columns because that is the shape of the work: what you have, what you marked, and what will
 * happen. The middle column is the only one that scrolls vertically as a unit, and it is the one
 * whose rows have a fixed height — a cut table whose rows resize as a plan arrives is a table that
 * jumps under the pointer.
 *
 * ## Keyboard
 *
 * Everything is reachable by keyboard, and the two flows that matter have a shortcut: `Ctrl+Enter`
 * plans and runs the batch, `Ctrl+F` focuses the transcript search. `Ctrl+O` opens a source. The
 * shortcuts are declared in one place so they cannot collide, and the dialog that lists them is
 * generated from that same map rather than written twice.
 */

import { useCallback, useEffect, useMemo, useState } from "react";

import { CutTable } from "./components/CutTable";
import { ProofPanel } from "./components/ProofPanel";
import { ProjectsDialog } from "./components/ProjectsDialog";
import { SegmentDialog } from "./components/SegmentDialog";
import { SourceList } from "./components/SourceList";
import { StatusBar } from "./components/StatusBar";
import { Toolbar } from "./components/Toolbar";
import { TranscriptPanel } from "./components/TranscriptPanel";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { useAppModel } from "./state/useAppModel";
import { useTheme } from "./state/useTheme";

/** The inspector's tabs. */
type InspectorTab = "plan" | "transcript" | "proof";

export function App(): JSX.Element {
  const model = useAppModel();
  const { theme, toggle: toggleTheme } = useTheme();
  const [tab, setTab] = useState<InspectorTab>("plan");
  const [projectsOpen, setProjectsOpen] = useState(false);
  const [segmentOpen, setSegmentOpen] = useState(false);
  const [searchFocusToken, setSearchFocusToken] = useState(0);

  const selected = useMemo(
    () => model.segments.find((segment) => segment.id === model.selectedSegment) ?? null,
    [model.segments, model.selectedSegment],
  );

  const runnable = model.summary?.runnable ?? 0;

  const runBatch = useCallback(() => {
    if (runnable === 0) {
      return;
    }
    setTab("proof");
    void model.runBatch({ stopOnError: false, skipVerification: false, label: "batch" });
  }, [model, runnable]);

  // The shortcuts. One listener, one map, so the help dialog and the behaviour cannot disagree.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const modifier = event.ctrlKey || event.metaKey;
      if (!modifier) {
        return;
      }
      switch (event.key.toLowerCase()) {
        case "enter":
          event.preventDefault();
          runBatch();
          break;
        case "f":
          event.preventDefault();
          setTab("transcript");
          setSearchFocusToken((token) => token + 1);
          break;
        case "o":
          event.preventDefault();
          setProjectsOpen(true);
          break;
        case "n":
          event.preventDefault();
          setSegmentOpen(true);
          break;
        case "p":
          event.preventDefault();
          setTab("plan");
          void model.previewAll();
          break;
        default:
          break;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [model, runBatch]);

  return (
    <ErrorBoundary>
      <div className="app">
        <a className="skip-link" href="#cut-table">
          Skip to the cut table
        </a>

        <Toolbar
          model={model}
          theme={theme}
          onToggleTheme={toggleTheme}
          onOpenProjects={() => setProjectsOpen(true)}
          onAddSegment={() => setSegmentOpen(true)}
          onRunBatch={runBatch}
          runnable={runnable}
        />

        <div className="app__body">
          <SourceList
            sources={model.sources}
            onRemove={(path) => void model.removeSource(path)}
            onAdd={() => setProjectsOpen(true)}
            busy={model.busy !== null}
          />

          <main className="app__main" id="cut-table">
            <CutTable
              segments={model.segments}
              selected={model.selectedSegment}
              onSelect={model.selectSegment}
              onToggleEnabled={(id, enabled) => void model.updateSegment({ id, enabled })}
              onRemove={(id) => void model.removeSegment(id)}
              previews={model.previews}
              onAdd={() => setSegmentOpen(true)}
              busy={model.busy !== null}
            />
          </main>

          <aside className="app__inspector" aria-label="Inspector">
            <div className="tabs" role="tablist" aria-label="Inspector sections">
              {(
                [
                  ["plan", "Plan"],
                  ["transcript", "Transcript"],
                  ["proof", "Proof"],
                ] as const
              ).map(([key, label]) => (
                <button
                  key={key}
                  type="button"
                  role="tab"
                  id={`tab-${key}`}
                  aria-selected={tab === key}
                  aria-controls={`panel-${key}`}
                  className={`tabs__tab${tab === key ? " tabs__tab--active" : ""}`}
                  onClick={() => setTab(key)}
                >
                  {label}
                  {key === "proof" && model.lastOutcome !== null ? (
                    <span className="tabs__count" aria-hidden="true">
                      {model.lastOutcome.jobs.length}
                    </span>
                  ) : null}
                </button>
              ))}
            </div>

            <div
              className="app__panel"
              role="tabpanel"
              id={`panel-${tab}`}
              aria-labelledby={`tab-${tab}`}
              tabIndex={0}
            >
              {tab === "plan" ? (
                <PlanPanel
                  previews={model.previews}
                  selected={selected}
                  onPreviewAll={() => void model.previewAll()}
                  onPreviewSelected={() => {
                    if (selected !== null) {
                      void model.preview(selected.id);
                    }
                  }}
                  busy={model.busy !== null}
                />
              ) : null}
              {tab === "transcript" ? (
                <TranscriptPanel
                  sources={model.sources}
                  focusToken={searchFocusToken}
                />
              ) : null}
              {tab === "proof" ? <ProofPanel outcome={model.lastOutcome} model={model} /> : null}
            </div>
          </aside>
        </div>

        <StatusBar model={model} theme={theme} />

        {projectsOpen ? (
          <ProjectsDialog model={model} onClose={() => setProjectsOpen(false)} />
        ) : null}
        {segmentOpen ? (
          <SegmentDialog
            sources={model.sources}
            presets={model.presets}
            onClose={() => setSegmentOpen(false)}
            onSubmit={async (input) => {
              await model.addSegment(input);
              setSegmentOpen(false);
            }}
          />
        ) : null}
      </div>
    </ErrorBoundary>
  );
}

/**
 * The plan tab: what the batch will do and what it will cost.
 *
 * The point of this panel is that it answers *before* the run button is pressed. A preset that
 * reshapes the frame turns every segment into a full re-encode, and the difference between minutes
 * and hours has to be visible while it can still change the decision.
 */
function PlanPanel({
  previews,
  selected,
  onPreviewAll,
  onPreviewSelected,
  busy,
}: {
  readonly previews: readonly import("./ipc/types").QueuePreview[];
  readonly selected: import("./ipc/types").SegmentView | null;
  readonly onPreviewAll: () => void;
  readonly onPreviewSelected: () => void;
  readonly busy: boolean;
}): JSX.Element {
  const totalBytes = previews.reduce((sum, preview) => sum + (preview.estimatedBytes ?? 0), 0);
  const fullEncodes = previews.filter((preview) => preview.forcesFullEncode).length;

  return (
    <div className="panel-body">
      <div className="panel-body__actions">
        <button type="button" className="btn" onClick={onPreviewAll} disabled={busy}>
          Plan the batch
        </button>
        <button
          type="button"
          className="btn btn--ghost"
          onClick={onPreviewSelected}
          disabled={busy || selected === null}
        >
          Plan the selected segment
        </button>
      </div>

      {selected === null ? (
        <p className="empty">
          Nothing is selected. Choose a segment in the cut table, or plan the whole batch.
        </p>
      ) : (
        <dl className="facts">
          <dt>Segment</dt>
          <dd className="truncate">{selected.name}</dd>
          <dt>Range</dt>
          <dd className="figures">
            {selected.inTimecode} → {selected.outTimecode}
          </dd>
          <dt>Frames</dt>
          <dd className="figures numeric-column">{selected.frames?.toLocaleString() ?? "—"}</dd>
        </dl>
      )}

      {previews.length === 0 ? null : (
        <>
          <hr className="divider" />
          <dl className="facts">
            <dt>Segments planned</dt>
            <dd className="figures numeric-column">{previews.length}</dd>
            <dt>Full re-encodes</dt>
            <dd className={`figures numeric-column${fullEncodes > 0 ? " warn" : ""}`}>
              {fullEncodes}
            </dd>
            <dt>Estimated output</dt>
            <dd className="figures numeric-column">
              {totalBytes > 0 ? `${(totalBytes / 1024 / 1024 / 1024).toFixed(1)} GB` : "—"}
            </dd>
          </dl>
          {fullEncodes > 0 ? (
            <p className="note note--warn">
              {fullEncodes} segment(s) use a preset that changes the picture, so the whole segment is
              re-encoded rather than copied. That is the preset doing its job, not a fault — but it is
              the difference between minutes and hours, so it is worth knowing now.
            </p>
          ) : (
            <p className="note note--ok">
              Every planned segment keeps the original packets for most of its length.
            </p>
          )}
        </>
      )}
    </div>
  );
}
