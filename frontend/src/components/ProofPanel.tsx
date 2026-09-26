/**
 * The proof panel: what happened, and what was measured against it.
 *
 * This is the panel a studio pays for. The question after a deliverable goes out is never
 * "did it run" — it is "prove the frames did not shift", and the answer has to be specific
 * and per check.
 *
 * ## How a verdict is shown
 *
 * Three levels, and the interface must never collapse them into "failed":
 *
 * * **measured and it matched** — the check passed;
 * * **measured and it did not** — the file is not necessarily wrong, but this run cannot
 *   vouch for it, and saying otherwise would be a lie in the other direction;
 * * **not measured** — a check that was skipped is shown as skipped, because "we did not
 *   look" and "we looked and it was right" must not look alike.
 *
 * The distinction is the whole value of the panel, so every row carries a word as well as a
 * colour: roughly one man in twelve cannot separate the red from the green.
 */

import { type OutcomeView } from "../api";
import { formatClock } from "../lib/format";

export function ProofPanel({ outcome }: { readonly outcome: OutcomeView | null }): JSX.Element {
  if (outcome === null) {
    return (
      <div className="panel-body">
        <p className="empty-state">
          Nothing has been cut yet. Trim a range and this fills in: the frames that were written, the
          plan that was followed, and every check made against the finished file — including the ones
          that were not run, which are not the same as the ones that passed.
        </p>
      </div>
    );
  }

  const passed = outcome.checks.filter((check) => check.status.kind === "passed").length;
  const failed = outcome.checks.filter((check) => check.status.kind === "failed").length;

  return (
    <div className="panel-body">
      <dl className="facts">
        <dt>Frames</dt>
        <dd className="figures numeric-column">{outcome.frames.toLocaleString()}</dd>
        <dt>Length</dt>
        <dd className="figures numeric-column">{formatClock(outcome.duration)}</dd>
        <dt>Verdict</dt>
        <dd
          className={`figures numeric-column ${
            outcome.verified === false ? "danger" : outcome.verified === true ? "ok" : "warn"
          }`}
        >
          {outcome.verified === false ? "check failed" : outcome.verified === true ? "verified" : "not measured"}
        </dd>
        <dt>Checks</dt>
        <dd className="figures numeric-column">
          {passed} passed{failed > 0 ? `, ${failed} failed` : ""}
        </dd>
        <dt>Overshoot</dt>
        <dd className={`figures numeric-column${outcome.overshoot > 0 ? " warn" : ""}`}>
          {outcome.overshoot} frame(s)
        </dd>
        {outcome.subtitles !== null ? (
          <>
            <dt>Captions</dt>
            <dd className="figures numeric-column">
              {outcome.subtitles.cues} cue(s)
              {outcome.subtitles.clamped > 0 ? `, ${outcome.subtitles.clamped} clamped` : ""}
            </dd>
          </>
        ) : null}
      </dl>

      {/*
        The plan that was followed, in the operator's own vocabulary. It is here rather than
        only in the range line because this is the record: what the file *is*, not what was
        asked for.
      */}
      <p className="note">
        {outcome.plan.mode === "copy"
          ? "Lossless copy: the in point was a keyframe, so every frame is the original packet."
          : outcome.plan.mode === "headpatch"
            ? `Head patch: ${outcome.plan.headFrames} frame(s) re-encoded from the in point to keyframe ` +
              `${outcome.plan.keyframe}, and ${outcome.plan.bodyFrames} copied untouched.`
            : "Full re-encode: no keyframe fell inside the range, so all of it was re-encoded."}
      </p>

      {outcome.overshoot > 0 ? (
        <p className="note note--warn">
          The copy stopped {outcome.overshoot} frame(s) past the out point, as a stream copy must — it
          ends on its own packet boundary. A sequence's out point trims them.
        </p>
      ) : null}

      <table className="checks">
        <caption className="sr-only">The checks made against this segment</caption>
        <thead>
          <tr>
            <th scope="col">Check</th>
            <th scope="col">Verdict</th>
            <th scope="col">What was measured</th>
          </tr>
        </thead>
        <tbody>
          {outcome.checks.map((check) => (
            <tr key={check.check}>
              <th scope="row" className="checks__name">
                {check.check}
              </th>
              <td>
                <span
                  className={`status ${
                    check.status.kind === "passed"
                      ? "status--ok"
                      : check.status.kind === "failed"
                        ? "status--danger"
                        : "status--idle"
                  }`}
                >
                  {check.status.kind === "passed"
                    ? "passed"
                    : check.status.kind === "failed"
                      ? "failed"
                      : "not checked"}
                </span>
              </td>
              <td className="checks__detail">{check.detail}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <dl className="facts">
        <dt>Output</dt>
        <dd title={outcome.output}>{outcome.output}</dd>
        <dt>Measured against</dt>
        <dd title={outcome.source}>{outcome.source}</dd>
      </dl>
    </div>
  );
}
