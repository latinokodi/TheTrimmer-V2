# TheTrimmer

Frame-exact, verified segment cutting for professional post-production. TheTrimmer takes a master
and two Premiere timecodes and gives back the segment — with the original packets, not a
transcode — then measures what it produced against the source and tells you, with numbers, whether
it is right. It is a Windows desktop application.

> **Status of this document.** The repository is being built in stages, and this README says which
> stage it is at. Four crates are real workspace members and compile: `trimmer-core` (the pure
> domain), `trimmer-media` (the only crate that starts a process), `trimmer-verify` (verdicts over
> measured facts) and `trimmer-export` (timeline documents). A fifth, `trimmer-app` (the
> application layer), is in the tree but is a commented-out workspace member while its modules are
> written. The project store, the command line, the headless daemon, the licensing layer and the
> desktop shell do **not** exist yet; where this document describes them, the section says so and
> describes what the build is being carried out against. Nothing here is claimed to be shipped
> that is not in the tree.

---

## What it does that a plain ffmpeg trim does not

`ffmpeg -ss S -to E -c copy` cannot start on an arbitrary frame: H.264 and HEVC frames are deltas
against earlier frames, so a stream copy begins at the last keyframe at or before your in point —
8.33 s early on the masters this was built for. TheTrimmer splits the cut at the first keyframe at
or after the in point: the frames before it are re-encoded at CRF 18, and every frame from that
keyframe on is the original packet data, copied. A two-hour master therefore loses a couple of
seconds of quality instead of two hours, and the job takes seconds instead of an hour.

The second difference is that the app does not ask you to trust it. The finished file is measured
against the source — decoded frame MD5s compared on each file's own frame grid, the re-encoded
head compared by similarity, every written caption cue compared against the source cue retimed —
and the result is a report with each check named, its status, and the numbers that produced it.
The three failures that make this method dangerous (a rescaled timescale, a codec mismatch, a
concat drift) all exit `0` from ffmpeg while producing a wrong file, and all three are checked
before the cut is made and again after it.

## Install and first run

**Not yet implemented.** There is no installer in this repository: no release workflow, no signed
artifact, no updater. What exists is the decision that there will be one, and the engineering
constraints that shape it.

The intended shape:

* A **Windows installer**. One executable, per-user or per-machine, no separate runtime to install
  by hand.
* **It works offline.** Nothing in the application's designed behaviour requires a network: the
  licence is a signed file verified against an embedded public key (see
  [ADR-015](docs/adr/015-licensing.md)), and the cut pipeline is local. There is no activation
  call, no telemetry and no update check that blocks launch.
* **ffmpeg is bundled.** Today, the engine resolves ffmpeg and ffprobe from `PATH` and honours
  `THE_TRIMMER_FFMPEG` / `THE_TRIMMER_FFPROBE` as overrides — which is what lets a studio point
  the app at a specific build, and what the `doctor` report is for. Bundling a known-good build in
  the installer removes the "which ffmpeg is on this machine" class of support call; a studio that
  wants its own build can still override.

## The workspace

**The desktop shell does not exist yet** — there is no `apps/` directory. This is the current focus
of the build, and this is the workspace it is being built toward. What *does* exist is the
application-layer design beneath it: the workspace model, transcript service, watch-folder rules
and batch queue are specified in `trimmer-app`, whose seams (`MediaEngine`, `TranscriptSource`,
`ProjectStore`, `Clock`) are what let each of these be tested without media and without a disk.

**Sources.** A project holds sources — masters, not copies. Each is probed and its facts
remembered: rate, frame count, container timescale, audio layout, size. A source that has gone
missing since the project was written is reported as unavailable rather than failing the project;
a project has to survive a drive being unplugged.

**Segments.** A segment is an in point, an out point and a delivery preset, named and saved. Marks
are entered as Premiere timecodes — `HH:MM:SS:FF`, or `HH:MM:SS;FF` on 29.97 and 59.94 material —
and the frame range they resolve to is shown as you type, before anything is written. Handles are
per-segment and clamped to the source.

**Transcript search.** If `clip.srt` sits beside `clip.mp4` (or `clip.en.srt`), it is indexed. You
search for a phrase, click a hit, and the in and out points are already frame-exact: hits are
snapped to sentence boundaries so a segment starts on the first word of a sentence rather than in
the middle of one, or to the nearest pauses when a cut must not clip a plosive. This is the
feature that changes what the tool is for — nobody should have to find the frame number of "the
bit where he says the thing about custody" by hand.

