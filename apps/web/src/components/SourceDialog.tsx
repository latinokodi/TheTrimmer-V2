/**
 * Choosing the video.
 *
 * ## Why this is a dialog and not a screen
 *
 * Everything else in the application is on the main window. This one thing is a dialog because it is
 * a question with a beginning and an end — which file — and because the window cannot answer a
 * question about a file it has not been told about yet.
 *
 * ## Why a path can be typed as well as picked
 *
 * The picker is the operating system's own dialog. It is the right way to find a file you can see,
 * and it is the wrong way to add a master whose path you already have: an editor with the file on a
 * mapped share, or on a second machine they are remoting into, reads the path out of the Finder, a
 * shot log or a message. Pasting it beats navigating a tree to it, and on a locked-down workstation
 * where the shell's file dialog is disabled by policy it is the only route that works.
 */

import { useState } from "react";

import { pickFile } from "../ipc/dialog";
import type { SourceView } from "../ipc/types";
import { useModal } from "../state/useModal";

export function SourceDialog({
  sources,
  onClose,
  onAdd,
}: {
  readonly sources: readonly SourceView[];
  readonly onClose: () => void;
  readonly onAdd: (path: string) => Promise<void>;
}): JSX.Element {
  const [path, setPath] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  // Focus in, focus trapped, focus returned, Escape closes. See `state/useModal.ts`.
  const dialogRef = useModal<HTMLDivElement>(onClose);

  async function pick(): Promise<void> {
    const chosen = await pickFile({
      title: "Choose a video",
      filters: [
        {
          name: "Video",
          extensions: ["mp4", "mov", "mkv", "m4v", "mxf", "avi", "webm", "mts", "m2ts"],
        },
      ],
    });
    if (chosen === null) {
      return;
    }
    await add(chosen);
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
          <button
            type="button"
            className="btn btn--primary"
            onClick={() => void pick()}
            disabled={saving}
          >
            Choose a file…
          </button>

          <div className="field">
            <label className="field__label" htmlFor="source-path">
              Or paste a path
            </label>
            <div className="row">
              <input
                id="source-path"
                type="text"
                value={path}
                placeholder="H:\masters\reel 2\A007C012_250312_R1QK.mov"
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
                className="btn"
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
