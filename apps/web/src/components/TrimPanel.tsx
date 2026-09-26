/**
 * Trimming: the whole product on one dense screen.
 *
 * ## The layout, and why it is this compact
 *
 * ```
 * ┌ MASTER ────────────────────────────────────────────────┬ QUEUE ─── 2 ────────┐
 * │ A007C012_250312_R1QK.mov                        [Change]│ #  NAME  IN  OUT  PLAN │
 * │ 3840×2160 · prores · 25 fps · 3,101 frames · 214 cues  │ 1  cold open  00:00:01:00 … │
 * ├────────────────────────────────────────────────────────┤ 2  exchange   00:01:12:04 … │
 * │ IN [00:00:01:00]  OUT [00:00:02:00]  PRESET [master ▾] │                    │
 * │    frame 25          frame 50          HANDLES [ 0 ]   │                    │
 * │                    VERIFY [strict ▾]                   │                    │
 * │ 26 frames · 1.04 s · lossless copy      [Queue it] [Trim]│                   │
 * ├────────────────────────────────────────────────────────┴────────────────────┤
 * │ FIND IN THE TRANSCRIPT [……………………………]  3 hits                              │
 * └─────────────────────────────────────────────────────────────────────────────┘
 * ```
 *
 * Nothing is folded away. An earlier version put the preset, the handles and the verification policy
 * behind three collapsed sections and left a column of empty space down the middle of a 1440 px
 * window; a professional tool that hides its own options behind three clicks has confused *tidy* with
 * *empty*. Every setting is now on one line with its current value visible, which is also what makes
 * the panel scannable: you read down the label column and the value column rather than opening things
 * to find out what they were set to.
 *
 * No card boxes inside the panel either. Grouping is done with a hairline and a label, because a box
 * around a two-field form in a dense window is a border that says nothing the label did not.
 *
 * ## The marks are the point
 *
 * The two timecode fields are the largest thing on the screen and they are planned as they are typed,
 * so the length line says "lossless copy" or "head patch, 13 frames re-encoded" while the decision can
 * still change. Behind a button, that answer arrives after the decision has been made.
 */

import type { SegmentDraft } from "../state/useSegmentDraft";
import type { PresetWire, QueuePreview, SourceView } from "../ipc/types";
import { formatDuration } from "../lib/format";

/** The verification policies, and what each one actually does. */
const POLICIES = [
  { id: "off", label: "Off", note: "do not measure the result" },
  { id: "standard", label: "Standard", note: "frame count, duration, audio alignment" },
  { id: "strict", label: "Strict", note: "also hash decoded frames at sample points" },
  { id: "forensic", label: "Forensic", note: "also compare the head pixel-wise, cue by cue" },
] as const;

