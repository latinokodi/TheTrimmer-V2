/**
 * The queue: one row per marked range.
 *
 * ## Columns
 *
 * The four that decide anything, and nothing else. A name to recognise it by, where it starts and
 * ends, how long it is, and what will happen to it — a copy, a head patch, or a full re-encode. The
 * source column is gone because this window holds one video at a time and a column whose every cell
 * says the same file name is a column that costs width and says nothing.
 *
 * ## Fixed row height
 *
 * `--row-height` is fixed, and that is functional rather than stylistic: a plan arrives
 * asynchronously and fills in the last column, and a row that grew when its plan landed would move
 * the table under the pointer mid-click.
 *
 * ## The enabled checkbox
 *
 * A batch runs what is enabled, so the checkbox is the control that decides whether a range is part
 * of the deliverable. It is a real checkbox with a real label, so it works by keyboard, by screen
 * reader and by click without any of the three being a special case.
 */

import type { QueuePreview, SegmentView } from "../ipc/types";
import { formatDuration, formatPercent } from "../lib/format";

export function CutTable({
  segments,
  selected,
  onSelect,
  onToggleEnabled,
  onRemove,
  previews,
  busy,
}: {
  readonly segments: readonly SegmentView[];
  readonly selected: string | null;
  readonly onSelect: (id: string | null) => void;
  readonly onToggleEnabled: (id: string, enabled: boolean) => void;
  readonly onRemove: (id: string) => void;
  readonly previews: readonly QueuePreview[];
  readonly busy: boolean;
}): JSX.Element {
  const planned = new Map(previews.map((preview) => [preview.segment, preview]));

  if (segments.length === 0) {
    return (
      <p className="empty-state">
        Type an in point and an out point, then press <strong>Queue it</strong>. Every range you queue
        is trimmed in one run, and every finished file is measured against the source.
      </p>
    );
  }

  return (
    <div className="cut-table" role="grid" aria-label="Queued segments" aria-rowcount={segments.length}>
      <div className="cut-table__head" role="row">
        <span className="cut-table__cell cut-table__cell--check" role="columnheader">
          <span className="sr-only">Included in the run</span>
        </span>
        <span className="cut-table__cell cut-table__cell--index" role="columnheader">
          <span className="sr-only">Position</span>
        </span>
        <span className="cut-table__cell cut-table__cell--name" role="columnheader">
          Segment
        </span>
        <span className="cut-table__cell cut-table__cell--tc" role="columnheader">
          In
        </span>
        <span className="cut-table__cell cut-table__cell--tc" role="columnheader">
          Out
        </span>
        <span className="cut-table__cell cut-table__cell--num" role="columnheader">
          Frames
        </span>
        <span className="cut-table__cell cut-table__cell--mode" role="columnheader">
          Plan
        </span>
        <span className="cut-table__cell cut-table__cell--actions" role="columnheader">
          <span className="sr-only">Remove</span>
        </span>
      </div>

      <div className="cut-table__body">
        {segments.map((segment, index) => {
          const isSelected = segment.id === selected;
          const preview = planned.get(segment.id);
          const blocked = segment.problems.length > 0;
          const plan = preview?.plan ?? null;

          return (
            <div
              key={segment.id}
              role="row"
              aria-rowindex={index + 1}
              aria-selected={isSelected}
              className={[
                "cut-table__row",
                isSelected ? "cut-table__row--selected" : "",
                blocked ? "cut-table__row--blocked" : "",
                segment.enabled ? "" : "cut-table__row--disabled",
              ]
                .filter(Boolean)
                .join(" ")}
              onClick={() => onSelect(isSelected ? null : segment.id)}
            >
              <span className="cut-table__cell cut-table__cell--check" role="gridcell">
                <input
                  type="checkbox"
                  checked={segment.enabled}
                  disabled={busy}
                  aria-label={`Include ${segment.name} in the run`}
                  onChange={(event) => onToggleEnabled(segment.id, event.target.checked)}
                  onClick={(event) => event.stopPropagation()}
                />
              </span>

              <span className="cut-table__cell cut-table__cell--index figures" role="gridcell">
                {index + 1}
              </span>

              <span className="cut-table__cell cut-table__cell--name" role="gridcell">
                <span className="cut-table__name truncate" title={segment.name}>
                  {segment.name}
                </span>
                {blocked ? (
                  <span className="cut-table__problem" title={segment.problems.join("; ")}>
                    <span className="status status--danger">cannot cut</span>
                  </span>
                ) : segment.notes.length > 0 ? (
                  <span className="cut-table__problem" title={segment.notes.join("; ")}>
                    <span className="status status--warn">note</span>
                  </span>
                ) : null}
              </span>

              <span className="cut-table__cell cut-table__cell--tc figures" role="gridcell">
                {segment.inTimecode}
              </span>

              <span className="cut-table__cell cut-table__cell--tc figures" role="gridcell">
                {segment.outTimecode}
              </span>

              <span
                className="cut-table__cell cut-table__cell--num figures"
                role="gridcell"
                title={segment.seconds === null ? undefined : formatDuration(segment.seconds)}
              >
                {segment.frames?.toLocaleString() ?? "—"}
              </span>

              <span className="cut-table__cell cut-table__cell--mode" role="gridcell">
                {plan === null ? (
                  <span className="status status--idle">not planned</span>
                ) : plan.mode === "copy" ? (
                  <span className="badge badge--ok" title="every frame is the original">
                    copy
                  </span>
                ) : plan.mode === "headPatch" ? (
                  <span
                    className="badge badge--head"
                    title={`${plan.headFrames} frames re-encoded, ${plan.bodyFrames} copied`}
                  >
                    head {formatPercent(preview?.reencodeFraction ?? 0)}
                  </span>
                ) : (
                  <span className="badge badge--warn" title="no keyframe falls inside the segment">
                    re-encode
                  </span>
                )}
              </span>

              <span className="cut-table__cell cut-table__cell--actions" role="gridcell">
                <button
                  type="button"
                  className="btn btn--ghost btn--icon"
                  disabled={busy}
                  aria-label={`Remove ${segment.name}`}
                  title="Remove this range from the queue"
                  onClick={(event) => {
                    event.stopPropagation();
                    onRemove(segment.id);
                  }}
                >
                  ×
                </button>
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