**The cut queue.** Cut many segments as one job. The queue runs one at a time, deliberately: a cut
is disk-bound, so four concurrent cuts on one spindle make all four slower and the progress display
meaningless. Each item reports its own outcome, and Cancel means "finish this one, stop".

**The proof panel.** For a finished cut, the report: every check, its status (`Passed`, `Warning`
or `Failed`), and the measurement behind it, plus the exact ffmpeg command lines that ran, how
long each took, the plan's invariants, and the frames of overshoot. A `Warning` is something true
and worth saying that does not make the file wrong; only a `Failed` does.

The workspace also shows a **timeline strip and a filmstrip** of thumbnails — one per segment,
extracted lazily and cached on disk, so a list of twenty cuts is readable at a glance. There is no
scrubbing preview and no decode while a cut is running: see [ADR-006](docs/adr/006-no-preview.md)
for what was refused and why.

## Features

### Cutting

* **Premiere timecodes in, frames out.** `HH:MM:SS:FF`, `HH:MM:SS;FF` for drop-frame on
  29.97/59.94, plain seconds if you prefer. Drop-frame is parsed from the separator, not guessed:
  an hour of 29.97 is 107,892 frames but 108,000 non-drop labels.
* **Head patch by default.** Only the frames between the in point and the next keyframe are
  re-encoded. The in-point-is-a-keyframe case re-encodes nothing at all; a segment with no keyframe
  inside it re-encodes whole, and says so in its notes.
* **Calibration.** The concat join is measured, and a whole-frame drift is folded into the head's
  length and re-cut once. A correction larger than 12 frames is refused, because that is not drift,
  it is a symptom.
* **Handles**, per segment, clamped to the source.
* **Overshoot, stated.** A stream copy stops on a packet boundary, so a cut may run one to three
  frames long. Nothing that was asked for is ever missing, and the report says how many extra
  frames there were.
* **Cancellation that is felt.** A long copy is polled and cancellable between polls, not awaited
  to completion.
* **Variable-rate warning.** A VFR source has no frame grid to be exact against; the app says so
  rather than pretending.

### Transcript

* **SRT beside the video is picked up automatically** — `<video>.srt` or `<video>.<lang>.srt` — or
  named explicitly, or switched off.
* **Retimed onto the segment**: cues shifted so the segment starts at `00:00:00,000`, renumbered
  from 1, and written as a sidecar named after the output so players and Premiere load it by
  themselves.
* **Cues that cross a mark are clamped** when at least 0.25 s of the cue survives and dropped when
  less does. The window used is the one you asked for, never the file that came out — the frames a
  copy adds past the out point must not grow captions.
* **The file keeps its own shape**, BOM, line endings and decimal separator included, because
  these files get diffed and re-imported.
* **Nothing inside the window** writes no file and logs why, with the hint that the transcript may
  already be segment-relative.
* **Search and cut by text**, with hits snapped to sentences or to silences.

### Delivery

Delivery presets, named. A project has a default preset; a segment can override it.

| Preset | What it delivers |
|---|---|
| `master` | Original packets, MP4, audio copied. Nothing re-encoded but the head. |
| `master_faststart` | As `master`, with the index at the front so it streams and scrubs instantly. |
| `prores_master` | ProRes 422 HQ in MOV, PCM audio, source geometry. For a finishing house that will not touch a long-GOP file. |
| `youtube_1080` | 1920×1080 H.264 CRF 18, AAC 320k, −14 LUFS. |
| `vertical` | 1080×1920 H.264, centre-cropped, −14 LUFS. Shorts, Reels, TikTok. |
| `vertical_blur` | 1080×1920 with the whole wide frame on a blurred bed. Keeps both speakers when a crop would cut one out. |
| `square_social` | 1080×1080 H.264, cropped, −14 LUFS. |
| `podcast_audio` | Audio only, AAC 192k in WAV, −16 LUFS. |
| `wav_split` | 48 kHz 24-bit stereo WAV, untouched. For a sound editor. |
| `broadcast_r128` | 1920×1080 H.264 high profile, PCM audio, EBU R128 −23 LUFS. |
| `mxf_op1a` | MXF OP1a, XDCAM HD422 50 Mbit at 1920×1080, PCM audio. For a broadcast server that will not take an MP4. |

Loudness is targeted by the preset rather than left to the platform, because a platform will only
ever turn you *down*. The targets are stated as data, with the standard they implement:

| Target | Standard |
|---|---|
| −14 LUFS, −1 dBTP | Streaming normalisation |
| −16 LUFS, −1 dBTP | Stereo podcast delivery |
| −23 LUFS, −1 dBTP | EBU R128 |
| −24 LUFS, −2 dBTP | ATSC A/85 |

