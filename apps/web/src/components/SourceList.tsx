/**
 * The source list: what you have, and whether it is still there.
 *
 * A source that has gone missing is the single most common way a project stops working — an editor
 * opens it on a laptop without the drive, or on a machine that has the share mapped differently —
 * and it is the one the interface must make obvious *before* a batch is started. So a missing source
 * is marked in the list, counted in the status bar, and named when the batch skips it.
 */

import type { SourceView } from "../ipc/types";
import { formatBytes, formatRate } from "../lib/format";

export function SourceList({
  sources,
  onRemove,
  onAdd,
  busy,
}: {
  readonly sources: readonly SourceView[];
  readonly onRemove: (path: string) => void;
  readonly onAdd: () => void;
  readonly busy: boolean;
}): JSX.Element {
  return (
    <aside className="rail" aria-label="Sources">
      <div className="rail__head">
        <span className="label">Sources</span>
        <span className="rail__count figures">{sources.length}</span>
      </div>

      {sources.length === 0 ? (
        <div className="empty empty--rail">
          <p>No sources yet.</p>
          <button type="button" className="btn" onClick={onAdd}>
            Add a master
          </button>
        </div>
      ) : (
        <ul className="rail__list scroll">
          {sources.map((source) => (
            <li key={source.path} className="rail__item">
              <div className="rail__item-head">
                <span
                  className={`status ${source.present ? "status--ok" : "status--danger"}`}
                  title={source.present ? "on disk" : "not on disk right now"}
                >
                  {source.present ? "present" : "missing"}
                </span>
                <button
                  type="button"
                  className="btn btn--ghost btn--icon rail__remove"
                  onClick={() => onRemove(source.path)}
                  disabled={busy}
                  aria-label={`Remove ${source.name} and the segments that cut it`}
                  title="Remove this source and every segment that cut it"
                >
                  ×
                </button>
              </div>

              <p className="rail__name truncate" title={source.path}>
                {source.name}
              </p>

              {source.media === null ? (
                <p className="rail__meta muted">{source.summary}</p>
              ) : (
                <>
                  <p className="rail__meta figures">
                    {source.media.width}×{source.media.height} · {source.media.codec} ·{" "}
                    {formatRate(
                      source.media.rate.num,
                      source.media.rate.den,
                    )}{" "}
                    fps
                  </p>
                  <p className="rail__meta figures">
                    {source.media.frameCount.toLocaleString()} frames ·{" "}
                    {formatBytes(source.media.sizeBytes)}
                  </p>
                </>
              )}

              {source.variableRate ? (
                <p className="rail__flag rail__flag--warn">
                  <span className="status status--warn">variable rate</span>
                  <span className="rail__flag-note">
                    the frame grid may not hold, so marks are approximate
                  </span>
                </p>
              ) : null}

              {source.transcript !== null ? (
                <p className="rail__flag">
                  <span className="status status--info">transcript</span>
                  <span className="rail__flag-note figures">
                    {source.transcriptCues ?? "?"} cues
                  </span>
                </p>
              ) : (
                <p className="rail__flag rail__flag--quiet">
                  <span className="status status--idle">no transcript</span>
                  <span className="rail__flag-note">cut by timecode only</span>
                </p>
              )}
            </li>
          ))}
        </ul>
      )}
    </aside>
  );
}
