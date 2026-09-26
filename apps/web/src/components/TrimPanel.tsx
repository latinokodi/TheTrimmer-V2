/**
 * Trimming a segment: the whole product on one screen.
 *
 * ## The shape of it
 *
 * ```
 * MASTER   [ Master.mp4 — 640×360, 25 fps, 125 frames, transcript   ▾ ]
 *
 * IN       [ 00:00:01:00 ]      frame 25        ─ first frame kept
 * OUT      [ 00:00:02:00 ]      frame 50        ─ last frame kept
 *
 * 26 frames · 1.04 s · lossless copy                    [ Trim this segment ]
 * ```
 *
 * Four things, top to bottom, in the order a person works: what to cut, where to cut it, what that
 * means, and the button. Nothing is behind a dialog, nothing is in a tab, and there is no state in
 * which the fields are missing — which is the difference between this and the version of the shell
 * that put the marks in a modal behind a "Mark a segment" button.
 *
 * ## The two lines under the fields are the point
 *
 * The frame number under each field is what turns a typo into something visible. It comes from the
 * domain's own `parse_timecode` — Rust, including drop-frame — so the number the interface shows is
 * the number the cutter will use, and not a second implementation's opinion.
 *
 * ## What the button says
 *
 * `Trim this segment` when there is no queue, `Queue it` when there is already something waiting and
 * this is clearly a second segment. The count of what is queued sits beside it, so "the button does
 * the batch" is never a surprise.
 */

import type { SegmentDraft } from "../state/useSegmentDraft";
import type { PresetWire, QueuePreview, SourceView } from "../ipc/types";
import { formatDuration } from "../lib/format";

