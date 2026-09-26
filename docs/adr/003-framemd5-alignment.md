# ADR-003: Measure alignment with framemd5, on each file's own frame grid

**Status:** accepted

**Context.** The body is a stream copy, so its decoded frames are identical to the source's.
Frame MD5s therefore answer "is this the same picture" exactly, with no tolerance and no
third-party dependency. But two files whose first timestamps differ by a fraction of a frame
answer "give me the frame at time t" differently, which made the measurement flicker between 0
and ±1 depending on the sample.

**Decision.** Extract both windows on their file's own grid: `start + n / rate`, using each
stream's `start_time` from ffprobe. A re-encoded head cannot be hashed at all, so it is checked
separately by SSIM against the source's frames either side of the mark.

**Consequences.** The alignment verdict is unambiguous and repeatable; the verification costs
two ffmpeg calls per sample point instead of one.

**Change in V2.** V1 compared one window and hoped. V2 says out loud that a fraction of a frame
of start offset is expected, and searches for it: `trimmer-verify`'s `FrameAlignment` check tries
the offsets in `ALIGNMENT_OFFSETS = [0, -1, 1]` — zero first, so the ordinary case wins a tie —
picks the offset under which every frame in the window matches, and reports which offset it was
in `CheckResult::measured`. A two-frame shift is not tolerated and fails, naming the first
differing frame; that is not sampling, it is a wrong cut.

The check is offset-tolerant and never offset-blind: an offset that does not explain the *whole*
window is not accepted, and a window with nothing to compare is `Skipped` with the reason
stated rather than counted as a pass. The head keeps the SSIM check V1 introduced
(`Check::HeadFidelity`), with the threshold at `HEAD_FIDELITY_MIN = 0.98` — a "is this the right
picture" gate, not a quality gate.
