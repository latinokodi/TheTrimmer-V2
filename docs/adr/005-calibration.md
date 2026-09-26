# ADR-005: Calibrate by measuring, not by trusting the arithmetic

**Status:** accepted

**Context.** The concat demuxer places the second file at the first file's duration, and that
duration is not always the head's content length. The drift is small (a few frames) but it is
exactly the kind of error nobody notices until the captions are late.

**Decision.** After a trim, measure the result against the source at two points; if the content
is off by a whole number of frames, re-cut once with the difference folded into the head's
length. An explicit `duration` directive in the concat list does the same job up front; the
measurement is what proves it worked.

**Consequences.** Trimming takes about twice as long by default; `--no-calibrate` skips it. A
measurement that disagrees between sample points is reported, not papered over.

**Change in V2.** Both halves of this are now structural rather than procedural.

*The statement up front* is a function with one job: `trimmer-media::executor::concat_list`
writes `duration <head_seconds>` into the concat list, because leaving it out lets the demuxer
use whatever duration the head's container declares — which on these sources is a few frames
short of the head's content, which is exactly how the body ends up repeating the end of the
head.

*The correction* is a pure function on the plan: `trimmer-core::plan::apply_calibration`. It
moves `offset_frames` from the body into the head and back onto `CutPlan::concat_offset`, so
`head_frames + body_frames` is unchanged — the total is what the user asked for, and a
correction that changed it would be a different cut, not a correction. It then re-runs
`verify_invariants`, and refuses a correction that would leave fewer than one head frame or a
negative body.

It also refuses an offset larger than `MAX_CONCAT_OFFSET_FRAMES` (12) with the words "this is not
drift, it is a symptom". Drift from a concat join is a handful of frames by construction; a
twelve-frame "correction" is a wrong keyframe, a wrong rate or a wrong file, and applying it
would hide the real fault behind a plausible number. Both bounds are asserted in `plan.rs`'s own
tests.
