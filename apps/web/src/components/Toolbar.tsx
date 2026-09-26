/**
 * The toolbar: what you can do, in the order you do it.
 *
 * Six controls, left to right in the order a session runs: open a project, change the output folder,
 * add a source, mark a segment, plan, run. The one primary button is **Run**, and it is disabled
 * with a reason rather than silently inert — a disabled control with no explanation is a dead end.
 */

import type { AppModel } from "../state/useAppModel";
import type { Theme } from "../state/useTheme";

export function Toolbar({
  model,
  theme,
  onToggleTheme,
  onOpenProjects,
  onAddSegment,
  onRunBatch,
  runnable,
}: {
  readonly model: AppModel;
  readonly theme: Theme;
  readonly onToggleTheme: () => void;
  readonly onOpenProjects: () => void;
  readonly onAddSegment: () => void;
  readonly onRunBatch: () => void;
  readonly runnable: number;
}): JSX.Element {
  const projectName = model.projects.find((project) => project.id === model.openProjectId)?.name;
  const busy = model.busy !== null;

  return (
    <header className="toolbar">
      <div className="toolbar__identity">
        <span className="toolbar__mark" aria-hidden="true">
          ▸
        </span>
        <div className="toolbar__words">
          <span className="toolbar__product">TheTrimmer</span>
          <span className="toolbar__project truncate" title={projectName ?? "no project open"}>
            {projectName ?? "no project open"}
          </span>
        </div>
      </div>

      <div className="toolbar__group">
        <button type="button" className="btn" onClick={onOpenProjects} disabled={busy}>
          Project…
        </button>
        <button
          type="button"
          className="btn"
          onClick={onAddSegment}
          disabled={busy || model.sources.length === 0}
          title={
            model.sources.length === 0
              ? "Add a source first: a segment has to cut something"
              : "Mark a segment (Ctrl+N)"
          }
        >
          Add segment
        </button>
      </div>

      <div className="spacer" />

      <div className="toolbar__group">
        <button
          type="button"
          className="btn"
          onClick={() => void model.previewAll()}
          disabled={busy || runnable === 0}
          title="Work out what every segment will do and cost, without cutting anything (Ctrl+P)"
        >
          Plan
        </button>
        <button
          type="button"
          className="btn btn--primary"
          onClick={onRunBatch}
          disabled={busy || runnable === 0}
          title={
            runnable === 0
              ? "Nothing to run: no segment is enabled and free of problems"
              : `Cut ${runnable} segment(s) (Ctrl+Enter)`
          }
        >
          {busy ? "Working…" : `Run ${runnable > 0 ? runnable : ""}`.trim()}
        </button>
        <button
          type="button"
          className="btn btn--ghost btn--icon"
          onClick={onToggleTheme}
          aria-label={`Switch to the ${theme === "dark" ? "light" : "dark"} theme`}
          title={`Switch to the ${theme === "dark" ? "light" : "dark"} theme`}
        >
          {theme === "dark" ? "☾" : "☀"}
        </button>
      </div>
    </header>
  );
}
