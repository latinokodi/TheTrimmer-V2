# ADR-008: The domain core is pure, and the V1 engine is its oracle

**Status:** accepted

**Context.** V1 is a working implementation — a Python package at `H:\THEROLLUPFILES\TheTrimmer`,
`trimmer/{timecode,ffmpeg,trim,verify,subtitles,cli,gui}.py` — whose rules were validated against
real broadcast material over a series of failures that all exited `0` while producing a wrong
file. That knowledge is the most valuable thing the project owns, and a rewrite is exactly the
moment it gets lost: the new code has no memory of the slow-motion timescale bug, the three-frame
audio-seek slip, or the `-frames:v` decode-order truncation except the notes somebody wrote down.

Two ways to keep it. The obvious one is to wrap V1: call the Python engine from the new product.
That was rejected — a Python runtime in a Windows installer is weight and a second upgrade
surface for a product whose value is a native media pipeline, and an engine that is called rather
than owned cannot be extended (presets, multi-segment projects, the daemon, the audit manifest)
without rewriting it anyway. So the engine was rewritten. What made the rewrite safe is the other
way of keeping it: **keep V1 as an oracle.**

**Decision.** `trimmer-core` is pure. Nothing in it opens a file, starts a process, reads the
clock, or touches a network; every function is a total function of its arguments. That is a hard
architectural rule, not a style preference, and it is the prerequisite for the oracle:

* A core that reads the filesystem cannot be replayed against V1 in-process. Every decision it
  makes would be entangled with a file that has to exist, and comparing two implementations
  would mean running two media pipelines rather than two functions.
* Purity is what makes the comparison *cheap enough to run on every change*: one JSON case in,
  one `CutPlan` out, no media, no ffmpeg, no disk.

The oracle mechanism, concretely:

1. The V1 engine is invoked **in place**, at `H:\THEROLLUPFILES\TheTrimmer`, and is never vendored
   into this repository. The hypothesis under test is "the new engine agrees with the old one",
   and a copied engine tests a copy of the hypothesis.
2. One case goes in as JSON: the media facts, the keyframe grid, the segment. That is precisely
   the input `trimmer_core::plan_cut` takes, and precisely the input `trimmer-core/src/domain.rs`
   documents as the boundary — `KeyframeGrid` exists because the core never asks ffprobe
   anything, and the caller hands in the grid it found.
3. One plan comes out, and the two are compared **field by field**: mode, start and end frames,
   keyframe, head and body frame counts, the concat offset, the timescale, the encoder. Not
   "roughly equal" — equal, or a named difference.
4. The V1 answer is ground truth. When the two disagree, the investigation starts by running V1
   to establish what actually happens, and only then asks whether the Rust expectation was wrong.
   That is how the `caption::render` line-ending bug in ADR-007 and the `Folded::span`
   highlight-offset bug were found: both were real, and both would have been invisible to a test
   suite that only knew what the new code already believed.

**Consequences.** The rewrite carries a permanent obligation: a change to `plan_cut` or to the
rate parser has to be checked against an implementation that is deliberately not in this
repository, written in another language, and correct. That is work the wrapped-Python option
would not have required.

It buys the thing the rewrite actually needed. Purity also makes the core exhaustively testable
on its own terms — the drop-frame timecode round trip is a `proptest` over `0..1_500_000` frames
at every supported rate, and `trimmer-verify`'s rules are proved against a `NoMeasurer` double
with no media at all — but those tests only prove the core is *self-consistent*. The oracle is
what proves it is *right*, and the reason the engine could be rewritten rather than wrapped is
that the oracle makes "right" checkable.

**What is verified today, and what is not.** The pinned expectations are in place: `timecode.rs`
asserts against triples produced by running V1 and states in the test source that the suite
doubles as the oracle's Rust half, and the rate parser resolves `29.97` the way V1's
`Fraction.limit_denominator` resolves it — `30000/1001` — because the two implementations have to
agree field for field (ADR-011). The structural prerequisite is in place: `trimmer-core` has no
I/O dependency and the workspace keeps panic-unwind enabled in dev because the oracle needs to
catch a panic per case rather than abort the run. The harness itself — a case table plus a runner
that shells out to the V1 engine — is **not yet in the repository**: there is no `tools/oracle`
directory and no `crates/trimmer-core/tests/oracle.rs` today, even though the module documentation
refers to the latter. Until it lands, the oracle is a rule the code was written against rather
than a test that can fail, and that gap should be closed before the engine is called finished.