export function TrimPanel({
  draft,
  sources,
  presets,
  preview,
  policy,
  queued,
  busy,
  onQueue,
  onTrimNow,
  onChooseMaster,
  onPolicy,
}: {
  readonly draft: SegmentDraft;
  readonly sources: readonly SourceView[];
  readonly presets: readonly PresetWire[];
  readonly preview: QueuePreview | null;
  readonly policy: string;
  readonly queued: number;
  readonly busy: boolean;
  readonly onQueue: () => void;
  readonly onTrimNow: () => void;
  readonly onChooseMaster: () => void;
  readonly onPolicy: (value: string) => void;
}): JSX.Element {
  const present = sources.filter((source) => source.present);
  const chosen = present.find((source) => source.path === draft.source) ?? present[0];

  if (chosen === undefined) {
    return (
      <div className="trim trim--empty">
        <div className="empty">
          <p className="empty__title">
            {sources.length === 0 ? "Choose a video to trim" : "That video is not on disk"}
          </p>
          <p>
            {sources.length === 0
              ? "Its frame rate is read first, because that is what decides what a timecode means. A caption file named after it is picked up automatically."
              : "TheTrimmer remembers where a file was. Reconnect the drive, or choose the file again from its new location."}
          </p>
          <button type="button" className="btn btn--primary" onClick={onChooseMaster} disabled={busy}>
            {sources.length === 0 ? "Choose a video…" : "Choose it again…"}
          </button>
        </div>
      </div>
    );
  }

  const media = chosen.media;
  const rate = media === null ? null : media.rate.num / media.rate.den;
  const seconds = draft.frames === null || rate === null ? null : draft.frames / rate;
  const mode = preview?.plan?.mode ?? null;

  return (
    <div className="trim">
      {/* ---- the master, one line, with the facts that matter on the row under it ---------- */}
      <div className="trim__master">
        <div className="trim__master-row">
          <span className="field__label">Video</span>
          {present.length === 1 ? (
            <span className="trim__master-name truncate" title={chosen.path}>
              {chosen.name}
            </span>
          ) : (
            <select
              className="trim__master-pick"
              value={draft.source}
              onChange={(event) => draft.setSource(event.target.value)}
              aria-label="Video"
            >
              {present.map((source) => (
                <option key={source.path} value={source.path}>
                  {source.name}
                </option>
              ))}
            </select>
          )}
          <span className="spacer" />
          <button type="button" className="btn btn--small" onClick={onChooseMaster} disabled={busy}>
            Change…
          </button>
        </div>
        <p className="trim__master-facts figures">
          {media === null
            ? chosen.summary
            : `${media.width}×${media.height} · ${media.codec} · ${rate?.toFixed(rate % 1 === 0 ? 0 : 3)} fps · ${media.frameCount.toLocaleString()} frames · ${chosen.transcript === null ? "no transcript" : `${chosen.transcriptCues ?? "?"} caption cues`}`}
        </p>
      </div>

      {/* ---- the marks and every setting, one row each ------------------------------------- */}
      <div className="trim__form">
        <div className="trim__marks">
          <Mark
            id="trim-in"
            label="In point"
            value={draft.inText}
            placeholder="00:00:00:00"
            help={draft.inFrame === null ? "first frame kept" : `frame ${draft.inFrame.toLocaleString()}`}
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
                ? "last frame kept, as Premiere's Out point works"
                : `frame ${draft.outFrame.toLocaleString()}, the last one kept`
            }
            invalid={draft.outText !== "" && draft.outFrame === null}
            onChange={draft.setOutText}
          />
        </div>

        <div className="trim__settings">
          <div className="field">
            <label className="field__label" htmlFor="trim-preset">
              Delivery
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
          </div>

          <div className="field field--narrow">
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
          </div>

          <div className="field">
            <label className="field__label" htmlFor="trim-policy">
              Verify
            </label>
            <select
              id="trim-policy"
              value={policy}
              onChange={(event) => onPolicy(event.target.value)}
            >
              {POLICIES.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.label} — {item.note}
                </option>
              ))}
            </select>
          </div>
        </div>
      </div>

      {/* ---- what it will do, and the two buttons ------------------------------------------ */}
      <div className="trim__go">
        <p className="trim__length" aria-live="polite">
          {draft.error !== null ? (
            <span className="danger">{draft.error}</span>
          ) : draft.frames === null ? (
            <span className="trim__hint">
              Type both timecodes. The frame numbers appear here as you type, from the domain&rsquo;s
              own arithmetic — including drop-frame.
            </span>
          ) : (
            <>
              <span className="figures">{draft.frames.toLocaleString()} frames</span>
              <span className="trim__dot" aria-hidden="true" />
              <span className="figures">{seconds === null ? "—" : formatDuration(seconds)}</span>
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
            </>
          )}
        </p>
        <button
          type="button"
          className="btn"
          disabled={!draft.ready || busy}
          onClick={onQueue}
          title="Add this range to the queue without trimming it yet"
        >
          Queue it
        </button>
        {/*
          Enabled whenever the *queue* has something in it, not only when a range is marked. The first
          version of this button required valid marks, which meant a queue of nine segments could not be
          trimmed until a tenth range had been typed into the fields — the button was gated on the wrong
          thing, and the fix is to ask whether there is anything to run rather than whether there is
          anything to add.
        */}
        <button
          type="button"
          className="btn btn--primary btn--big"
          disabled={busy || (queued === 0 && !draft.ready)}
          onClick={onTrimNow}
          title={
            queued > 0
              ? `Cut the ${queued} queued segment(s), then check every finished file (Ctrl+Enter)`
              : "Cut the marked range, then check the finished file (Ctrl+Enter)"
          }
        >
          {busy ? "Working…" : queued > 0 ? `Trim ${queued}` : "Trim"}
        </button>
      </div>
    </div>
  );
}

/**
 * One mark.
 *
 * The label, the field and the sentence under it are wired together with `aria-describedby`, so a
 * screen reader reads "Out point, edit, frame 50, the last one kept" rather than a bare box in a form
 * of six.
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
