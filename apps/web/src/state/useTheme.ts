/**
 * The theme, and remembering it.
 *
 * Two themes, one token set. The choice is stored under the operating system's per-user preferences
 * so it survives a restart, and it defaults to the OS preference rather than to dark: an editor who
 * has set their machine to light should not have to tell this application as well.
 *
 * The theme is applied as a `data-theme` attribute on the root element, which is what the token
 * file keys off. Nothing in a component reads the theme, which is why nothing in a component has to
 * know that a light theme exists.
 */

import { useCallback, useEffect, useState } from "react";

export type Theme = "dark" | "light";

const STORAGE_KEY = "thetrimmer.theme";

/** The theme the system asks for, as a fallback. */
function systemPreference(): Theme {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return "dark";
  }
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

/** The stored theme, or the system's. */
function initialTheme(): Theme {
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (stored === "dark" || stored === "light") {
      return stored;
    }
  } catch {
    // A webview with storage disabled is a webview that will use the system preference. Not being
    // able to remember the choice is not a reason to fail to start.
  }
  return systemPreference();
}

export function useTheme(): { readonly theme: Theme; readonly toggle: () => void } {
  const [theme, setTheme] = useState<Theme>(initialTheme);

  useEffect(() => {
    document.documentElement.dataset["theme"] = theme;
    try {
      window.localStorage.setItem(STORAGE_KEY, theme);
    } catch {
      // Ignored on purpose; see above.
    }
  }, [theme]);

  const toggle = useCallback(() => {
    setTheme((current) => (current === "dark" ? "light" : "dark"));
  }, []);

  return { theme, toggle };
}
