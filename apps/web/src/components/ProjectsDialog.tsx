/**
 * Opening, and creating, projects.
 *
 * Two jobs in one dialog because they are the same decision: which project am I working on. The
 * store is the application's own database, so this is also where a source is added — a project with
 * no sources has nothing to cut.
 */

import { useState } from "react";

import type { AppModel } from "../state/useAppModel";
import { formatTimestamp } from "../lib/format";
import { pickFile } from "../ipc/dialog";

export function ProjectsDialog({
  model,
  onClose,
}: {
  readonly model: AppModel;
  readonly onClose: () => void;
}): JSX.Element {
  const [newName, setNewName] = useState("");
  const [sourcePath, setSourcePath] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function addSource(): Promise<void> {
    const picked = await pickFile({
      title: "Add a master",
      filters: [
        {
          name: "Video",
          extensions: ["mp4", "mov", "mkv", "m4v", "mxf", "avi", "webm", "mts", "m2ts"],
        },
      ],
    });
    if (picked === null) {
      return;
    }
    setError(null);
    await model.addSource(picked);
  }

  /** Add a source from a typed or pasted path. */
  async function addByPath(): Promise<void> {
    const trimmed = sourcePath.trim();
    if (trimmed.length === 0) {
      return;
    }
    setError(null);
    try {
      await model.addSource(trimmed);
      setSourcePath("");
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  }

  return (
    <div
      className="dialog-layer"
      role="presentation"
      /*
       * Only a click on the backdrop itself closes the dialog. Clicks from the fields and buttons
       * inside it bubble up to here, and treating those as a dismissal would throw away a typed
       * project name the moment the user clicked beside it. See the same rule in `SegmentDialog`.
       */
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
        aria-labelledby="projects-title"
      >
        <h2 className="dialog__title" id="projects-title">
          Projects
        </h2>

        <section className="dialog__section">
          <h3 className="dialog__subtitle">Open</h3>
          {model.projects.length === 0 ? (
            <p className="empty empty--tight">No projects yet. Create one below.</p>
          ) : (
            <ul className="project-list scroll">
              {model.projects.map((project) => (
                <li key={project.id}>
                  <button
                    type="button"
                    className={`project-list__item${
                      project.id === model.openProjectId ? " project-list__item--open" : ""
                    }`}
                    onClick={() => {
                      void model.openProject(project.id);
                      onClose();
                    }}
                  >
                    <span className="project-list__name truncate">{project.name}</span>
                    <span className="project-list__when figures">
                      {formatTimestamp(project.updatedAt)}
                    </span>
                    {project.id === model.openProjectId ? (
                      <span className="status status--ok">open</span>
                    ) : null}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="dialog__section">
          <h3 className="dialog__subtitle">Create</h3>
          <div className="field">
            <label className="field__label" htmlFor="new-project-name">
              Name
            </label>
            <input
              id="new-project-name"
              type="text"
              value={newName}
              placeholder="Andy Ross interview"
              onChange={(event) => setNewName(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && newName.trim().length > 0) {
                  void model.createProject(newName.trim()).then(onClose);
                }
              }}
            />
          </div>
          <button
            type="button"
            className="btn btn--primary"
            disabled={newName.trim().length === 0 || model.busy !== null}
            onClick={() => void model.createProject(newName.trim()).then(onClose)}
          >
            Create and open
          </button>
        </section>

        {model.openProjectId !== null ? (
          <section className="dialog__section">
            <h3 className="dialog__subtitle">Sources</h3>
            <p className="dialog__help">
              Add the masters this project cuts. A caption file named after a master
              (<code>&lt;master&gt;.srt</code>) is picked up automatically and retimed with every
              segment.
            </p>
            <div className="row">
              <button
                type="button"
                className="btn"
                onClick={() => void addSource()}
                disabled={model.busy !== null}
              >
                Add a master…
              </button>
            </div>

            {/*
              A path can also be typed or pasted, and this is not a fallback for the picker — it is
              the faster route for the work this product is for. An editor adding a master that is on
              a mapped share or a second machine reads the path out of the Finder, the shot log or a
              message, and pasting it beats navigating a tree to it. It is also the route that works
              on a locked-down workstation where the shell's own file dialog is disabled by policy.
            */}
            <div className="field">
              <label className="field__label" htmlFor="source-path">
                Or paste a path
              </label>
              <div className="row">
                <input
                  id="source-path"
                  type="text"
                  value={sourcePath}
                  placeholder="H:\masters\reel 2\take 07.mov"
                  spellCheck={false}
                  onChange={(event) => setSourcePath(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" && sourcePath.trim().length > 0) {
                      void addByPath();
                    }
                  }}
                />
                <button
                  type="button"
                  className="btn"
                  disabled={sourcePath.trim().length === 0 || model.busy !== null}
                  onClick={() => void addByPath()}
                >
                  Add
                </button>
              </div>
            </div>
          </section>
        ) : null}

        {error !== null ? (
          <p className="note note--danger" role="alert">
            {error}
          </p>
        ) : null}
        {model.error !== null ? (
          <p className="note note--danger" role="alert">
            {model.error.message}
          </p>
        ) : null}

        <div className="dialog__actions">
          <div className="spacer" />
          <button type="button" className="btn" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
