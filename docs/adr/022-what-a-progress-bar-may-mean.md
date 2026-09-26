# ADR-022: What a progress bar is allowed to mean

**Status:** accepted
**Date:** the day the window was asked for progress bars and detailed logs during processing

## Context

> *"I want progress bars and/or detailed logs of what the app is doing during processing."*

There was a bar, and it could not move. The engine's `Progress` vocabulary was:
```rust
enum Progress {
    Step { label: String },
    Command { text: String, args: Vec<String> },
    Elapsed { seconds: f64 },
    Finished { label: String, seconds: f64, ok: bool },
    Message { text: String },
}
```

No fraction. No rate. No frames, no bytes, no position of any kind. `Elapsed` was a heartbeat — "still
alive" — and the interface drew an indeterminate sweep because there was genuinely nothing to fill a bar
with. The comment in the stylesheet said so, and it was right:

> *The engine reports per-step timing but not a fraction of the whole, and a bar that filled to 40 % and
> stopped would be inventing a number.*

So the honest reading of the request is not "draw a bar". It is **"make the engine report where it has
got to"**, and then decide what to do with the numbers.

### The numbers that were available

A cut is a **batch of segments**, each of which is a **sequence of ffmpeg passes**, each of which knows
**how far through itself it is**. Three levels, and they are not interchangeable:

| Level | Source | Fine-grained? | True? |
|---|---|---|---|
| segments finished / total | `QueueEvent::Started` and `Finished` | no — moves five times in a nine-segment batch | yes |
| passes finished within a segment | `Progress::Step`/`Finished` | no — the head encode is most of the wall time and the join is none of it | yes, and useless |
| position within the current pass | ffmpeg's own `-progress` | yes, every half-second | yes, because it is a measurement |

Only the third makes a bar that moves *and* means something. The second is the tempting one, and it is
the one that would produce a bar sitting at 20 % for four minutes on a nine-minute job: real numbers,
arranged into a lie.

### Getting it

ffmpeg will say, if asked. `-progress pipe:1 -nostats -stats_period 0.5` makes it write a block of
`key=value` lines to standard output every half-second:

```text
frame=57
total_size=158476
out_time_us=2200000
speed=4.27x
progress=continue
```

`out_time_us` is microseconds of output timeline written, which against a length the pass already knows
is a real fraction. `speed` is ffmpeg's own throughput as a multiple of real time, measured by the
program doing the work rather than extrapolated from how long we have been waiting — which is where the
estimate comes from.

## Decision

### 1. The engine reports the position, and nothing above it decides what it is for

`Progress::Ticks { ticks: ProgressTicks }`, where

```rust
pub struct ProgressTicks {
    pub out_seconds: f64,
    pub frame: Option<u64>,
    pub speed: Option<f64>,
    pub bytes: Option<u64>,
    pub expected_seconds: Option<f64>,
}
```

`trimmer-media` runs one process and has no idea what it is for, so it reports what it was told. The
expectation travels with it only because the caller already knew it — `Prepared` gained
`expected_seconds`, set by each argument builder from the very number it already hands ffmpeg as `-t`.
Dividing is the interface's business.

Three rules came out of writing it, and each is a test:

* **A block with no position is dropped, not read as zero.** ffmpeg's opening block carries
  `out_time_us=N/A`, and a pass that produced no timestamp carries none at all. Reporting either as `0.0`
  would put the bar back to the start mid-job.
* **`out_time_ms` is microseconds too.** ffmpeg's own naming bug, not a misreading. Both keys are parsed
  as microseconds.
* **`N/A`, a negative rate and a non-numeric frame are ignored rather than fatal.** They occur in normal
  operation and none of them is a reason to lose the tick that follows.

The flags are **opt-in per run** (`RunOptions.watch`), because `-progress pipe:1` takes standard output
and one caller already owns it: the frame-hash and SSIM passes read `framemd5` from stdout and would be
corrupted by a progress stream mixed into it.

### 2. The bar means the current pass, and the readout says everything else

```
[########------------] 42%   head encode, frames 250..375    segment 2 of 5   1:04   4.27×   ~1:16 left
```