A preset that changes the frame geometry — or anything else that cannot be satisfied by copying
packets — forfeits passthrough: the whole segment is re-encoded, and the app says so before the job
starts rather than quietly degrading. A hundred-segment batch on `vertical` is a hundred full
transcodes and you should know that when you ask for it.

**Timeline export** is implemented today, in `trimmer-export`: four formats from one request, so
they cannot disagree about what the timeline is.

| Format | Notes |
|---|---|
| **Premiere XML** | FCP7 `xmeml` version 4. Premiere Pro and Resolve both import it. |
| **FCPXML 1.11** | Times are exact rational seconds built by integer arithmetic — 100 frames at 25 fps is `4/1s`, one frame at 29.97 is `1001/30000s`. Never a float. |
| **CMX3600 EDL** | Inclusive out points, the way the format wants them. |
| **CSV** | One RFC 4180 row per segment. |

The export refuses to guess: a segment whose source runs at a different rate from the timeline is
written on its own source rate and reported as a warning rather than silently rescaled, and a
segment whose source is offline is skipped with a warning so the rest of the project still exports.

### Verification

Nine named checks, each with a sentence saying what passing it proves. They are pure functions of
measured facts, which is why they can be tested exhaustively with no media at all.

| Check | What it proves |
|---|---|
| `frames` | Every frame the plan asked for is present. A short file fails. |
| `duration` | The picture is as long as the frames it holds, at the plan's own rational rate. This is the rescaled-timescale and lying-container detector. |
| `audio alignment` | The audio track ends level with the picture. |
| `frame alignment` | The delivered frames are the source's own frames, on each file's own grid, tolerating a one-frame start offset and recording which offset matched. |
| `head fidelity` | The first delivered frame is the source frame at the in point, not a frame from somewhere else. |
| `captions` | Every written cue is the source cue retimed onto the window, compared against the retiming function itself. |
| `codec` | The delivered codec is the source codec, so the track describes one kind of sample. |
| `timescale` | A re-encoded head carries the source timescale, so the muxer does not rescale the copied body into slow motion. |
| `overshoot` | Extra frames come from a packet boundary, not from a wrong out point. A warning. |

Verification policy is a project setting: `off`, `standard`, `strict` or `forensic`. `strict` is
the default — it is the strongest check that does not require decoding a whole segment — and
`forensic` adds the head's similarity check and the caption comparison. With `off` every check is
`Skipped` with that reason, because "do not measure" has to mean the report never claims more than
was done.

### Automation

* **Batch queue**, sequential by design, with per-item outcomes, progress, and cancellation.
* **Watch folders** (`notify`-based rules): what to do when a file and a marker list appear — cut,
  cut and verify, or queue for review.
* **Headless API** so a studio's pipeline can drive the same engine the window drives (below).
* **Audit manifest**: every run's entries appended in order and hashed as a whole over a canonical
  form of the data, so a manifest that has been pretty-printed or had its keys reordered still
  digests the same, and editing any entry after the fact changes the digest.
* **Command line**, so a cut can be scripted end to end (below).

## Command line

**Not yet implemented.** There is no `thetrimmer` binary in the workspace; the crates that would
back it (`trimmer-cli`, `trimmer-daemon`, `trimmer-app`) are commented-out workspace members. The
command surface below is the design the build is being carried out against; the flags follow the
engine's real knobs, and the V1 engine that this product descends from
(`H:\THEROLLUPFILES\TheTrimmer`) offers the working equivalent of `probe`, `cut` and `doctor`
today.

```
thetrimmer probe      describe a source: rate, frames, timescale, audio, warnings
thetrimmer cut        cut one segment
thetrimmer batch      cut every segment in a project, or a list of marker files
thetrimmer project    create, list, inspect and export projects
thetrimmer export     write a timeline document
thetrimmer verify     check a finished file against its source
thetrimmer doctor     what the app found: ffmpeg, its encoders, ffprobe, WebView2, the licence
thetrimmer licence    install, inspect and report the status of a licence file
thetrimmer daemon     run the local HTTP/JSON API
thetrimmer watch      run the watch-folder rules in the foreground
```

Worked examples of the first four:

