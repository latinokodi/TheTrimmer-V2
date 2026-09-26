# TheTrimmer

Frame-exact, verified segment cutting for professional post-production. TheTrimmer takes a master
and two Premiere timecodes and gives back the segment — with the original packets, not a
transcode — then measures what it produced against the source and tells you, with numbers, whether
it is right. It is a Windows desktop application.

> **Status of this document.** Every crate named below is a real workspace member and compiles:
> `trimmer-core` (the pure domain), `trimmer-media` (the only crate that starts a process),
> `trimmer-verify` (verdicts over measured facts), `trimmer-export` (timeline documents),
> `trimmer-app` (the application layer), `trimmer-store` (SQLite persistence), `trimmer-cli` (the
> command line), `trimmer-daemon` (the local API) and `apps/desktop` (the Tauri shell and the
> interface it drives). Ten crates, 404 tests, and a release build that produces
> `thetrimmer.exe`, `ttrim.exe` and `trimmer-daemon.exe`.
>
> The end-to-end test generates a clip with ffmpeg, cuts it through the head-patch method and
> asserts that the copied body is byte-identical to the source — and the differential oracle
> compares 626 decisions against the V1 engine. Nothing here is claimed that is not in the tree,
> and `docs/TRUTH.md` is the record of how each claim is checked.

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

There is **no installer**. The product is four executables that run from anywhere, and copying them
is the install — no elevation, no registry entry, no uninstaller, nothing to sign before Windows will
let them run.

| Binary | What it is |
|---|---|
| `thetrimmer-desktop.exe` | the window. Its interface is compiled into it, so it needs no asset folder beside it. |
| `thetrimmer.exe` | the command line: `doctor`, `probe`, `cut`, `batch`, `project`, `export`, `verify`, `watch` |
| `ttrim.exe` | the same command line under a short name |
| `trimmer-daemon.exe` | the loopback HTTP/JSON API |

```powershell
cargo build --release            # all four, into target\release
.\target\release\thetrimmer-desktop.exe
```

`start.bat` does the same thing from a double-click, and it is the better route: `tools/start.ps1`
rebuilds only what has actually changed, so a launch with nothing to do takes about two seconds
instead of relinking the window. It closes a window that is already open before linking — Windows will
not let the linker replace a running executable, and cargo reports that as `Access denied`, which reads
like a permissions problem. It also builds all four binaries rather than only the window.

```
start.bat                      build what changed, then launch
pwsh tools\start.ps1 -Check    say what is out of date and exit
pwsh tools\start.ps1 -Rebuild  rebuild both, then launch
```

`tools/smoke-window.ps1` starts the built window and checks it is really the application: that the
interface rendered *from the build just made*, and that the footer line reports what `doctor` returned —
a full round trip through `invoke` to Rust and back.

* **Uninstalling is deleting the file.** Your projects live in `%APPDATA%\TheTrimmer\projects.db` and
  are not part of the program, so removing one never touches the other. `project export` writes a
  project out as a readable document if you want it somewhere else.
* **ffmpeg is resolved, not bundled.** The engine finds ffmpeg and ffprobe on `PATH` and honours
  `THE_TRIMMER_FFMPEG` / `THE_TRIMMER_FFPROBE`, which is what lets a studio point the app at a build it
  has validated and what the `doctor` report is for. `doctor` names the prerequisite, exits 1 when it
  is missing, and prints the command that fixes it.
* **The executables are unsigned.** SmartScreen will warn on first run. That is what a code-signing
  certificate buys, and it is a commercial decision rather than a code change.
* **There is no auto-update.** Nothing publishes and nothing checks. An update feed is a server, a
  signing key and a decision about who hosts it, and it will be designed when it is wanted rather
  than left half-present — see [ADR-017](docs/adr/017-shipping.md), which also records the installers
  that were built, verified and then removed on request.
* **It works offline.** Nothing in the application does anything over a network. Every decision, every
  cut and every check is local: there is no activation call, no telemetry and no update check that
  blocks launch. This build has no licensing feature at all — see
  [ADR-015](docs/adr/015-licensing.md) for why, and for what was built and then removed.

