/**
 * The application shell.
 *
 * ## The whole window
 *
 * ```
 * ┌────────────────────────────────────────────────────────────────────────────────┐
 * │ ▸ TheTrimmer  Frame-exact, lossless segment cutting        [project] [theme]   │
 * ├────────────────────────────────────────────────────────────────────────────────┤
 * │  VIDEO   Master.mp4 — 640×360, 25 fps, 125 frames, 3 caption cues              │
 * │                                                                                │
 * │  IN      [ 00:00:01:00 ]        OUT     [ 00:00:02:00 ]                        │
 * │          frame 25                       frame 50, the last one kept            │
 * │                                                                                │
 * │  26 frames · 1.04 s · lossless copy        [ Queue it ] [ Trim this segment ]  │
 * ├────────────────────────────────────────────────────────────────────────────────┤
 * │  ▸ Queued (2)   ▸ What happens when you trim   ▸ Transcript                    │
 * ├────────────────────────────────────────────────────────────────────────────────┤
 * │ ● ffmpeg ready · 2 of 2 ready · Master.mp4 640×360 …                 dark      │
 * └────────────────────────────────────────────────────────────────────────────────┘
 * ```
 *
 * One column. The thing the product does — cut a range out of a video — is at the top, always
 * visible, and everything a person needs in order to do it is on that one line of sight: which file,
 * where it starts, where it ends, how long that is, and the button.
 *
 * ## What changed, and why
 *
 * The first version of this shell was a three-column dashboard: a rail of masters, a table of
 * segments, and an inspector with three tabs, with the marks themselves hidden inside a modal. It was
 * defensible as an information architecture and it was the wrong answer, because it made a *simple
 * tool* look like a *complicated one*. An editor opening a trimming application wants to trim
 * something, and every panel that is not about the trim in front of them is a thing to read first.
 *
 * So the queue, the explanation of the method and the transcript are below the fold in three
 * collapsed sections, and none of them has to be opened to trim a segment. They are still there —
 * trimming nine segments by hand is a worse workflow than queueing nine and pressing once — but they
 * are answers to a question a person asks *after* the first cut, not before it.
 *
 * ## Keyboard
 *
 * `Ctrl+Enter` trims, `Ctrl+Q` queues the marked range, `Ctrl+O` opens a project, `Ctrl+M` chooses a
 * video, `Ctrl+E` exports, `Ctrl+F` finds in the transcript. One listener, one map.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { CutTable } from "./components/CutTable";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { ExportDialog } from "./components/ExportDialog";
import { ProofPanel } from "./components/ProofPanel";
import { ProjectsDialog } from "./components/ProjectsDialog";
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
  const [projectsOpen, setProjectsOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [details, setDetails] = useState<"none" | "queue" | "method" | "transcript">("none");
  const [searchFocusToken, setSearchFocusToken] = useState(0);
  const [trimmedSomething, setTrimmedSomething] = useState(false);

  const projectName =
    model.projects.find((project) => project.id === model.openProjectId)?.name ?? "TheTrimmer";
  const runnable = model.summary?.runnable ?? 0;
  const queued = model.segments.length;

  /** What the plan says about the range currently marked, if it has been planned. */
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
   * you. Behind a button, that answer arrives after the decision has been made; asked here, on a
   * delay that keeps it to about one call per pause in typing, it arrives while the marks are still
   * being changed.
   *
   * ## Why the function is held in a ref
   *
   * The model object is rebuilt on every render — deliberately, because it is a cache of the last
   * answer and not a store. Naming it in the dependency list would therefore re-run this effect on
   * every render; the effect sets previews, previews re-render, and the page spins. The ref carries
   * the current function without making the effect depend on the object that holds it, and the only
   * things this effect actually cares about are the two frame numbers and the delivery settings.
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

  const chooseMaster = useCallback(() => setProjectsOpen(true), []);

  const queue = useCallback(async () => {
    const request = draft.request();
    if (request === null) {
      return;
    }
    await model.addSegment(request);
    draft.reset();
    setDetails("queue");
  }, [draft, model]);

  const trimNow = useCallback(async () => {
    // A marked range that is not queued yet is what the button means: "cut this". Queueing it first
    // and then running the batch is one code path rather than two, so a single trim and a batch of
    // nine go through exactly the same queue.
    const request = draft.request();
    if (request !== null) {
      await model.addSegment(request);
      draft.reset();
    }
    await model.runBatch({ stopOnError: false, skipVerification: false, label: "trim" });
    setTrimmedSomething(true);
    setDetails("queue");
  }, [draft, model]);

  const clearQueue = useCallback(async () => {
    for (const segment of model.segments) {
      await model.removeSegment(segment.id);
    }
  }, [model]);

  // The shortcuts. One listener, one map, so the tooltips and the behaviour cannot disagree.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const modifier = event.ctrlKey || event.metaKey;
      if (!modifier) {
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
          event.preventDefault();
          setProjectsOpen(true);
          break;
        case "m":
          event.preventDefault();
          setProjectsOpen(true);
          break;
        case "e":
          event.preventDefault();
          setExportOpen(true);
          break;
        case "f":
          event.preventDefault();
          setDetails("transcript");
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

        <Toolbar
          projectName={projectName}
          projectOpen={model.openProjectId !== null}
          theme={theme}
          busy={model.busy !== null}
          onToggleTheme={toggleTheme}
          onOpenProjects={() => setProjectsOpen(true)}
        />

        <main className="app__main">
          <TrimPanel
            draft={draft}
            sources={model.sources}
            presets={model.presets}
            preview={markedPlan}
            queued={queued}
            busy={model.busy !== null}
            onQueue={() => void queue()}
            onTrimNow={() => void trimNow()}
            onClearQueue={() => void clearQueue()}
            onAddMaster={chooseMaster}
            showQueueHint={queued > 0}
          />

          <div className="details">
            <Section
              id="queue"
              title="Queued"
              count={queued}
              open={details === "queue"}
              onToggle={(open) => setDetails(open ? "queue" : "none")}
              hint={
                queued === 0
                  ? "nothing queued yet"
                  : `${runnable} of ${queued} ready to trim`
              }
            >
              <CutTable
                segments={model.segments}
                selected={model.selectedSegment}
                onSelect={model.selectSegment}
                onToggleEnabled={(id, enabled) => void model.updateSegment({ id, enabled })}
                onRemove={(id) => void model.removeSegment(id)}
                previews={model.previews}
                projectOpen={model.openProjectId !== null}
                hasSource={model.sources.length > 0}
                busy={model.busy !== null}
              />
              {trimmedSomething ? (
                <ProofPanel outcome={model.lastOutcome} model={model} />
              ) : (
                <p className="details__note">
                  Press <strong>Trim this segment</strong> and the finished file is measured against
                  the source: frame count, duration, audio alignment, and — at the stricter policies —
                  the decoded frames themselves. The verdict appears here.
                </p>
              )}
            </Section>

            <Section
              id="method"
              title="What it does"
              open={details === "method"}
              onToggle={(open) => setDetails(open ? "method" : "none")}
              hint="the head patch, in four lines"
            >
              <ol className="method">
                <li>
                  The frames before the first keyframe at or after the in point are re-encoded. That is
                  the smallest number of frames a cut can avoid re-encoding.
                </li>
                <li>
                  Every frame after that keyframe is the original packet, copied — which is what makes
                  the result lossless rather than nearly lossless.
                </li>
                <li>
                  The two are joined at the source&rsquo;s own timescale, so the clip cannot come out
                  in slow motion.
                </li>
                <li>
                  The finished file is measured and checked. A file that is short, misaligned or at the
                  wrong timescale is reported rather than delivered quietly.
                </li>
              </ol>
              <p className="details__note">
                A range that starts on a keyframe needs no re-encoding at all, and the button says so
                before you press it.
              </p>
            </Section>

            <Section
              id="transcript"
              title="Transcript"
              open={details === "transcript"}
              onToggle={(open) => setDetails(open ? "transcript" : "none")}
              hint={
                model.sources.some((source) => source.transcript !== null)
                  ? "search a caption file and mark a range from it"
                  : "no caption file beside the video"
              }
            >
              <TranscriptPanel
                sources={model.sources}
                focusToken={searchFocusToken}
                onMark={async (input) => {
                  await model.addSegment(input);
                }}
              />
            </Section>
          </div>
        </main>

        <StatusBar model={model} theme={theme} />

        {projectsOpen ? (
          <ProjectsDialog model={model} onClose={() => setProjectsOpen(false)} />
        ) : null}
        {exportOpen ? (
          <ExportDialog
            projectName={projectName}
            onClose={() => setExportOpen(false)}
            onExport={(input) => model.exportTimeline(input)}
          />
        ) : null}
      </div>
    </ErrorBoundary>
  );
}

