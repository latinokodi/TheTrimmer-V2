/**
 * The status bar: the facts that must always be visible.
 *
 * Two questions this answers without being asked, and each one has cost somebody a morning:
 *
 * * **Which ffmpeg is this using?** A machine with three builds on it is a machine where a cut
 *   behaves differently depending on the `PATH`. The version is always on screen and the tooltip
 *   gives the resolved path.
 * * **Is anything still running?** A cut takes minutes; a window that looks idle during one invites
 *   a second click.
 */

import type { AppModel } from "../state/useAppModel";
import type { Theme } from "../state/useTheme";

export function StatusBar({
  model,
  theme,
}: {
  readonly model: AppModel;
  readonly theme: Theme;
}): JSX.Element {
  const summary = model.summary;
  const missing = summary?.missingSources ?? 0;

  return (
    <footer className="statusbar">
      <span className="statusbar__item">
        <span className={`status ${model.doctor?.libx264 === true ? "status--ok" : "status--danger"}`}>
          {model.doctor === null
            ? "checking ffmpeg…"
            : model.doctor.libx264
              ? "ffmpeg ready"
              : "no H.264 encoder"}
        </span>
        {model.doctor !== null ? (
          <span className="statusbar__detail truncate figures" title={model.doctor.ffmpeg}>
            {model.doctor.ffmpeg}
          </span>
        ) : null}
      </span>

      <span className="statusbar__sep" aria-hidden="true">
        ·
      </span>

      <span className="statusbar__item figures">
        {summary === null
          ? "no project open"
          : `${summary.runnable} of ${summary.segments} segment(s) runnable`}
      </span>

      {missing > 0 ? (
        <>
          <span className="statusbar__sep" aria-hidden="true">
            ·
          </span>
          <span className="statusbar__item">
            <span className="status status--danger">
              {missing} source{missing === 1 ? "" : "s"} not on disk
            </span>
          </span>
        </>
      ) : null}

      <span className="spacer" />

      {model.notice !== null ? (
        <span className="statusbar__item statusbar__notice truncate" role="status">
          {model.notice}
        </span>
      ) : null}

      {model.busy !== null ? (
        <span className="statusbar__item" role="status" aria-live="polite">
          <span className="status status--info">{model.busy}</span>
          <span className="spinner" aria-hidden="true" />
          {model.openProjectId !== null ? (
            <button
              type="button"
              className="btn btn--ghost btn--small"
              onClick={() => void model.cancelBatch()}
            >
              Stop
            </button>
          ) : null}
        </span>
      ) : null}

      {model.error !== null ? (
        <span className="statusbar__item">
          <button
            type="button"
            className="btn btn--danger btn--small"
            onClick={model.clearMessages}
            title={model.error.message}
          >
            {firstLine(model.error.message)}
          </button>
        </span>
      ) : null}

      <span className="statusbar__sep" aria-hidden="true">
        ·
      </span>
      <span className="statusbar__item faint" title="the theme is remembered between runs">
        {theme}
      </span>
    </footer>
  );
}

/** A long error shown in a status bar is unreadable; the first line is the sentence that matters. */
function firstLine(message: string): string {
  const line = message.split("\n")[0] ?? message;
  return line.length > 90 ? `${line.slice(0, 87)}…` : line;
}