```powershell
# What is this file? Rate, frame count, container timescale, audio, and any warning.
thetrimmer probe "M:\masters\interview.mp4"
thetrimmer probe "M:\masters\interview.mp4" --json

# One segment. Timecodes in, frames out; the plan prints before anything is written.
thetrimmer cut "M:\masters\interview.mp4" `
    --in 00:58:45:27 --out 01:10:02:12 `
    --preset master `
    --output "M:\cuts\custody-section.mp4" `
    --verify

# The same cut, dry, as a JSON plan and the exact argument vectors it would run.
thetrimmer cut "M:\masters\interview.mp4" --in 00:58:45:27 --out 01:10:02:12 --dry-run --json

# Every segment in a project, as one job. Sequential; one item's failure does not stop the rest.
thetrimmer batch "M:\projects\documentary.thetrimmer" --preset master --verify --continue-on-error
thetrimmer batch --markers "M:\markers\*.csv" --preset youtube_1080 --jobs 1

# Projects.
thetrimmer project new --name "Documentary" --source "M:\masters\interview.mp4"
thetrimmer project list
thetrimmer project show "M:\projects\documentary.thetrimmer" --segments
thetrimmer project export "M:\projects\documentary.thetrimmer" --format json --output project.json
```

Notes on the real knobs behind those flags:

* `--in` / `--out` are **inclusive** frames, the way Premiere's In and Out work. `--out-exclusive`
  treats `--out` as the first frame not kept, and `--no-drop-frame` refuses a drop-frame reading
  rather than guessing one. The summary always prints the inclusive range, so there is no doubt.
* `--preset` names one of the delivery presets above; quality and speed come from the preset rather
  than from a bare CRF, because "give me the vertical cut" is the question an editor actually asks.
  (The encoder's own x264/x265 speed preset is a separate setting on the re-encoded head, not a
  delivery choice.)
* `--calibrate` / `--no-calibrate` controls the measure-and-re-cut pass; `--concat-offset N`
  cancels N frames of concat drift by hand. `--crf` and the encoder speed preset remain available
  for the re-encoded head.
* `--json` on any command makes the output machine-readable — the same shapes the daemon serves.
* `--jobs` defaults to 1. A cut is disk-bound rather than CPU-bound, so concurrency above 1 is
  opt-in and capped; the queue's design note is in ADR-013 and in `trimmer-app`.

## The headless API

**Not yet implemented.** `axum` is declared in the workspace dependency table for it, but the
daemon crate does not exist.

The intended shape: a local HTTP/JSON API on a **loopback port only**, requiring a **bearer
token** that the app generates and never sends anywhere, so a studio's pipeline script can drive
the same engine the window drives. One resource shape per noun, `camelCase` on the wire, and errors
as a discriminated union carrying the numbers that produced them — so a client pattern-matches
instead of parsing English prose.

```
POST   /v1/probe            probe a path
POST   /v1/plans            plan a cut without running it
POST   /v1/cuts             run a cut
GET    /v1/cuts/{id}        its state, progress and outcome
DELETE /v1/cuts/{id}        cancel it
GET    /v1/cuts/{id}/report the verification report
GET    /v1/checks           the check vocabulary and what each one proves
POST   /v1/exports          write a timeline document
GET    /v1/projects         list projects
```

A local API over tRPC or GraphQL was rejected: the consumers are the desktop shell, the CLI and a
studio's pipeline script, and a pipeline script speaks HTTP, not a typed client. The API binds to
loopback and is not reachable from the network.

## How correctness is established

Four independent mechanisms, in decreasing order of how much they can prove.

**1. The differential oracle against the V1 engine.** V1 is a working Python implementation whose
rules were validated on real broadcast material, and it is kept as ground truth rather than as
history. The oracle feeds one JSON case — media facts, keyframe grid, segment — into both engines
and compares the resulting plan **field by field**. V1 is invoked in place and never vendored: a
copied engine tests a copy of the hypothesis. This only works because `trimmer-core` is pure — a
core that reads the filesystem cannot be replayed in-process against another implementation, which
is the whole architectural reason the rewrite was possible at all
([ADR-008](docs/adr/008-pure-core-oracle.md)). The working rule when the two disagree is that V1 is
run first to establish what actually happens and only then is the Rust expectation questioned;
`caption::render`'s line-ending bug and a search-highlight offset bug were both found that way, by
tracing failures to a cause instead of patching an expectation.

**2. Invariant checks on every plan.** Eight named invariants are carried on the `CutPlan` itself —
the range is inside the source, a copied body begins on a keyframe, head and body share a codec,
the head carries the source timescale, the concat offset is explicit and bounded, the frames add
up, re-encoding is confined to the head, the frame grid is trustworthy. They are re-checked
whenever a plan is read back from a file or handed across a process boundary, because a plan is
data and data can be edited by hand or written by a future version.

