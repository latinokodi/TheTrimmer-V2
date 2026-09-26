/**
 * Marking a segment.
 *
 * ## The one field that matters, and why it has help text
 *
 * `--out` is the **last frame kept**, the way Premiere's Out point works. Every other trimming tool
 * treats the out point as exclusive, so this is the single most likely thing to be got wrong — and
 * getting it wrong is off by one frame, which nobody notices until the captions are late. So the
 * field says so next to itself, the parsed frame numbers appear live underneath as they are typed,
 * and the range is validated before the segment is created rather than three processes into a cut.
 */

import { useEffect, useMemo, useState } from "react";

import { IpcFailure, commands } from "../ipc/commands";
import type { SourceView } from "../ipc/types";
import type { PresetViewLite } from "./SegmentDialog.types";

export function SegmentDialog({
  sources,
  presets,
  onClose,
  onSubmit,
}: {
  readonly sources: readonly SourceView[];
  readonly presets: readonly PresetViewLite[];
  readonly onClose: () => void;
  readonly onSubmit: (input: {
    readonly source: string;
    readonly name: string;
    readonly startFrame: number;
    readonly endFrame: number | null;
    readonly preset: string | null;
    readonly handleFrames: number;
  }) => Promise<void>;
}): JSX.Element {
  const usable = useMemo(() => sources.filter((source) => source.present), [sources]);
  const [source, setSource] = useState<string>(usable[0]?.path ?? "");
  const [name, setName] = useState("");
  const [inText, setInText] = useState("");
  const [outText, setOutText] = useState("");
  const [preset, setPreset] = useState<string>("");
  const [handles, setHandles] = useState(0);
  const [inFrame, setInFrame] = useState<number | null>(null);
  const [outFrame, setOutFrame] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  // Parse both marks as they are typed, so the frame numbers are visible before the segment exists.
  // This is the whole reason the domain has a `parse_timecode` command: the interface must not
  // reimplement drop-frame arithmetic, and a second implementation would drift from the real one.
  useEffect(() => {
    let cancelled = false;
    if (source === "" || inText.trim() === "") {
      setInFrame(null);
      return;
    }
    void commands
      .parseTimecode(inText, source)
      .then((parsed) => {
        if (!cancelled) {
          setInFrame(parsed.frame);
          setError(null);
        }
      })
      .catch((caught: unknown) => {
        if (!cancelled) {
          setInFrame(null);
          setError(caught instanceof IpcFailure ? caught.message : String(caught));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [inText, source]);

  useEffect(() => {
    let cancelled = false;
    if (source === "" || outText.trim() === "") {
      setOutFrame(null);
      return;
    }
    void commands
      .parseTimecode(outText, source)
      .then((parsed) => {
        if (!cancelled) {
          // The out point is inclusive, so the exclusive end the domain wants is one past it.
          setOutFrame(parsed.frame + 1);
          setError(null);
        }
      })
      .catch((caught: unknown) => {
        if (!cancelled) {
          setOutFrame(null);
          setError(caught instanceof IpcFailure ? caught.message : String(caught));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [outText, source]);

  const frames =
    inFrame !== null && outFrame !== null ? Math.max(0, outFrame - inFrame) : null;
  const ready =
    source !== "" && inFrame !== null && outFrame !== null && (frames ?? 0) > 0 && !saving;

  async function submit(): Promise<void> {
    if (inFrame === null || outFrame === null) {
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await onSubmit({
        source,
        name: name.trim() === "" ? `Segment at ${inText}` : name.trim(),
        startFrame: inFrame,
        endFrame: outFrame,
        preset: preset === "" ? null : preset,
        handleFrames: handles,
      });
    } catch (caught) {
      setError(caught instanceof IpcFailure ? caught.message : String(caught));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="dialog-layer" role="presentation" onClick={onClose}>
      <div
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="segment-title"
        onClick={(event) => event.stopPropagation()}
      >
        <h2 className="dialog__title" id="segment-title">
          Mark a segment
        </h2>

        {usable.length === 0 ? (
          <p className="empty">
            No source is available. Add one, and make sure it is on disk.
          </p>
        ) : (
          <>
            <div className="field">
              <label className="field__label" htmlFor="segment-source">
                Source
              </label>
              <select
                id="segment-source"
                value={source}
                onChange={(event) => setSource(event.target.value)}
              >
                {usable.map((item) => (
                  <option key={item.path} value={item.path}>
                    {item.name} — {item.summary}
                  </option>
                ))}
              </select>
            </div>

            <div className="field">
              <label className="field__label" htmlFor="segment-name">
                Name
              </label>
              <input
                id="segment-name"
                type="text"
                value={name}
                placeholder="cold open"
                onChange={(event) => setName(event.target.value)}
              />
              <p className="field__help">Used for the output file name and the export metadata.</p>
            </div>

            <div className="field-grid">
              <div className="field">
                <label className="field__label" htmlFor="segment-in">
                  In point
                </label>
                <input
                  id="segment-in"
                  className="timecode-field"
                  type="text"
                  value={inText}
                  placeholder="00:00:10:00"
                  aria-describedby="segment-in-frame"
                  aria-invalid={inText !== "" && inFrame === null}
                  onChange={(event) => setInText(event.target.value)}
                />
                <p className="field__help figures" id="segment-in-frame">
                  {inFrame === null ? "first frame kept" : `frame ${inFrame.toLocaleString()}`}
                </p>
              </div>

              <div className="field">
                <label className="field__label" htmlFor="segment-out">
                  Out point
                </label>
                <input
                  id="segment-out"
                  className="timecode-field"
                  type="text"
                  value={outText}
                  placeholder="00:02:31:12"
                  aria-describedby="segment-out-frame"
                  aria-invalid={outText !== "" && outFrame === null}
                  onChange={(event) => setOutText(event.target.value)}
                />
                <p className="field__help figures" id="segment-out-frame">
                  {outFrame === null
                    ? "last frame kept, as Premiere's Out point works"
                    : `last frame kept is ${(outFrame - 1).toLocaleString()}`}
                </p>
              </div>
            </div>

            <div className="field-grid">
              <div className="field">
                <label className="field__label" htmlFor="segment-preset">
                  Delivery preset
                </label>
                <select
                  id="segment-preset"
                  value={preset}
                  onChange={(event) => setPreset(event.target.value)}
                >
                  <option value="">the project default</option>
                  {presets.map((item) => (
                    <option key={item.name} value={item.name}>
                      {item.name}
                      {item.preservesPicture ? "" : " — re-encodes the picture"}
                    </option>
                  ))}
                </select>
              </div>

              <div className="field">
                <label className="field__label" htmlFor="segment-handles">
                  Handles
                </label>
                <input
                  id="segment-handles"
                  type="number"
                  min={0}
                  step={1}
                  value={handles}
                  onChange={(event) => setHandles(Number(event.target.value))}
                />
                <p className="field__help">
                  Extra frames either side, for a crossfade. Clamped at the ends of the source.
                </p>
              </div>
            </div>

            <dl className="facts facts--inline">
              <dt>Length</dt>
              <dd className="figures">
                {frames === null ? "—" : `${frames.toLocaleString()} frames`}
              </dd>
            </dl>

            {error !== null ? (
              <p className="note note--danger" role="alert">
                {error}
              </p>
            ) : null}

            <div className="dialog__actions">
              <div className="spacer" />
              <button type="button" className="btn" onClick={onClose}>
                Cancel
              </button>
              <button
                type="button"
                className="btn btn--primary"
                disabled={!ready}
                onClick={() => void submit()}
              >
                {saving ? "Adding…" : "Add the segment"}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
