/**
 * The question a cut asks before it replaces something.
 *
 * ## Why this is a dialog and not a line of warning text
 *
 * Replacing a segment destroys the previous one, and there is no undo. The plan row can say "this
 * file is already there" and it is still a sentence somebody reads while reaching for the button.
 * A modal is the only thing on this screen that stops and waits for an answer, which is what the
 * decision is worth: everything else about a cut is cheap to get wrong twice.
 *
 * ## The vocabulary is the stylesheet's, not this file's
 *
 * `.dialog-layer`, `.dialog`, `.dialog__head`, `.dialog__body`, `.dialog__actions` and `.dialog__help`
 * were already in `app.css`, written for dialogs that no longer exist in this build — the
 * stylesheet's own comment mentions "both dialogs" and cites `docs/adr/018-visual-language.md`,
 * which is not in the tree. They are used here rather than a second set of rules invented beside
 * them: one dialog vocabulary in a stylesheet is worth more than two, and the alternative leaves
 * the old one as dead weight that looks live.
 *
 * ## The parts that are not decoration
 *
 * * The files are named. "Overwrite?" is a question about nothing in particular.
 * * Escape and a click on the layer both mean no. The safe answer is the one that needs no aiming.
 * * Focus lands on *Keep the file*, not on Replace. A dialog that puts the destructive answer under
 *   the return key turns a stray keystroke into a lost file.
 * * Focus is trapped while it is open, so Tab cannot walk out of the question into the form behind
 *   it, and it goes back to the button that opened it when it closes.
 * * Replace is the only destructive control in the window, so it is the only one drawn as one.
 */

import { useCallback, useEffect, useRef } from "react";

import { overwriteReason, overwriteTitle, type OverwriteAsk } from "../state/overwriteAsk";

export interface OverwriteDialogProps {
  readonly ask: OverwriteAsk;
  /** Write over the files. */
  readonly onReplace: () => void;
  /** Change the mind, or the name. Nothing is written. */
  readonly onCancel: () => void;
}

/** Everything the dialog is allowed to move focus to, in the order Tab reaches them. */
const FOCUSABLE =
  'button:not([disabled]), [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

export function OverwriteDialog({ ask, onReplace, onCancel }: OverwriteDialogProps): JSX.Element {
  const panel = useRef<HTMLDivElement>(null);
  const keep = useRef<HTMLButtonElement>(null);

  // Where focus was, so it can be put back. Closing a dialog and dropping focus is how a keyboard
  // user ends up at the top of the document having lost their place in the form.
  const cameFrom = useRef<Element | null>(null);

  useEffect(() => {
    cameFrom.current = document.activeElement;
    keep.current?.focus();
    return () => {
      const previous = cameFrom.current;
      if (previous instanceof HTMLElement && document.contains(previous)) {
        previous.focus();
      }
    };
  }, []);

  const onKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCancel();
        return;
      }
      if (event.key !== "Tab") {
        return;
      }
      // The trap. Without it Tab walks into the form behind a modal, which is both confusing and,
      // for a screen reader, a different document than the one on screen.
      const found = panel.current?.querySelectorAll<HTMLElement>(FOCUSABLE);
      if (found === undefined || found.length === 0) {
        return;
      }
      const first = found[0];
      const last = found[found.length - 1];
      if (first === undefined || last === undefined) {
        return;
      }
      const here = document.activeElement;
      if (event.shiftKey && here === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && here === last) {
        event.preventDefault();
        first.focus();
      }
    },
    [onCancel],
  );

  return (
    <div
      className="dialog-layer"
      // A press that starts and ends on the layer is a press on the layer. One that starts inside
      // the panel and ends here is a drag, and dismissing on a drag would close the dialog
      // somebody was in the middle of reading.
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          onCancel();
        }
      }}
    >
      <div
        ref={panel}
        className="dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="overwrite-title"
        aria-describedby="overwrite-detail"
        onKeyDown={onKeyDown}
      >
        <div className="dialog__head">Replace a segment</div>
        <div className="dialog__body">
          <h2 className="dialog__title" id="overwrite-title">
            {overwriteTitle(ask.files)}
          </h2>
          <p className="dialog__help" id="overwrite-detail">
            {overwriteReason(ask.source)}
          </p>
          {/*
            The names, in the data face because they are filenames rather than prose, and as a list
            because there can be two and *which* two is the thing being decided.
          */}
          <ul className="source-list">
            {ask.files.map((file) => (
              <li key={file} className="source-list__item" title={file}>
                <span className="dialog__file">{file}</span>
              </li>
            ))}
          </ul>
          <p className="dialog__where" title={ask.output}>
            {ask.output}
          </p>
        </div>
        <div className="dialog__actions">
          <span className="spacer" />
          {/* First in the DOM *and* focused, so Enter means the safe answer. */}
          <button ref={keep} type="button" className="btn" onClick={onCancel}>
            Keep the file
          </button>
          <button type="button" className="btn btn--danger" onClick={onReplace}>
            Replace
          </button>
        </div>
      </div>
    </div>
  );
}