* **The bar** is `out_seconds / expected_seconds` for the pass running now — the only fraction that is
  both fine-grained and true.
* **`segment 2 of 5`** is the batch position, which is exactly what it is: coarse, and never dressed up
  as the bar.
* **`4.27×`** and **`~1:16 left`** come from ffmpeg's own measurement. The estimate is
  `remaining_output / speed` — a number the program doing the work produced.
* Each figure is **omitted when unknown** rather than shown as a zero. A readout that says `0:00 left` at
  the start of a nine-minute job is worse than one that says nothing. Under a second it says `<1s`,
  because `0:00 left` reads as "done" at the moment it is least true.
* When a pass does not know its own length the fraction is `null` and the bar goes back to the
  indeterminate sweep — and it is then **not a `<progress>` element at all**, because there is no value
  to announce. One element per meaning.

It is a real `<progress>`, not a decorated `div`: a screen reader announces its value, and the percentage
printed beside it is the same figure, asserted to agree to within a rounding step.

### 3. The log carries what happened, when, and at what cost

Every line is stamped from a fixed-width clock (`formatClock`: `0:08`, `3:12`, `1:02:03`) — deliberately
not `formatDuration`, which answers "how big is this" and says `8.4s` then `3m 12s`, changing width as it
counts. A column read by scanning cannot change width.

The engine also gained **live diagnostic forwarding**: ffmpeg's standard error is reported as it arrives
rather than only as a tail once a pass has ended, bounded at 200 lines so a pathological run cannot fill
the log with its own noise. `-v error` means a successful pass usually says nothing, so this costs
nothing on the common path and turns a failure into something readable *while* it is happening.

## The two defects this work exposed

### 1. Every head patch of a master with sound failed

The first run of the new CLI progress display on a real clip produced this:

```text
  · body mux, picture and sound
  [in#0 @ 000001efc1ad5080] Error opening input: No such file or directory
  Error opening input file PICTURE.
  FAILED body mux, picture and sound (0.0s)
```

The mux step — the third of the body's three passes, and the one that joins the picture and the sound
copied out of the original packets — was built with the literal inputs `PICTURE` and `SOUND`, and
**nothing in the codebase ever replaced them.** `grep -r PICTURE crates/` returned two hits, both the
literals themselves.

So **the flagship operation of the product — re-encode only the keyframe head, copy the body — failed
for every source with a soundtrack.** Which is every source.

It survived because of a fixture. The head-patch end-to-end test's clip is generated with no audio
track, and the source comment says so:

```rust
// And the steps are on the record, which is what the proof panel renders. The fixture has no
// audio, so the body is one copy rather than a picture copy, a sound copy and a mux.
```

One copy, not three. The third pass **had never been executed by any test in the suite.** The
contract test's cut happens to start on a keyframe, so it takes the lossless-copy path and never
reaches the body mux either.

This is [ADR-021](021-the-fixture-could-not-hold-two.md)'s lesson for the fourth time: *a fixture that
cannot represent the state a fault lives in will not find it.* The state here was "a video with sound",
which is not an edge case — it is what a master is.

The fix makes the mistake unrepresentable rather than merely absent. `prepare_body` takes the two
intermediate paths as an argument:

```rust
pub struct BodyHalves<'a> { pub picture: &'a Path, pub sound: &'a Path }
```

It cannot invent them, because it is handed them, and the mux step is built from the same values the
caller then writes to. A preview passes `BodyHalves::placeholders()`, whose names are deliberately
angle-bracketed (`<body picture>`) so that a placeholder which looks like a path cannot become the same
trap one refactor later.

It is asserted twice: `the_mux_step_opens_the_two_halves_it_was_given` on the arguments, needing no
media, and `a_head_patch_of_a_master_with_sound_muxes_the_two_halves` end to end on a *new* clip that
has a soundtrack — asserting the five passes ran, that the deliverable holds both a video and an audio
stream, and that it is the length that was asked for. Reinstating the literals makes the second fail
with the message above.

### 2. Both event vocabularies have a `finished`, and the listener conflated them