The window is checked by starting it, not by reading it: `tools/smoke-window.ps1` launches the built
binary, waits for the interface to render **from the build that was just made**, and asserts that the
status bar reports what the `doctor` command returned — a full round trip through `invoke` to Rust and
back. It runs in CI as the `desktop` job.

## The workspace

The desktop shell is `apps/desktop`: a Tauri v2 window over a React interface in `apps/web`. The
page holds one capability — a file picker — and every other operation goes through a named Rust
command, so the webview never has a filesystem, a process or a database handle it could be tricked
into using. The application layer beneath it is `trimmer-app`, whose seams (`MediaEngine`,
`TranscriptSource`, `ProjectStore`, `Clock`) are what let the workspace model, transcript service,
watch-folder rules and batch queue be tested without media and without a disk.

### Working on the interface needs no build

The interface runs in a plain browser. `apps/web/src/ipc/stub.ts` installs a `window.__TAURI__` of the
same shape the window provides and answers the commands with a realistic fixture project, so
`npm run dev` gives the whole application — every empty state, the marks, the live frame numbers, the
queue, the proof panel — with no Rust, no WebView2, and no compilation. A CSS change is hot-reloaded
in under 100 ms.

```powershell
cd apps/web
npm ci
npm run dev            # the interface, in a browser, with a fixture project
npm test               # the formatting arithmetic, ~0.3 s
npm run e2e            # 10 browser tests over the real interface, ~6 s
```

The stub refuses to replace a bridge that is already present, so `installStub()` is called
unconditionally and is a no-op inside the real window. A command added to `COMMAND_NAMES` and not
stubbed is a compile error rather than a control that works in the window and does nothing in a test.

**The contract has two tests, of two different things.** The browser tests drive the real interface
and prove its behaviour; `cargo test -p thetrimmer-desktop --test ipc_contract -- --ignored` drives
the real commands through Tauri's real invoke handler and proves the *shape* of what Rust sends. The
stub agrees with the interface by construction, so it can never catch a renamed field — that is what
the second test is for, and it is the one that found `add_source` answering a different object from
the `sources` command, `get_verify_policy` refusing to answer before a project was open, and an out
point producing a segment one frame longer than the dialog promised. See
[ADR-016](docs/adr/016-dev-loop.md).

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

The proof panel is the whole of the verification surface: no scrubbing preview, no decode while a cut
is running, and no thumbnail strip — see [ADR-006](docs/adr/006-no-preview.md) for what was refused
and why.

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

**Implemented.** `cargo build --release` produces `thetrimmer.exe` and `ttrim.exe`, and every
subcommand below is wired to the engine. `thetrimmer doctor` on this machine reports ffmpeg 8.0.1
with libx264, libx265, aac, prores_ks, mpeg2video, loudnorm, ssim and the mp4 and mxf muxers all
present. Exit codes are `0` worked, `1` a check failed, `2` a usage error or a domain refusal, `130`
interrupted.

