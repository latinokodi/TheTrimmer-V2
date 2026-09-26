# ADR-004: Let the copy stop on its own packet boundary

**Status:** accepted

**Context.** A stream copy stops on a packet boundary, so the body runs a frame or two past the
out point. The first implementation pinned the count with `-frames:v N` after the join, on the
reasoning that an MP4 holds one packet per video frame. It does — but with B-frames `-frames:v`
counts packets in **decode** order, which is not presentation order. Measured on a 91-frame cut:
the last frame kept was the source's 151st, and the 150th that had been asked for was gone. The
file still reported "91 frames, exactly as asked", which is why it took a sample at the tail of
the requested range to see it.

**Decision.** Bound the tail by *time* only (`-t`), accept the one-to-three frames of overshoot,
and say so in the report. Verification samples the **last twelve frames of the requested
range**, so a dropped frame at the end fails the check instead of hiding.

**Consequences.** The delivered file is occasionally a few frames long; nothing that was asked
for is ever missing. A sequence's out point trims the extra frames, which is what the
accompanying Premiere XML does.

**Change in V2.** The tempting flag is gone from the code rather than from the notes.
`trimmer-media::executor::prepare_copy` emits an input seek, a stream copy and a `-t`, and never
a frame count; the whole-segment copy is bounded by `requested_frames() + 1` frames' worth of
time on purpose, which is what lets the copy stop on its own boundary instead of one frame short
of it. The test `the_copy_path_is_bounded_by_time_and_never_by_a_frame_count` asserts the plan's
`-t` is present and that `-frames:v` is **absent**, with the decode-order reason written into the
assertion message. Reintroducing the flag fails that test.

The tail sampling rule from V1 is retained in the measurement layer: the sample window the
verifier is handed ends on the last frames of the *requested* range, because that is the sample
that caught the original defect and the only sample that can catch it. What the verdict does
with an overshoot, and why it is a warning rather than a failure, is ADR-014.
