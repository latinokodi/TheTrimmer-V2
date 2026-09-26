/**
 * The behaviour a modal dialog has to have, in one place.
 *
 * ## Why this exists
 *
 * Both dialogs declared `aria-modal="true"` and neither enforced it. Tab from the last control in the
 * export dialog went to the Cancel button in the panel *behind* the scrim, so a keyboard user could
 * operate the application through a modal they could not see past — and Escape, which every Windows
 * user tries first, did nothing at all. `aria-modal` is a claim; a claim nothing enforces is worse than
 * no claim, because a screen reader then refuses to read the content that is in fact reachable.
 *
 * Three things, and a modal is not one without all three:
 *
 * 1. **Focus moves in** when it opens, to the first control, so the first Tab is inside the dialog.
 * 2. **Focus stays in** — Tab and Shift+Tab cycle, and do not leave.
 * 3. **Focus goes back** to whatever opened it, so the operator's place in the panel is not lost.
 *
 * Escape closes. It is not one of the three, but it is the one every user already knows.
 *
 * ## Why `onClose` is held in a ref
 *
 * Both call sites pass an inline arrow, so the prop has a new identity on every render. Naming it in the
 * effect's dependency list would tear the listener down and rebuild it on every keystroke in the path
 * field — and, worse, re-run the "focus the first control" step, which would drag the caret out of the
 * field the operator is typing in.
 */

import { useEffect, useRef } from "react";
import type { RefObject } from "react";

/** Everything the platform makes focusable by Tab, minus what is disabled or explicitly excluded. */
const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "summary",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

export function useModal<T extends HTMLElement>(onClose: () => void): RefObject<T> {
  const ref = useRef<T>(null);
  const close = useRef(onClose);
  close.current = onClose;

  /*
   * Where the keyboard was, captured **during render** rather than in the effect.
   *
   * This is not a style choice. An effect runs after the commit, and by then the dialog's own first
   * control may already have focus — React applies `autoFocus` during the commit — so the effect would
   * record the dialog's input as the thing to return to, and on close it would call `.focus()` on an
   * element that had just been removed from the document. Focus then falls to `<body>`, and the
   * operator is dropped at the top of the window with no indication of where they were.
   *
   * Render happens before the commit, so at this moment `document.activeElement` is still the control
   * that opened the dialog. The ref makes it idempotent across StrictMode's double render.
   */
  const opener = useRef<HTMLElement | null>(null);
  if (opener.current === null && typeof document !== "undefined") {
    opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  }

  useEffect(() => {
    const dialog = ref.current;
    if (dialog === null) {
      return;
    }

    const focusable = (): HTMLElement[] =>
      [...dialog.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
        (element) => element.offsetWidth > 0 || element.offsetHeight > 0,
      );

    (focusable()[0] ?? dialog).focus();

    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        close.current();
        return;
      }
      if (event.key !== "Tab") {
        return;
      }

      const items = focusable();
      if (items.length === 0) {
        event.preventDefault();
        return;
      }
      const first = items[0] as HTMLElement;
      const last = items[items.length - 1] as HTMLElement;
      const active = document.activeElement;

      if (event.shiftKey && (active === first || active === dialog)) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && active === last) {
        event.preventDefault();
        first.focus();
      }
    };

    dialog.addEventListener("keydown", onKeyDown);
    return () => {
      dialog.removeEventListener("keydown", onKeyDown);
      // `isConnected` because the opener can have been unmounted while the dialog was up — the Trim
      // button, for instance, is disabled while a run is in flight and can be gone by the time we return.
      const target = opener.current;
      if (target !== null && target.isConnected) {
        target.focus();
      }
    };
  }, []);

  return ref;
}
