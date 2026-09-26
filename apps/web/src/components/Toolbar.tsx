/**
 * The title bar.
 *
 * Four things, and none of them is an action: what the product is, what it claims, which project is
 * open, and the theme. The actions live under the trim panel, next to the thing they act on — a
 * toolbar at the top of a single-purpose window is a second place to look for the same button.
 *
 * The project control is deliberately a button rather than a menu: clicking it opens the one dialog
 * that holds every project decision (open, create, delete, add a master), which is the only place in
 * the product where a list of things is the right answer.
 */

import type { Theme } from "../state/useTheme";

export function Toolbar({
  projectName,
  projectOpen,
  theme,
  busy,
  onToggleTheme,
  onOpenProjects,
}: {
  readonly projectName: string;
  readonly projectOpen: boolean;
  readonly theme: Theme;
  readonly busy: boolean;
  readonly onToggleTheme: () => void;
  readonly onOpenProjects: () => void;
}): JSX.Element {
  return (
    <header className="titlebar">
      <span className="titlebar__mark" aria-hidden="true">
        ▸
      </span>
      <div className="titlebar__identity">
        <h1 className="titlebar__product">TheTrimmer</h1>
        <p className="titlebar__tagline">
          Frame-exact, lossless segment cutting · only the keyframe head is re-encoded
        </p>
      </div>

      <div className="spacer" />

      <button
        type="button"
        className="btn btn--ghost titlebar__project"
        onClick={onOpenProjects}
        disabled={busy}
        title={
          projectOpen
            ? "Open another project, add a master, or delete this one"
            : "Open or create a project, then add the videos to cut"
        }
      >
        <span className={`status ${projectOpen ? "status--ok" : "status--idle"}`}>
          {projectOpen ? projectName : "no project"}
        </span>
      </button>

      <button
        type="button"
        className="btn btn--ghost btn--icon"
        onClick={onToggleTheme}
        disabled={busy}
        aria-label={`Switch to the ${theme === "dark" ? "light" : "dark"} theme`}
        title={`Switch to the ${theme === "dark" ? "light" : "dark"} theme`}
      >
        {theme === "dark" ? "☾" : "☀"}
      </button>
    </header>
  );
}
