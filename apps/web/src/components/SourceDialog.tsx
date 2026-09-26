/**
 * Choosing the video, when the operating system's dialog is not the way to do it.
 *
 * ## Why this is now a fallback rather than the first step
 *
 * `Browse` on the panel opens the **Windows file dialog** directly. It used to open this instead, which
 * added a click to a one-click job and hid the operating system's own picker — where the operator's
 * favourites, recent folders, network shares and search all live — behind a button inside a dialog they
 * had to find first.
 *
 * So this opens in exactly one situation: `pickFile` answered `unavailable`, which means there is no
 * picker to show. That is a real situation (a build without the dialog plugin, or a page with no window
 * behind it) and it needs a real answer, not a dead button. Both things this dialog can do are things
 * the panel cannot:
 *
 * * take a **typed or pasted path** as the primary action, and
 * * list the masters already in this session, with whether each is still on disk.
 *
 * A picker that is merely *cancelled* does not come here. Cancelling is a decision and it is respected.
 */

import { useState } from "react";

import { VIDEO_FILTERS, pickFile } from "../ipc/dialog";
import type { SourceView } from "../ipc/types";
import { useModal } from "../state/useModal";

export function SourceDialog({
  sources,
  notice,
  onClose,
  onAdd,
}: {
  readonly sources: readonly SourceView[];
  /** Why this dialog is open instead of the operating system's picker, when there is a reason. */
  readonly notice?: string;
  readonly onClose: () => void;
  readonly onAdd: (path: string) => Promise<void>;
}): JSX.Element {
  const [path, setPath] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  // Focus in, focus trapped, focus returned, Escape closes. See `state/useModal.ts`.
  const dialogRef = useModal<HTMLDivElement>(onClose);

  async function pick(): Promise<void> {
    const picked = await pickFile({ title: "Choose a video", filters: VIDEO_FILTERS });
    if (picked.kind === "picked") {
      await add(picked.path);
    } else if (picked.kind === "unavailable") {
      setError(picked.reason);
    }
  }

  async function add(candidate: string): Promise<void> {
    const trimmed = candidate.trim();
    if (trimmed === "") {
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await onAdd(trimmed);
      onClose();
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="dialog-layer"
      role="presentation"
      onClick={(event) => {
        if (event.target === event.currentTarget) {
          onClose();
        }
      }}
    >
      <div
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="source-title"
        tabIndex={-1}
        ref={dialogRef}
      >
        <h2 className="dialog__head" id="source-title">
          Choose a video
        </h2>

        <div className="dialog__body">
          {notice !== undefined ? (
            <p className="note note--warn" role="status">
              {notice}
            </p>
          ) : null}

          <div className="field">
            <label className="field__label" htmlFor="source-path">
              Path to the video
            </label>
            <div className="row">
              <input
                id="source-path"
                type="text"
                value={path}
                placeholder="D:\masters\A007C012.mov"
                spellCheck={false}
                onChange={(event) => setPath(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    void add(path);
                  }
                }}
              />
              <button
                type="button"
                className="btn btn--primary"
                disabled={path.trim().length === 0 || saving}
                onClick={() => void add(path)}
              >
                Add
              </button>
            </div>
            <p className="field__help">
              A caption file named after the video (<code>&lt;video&gt;.srt</code>) is picked up
              automatically and retimed with every segment.
            </p>
          </div>

          {/*
            Kept even here, because "no picker" can be wrong: a machine where the picker failed to
            install once may still have it working, and this is the only way to find out without
            restarting. It is no longer the first thing on screen.
          */}
          <div className="row">
            <button type="button" className="btn" onClick={() => void pick()} disabled={saving}>
              Try the Windows file dialog
            </button>
          </div>

          {sources.length > 0 ? (
            <div className="field">
              <span className="field__label">Already in this session</span>
              <ul className="source-list">
                {sources.map((source) => (
                  <li key={source.path} className="source-list__item">
                    <span className={`status ${source.present ? "status--ok" : "status--danger"}`}>
                      {source.present ? "present" : "missing"}
                    </span>
                    <span className="truncate" title={source.path}>
                      {source.name}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}

          {error !== null ? (
            <p className="note note--danger" role="alert">
              {error}
            </p>
          ) : null}
        </div>

        <div className="dialog__actions">
          <div className="spacer" />
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