```
thetrimmer probe      describe a source: rate, frames, timescale, audio, warnings
thetrimmer cut        cut one segment
thetrimmer batch      cut every segment in a project, or a list of marker files
thetrimmer project    create, list, inspect and export projects
thetrimmer export     write a timeline document
thetrimmer verify     check a finished file against its source
thetrimmer doctor     what the app found: ffmpeg, its encoders, ffprobe, the project store
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

`cargo build --release` also produces `trimmer-daemon.exe`. The API is a local HTTP/JSON surface on a
**loopback port only** — `DaemonConfig::validate` refuses any other bind, because this surface can
cut files and delete projects and is therefore a control surface for one machine rather than a
service — requiring a **bearer token** of at least 16 characters, compared in constant time. One
resource shape per noun, `camelCase` on the wire, and errors carrying a machine-readable word
alongside the sentence so a client can branch instead of parsing English prose.

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

**How each claim is checked.**

* The **differential oracle** is `crates/trimmer-core/tests/oracle.rs` with
  `tools/oracle/run_oracle.py`. It generates cases, runs each through the Rust core and through the
  V1 engine — imported from its own checkout, never vendored — and compares the answers: 384
  timecode renderings, 226 parses, 11 cut plans and 5 caption retimes. It skips loudly when Python
  or the V1 checkout is absent, because a test that cannot tell "not applicable here" from "the
  engine is wrong" gets deleted by the first person it inconveniences.
* The **end-to-end trim** is `crates/trimmer-media/tests/end_to_end.rs`. It generates a 300-frame
  clip with a known keyframe grid, cuts a range that deliberately does *not* begin on a keyframe,
  and asserts the three things a silent bug would break: the copied body decodes to the same MD5s
  as the source, the head lands on the mark (by SSIM against its neighbours), and the length is
  exact. It also asserts the head encode carries `-video_track_timescale 90000` and does *not*
  carry `-frames:v`, because those two flags are the defects that cost the most time in V1.
* **Continuous integration** is `.github/workflows/ci.yml`: formatting, lints at `-D warnings`,
  the full suite, and the end-to-end cut, on Windows with ffmpeg installed.
* **`doctor` against the real ffmpeg** is asserted by a test, not only printed. The capability
  report was wrong twice — it claimed a good build had no `mp4` muxer and no `loudnorm` — and a
  fixture that does not match reality is exactly what hid it.

The suites that run are `cargo test --workspace` over ten crates: 404 tests.

## Requirements

* **Windows 10 version 1809 (build 17763) or later**, or Windows 11. There is no installer to refuse
  an older build, so this is what the window is built and tested against.
* **WebView2**, the rendering engine the desktop shell uses. It is present on Windows 11 and on
  updated Windows 10, and Microsoft ships it as an evergreen runtime; a machine that has never had it
  needs it once, from Microsoft, before the window will open. The command line and the daemon do not
  use it and run without it.
* **ffmpeg and ffprobe**, resolved from `PATH`. A studio that wants its own build can point
  `THE_TRIMMER_FFMPEG` and `THE_TRIMMER_FFPROBE` at it; `thetrimmer doctor` reports which build was
  found and which encoders it actually has. A codec pack's three-year-old ffmpeg with no libx265 is
  the case this override exists for.
* **Disk**: a cut reads the master and writes a segment, so the output volume needs room for the
  delivered file plus a head and body of working files during the run. Working files are cleaned up
  on success and on cancellation.
* **To build from source**: Rust 1.85 or later (`rust-version` in the workspace manifest), and a
  Node 20 or later toolchain for the interface — the interface build is independent of the Rust one,
  and the interface runs in a browser without Rust at all (see *Working on the interface needs no
  build* above).

## Documentation map

| Document | What is in it |
|---|---|
| [docs/DESIGN.md](docs/DESIGN.md) | The index of architecture decisions, V1 carried forward and V2 new. |
| [docs/adr/](docs/adr/) | The seventeen decision records themselves. |
| [docs/SKILLS-APPLIED.md](docs/SKILLS-APPLIED.md) | Which engineering practice informed which decision, and which were deliberately rejected. |
| [docs/TRUTH.md](docs/TRUTH.md) | Every claim this product makes and the mechanism that checks it, plus what is deliberately not checked. |
| [docs/audit/security.md](docs/audit/security.md) | The threat model and the security audit: argv-array spawning at all four spawn sites, no shell, no `unsafe`, path canonicalisation, parameterised queries, and the daemon's loopback-only bind. |
| [docs/audit/](docs/audit/) | Five independent audits, each with findings by severity and a "checked and clean" section. |
| [`.github/workflows/ci.yml`](.github/workflows/ci.yml) | Formatting, lints at `-D warnings`, the full Rust suite, the IPC contract, the browser suite, and a real cut on every push. |

The V1 engine lives at `H:\THEROLLUPFILES\TheTrimmer` — a Python package, its own test suite, and
its own `docs/DESIGN.md`. It is the oracle (above) and the source of most of the reasoning in
`docs/adr/`.

## Software licence

TheTrimmer is a commercial product, licensed and not sold, under a proprietary licence
(`LicenseRef-Proprietary` in the crate manifests). It is not open source and is not licensed for
redistribution.

That is the *software* licence — the terms the code is distributed under. It is a separate thing
from a *licensing feature* inside the product, which this build does not have: there is no
entitlement check, no seat count and no key to install. Distribution and pricing are decided by how
the application is sold and delivered rather than by anything it verifies at startup. An earlier
draft specified an offline signed licence file and a crate was built and tested to that
specification; it was removed at the product owner's direction, and
[ADR-015](docs/adr/015-licensing.md) records both the mechanism and the decision so the work is not
lost.

