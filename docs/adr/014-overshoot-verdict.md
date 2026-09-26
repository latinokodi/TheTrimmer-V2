# ADR-014: Overshoot is a warning; a missing frame is a failure

**Status:** accepted

**Context.** Two rules that look like one, and a contradiction that appeared when they were
implemented as one.

*Rule one.* Every frame that was asked for must be present. A file that is short by one frame is
wrong: somebody marked a range and did not get it.

*Rule two.* Running past the out point is not a defect. A stream copy stops on a packet boundary,
so the out point lands inside a packet and the copy carries on to the end of it — one to three
frames, by the intended method doing what the intended method does. Those frames are worth saying
out loud, and a sequence's out point trims them.

A verifier that failed a file for being two frames long would fail the correct output of the
intended method, every time, on every segment. A verifier that fails correct files is one people
learn to ignore, and then the failures that matter go unread. So the two cases have to be separate
verdicts: `Check::Frames` fails when frames are missing, `Check::Overshoot` warns when frames are
extra, and neither may do the other's job.

**The contradiction.** The first implementation of the duration rule did not respect that
separation. `check_duration` compared the picture's measured length against the **requested** frame
count: `requested × 1/rate` seconds. On a routine two-frame overshoot, the picture is legitimately
two frames longer than the request, which is more than the one frame of tolerance the check allows
— so the duration check **failed**, and the report said `NOT OK` about a file that rule two calls
correct. The overshoot warning was defeated by a duration failure two lines later, which is
exactly the outcome the split into two verdicts existed to prevent. It also meant a report could
not be read as a whole: two checks disagreed about the same file and one of them was wrong.

**Decision.** Each rule owns one question and only one.

* **`Frames` owns requested-versus-delivered.** `delivered >= requested` passes; `delivered <
  requested` fails, naming how many frames that were asked for are not in the file. Extra frames
  are not this check's business at all.
* **`Overshoot` owns the extra frames.** It warns with the count and the reason, and the report
  stays `ok`. The delivered length is never used to fail anything.
* **`Duration` owns "the picture is not the length its own frames say".** It compares the measured
  duration against the frames the file *actually holds*, at the plan's own numerator and
  denominator rather than a rounded rate — 600 frames at `30000/1001` is 20.02 s, and a check that
  read that as 29.97 would accept a file a third of a second wrong. The tolerance is one frame.

That third rule is what `Duration` needs to be: the detector for a rescaled timescale and a
container that lies about its length — the two failures from ADR-001 and ADR-010 that produce a
file of the right frame count at the wrong length. It is not a second frame-count check, and once
it stopped being one, the contradiction disappeared.

**Consequences.** The verdict vocabulary is now honest about three states rather than two:
`Passed`, `Warning` and `Failed`, with only `Failed` making a report not `ok`. A `Warning` is
something true and worth saying that does not make the file wrong, and it is a real state rather
than a soft failure. The cost is that a reader has to understand that distinction — a report with
a warning is a clean report — and that `check_duration`'s frame count and `check_frames`'s frame
count are answering different questions, which is why both of them carry the reason in their doc
comments.

Every one of these rules is a pure function of measured facts, so the boundaries are tested
directly, including the exact case that produced the bug: an overshoot of two frames must leave
`Overshoot` warning and `Duration` passing.
