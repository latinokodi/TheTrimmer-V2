/**
 * The export dialog.
 *
 * Four formats, one question each: where should it go, and what should the sequence be called. The
 * format is picked from a list that says what each one is *for* rather than only naming it — an
 * editor reaching for "EDL" and an assistant reaching for "CSV" want different things, and a list of
 * four acronyms does not help either of them.
 *
 * The path has a sensible default, because the store already knows where the project's outputs go
 * and an empty field that has to be filled in before anything can happen is a wasted step.
 */

import { useState } from "react";

import { pickSave } from "../ipc/dialog";
import { useModal } from "../state/useModal";

/** The formats, and the sentence that says who wants each one. */
const FORMATS = [
  {
    id: "premiere",
    label: "Premiere Pro XML",
    note: "a sequence you can open and keep cutting in",
    extension: "xml",
  },
  {
    id: "fcpxml",
    label: "Final Cut Pro XML",
    note: "FCPXML 1.11, for Final Cut Pro and Resolve",
    extension: "fcpxml",
  },
  {
    id: "edl",
    label: "CMX3600 EDL",
    note: "the lowest common denominator; every suite reads it",
    extension: "edl",
  },
  {
    id: "csv",
    label: "CSV",
    note: "a spreadsheet of the timecodes, for a shot log or a review",
    extension: "csv",
  },
] as const;

export function ExportDialog({
  projectName,
  onClose,
  onExport,
}: {
  readonly projectName: string;
  readonly onClose: () => void;
  readonly onExport: (input: {
    readonly format: "premiere" | "fcpxml" | "edl" | "csv";
    readonly path: string;
    readonly sequenceName: string;
  }) => Promise<readonly string[] | null>;
}): JSX.Element {
  const [format, setFormat] = useState<(typeof FORMATS)[number]["id"]>("premiere");
  const [sequenceName, setSequenceName] = useState(projectName);
  const [path, setPath] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [warnings, setWarnings] = useState<readonly string[]>([]);
  const [saving, setSaving] = useState(false);
  // Focus in, focus trapped, focus returned, Escape closes. See `state/useModal.ts`.
  const dialogRef = useModal<HTMLDivElement>(onClose);

  const chosen = FORMATS.find((entry) => entry.id === format) ?? FORMATS[0];
  const ready = path.trim().length > 0 && sequenceName.trim().length > 0 && !saving;

  async function browse(): Promise<void> {
    const picked = await pickSave({
      title: `Write the ${chosen.label}`,
      defaultPath: `${sequenceName.trim()}.${chosen.extension}`,
      filters: [{ name: chosen.label, extensions: [chosen.extension] }],
    });
    if (picked.kind === "picked") {
      setError(null);
      setPath(picked.path);
    } else if (picked.kind === "unavailable") {
      setError(picked.reason);
    }
  }

  async function submit(): Promise<void> {
    setSaving(true);
    setError(null);
    try {
      const reported = await onExport({
        format,
        path: path.trim(),
        sequenceName: sequenceName.trim(),
      });
      setWarnings(reported ?? []);
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
        aria-labelledby="export-title"
        tabIndex={-1}
        ref={dialogRef}
      >
        <h2 className="dialog__head" id="export-title">
          Export the timeline
        </h2>
        <div className="dialog__body">
        <p className="dialog__help">
          The segments, as a timeline another application can open. The files this project has already
          cut are not touched.
        </p>

        <fieldset className="choice">
          <legend className="field__label">Format</legend>
          {FORMATS.map((entry) => (
            <label key={entry.id} className="choice__option">
              <input
                type="radio"
                name="export-format"
                value={entry.id}
                checked={format === entry.id}
                onChange={() => setFormat(entry.id)}
              />
              <span className="choice__label">{entry.label}</span>
              <span className="choice__note">{entry.note}</span>
            </label>
          ))}
        </fieldset>

        <div className="field">
          <label className="field__label" htmlFor="export-sequence">
            Sequence name
          </label>
          <input
            id="export-sequence"
            type="text"
            value={sequenceName}
            onChange={(event) => setSequenceName(event.target.value)}
          />
        </div>

        <div className="field">
          <label className="field__label" htmlFor="export-path">
            Write it to
          </label>
          <div className="row">
            <input
              id="export-path"
              type="text"
              value={path}
              placeholder={`${sequenceName.trim() || "timeline"}.${chosen.extension}`}
              spellCheck={false}
              onChange={(event) => setPath(event.target.value)}
            />
            <button type="button" className="btn" onClick={() => void browse()}>
              Browse…
            </button>
          </div>
          <p className="field__help">
            The full path, including the file name. A path typed here works whether or not the file
            picker is available.
          </p>
        </div>

        {warnings.length > 0 ? (
          <div className="note note--warn" role="status">
            <p>Written, with {warnings.length} thing(s) worth knowing:</p>
            <ul className="note__list">
              {warnings.map((warning) => (
                <li key={warning}>{warning}</li>
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
            Close
          </button>
          <button
            type="button"
            className="btn btn--primary"
            disabled={!ready}
            onClick={() => void submit()}
          >
            {saving ? "Writing…" : "Write it"}
          </button>
        </div>
      </div>
    </div>
  );
}
