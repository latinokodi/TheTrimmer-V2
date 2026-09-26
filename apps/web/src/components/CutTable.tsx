/**
 * The cut table: the workspace's centre.
 *
 * ## Why a real table and not a list of divs
 *
 * A screen reader announcing a cut needs to say which column it is in. `role="grid"` with real
 * `role="row"`, `role="columnheader"` and `role="gridcell"` is what makes "out timecode, 00:01:12;04"
 * comprehensible rather than a stream of numbers. It also gives the browser arrow-key semantics for
 * free, which is why this table needs no keyboard handler at all beyond the row's own selection.
 *
 * ## Why the rows are a fixed height
 *
 * A plan arrives asynchronously and fills in a cost column. If a row grew when its plan landed, the
 * table would move under the pointer mid-click. Every row is `--row-height` tall and the cost is
 * rendered inside it.
 *
 * ## The enabled checkbox
 *
 * A batch runs what is enabled, so the checkbox is the control that decides whether a segment is
 * part of the deliverable. It is a real checkbox with a real label, so it works by keyboard, by
 * screen reader and by click without any of the three being a special case.
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
  onAdd,
  busy,
}: {
  readonly segments: readonly SegmentView[];
  readonly selected: string | null;
  readonly onSelect: (id: string | null) => void;
  readonly onToggleEnabled: (id: string, enabled: boolean) => void;
  readonly onRemove: (id: string) => void;
  readonly previews: readonly QueuePreview[];
  readonly onAdd: () => void;
  readonly busy: boolean;
}): JSX.Element {
  const planned = new Map(previews.map((preview) => [preview.segment, preview]));

  if (segments.length === 0) {
    return (
      <div className="cut-table cut-table--empty">
        <div className="empty">
          <p className="empty__title">No segments yet</p>
          <p>
            A segment is a range in a source: two Premiere timecodes, and a name. The in point is the
            first frame kept; the out point is the last frame kept, the way Premiere&rsquo;s Out
            point works.
          </p>
          <button type="button" className="btn btn--primary" onClick={onAdd} disabled={busy}>
            Mark the first segment
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="cut-table" role="grid" aria-label="Segments" aria-rowcount={segments.length}>
      <div className="cut-table__head" role="row">
        <span className="cut-table__cell cut-table__cell--check" role="columnheader">
          <span className="sr-only">Included in the batch</span>
        </span>
        <span className="cut-table__cell cut-table__cell--index" role="columnheader">
          <span className="sr-only">Position</span>
        </span>
        <span className="cut-table__cell cut-table__cell--name" role="columnheader">
          Segment
        </span>
        <span className="cut-table__cell cut-table__cell--source" role="columnheader">
          Source
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
        <span className="cut-table__cell cut-table__cell--num" role="columnheader">
          Length
        </span>
        <span className="cut-table__cell cut-table__cell--mode" role="columnheader">
          Plan
        </span>
        <span className="cut-table__cell cut-table__cell--actions" role="columnheader">
          <span className="sr-only">Actions</span>
        </span>
      </div>

      <div className="cut-table__body scroll">
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
                  aria-label={`Include ${segment.name} in the batch`}
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

              <span className="cut-table__cell cut-table__cell--source truncate" role="gridcell">
                {segment.sourceName}
              </span>

              <span className="cut-table__cell cut-table__cell--tc figures" role="gridcell">
                {segment.inTimecode}
              </span>

              <span className="cut-table__cell cut-table__cell--tc figures" role="gridcell">
                {segment.outTimecode}
              </span>

              <span className="cut-table__cell cut-table__cell--num figures" role="gridcell">
                {segment.frames?.toLocaleString() ?? "—"}
              </span>

              <span className="cut-table__cell cut-table__cell--num figures" role="gridcell">
                {segment.seconds === null ? "—" : formatDuration(segment.seconds)}
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
                  title="Remove this segment from the project"
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