/**
 * A collapsed section.
 *
 * `details`/`summary` rather than a hand-rolled disclosure: the element already has the keyboard
 * behaviour, the expanded state and the announcement, and reimplementing three things that work is
 * how an interface ends up with a disclosure a screen reader cannot open.
 *
 * It is deliberately **uncontrolled** — `open` is the initial state and the browser owns it from
 * there. Binding `open` to React state as well means a click on the summary fights the element's own
 * toggle, and the section ends up needing two clicks to open. The parent is *told* what happened
 * instead of deciding it, and uses that to close whichever section was open before.
 */
function Section({
  id,
  title,
  count,
  hint,
  open,
  onToggle,
  children,
}: {
  readonly id: string;
  readonly title: string;
  readonly count?: number;
  readonly hint: string;
  readonly open: boolean;
  readonly onToggle: (open: boolean) => void;
  readonly children: React.ReactNode;
}): JSX.Element {
  return (
    <details className="section" open={open} id={`section-${id}`}>
      <summary
        className="section__head"
        onClick={(event) => {
          // Stop the element toggling itself, then let the parent decide: it has to close the other
          // section, which is a decision this element cannot make.
          event.preventDefault();
          onToggle(!open);
        }}
      >
        <span className="section__chevron" aria-hidden="true">
          {open ? "▾" : "▸"}
        </span>
        <span className="section__title">{title}</span>
        {count !== undefined && count > 0 ? (
          <span className="section__count figures">{count}</span>
        ) : null}
        <span className="spacer" />
        <span className="section__hint">{hint}</span>
      </summary>
      <div className="section__body">{children}</div>
    </details>
  );
}