Writing the progress model turned up a bug in the listener that had been there since it was written:

* `trimmer_media::Progress::Finished { label, seconds, ok }` — this ffmpeg pass ended;
* `trimmer_app::QueueEvent::Finished { job, name, status }` — this segment ended.

The listener sent **every** `finished` to the queue handler. So the log never showed a single pass's
timing: every ffmpeg pass was announced as `segment finished`, and the count of segments that had
finished was in fact the count of passes that had. Nothing caught it because nothing in a browser could
produce either event — the same fixture fault as above, in the same file.

A queue `finished` is the one that names its job, and that is the test. Reverting the discriminator
makes `progress.spec.ts` report **zero** pass verdicts where it expects five, which is the defect
precisely.

## Consequences

**Good.**

* A four-minute cut reports a moving bar, a real percentage, a rate and an estimate, all measured.
* The estimate is ffmpeg's own, so a step that is going to take an hour says so in the first minute rather
  than after twenty.
* The log answers "which of these took the four minutes" — per-pass timings against a clock column, plus
  the exact command line that ran.
* **The engine's flagship operation works for a source with sound**, which it did not before. That was not
  the request; it is what the request's instrumentation found in its first hour.
* `Progress::Ticks` is asserted key by key against the real serialisation in
  `ipc_contract.rs::the_progress_events_carry_the_keys_the_interface_reads`, because the browser suite
  drives the entire display from stub-emitted events and a plausible payload is not a correct one.
  Removing `rename_all = "camelCase"` from `ProgressTicks` fails it with `left: Null, right: 2.2`.
* The stub now **narrates a compressed run**, so `npm run dev` shows a working progress bar and the whole
  zone is testable in a browser. That is the fix for the fixture gap that hid both defects above.

**Costs, stated plainly.**

* `crates/trimmer-media` changed. The flags are additive — `-progress` only affects reporting, and no flag
  that decides *output* was touched — and the differential oracle is unaffected because it compares cut
  plans rather than command lines. But it is the engine, and the argument for touching it is that a real
  fraction cannot come from anywhere else.
* The recorded command line in the audit log now includes the four watch flags. That is a change to a
  shipped artefact, and it is the right direction: the log records what actually ran.
* A watched run emits an event every half a second. A ten-minute copy is about twelve hundred events, and
  the interface keeps only the latest for the bar — but it is traffic that did not exist before.
* The estimate is only as good as `speed`, which is a recent average. Early in a pass it is
  unrepresentative, which is why there is no estimate until both a rate and a position exist.

## Alternatives considered

**Rejected — a bar from elapsed time against an expected duration.** It needs no engine change and would
have been a tenth of the work. It was rejected because the expectation would have to come from somewhere
and there is nowhere honest: the length of a pass is known, but how long it will *take* is not, and a bar
driven by a guessed duration is a progress bar that lies smoothly. The whole value of this one is that
every figure in it is a measurement.

**Rejected — counting ffmpeg passes as the fraction.** Real numbers, and a lie: nine-tenths of the wall
time is in one of the five passes, so the bar would sit at 20 % for four minutes and then jump. This is
the alternative most likely to be proposed by someone who has not timed a head patch.

**Rejected — an overall batch bar of `finished / total`.** True and nearly useless: five movements in a
nine-segment batch, none of them during the four minutes the operator is actually waiting. It is shown as
`segment 2 of 5`, which is what it is.

**Rejected — `-v warning` so ffmpeg chats more.** The existing `-v error` is a deliberate decision with a
reason in the source: the log is built from the steps this crate records, and ffmpeg's informational
chatter buries it. Live forwarding of what *is* on stderr gets the diagnostic value — a failure readable
while it happens — without the noise.

**Rejected — a second strip row for the batch bar.** The window is maximized rather than fullscreen since
[ADR-020](020-the-window-has-a-titlebar.md), which is about 80 logical pixels tighter than it was, and the
progress zone is fractional. A second row would come out of the log's height, and the log is what answers
the question afterwards. One bar plus a readout fits in the row that was already there.
