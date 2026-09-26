/**
 * The proof panel: what happened, segment by segment.
 *
 * This is the panel a studio pays for. The question after a deliverable goes out is never "did it
 * run" — it is "prove the frames did not shift", and the answer has to be per segment and specific.
 *
 * ## How a verdict is shown
 *
 * Three levels, and the interface must never collapse them into "failed":
 *
 * * **Written and verified** — every check passed.
 * * **Written but not certified** — the file is on disk and a check failed or could not run. The
 *   file is not necessarily wrong, and saying so would be a lie in the other direction; what is true
 *   is that this run cannot vouch for it.
 * * **Not written** — failed, cancelled or skipped, each with the sentence that says which.
 *
 * A check that was *skipped* is shown as skipped rather than as passed. That distinction is the
 * whole value of the panel: "we did not look" and "we looked and it was right" must not look alike.
 */

import { useState } from "react";

import type { BatchOutcomeWire, CheckResultWire, JobStatus } from "../ipc/types";
import type { AppModel } from "../state/useAppModel";
import { formatBytes, formatDuration } from "../lib/format";

export function ProofPanel({
  outcome,
  model,
}: {
  readonly outcome: BatchOutcomeWire | null;
  readonly model: AppModel;
}): JSX.Element {
  const [expanded, setExpanded] = useState<number | null>(null);

  if (outcome === null) {
    return (
      <div className="panel-body">
        <p className="empty-state">
          Nothing has been run yet. Plan the batch to see what it will do, then run it; every segment
          is reported here with the checks that were made against it.
        </p>
        {model.busy !== null ? (
          <p className="note">
            <span className="status status--info">{model.busy}</span>
          </p>
        ) : null}
      </div>
    );
  }

  const succeeded = outcome.jobs.filter((job) => job.status.kind === "succeeded").length;
  const unverified = outcome.jobs.filter((job) => job.status.kind === "unverified").length;
  const failed = outcome.jobs.filter((job) => job.status.kind === "failed").length;
  const skipped = outcome.jobs.filter((job) => job.status.kind === "skipped").length;

  return (
    <div className="panel-body">
      <dl className="facts">
        <dt>Segments</dt>
        <dd className="figures numeric-column">{outcome.jobs.length}</dd>
        <dt>Verified</dt>
        <dd className="figures numeric-column ok">{succeeded}</dd>
        <dt>Uncertified</dt>
        <dd className={`figures numeric-column${unverified > 0 ? " warn" : ""}`}>{unverified}</dd>
        <dt>Failed</dt>
        <dd className={`figures numeric-column${failed > 0 ? " danger" : ""}`}>{failed}</dd>
        <dt>Skipped</dt>
        <dd className="figures numeric-column">{skipped}</dd>
        <dt>Delivered</dt>
        <dd className="figures numeric-column">
          {outcome.deliveredFrames.toLocaleString()} frames
        </dd>
        <dt>Took</dt>
        <dd className="figures numeric-column">{formatDuration(outcome.elapsedSeconds)}</dd>
      </dl>

      {outcome.cancelled ? (
        <p className="note note--warn" role="status">
          The run was cancelled. The segments below were completed before it stopped; the rest were
          not attempted.
        </p>
      ) : null}

      {outcome.digest.length > 0 ? (
        <details className="digest">
          <summary className="digest__summary">Run signature</summary>
          <p className="digest__help">
            A digest of every step this run recorded. Two runs of the same project with the same
            sources produce the same digest; a run whose log was edited afterwards does not.
          </p>
          <code className="digest__value selectable figures">{outcome.digest}</code>
        </details>
      ) : null}

      <ul className="proof">
        {outcome.jobs.map((job) => {
          const open = expanded === job.job;
          return (
            <li key={job.job} className="proof__item">
              <button
                type="button"
                className="proof__head"
                aria-expanded={open}
                onClick={() => setExpanded(open ? null : job.job)}
              >
                <Verdict status={job.status} />
                <span className="proof__name truncate" title={job.name}>
                  {job.name}
                </span>
                <span className="proof__summary truncate">{summarise(job.status)}</span>
              </button>

              {open ? (
                <div className="proof__detail">
                  {job.status.kind === "succeeded" || job.status.kind === "unverified" ? (
                    <>
                      <button
                        type="button"
                        className="btn btn--ghost"
                        onClick={() => void model.reveal(job.status.kind === "succeeded" || job.status.kind === "unverified" ? job.status.output : "")}
                      >
                        Show the file
                      </button>
                      <table className="checks">
                        <caption className="sr-only">
                          The checks made against {job.name}
                        </caption>
                        <thead>
                          <tr>
                            <th scope="col">Check</th>
                            <th scope="col">Verdict</th>
                            <th scope="col">Measured</th>
                            <th scope="col">Expected</th>
                          </tr>
                        </thead>
                        <tbody>
                          {job.status.checks.map((check) => (
                            <CheckRow key={check.check} check={check} />
                          ))}
                        </tbody>
                      </table>
                      {(job.status.kind === "succeeded" || job.status.kind === "unverified") && job.status.checks.length === 0 ? (
                        <p className="note note--warn">
                          No checks were recorded for this segment. That is not the same as passing:
                          it means this run did not look.
                        </p>
                      ) : null}
                    </>
                  ) : (
                    <p className="note note--danger">{summarise(job.status)}</p>
                  )}
                </div>
              ) : null}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/** The dot and word that classify a job, never the colour alone. */
function Verdict({ status }: { readonly status: JobStatus }): JSX.Element {
  switch (status.kind) {
    case "succeeded":
      return <span className="status status--ok">verified</span>;
    case "unverified":
      return <span className="status status--warn">uncertified</span>;
    case "failed":
      return status.cancelled ? (
        <span className="status status--warn">cancelled</span>
      ) : (
        <span className="status status--danger">failed</span>
      );
    case "skipped":
      return <span className="status status--idle">skipped</span>;
  }
}

/** One line saying what happened, without the checks. */
function summarise(status: JobStatus): string {
  switch (status.kind) {
    case "succeeded":
      return `${status.frames.toLocaleString()} frames in ${formatDuration(status.seconds)}${
        status.overshoot > 0
          ? ` · ${status.overshoot} frame(s) past the out point, as a stream copy does`
          : ""
      }`;
    case "unverified": {
      // Two different reasons a file is uncertified, and they must not read alike: a check that
      // FAILED means the file is suspect, whereas a check that never RAN means this run cannot vouch
      // for it. Collapsing them would either libel a good file or excuse a bad one.
      const failed = status.checks.filter((check) => check.status.kind === "failed").length;
      const missing = status.checks.filter((check) => check.status.kind === "skipped").length;
      const why = [
        failed > 0 ? `${failed} check(s) failed` : null,
        missing > 0 ? `${missing} check(s) were not run` : null,
      ]
        .filter(Boolean)
        .join(" and ");
      return `${status.frames.toLocaleString()} frames written, but ${why || "not every check ran"}`;
    }
    case "failed":
      return status.reason;
    case "skipped":
      return status.reason;
  }
}

/** One check's row. */
function CheckRow({ check }: { readonly check: CheckResultWire }): JSX.Element {
  const verdict =
    check.status.kind === "passed" ? (
      <span className="status status--ok">passed</span>
    ) : check.status.kind === "failed" ? (
      <span className="status status--danger" title={check.status.detail}>
        failed
      </span>
    ) : check.status.kind === "warning" ? (
      <span className="status status--warn" title={check.status.detail}>
        note
      </span>
    ) : (
      <span className="status status--idle" title={check.status.reason}>
        not checked
      </span>
    );

  const detail =
    check.status.kind === "failed"
      ? check.status.detail
      : check.status.kind === "warning"
        ? check.status.detail
        : check.status.kind === "skipped"
          ? check.status.reason
          : "";

  return (
    <tr>
      <th scope="row" className="checks__name">
        {check.check}
      </th>
      <td>{verdict}</td>
      <td className="figures">{check.measured === null ? "—" : check.measured.toFixed(3)}</td>
      <td className="figures">{check.expected === null ? "—" : check.expected.toFixed(3)}</td>
      {detail.length > 0 ? (
        <td className="checks__detail" colSpan={4}>
          {detail}
        </td>
      ) : null}
    </tr>
  );
}