export function TrimPanel({
  draft,
  sources,
  presets,
  preview,
  queued,
  busy,
  onQueue,
  onTrimNow,
  onClearQueue,
  onAddMaster,
  showQueueHint,
}: {
  readonly draft: SegmentDraft;
  readonly sources: readonly SourceView[];
  readonly presets: readonly PresetWire[];
  readonly preview: QueuePreview | null;
  readonly queued: number;
  readonly busy: boolean;
  readonly onQueue: () => void;
  readonly onTrimNow: () => void;
  readonly onClearQueue: () => void;
  readonly onAddMaster: () => void;
  readonly showQueueHint: boolean;
}): JSX.Element {
  const present = sources.filter((source) => source.present);

  if (present.length === 0) {
    return (
      <section className="trim" aria-label="Trim a segment">
        <div className="empty empty--trim">
          <p className="empty__title">
            {sources.length === 0 ? "No video yet" : "The video is not on disk"}
          </p>
          <p>
            {sources.length === 0
              ? "Choose the file to trim. Its frame rate is read first, because that is what decides what a timecode means."
              : "TheTrimmer remembers where a file was. Reconnect the drive, or add the file again from its new location."}
          </p>
          <button type="button" className="btn btn--primary" onClick={onAddMaster} disabled={busy}>
            {sources.length === 0 ? "Choose a video…" : "Choose it again…"}
          </button>
        </div>
      </section>
    );
  }

  const chosen = present.find((source) => source.path === draft.source) ?? present[0];
  if (chosen === undefined) {
    // Unreachable — `present.length` was checked above — but the index signature is not narrowed by
    // that check, and a `!` here would hide a real regression rather than one that cannot happen.
    return <section className="trim" aria-label="Trim a segment" />;
  }
  const media = chosen.media;
  const mode = preview?.plan?.mode ?? null;
  const rate = media === null ? null : media.rate.num / media.rate.den;
  // The range as seconds: frames over the frame rate. `null` until both marks parse.
  const seconds =
    draft.frames === null || rate === null ? null : draft.frames / rate;

  return (
    <section className="trim" aria-label="Trim a segment">
      <div className="trim__master">
        <span className="field__label" id="trim-master-label">
          Video
        </span>
        {present.length === 1 ? (
          <p className="trim__master-name truncate" title={chosen.path}>
            {chosen.name}
          </p>
        ) : (
          <select
            aria-labelledby="trim-master-label"
            value={draft.source}
            onChange={(event) => draft.setSource(event.target.value)}
          >
            {present.map((source) => (
              <option key={source.path} value={source.path}>
                {source.name}
              </option>
            ))}
          </select>
        )}
        <p className="trim__master-facts figures">
          {media === null
            ? chosen.summary
            : `${media.width}×${media.height} · ${media.codec} · ${rate?.toFixed(rate % 1 === 0 ? 0 : 3)} fps · ${media.frameCount.toLocaleString()} frames · ${chosen.transcript === null ? "no transcript" : `${chosen.transcriptCues ?? "?"} caption cues`}`}
        </p>
      </div>

      <div className="trim__marks">
        <Mark
          id="trim-in"
          label="In point"
          value={draft.inText}
          placeholder="00:00:00:00"
          help={
            draft.inFrame === null
              ? "the first frame kept"
              : `frame ${draft.inFrame.toLocaleString()}`
          }
          invalid={draft.inText !== "" && draft.inFrame === null}
          onChange={draft.setInText}
        />
        <Mark
          id="trim-out"
          label="Out point"
          value={draft.outText}
          placeholder="00:00:10:00"
          help={
            draft.outFrame === null
              ? "the last frame kept, as Premiere's Out point works"
              : `frame ${draft.outFrame.toLocaleString()}, the last one kept`
          }
          invalid={draft.outText !== "" && draft.outFrame === null}
          onChange={draft.setOutText}
        />
      </div>

      <div className="trim__summary">
        {draft.error !== null ? (
          <p className="note note--danger" role="alert">
            {draft.error}
          </p>
        ) : draft.frames === null ? (
          <p className="trim__hint">
            Type both timecodes. The frame numbers appear here as you type, from the domain's own
            arithmetic — including drop-frame.
          </p>
        ) : (
          <p className="trim__length">
            <span className="figures">{draft.frames.toLocaleString()} frames</span>
            <span className="trim__dot" aria-hidden="true" />
            <span className="figures">
              {seconds === null ? "—" : formatDuration(seconds)}
            </span>
            {mode !== null ? (
              <>
                <span className="trim__dot" aria-hidden="true" />
                <span className={mode === "copy" ? "ok" : "warn"}>
                  {mode === "copy"
                    ? "lossless copy — every frame is the original"
                    : mode === "headPatch"
                      ? `head patch — ${preview?.plan?.headFrames ?? 0} frames re-encoded`
                      : "full re-encode — no keyframe inside this range"}
                </span>
              </>
            ) : null}
          </p>
        )}

        <div className="trim__buttons">
          {showQueueHint ? (
            <span className="trim__queue-hint figures">
              {queued} queued
              <button
                type="button"
                className="btn btn--ghost btn--small"
                onClick={onClearQueue}
                disabled={busy}
              >
                clear
              </button>
            </span>
          ) : null}
          <button
            type="button"
            className="btn"
            disabled={!draft.ready || busy}
            onClick={onQueue}
            title="Add this range to the queue without trimming it yet"
          >
            Queue it
          </button>
          <button
            type="button"
            className="btn btn--primary btn--big"
            disabled={!draft.ready || busy}
            onClick={onTrimNow}
            title="Cut this segment, then check the file against the source (Ctrl+Enter)"
          >
            {busy ? "Working…" : queued > 0 ? "Trim the queue" : "Trim this segment"}
          </button>
        </div>
      </div>

      {/*
        Delivery: the two settings that change what comes out, folded away because they do not change
        what is cut. The project default is right for almost every cut, and a select and a number
        field sitting next to the marks would imply they need a decision.
      */}
      <details className="trim__delivery">
        <summary className="trim__delivery-head">
          <span className="field__label">Delivery</span>
          <span className="trim__delivery-value">
            {draft.preset === "" ? "the project default" : draft.preset}
            {draft.handles > 0 ? ` · ${draft.handles} frame handles` : ""}
          </span>
        </summary>
        <div className="field-grid">
          <div className="field">
            <label className="field__label" htmlFor="trim-preset">
              Preset
            </label>
            <select
              id="trim-preset"
              value={draft.preset}
              onChange={(event) => draft.setPreset(event.target.value)}
            >
              <option value="">the project default</option>
              {presets.map((item) => (
                <option key={item.name} value={item.name}>
                  {item.name}
                  {item.preservesPicture ? "" : " — re-encodes the picture"}
                </option>
              ))}
            </select>
            <p className="field__help">
              A preset that reshapes the picture or changes the loudness turns the whole segment into
              a re-encode, and the length line above says so before you press the button.
            </p>
          </div>
          <div className="field">
            <label className="field__label" htmlFor="trim-handles">
              Handles
            </label>
            <input
              id="trim-handles"
              type="number"
              min={0}
              step={1}
              value={draft.handles}
              onChange={(event) => draft.setHandles(Math.max(0, Number(event.target.value) || 0))}
            />
            <p className="field__help">
              Extra frames kept either side of the marks, for a crossfade. Clamped at the ends of the
              source.
            </p>
          </div>
        </div>
      </details>
    </section>
  );
}

/**
 * One mark.
 *
 * The label, the field and the sentence under it are wired together with `aria-describedby`, so a
 * screen reader reads "Out point, edit, frame 50, the last one kept" rather than a bare edit box in a
 * form of six.
 */
function Mark({
  id,
  label,
  value,
  placeholder,
  help,
  invalid,
  onChange,
}: {
  readonly id: string;
  readonly label: string;
  readonly value: string;
  readonly placeholder: string;
  readonly help: string;
  readonly invalid: boolean;
  readonly onChange: (value: string) => void;
}): JSX.Element {
  return (
    <div className="trim__mark">
      <label className="field__label" htmlFor={id}>
        {label}
      </label>
      <input
        id={id}
        className="timecode-field"
        type="text"
        inputMode="numeric"
        autoComplete="off"
        spellCheck={false}
        value={value}
        placeholder={placeholder}
        aria-describedby={`${id}-help`}
        aria-invalid={invalid}
        onChange={(event) => onChange(event.target.value)}
      />
      <p className="field__help figures" id={`${id}-help`}>
        {help}
      </p>
    </div>
  );
}