**3. Property and boundary tests.** Timecode round-trips are a `proptest` over 1.5 million frames
at every supported rate, and a property asserts drop-frame rendering never emits a skipped label.
The verdict logic is tested exhaustively against a `NoMeasurer` double, including every boundary —
which is affordable precisely because a verdict is a pure function of numbers. The executor's
argument vectors are asserted directly, so dropping `-video_track_timescale`, adding `-frames:v`
back to the copy path, or merging the body's two passes fails a test rather than shipping.

**4. Real-process tests.** `ProcessRunner` is exercised against a real executable — argv arrays,
exit codes, error tails, a timeout that kills a process that will not finish, a large output that
must not deadlock, and a cancellation that has to be noticed mid-run rather than after it. This is
what makes the claims about cancellation and about no-shell spawning testable rather than
asserted.

**What is not yet true.** Not one of the four is complete, and saying which parts are missing is
more useful than claiming the whole.

* The **oracle harness** is not in the repository. `trimmer-core`'s module documentation refers to
  `tests/oracle.rs`, and there is no `tools/oracle` directory; what exists is the design, the
  purity that makes it possible, and the expectations pinned against V1's answers inside the unit
  tests. It is the single most valuable thing left to build.
* There is **no end-to-end trim in the suite** — no generated clip cut and measured. The
  executor's tests assert the argument vectors it produces (which is where the silent failures
  live), and `ProcessRunner` is tested against a real process, but nothing today cuts media and
  checks the result.
* There is **no continuous integration workflow**: `.github/workflows` does not exist.
* `trimmer-app` is not a workspace member, so its modules are not compiled or run by `cargo test`
  until the missing ones are written.

The suites that do run are `cargo test` over the four workspace members.

## Requirements

* **Windows 10 version 1809 (build 17763) or later**, or Windows 11. Windows 10 earlier than 1809
  is refused by the installer rather than installed and broken.
* **WebView2**, the rendering engine the desktop shell uses. It is present on Windows 11 and on
  updated Windows 10; the installer fetches and installs it when it is absent, so an offline
  machine needs the runtime bundled with the installer media.
* **ffmpeg and ffprobe**, bundled with the installer. A studio that wants its own build can point
  `THE_TRIMMER_FFMPEG` and `THE_TRIMMER_FFPROBE` at it; `thetrimmer doctor` reports which build was
  found and which encoders it actually has. A codec pack's three-year-old ffmpeg with no libx265 is
  the case this override exists for.
* **Disk**: a cut reads the master and writes a segment, so the output volume needs room for the
  delivered file plus a head and body of working files during the run. Working files are cleaned up
  on success and on cancellation.
* **To build from source**: Rust 1.85 or later (`rust-version` in the workspace manifest), and a
  Cargo workspace with four members. `cargo test` in the repository root runs the suites that
  exist.

## Documentation map

| Document | Status | What is in it |
|---|---|---|
| [docs/DESIGN.md](docs/DESIGN.md) | written | The index of architecture decisions, V1 carried forward and V2 new. |
| [docs/adr/](docs/adr/) | written | The fifteen decision records themselves. |
| [docs/SKILLS-APPLIED.md](docs/SKILLS-APPLIED.md) | written | Which engineering practice informed which decision, and which were deliberately rejected. |
| `docs/TRUTH.md` | **not yet written** | What is implemented, what is not, and how each claim is checked. |
| `docs/SECURITY.md` | **not yet written** | The threat model: argv-array spawning, no shell, no `unsafe`, path canonicalisation, the licence file's size cap. |
| `docs/RELEASE.md` | **not yet written** | The release checklist and its rollback: build, sign, checksum, publish, verify the update feed, keep the previous artifact addressable. |
| `docs/PRICING.md` | **not yet written** | The editions, the terms, and what each includes. |
| `docs/CODE-REVIEW.md` | **not yet written** | The review brief and checklist the code is held to. |

The last five are named rather than linked because they do not exist yet; linking a document that
is not there is the kind of small lie this README is trying not to tell.

The V1 engine lives at `H:\THEROLLUPFILES\TheTrimmer` — a Python package, its own test suite, and
its own `docs/DESIGN.md`. It is the oracle (above) and the source of most of the reasoning in
`docs/adr/`.

## Licence

TheTrimmer is a commercial product, licensed, not sold, under a proprietary licence
(`LicenseRef-Proprietary` in the crate manifests). It is not open source and is not licensed for
redistribution.

The editions, what each includes, and the terms are set out in `docs/PRICING.md` — **which is not
yet written**, so this document deliberately states no prices and no terms. Licensing is verified
offline from a signed licence file, with no activation call: see
[ADR-015](docs/adr/015-licensing.md) for the mechanism and an honest account of its trade — an
offline licence cannot be revoked, and that is accepted in exchange for a tool that opens on a
plane.
