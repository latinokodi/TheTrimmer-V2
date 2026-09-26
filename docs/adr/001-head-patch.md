# ADR-001: Cut with a re-encoded head, copy the body

**Status:** accepted

**Context.** `ffmpeg -ss S -to E -c copy` cannot start on an arbitrary frame. H.264 and HEVC
frames are deltas against earlier frames, so a stream copy begins at the last keyframe at or
before `S` — 8.33 s early on the sources this was built for. The end is not constrained: every
frame up to it is decodable.

**Decision.** Split the cut at `K`, the first keyframe at or after the in point. Frames
`[S, K)` are re-encoded with libx264/libx265 at `--crf` (default 18) and the frames from `K` on
are the original packets. The two are joined with the concat demuxer.

**Consequences.** A two-hour source loses a couple of seconds of quality instead of two hours,
and the job takes seconds instead of an hour. The cost is complexity: a splice to get right,
and a body-copy step that had two hidden failure modes (ADR-002, ADR-004).

**Change in V2.** The split is the same, and now it is the *only* shape available. `CutMode`
has three variants — `Copy`, `HeadPatch` and `Reencode` — and none of them expresses "copy a
body that does not begin on a keyframe", because there is no such mode to write down
(`crates/trimmer-core/src/domain.rs`, and `plan_cut` in `crates/trimmer-core/src/plan.rs`
refuses a keyframe that falls outside the segment). The two ways this ADR goes wrong silently
are now named invariants carried on the plan itself:

* `PlanInvariant::HeadMatchesSourceTimebase` requires the plan to carry the source's timescale
  forward. `plan_cut` fills `video_timescale` from `media.timebase.ticks()`, and
  `CutPlan::violated_invariants` reports the invariant as broken when a head-patch plan has a
  non-positive timescale or re-encodes frames without one. This is the slow-motion failure
  below, promoted from prose into a check that runs before ffmpeg is started.
* `PlanInvariant::HeadMatchesBodyCodec` requires a non-empty encoder and codec, so a head
  cannot be encoded by something that is not the body's codec family.

The flag that keeps the timescale is emitted at the one place that builds the command:
`trimmer-media::executor::prepare_head` writes `-video_track_timescale` with the plan's
`video_timescale`, alongside `-r` pinned to the source rate. A future change that drops it
fails a test in `executor.rs` rather than producing a slow-motion deliverable three months
later.
