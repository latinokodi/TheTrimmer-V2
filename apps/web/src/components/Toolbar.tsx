/**
 * The title bar.
 *
 * Three things and no actions: what the product is, what it claims, and the theme. Everything you can
 * *do* is next to the thing it acts on — the video chooser, the marks, the settings and the trim
 * button are all in the one panel below, and the tools indicator is in the status bar where it has
 * always been.
 */

import type { Theme } from "../state/useTheme";

export function Toolbar({
  theme,
  busy,
  onToggleTheme,
}: {
  readonly theme: Theme;
  readonly busy: boolean;
  readonly onToggleTheme: () => void;
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
