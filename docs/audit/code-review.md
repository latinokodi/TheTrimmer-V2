# TheTrimmer V2 — code review audit

> **This is a record of a review, not a list of open work.** It was written against the tree as it
> stood when it was run, and the findings that mattered have since been fixed — the fix for each is a
> commit, and the current state of the product is [docs/TRUTH.md](../TRUTH.md) and the test suite.
> Line numbers below refer to the revision that was read and will have moved.
>
## Scope

This review covers the Rust workspace at `H:\THEROLLUPFILES\TheTrimmer-V2` as it stands in the working tree, excluding the generated `target/` and `node_modules/` trees. I read the following source files in full, by direct inspection with line-level reading rather than by trusting the crate summaries:

* `crates/trimmer-core/src/`: `lib.rs`, `error.rs`, `timecode.rs`, `domain.rs`, `plan.rs`, `caption.rs`, `delivery.rs`, `transcript.rs` (plus `tests/oracle.rs` for the pinned expectations).
* `crates/trimmer-media/src/`: `lib.rs`, `tool.rs`, `probe.rs`, `process.rs`, `executor.rs`.
* `crates/trimmer-app/src/`: `ports.rs`, `workspace.rs`, `queue.rs` (the verification and status-transition paths), plus `crates/trimmer-app/tests/application.rs` for the intended contracts.
* `crates/trimmer-store/src/`: `lib.rs`, `schema.rs`, `store.rs`, `document.rs`.
* `crates/trimmer-verify/src/`: `lib.rs`, `check.rs`, `facts.rs`, `measure.rs`, `audit.rs` (I read `check.rs` myself at the points that decide pass/fail and delegated a full read of the file).
* `crates/trimmer-export/src/`: `lib.rs`, `request.rs`, `xml.rs`, `write.rs`, `edl.rs`, `premiere.rs`, `clips.rs` (and a full read of `fcpxml.rs`, `csv.rs` by a second reviewer whose quotations I checked against the file).
* `crates/trimmer-daemon/src/`: `lib.rs`, `config.rs`, `state.rs`, `routes.rs` (full), `main.rs`, `openapi.rs`, plus the `axum` 0.8.9 / `axum-core` 0.5.6 sources in the local registry for extractor behaviour.
* `crates/trimmer-cli/src/`: `cli.rs`, `context.rs`, `lib.rs`, `project.rs`, and the `verify`, `run`, `export`, `daemon` and `watch` paths of `commands.rs`.
* `apps/desktop/src-tauri/src/`: the workspace member is in scope because it exposes the same operations to a web renderer; I read its `lib.rs`, `state.rs` and the `commands.rs` sites that touch the domain (`:204`, `:538`, `:736`, `:876-956`) directly and commissioned a full read of the file. Its findings are folded into the sections below and identified as such.

Where a claim depends on a runtime behaviour I could not execute in this environment (ffmpeg, a second process holding the SQLite file, an FCPXML importer), I say so at the finding. Findings marked *latent* are real defects in code that no production path currently reaches; I say which path reaches it, if any. Two areas are explicitly **out of scope** and were not verified: the TypeScript front end under `apps/desktop/src`, and the CI configuration under `.github/`.

## Findings

Severity is assigned on impact, not on how unusual the trigger is:

* **critical** — silently produces a wrong file or a wrong claim about a file, with no way for the operator to notice.
* **major** — wrong or lost output, a dead feature, or a panic, in a plausible scenario.
* **minor** — a real defect with a narrow trigger or a recoverable consequence.
* **nit** — misleading comment, dead code, or style.

### trimmer-core

| # | Severity | Location | What is wrong |
|---|---|---|---|
| C1 | major | `timecode.rs:728-757` | `format_timecode` divides by zero for a rate whose `nominal()` is 0 |
| C2 | major | `timecode.rs:618-623` | Unbounded `hours` with unchecked arithmetic overflows the label silently |
| C3 | major | `delivery.rs:602-614` (+ `media/executor.rs:858`) | `podcast_audio` asks for AAC in a `.wav`; audio-only presets encode video into WAV |
| C4 | major | `delivery.rs:313-328` | `Geometry::is_native()` makes every `fit: Native` preset ignore its own target size |
| C5 | minor | `caption.rs:104-106`, `394-400` | `Transcript.trailing` is documented as kept but is always empty |
| C6 | minor | `domain.rs:26-28`, `147-164` | `MediaPath` derives `Eq`/`Hash` case-sensitively but implements `Ord` case-insensitively |
| C7 | minor | `plan.rs:527-540`, `domain.rs:622-628` | Unchecked `+`/`-` on handles; `resolve_range`'s doc names an error it cannot return |
| C8 | nit | `domain.rs:622-628`, `plan.rs:527-540`, `export/clips.rs:145-148` | Three hand-kept copies of the handle clamp; one is dead and diverges |

#### C1 — major — `format_timecode` panics (divide by zero) on a rate the crate accepts

`crates/trimmer-core/src/timecode.rs:749-754`:

```rust
    let hours = frame / (nominal * 3600);
    let rest = frame % (nominal * 3600);
```

`nominal` comes from `FrameRate::nominal()` (`timecode.rs:240-247`), which returns `rounded as i64` whenever `|value - round(value)| < 0.001` — so **any** rate below 0.001 fps has `nominal() == 0`. `FrameRate::new` accepts such rates deliberately; the crate's own test asserts that a container timescale is legal (`timecode.rs:1065-1069`):

```rust
        // A container timescale is a legitimate low rate and must not be refused.
        assert_eq!(FrameRate::parse("1/90000").expect("ok").as_ffmpeg(), "1/90000");
```

`FrameRate::parse("1/1001")` is likewise accepted and yields `nominal() == 0`. `format_timecode` then computes `frame / 0`, which panics on every Rust build (unlike overflow, this is not affected by `overflow-checks`). The release profile sets `panic = "abort"` (`Cargo.toml:57-61`), so in the daemon this kills the process.

Reachable paths I verified:

* `MediaInfo::timecode_of` (`domain.rs:341-343`) → used by the CLI (`commands.rs:113-114`, `159-160`, `337`) and by `Workspace::output_path` (`workspace.rs:638`).
* `CutPlan::describe` (`plan.rs:321-322`).
* The EDL writer's *source* stamps: `format_timecode(clip.source_in, clip.rate, None)` (`trimmer-export/src/edl.rs:84-85`). The export layer guards the *timeline* rate (`export/clips.rs:227-232` checks `rate.nominal() <= 0`) but not each clip's own rate, so a source whose probed rate has `nominal() == 0` panics the export.

Fix: make the precondition explicit. Either reject the un-timecodable rates at construction (`FrameRate::new`: refuse `as_f64() < RATE_EPSILON` unless the rate is never used as a timecode grid — the `Timescale` type exists precisely so that timescales are not frame rates, so `Timescale::from_ffprobe_time_base` should stop going through `FrameRate::parse`), or change `format_timecode` to return `CoreResult<String>`/`Option<String>` and have `nominal()` saturate at 1 with a documented refusal. Nothing should be able to call `format_timecode` with a zero nominal.

#### C2 — major — unchecked arithmetic in `parse_timecode` silently wraps in release

`crates/trimmer-core/src/timecode.rs:618-623`:

```rust
    let mut label = ((hours * 3600 + minutes * 60 + seconds) * nominal) + frames;
    if drop {
        let skipped = rate.drop_labels_per_minute();
        let total_minutes = hours * 60 + minutes;
        label -= skipped * (total_minutes - total_minutes / 10);
```

`hours`, `minutes`, `seconds` and `frames` are parsed as `i64` with only two bounds applied (`minutes > 59`, `seconds > 59`, `frames >= nominal`); `hours` is unbounded (`timecode.rs:583-595`). `is_plain_number` accepts arbitrarily many digits, so `parse_timecode("2562047788015216:00:00:00", rate)` overflows `hours * 3600` and a value such as `"300000000000000:00:00:00"` overflows the multiplication by `nominal`. In a debug build this panics; in the release profile (overflow checks off) it wraps and returns a plausible-looking frame number, which `plan_cut` then accepts if it happens to land inside the source (`plan.rs:392-418` checks only sign and the end of the file). For a product whose entire claim is frame-exactness, a timecode typo silently becoming a different, valid frame is the worst available outcome.

Fix: bound the hours field to something a source can contain (or to `9999`) and use `checked_mul`/`checked_add` with `CoreError::Timecode { reason: "the hours field is out of range" }`, so an unrepresentable request is refused rather than wrapped.

#### C3 — major — the two audio-only presets cannot be produced

`crates/trimmer-core/src/delivery.rs:602-614`:

```rust
        DeliveryPreset {
            name: "podcast_audio".to_owned(),
            description: "Audio only, AAC 192k in WAV, normalised to -16 LUFS. For an \
                          audio-first feed."
                .to_owned(),
            container: Container::Wav,
            video: VideoTreatment::None,
            audio: AudioTreatment::aac_stereo("192k"),
```

A WAV file is PCM; there is no configuration in which ffmpeg writes AAC into a `.wav`. `DeliveryPreset::validate` (`delivery.rs:394-456`) checks the video treatment against the container and the audio *rates*, but never checks that the audio encoder can live in the container, so `every_standard_preset_is_valid` (`delivery.rs:701-708`) passes and the defect ships.

The executor then makes it worse rather than catching it: `prepare_reencode` (`crates/trimmer-media/src/executor.rs:858-869`) always maps and encodes the picture —

```rust
    args.extend(map_streams(plan.has_audio));
    args.extend(["-vf".to_owned(), filters.join(",")]);
    args.extend([
        "-c:v".to_owned(),
        encoder,
```

— and `Container::carries_video()` has no caller anywhere in the workspace (`grep carries_video` matches only `delivery.rs`). `preserves_picture` is false for `VideoTreatment::None`, so `cut_with_plan` routes these presets to `reencode_segment`, which writes an H.264 stream into a `.wav`. ffmpeg fails with a container error and the job fails; both presets are `batch_safe: true`, so an unattended batch fails every job it selects.

Fix: either express the audio-only presets correctly (`container: Mp4`/a new `M4a`-style extension with AAC, or `AudioTreatment::Encode { encoder: "pcm_s24le", .. }` in WAV and update the description), add a container/codec compatibility rule to `validate()`, and honour `container.carries_video()` in `prepare_reencode` by omitting `-map 0:v:0`, `-c:v` and the video filter. See also C16.

#### C4 — major — `Geometry::is_native` makes 1080p presets deliver the source's resolution

`crates/trimmer-core/src/delivery.rs:313-328`:

```rust
    pub const fn is_native(self) -> bool {
        self.width == 0 || self.height == 0 || matches!(self.fit, AspectFit::Native)
    }
...
    pub fn forces_encode(self, source_width: u32, source_height: u32) -> bool {
        if self.is_native() {
            return false;
        }
```

`Geometry::HD_1080` is `{ width: 1920, height: 1080, fit: AspectFit::Native }` (`delivery.rs:289-293`), so `is_native()` is true for it, `forces_encode(3840, 2160)` is false, and `filters()` (`delivery.rs:336-339`) returns an empty vector. `prepare_reencode` therefore applies no `scale` at all (`executor.rs:857`), and the `youtube_1080` preset — described as "1920x1080 H.264 at CRF 18" (`delivery.rs:532`) — delivers a re-encoded 4K file on a 4K source. `broadcast_r128` and `mxf_op1a` have the same geometry and the same problem. The existing test only exercises the case where the source is already 1920x1080 (`delivery.rs:741`), which is exactly the case where the bug is invisible.

Fix: `is_native` cannot be a property of the geometry alone — the target size has to be compared with the source's. Either drop `AspectFit::Native` from the sized geometries (leaving the unscaled case expressed as `Geometry::NATIVE`, width/height 0) or make `forces_encode` compare `width != source_width || height != source_height` before consulting `fit`, and have `filters()` emit a `scale` when the size differs.

#### C5 — minor — `Transcript.trailing` is documented as preserved and is always dropped

`crates/trimmer-core/src/caption.rs:105-106`:

```rust
    /// The text after the last cue, kept verbatim so nothing is silently lost.
    pub trailing: String,
```

`read()` (`caption.rs:394-400`) always sets `trailing: String::new()`, and no other code path ever writes it (`grep trailing` matches only the field, the two initialisers, and the tests). Any text after the last cue — including an entire file that happens to carry no timing line — is dropped without a warning, while the field's documentation promises the opposite.

Fix: populate it in `read` (the parser already knows where the last block ended) or delete the field and the claim; a field that promises preservation and never delivers it is worse than not having it.

#### C6 — minor — `MediaPath`'s `Eq` and `Ord` disagree on Windows

`crates/trimmer-core/src/domain.rs:26-28` derives equality and hashing over a `PathBuf` (case-sensitive), while `domain.rs:147-164` hand-implements `Ord` by lowercasing both sides on Windows. The doc comment claims both follow the platform's rules:

```rust
/// Ordering and equality follow the platform's case rules.
```

They do not: `H:\a.mp4` and `h:\a.mp4` compare `Ordering::Equal` and are `!=`. That violates the `Ord`/`Eq` consistency contract, and it is load-bearing: `BTreeMap<MediaPath, SegmentSource>` collapses the two spellings (which is the stated intent) while a `HashMap` or a `dedup()` would keep both, and the store's row loop silently overwrites one with the other (`trimmer-store/src/store.rs:499-511`). `MediaPath::same_file_as` (`domain.rs:99-107`) is the honest comparison and is not used here.

Fix: implement `PartialEq`/`Eq`/`Hash` to fold case the same way `Ord` does, so the two relations agree; or keep `Eq` exact and make `Ord` exact too, moving the case-folding into an explicit `key()` used by the maps.

#### C7 — minor — unchecked handle arithmetic, and a doc that names an error the code cannot return

`crates/trimmer-core/src/plan.rs:527-540`:

```rust
    let handles = segment.handle_frames.max(0);
    let raw_end = segment.end_frame.unwrap_or(media.frame_count);
    let start = (segment.start_frame - handles).max(0);
    let end = (raw_end + handles).min(media.frame_count);
```

Handle arithmetic is unchecked here and in the near-duplicate `Segment::range_with_handles` (`domain.rs:622-628`), while the export layer does the same sum with saturating operations (`trimmer-export/src/clips.rs:147-148`) — the safe pattern exists in this workspace and was not used here. `handle_frames` is attacker-controllable through the daemon: `add_segment` rejects only negative values (`routes.rs:438-443`), so `handle_frames = 9223372036854775807` with a non-zero `raw_end` overflows at line 531: a panic in a debug build, a wrapped (negative) end in release, where it degrades to an `EmptyRange` refusal.

The doc block above it also claims a contract the code does not implement: "Returns `CoreError::BeforeStart` or `CoreError::PastEnd` when the segment names frames the source does not have". `resolve_range` can only return `PastEnd`; a negative `start_frame` is silently clamped to 0 by `.max(0)` — it is `plan_cut` that refuses that case (`plan.rs:392-396`).

Fix: use `saturating_sub`/`saturating_add`, and either make `resolve_range` refuse a negative start or correct the doc to say that the caller must validate first.

#### C8 — nit — three copies of the handle clamp, one dead and divergent

`Segment::range_with_handles` (`domain.rs:622-628`) is called only from its own tests; `plan::resolve_range` (`plan.rs:527-540`) is the one the planner uses; and `trimmer-export/src/clips.rs:142-148` is a third copy whose comment admits the duplication ("`plan::resolve_range` is the same rule, kept in step by hand"). The three differ: only the export copy saturates, and only `range_with_handles` ends with `.max(start)`. Fix: one public function in `trimmer-core` that returns the clamped range plus a flag saying whether it was clamped, used by all three.

### trimmer-media

| # | Severity | Location | What is wrong |
|---|---|---|---|
| M1 | major | `probe.rs:136-146` | An unreadable frame rate silently becomes 30 fps |
| M2 | major | `executor.rs:874-893` | `preset.audio` is ignored: AAC is hard-coded, "drop audio" is impossible |
| M3 | minor | `executor.rs:453-459` | The existing output is deleted first: a TOCTOU that also fails the cut |
| M4 | minor | `probe.rs:152-156` | An unreadable timebase silently becomes 90 kHz |
| M5 | minor | `probe.rs:252` | A non-UTF-8 path becomes an empty ffprobe argument |
| M6 | minor | `process.rs:295-310` | Unbounded output buffering; a read error is treated as end-of-file |
| M7 | nit | `process.rs:204-211` | A poisoned mutex is reported as "no events" |
| M8 | major | `app/ports.rs:250-265` | A failed keyframe probe is silently downgraded to a full re-encode |

#### M1 — major — a frame rate that cannot be read becomes 30 fps

`crates/trimmer-media/src/probe.rs:136-146`:

```rust
        let rate = video
            .r_frame_rate
            .as_deref()
            .and_then(|text| FrameRate::parse(text).ok())
            .or_else(|| {
                video
                    .avg_frame_rate
                    .as_deref()
                    .and_then(|text| FrameRate::parse(text).ok())
            })
            .unwrap_or(FrameRate::FPS_30);
```

ffprobe omits `r_frame_rate`/`avg_frame_rate` on some streams and reports the literal `0/0` on others; `FrameRate::parse("0/0")` fails on the zero denominator (`timecode.rs:149-151`), so the probe falls through to **30 fps**. Every derived fact then uses the wrong grid: `frame_count` is `duration × rate` when `nb_frames` is absent (line 175), `seconds_of` positions the `-ss`/`-t` arguments (`executor.rs:614, 687, 717, 798, 846`), and `frames_in` converts the user's timecodes. The domain's own documentation names this exact failure — "guessing a rate is exactly how timecode silently becomes wrong" (`domain.rs:733-735`) — and `Project::rate_for` refuses to guess for the same reason. Here the guess is made silently, with no note in the plan.

Fix: treat an unparseable frame rate as a probe failure (`MediaError::BadProbe` naming the file and the text ffprobe returned). A file the tool cannot count frames on is not a file it can cut frame-exactly.

#### M2 — major — the delivery preset's audio treatment is never applied

`crates/trimmer-media/src/executor.rs:874-893` (`prepare_reencode`):

```rust
    if plan.has_audio {
        if let Some(audio) = &media.audio {
            let mut audio_filter = vec!["asetpts=PTS-STARTPTS".to_owned()];
            if let Some(loudness) = preset.loudness {
                audio_filter.push(loudness.filter_args());
            }
            args.extend([
                "-af".to_owned(),
                audio_filter.join(","),
                "-c:a".to_owned(),
                "aac".to_owned(),
                "-b:a".to_owned(),
                config.audio_bitrate.clone(),
                "-ar".to_owned(),
                audio.sample_rate.to_string(),
                "-ac".to_owned(),
                audio.channels.to_string(),
            ]);
```

`preset.audio` is never read anywhere in the crate (`grep AudioTreatment` in `trimmer-media` matches nothing), and `-c:a aac` is hard-coded from the *source's* sample rate and channel count. The consequences, all silent:

* `AudioTreatment::None` ("Drop the audio entirely", `delivery.rs:147`) does not drop it — there is no `-an` anywhere in the workspace.
* `AudioTreatment::Copy` re-encodes to AAC in the full-encode path.
* `AudioTreatment::Encode { encoder: "pcm_s24le", .. }` — requested by `prores_master`, `wav_split` and `broadcast_r128` — produces AAC instead of PCM, so a ProRes master or a broadcast delivery goes out with the wrong audio codec, and `wav_split` ("48 kHz 24-bit stereo WAV, untouched") is neither 24-bit nor untouched.
* `bitrate: "unused"` in those presets documents the mismatch rather than fixing it.

Fix: match on `preset.audio` to emit `-an`, `-c:a copy`, or the named encoder with its own bitrate/sample rate/channel count, and honour `container.carries_video()` (see C3). Note the head-patch path deliberately re-encodes the head's audio (`executor.rs:637-653`); that is a separate decision and is documented there.

#### M3 — major — the deliverable is written to the wrong directory, and the frame field is eaten

`crates/trimmer-media/src/executor.rs:453-459`:

```rust
        if output.exists() {
            std::fs::remove_file(output).map_err(|error| MediaError::WorkingFile {
                path: output.display().to_string(),
                reason: error.to_string(),
            })?;
        }
```

This is a check-then-act on a path another process may own: if the file is removed between the two calls, `remove_file` returns `NotFound` and the whole cut fails even though the following rename would have succeeded. It also deletes whatever is at the output path with no confirmation — and the other two cut paths (`copy_segment`, `reencode_segment`) pass the same path to ffmpeg, which is invoked with `-y` (`executor.rs:584`), so they overwrite in place. Whether any of that is even the *right* path is the application layer's answer, and that answer is wrong; see A1.

#### M4 — minor — an unreadable timebase becomes 90 kHz

`crates/trimmer-media/src/probe.rs:152-156` falls back to `Timescale::NINETY_KHZ` when `time_base` is missing or not of the form `1/n`. This is the same silent substitution that `PlanInvariant::HeadMatchesSourceTimebase` exists to prevent: the head is encoded at a timescale the body may not share, which is the V1 ADR-001 slow-motion defect that the whole design is organised around. The practice is common enough (90 kHz is the MP4 norm) that the risk is lower than M1, but the substitution is still unreported. Fix: refuse, or record a note on the plan that the timescale was assumed.

#### M5 — minor — a non-UTF-8 path becomes an empty ffprobe argument

`crates/trimmer-media/src/probe.rs:252`:

```rust
            media.path.as_path().to_str().unwrap_or_default(),
```

A path that is not valid UTF-8 (legal on Windows and on Linux) silently becomes `""`, so ffprobe is invoked with an empty argument and fails with a message about the wrong thing. The sibling function does it correctly: `probe_json` uses `path.as_os_str().to_os_string()` (`probe.rs:293`), and `ProcessRunner` takes `OsString` arguments throughout. Fix: `media.path.as_path().as_os_str().to_os_string()`.

#### M6 — minor — unbounded capture, and a read error reported as end-of-file

`crates/trimmer-media/src/process.rs:295-310`:

```rust
        match tokio::time::timeout(Duration::from_millis(1), pipe.read(chunk)).await {
            Ok(Ok(0) | Err(_)) => return (total, true),
```

Two issues in one function. (a) `into.extend_from_slice(...)` accumulates the child's entire stdout and stderr in memory with no cap, so a tool that emits gigabytes can exhaust the process (`-v error` keeps ffmpeg quiet, but the runner is generic and the daemon is long-lived). (b) A read *error* is reported as end-of-file, so a transient error truncates the captured output — including the ffmpeg diagnostics that explain a failure, which is precisely what the module's doc block argues against treating as equivalent. Fix: cap both buffers (keep a head and a tail, as `stderr_tail` already does for display), and distinguish `Err(_)` from `Ok(0)`.

#### M7 — nit — a poisoned mutex reads as "no events"

`crates/trimmer-media/src/process.rs:204-211`: `CollectingSink::events` uses `.lock().map(...).unwrap_or_default()`, so a test whose reporting thread panicked sees an empty event list and fails with a misleading assertion instead of the poison. `report` at line 216 already ignores the error silently. Fix: `expect("the sink lock")` or propagate.

#### M8 — major — a failed keyframe probe is silently converted into a full re-encode

`crates/trimmer-app/src/ports.rs:250-265`:

```rust
    async fn plan(&self, media: &MediaInfo, segment: &Segment) -> CoreResult<CutPlan> {
        let to = segment.end_frame.unwrap_or(media.frame_count);
        // A keyframe listing that fails is not a reason to refuse the cut. ...
        let grid = self
            .executor
            .prober()
            .keyframes(media, segment.start_frame, to)
            .await
            .unwrap_or_else(|_| {
                trimmer_core::KeyframeGrid::new(Vec::new(), segment.start_frame, to)
            });
        trimmer_core::plan_cut(media, segment, &grid)
    }
```

The ffprobe failure is discarded. An empty grid makes `plan_cut` take the `unusable` branch (`plan.rs:463-477`), which produces `CutMode::Reencode` and attaches the note "no keyframe between frames {start} and {end}: the whole segment is re-encoded, because a stream-copied body has to begin on a keyframe". The operator is told something about their source that may be false (the source may be full of keyframes — ffprobe just could not be run), and the lossless result they paid for is silently replaced by a full transcode. The `preview` route shows the same misleading note.

Fix: keep the degradation if it must exist, but preserve the cause — either propagate the error (`CoreError` has no variant for it today, so add one) or add a distinct note/warning such as "the keyframe listing failed (<error>); the segment is re-encoded in full", and surface it in `QueuePreview.problems` so the operator can act on it.

### trimmer-app

| # | Severity | Location | What is wrong |
|---|---|---|---|
| A1 | major | `workspace.rs:621-644` + `core/domain.rs:87-91` | Deliverables land in the wrong directory, and the frame field is eaten so names collide |
| A2 | minor | `queue.rs:826-843` | `delivered_seconds` is hard-coded to `0.0` in the report and every JSON response |
| A3 | minor | `queue.rs:612-624` | `stop_on_error` reports the run as *cancelled* |
| A4 | minor | `queue.rs:620-624` | Segments after an early stop vanish from `jobs`, so `total()` understates the batch |
| A5 | minor | `workspace.rs:250-268` | `refresh_sources` swallows the probe error its doc promises to return |
| A6 | minor | `workspace.rs:332-336` | A vanished source with cached facts is still described and counted runnable |
| A7 | minor | `workspace.rs:498-503` | A failed `preview` becomes "no commands" instead of a problem |
| A8 | minor | `watch.rs:140-144`, `253`, `265-276`, `379` | Watch folder: own outputs are eligible for re-cutting, the sidecar has a TOCTOU, casing is folded before the filesystem |
| A9 | nit | `queue.rs:60-62` | `JobState::Done` is never emitted |
| A10 | nit | `queue.rs:313-318`, `transcript.rs:239`,`361`, `media/process.rs:204-211` | A poisoned mutex is reported as "no events" |

#### A1 — major — the output directory is used as a file name, and the frame field is discarded

`crates/trimmer-app/src/workspace.rs:621-644` computes where a deliverable goes:

```rust
    pub fn output_path(&self, segment: &Segment) -> MediaPath {
        let directory = self.project.output_dir.clone().unwrap_or_else(|| {
            MediaPath::new(
                segment
                    .source
                    .as_path()
                    .parent()
                    .unwrap_or(std::path::Path::new(".")),
            )
        });
...
        let start = media.map_or_else(
            || format!("frame{}", segment.start_frame),
            |media| {
                media
                    .timecode_of(segment.start_frame)
                    .replace([':', ';'], ".")
            },
        );
        let sanitised = sanitise_name(&segment.name);
        directory.with_suffix(&format!(" {sanitised} {start}"), extension)
    }
```

`directory` is a **directory** (`project.output_dir`, or the source's parent). It is then handed to `MediaPath::with_suffix` (`crates/trimmer-core/src/domain.rs:87-91`), which is a *file-name* operation:

```rust
    pub fn with_suffix(&self, suffix: &str, extension: &str) -> Self {
        let mut name = self.stem();
        name.push_str(suffix);
        Self(self.0.with_file_name(name).with_extension(extension))
    }
```

Two defects follow.

**(a) The last path component of the directory is replaced, so the file lands one level up.** `Path::with_file_name` is `pop()` + `push()`, i.e. it replaces the final component rather than adding one. For a source `H:\THEROLLUPFILES\session1\master.mp4` with `output_dir: None`, the directory is `H:\THEROLLUPFILES\session1`, whose stem is `session1`; `with_file_name` drops `session1` and pushes `session1 begging 00.00.33.10`, leaving `H:\THEROLLUPFILES\session1 begging 00.00.33.10`, and `with_extension("mp4")` then rewrites what it takes to be the extension (see (b)), giving `H:\THEROLLUPFILES\session1 begging 00.00.33.mp4` — in `H:\THEROLLUPFILES`, not beside the source. With `output_dir = H:\deliver` the deliverable is `H:\deliver begging 00.00.33.mp4`, i.e. in `H:\`, not in `H:\deliver`. Both contradict `Project::output_dir` ("Where outputs are written. `None` means beside each source", `domain.rs:681`) and `output_path`'s own doc ("One place, so a batch, an export and the interface cannot disagree"). A folder whose name contains a dot (`deliver.2024`) also loses the `.2024`, because `stem()` stops at the first dot. On a read-only parent (a drive root, a NAS top level, or a session folder beside a locked share) every job in the batch fails, and nothing in the code or the tests covers this: `output_path` has no test at all (`grep output_path` finds only its definition and two call sites).

**(b) `with_extension` eats the frame field, so two different segments can collide.** `start` is `00.00.33.10`, and `with_suffix` ends in `with_extension(extension)`, which replaces everything after the **last dot** — so the frame field is always discarded (`... 00.00.33.mp4`), defeating the stated purpose ("the range is dotted rather than colon-separated because a colon is not legal in a Windows file name", `workspace.rs:616-619`). Names are therefore unique only to the second: two segments named `intro` (or two unnamed segments, which `sanitise_name` turns into `segment`, `workspace.rs:749-750`) starting anywhere in the same second produce the same path. Nothing checks for an existing output, and ffmpeg runs with `-y`, so the second job silently replaces the first deliverable and both are reported as `Succeeded`.

Fix: build the file name explicitly and join it to the directory — `MediaPath::new(directory.as_path().join(format!("{source_stem} {sanitised} {start}.{extension}")))` — include the source's own stem (the segment name alone is not unique), and either refuse to overwrite an existing output or disambiguate it with a warning. Then make the publish step atomic (write to a temp name in the destination and rename over it, tolerating `NotFound`; see M3).

#### A2 — minor — the batch report's delivered duration is always zero

`crates/trimmer-app/src/queue.rs:826-843`:

```rust
fn delivered_of(status: &JobStatus) -> Option<(i64, f64)> {
    match status {
        JobStatus::Succeeded {
            frames,
            verification,
            ..
        }
        | JobStatus::Unverified {
            frames,
            verification,
            ..
        } => {
            let _ = verification;
            Some((*frames, 0.0))
        }
```

`delivered_seconds` is accumulated from this (`queue.rs:579-582`), printed by `report()` (`queue.rs:427-431`), and emitted as `deliveredSeconds` by the daemon (`trimmer-daemon/src/routes.rs:886`) and the desktop shell (`apps/desktop/src-tauri/src/commands.rs:736`). All of them always show `0.0` / `0:00:00`. The `let _ = verification;` is the discarded data that should have supplied the duration. Fix: derive the seconds from the delivered frames and the plan's rate (or carry them on `JobStatus`), and delete the `let _`.

#### A3 — minor — a `stop_on_error` break is reported as a cancellation

`crates/trimmer-app/src/queue.rs:612-624`:

```rust
            let stop = options.stop_on_error
                && matches!(
                    status,
                    JobStatus::Failed {
                        cancelled: false,
                        ..
                    }
                );
            jobs.push((job, *id, name, status));
            if stop || cancelled {
                cancelled = cancelled || stop;
                break;
            }
```

`cancelled = cancelled || stop` makes `stop` set `cancelled`, so a batch that stopped because one segment was bad reports `BatchOutcome.cancelled == true` ("True when the run was cancelled", `queue.rs:340-341`) and prints "the run was cancelled; the segments below were not attempted" (`queue.rs:421-426`). The CLI surfaces it verbatim as `"cancelled": true` (`commands.rs:532`), so a UI cannot tell "the operator stopped this" from "one segment was defective". Fix: keep the two concepts separate — `let stopped_early = stop || cancelled; break;` — and never fold `stop` into `cancelled`.

#### A4 — minor — segments after an early stop are missing from the outcome

The `break` at `queue.rs:621-624` skips the remaining segments without recording them, while the field is documented as "One entry per segment, in order" (`queue.rs:332-333`). The pre-start cancellation path does the opposite and records one `Failed { cancelled: true }` per remaining segment (`queue.rs:555-567`), and the suite pins both shapes (`tests/application.rs`: `total() == 3` for the first, `total() == 1` for the second), so callers cannot rely on either. `BatchOutcome::total()`, the report's "batch N jobs" line and the CLI's `"total"` (`commands.rs:527`) all understate a hundred-segment batch that stopped at the first job. Fix: before breaking, push a `Skipped`/`Failed { cancelled: true }` entry for every remaining id so the outcome always has one row per segment.

#### A5 — minor — `refresh_sources` cannot return the error it documents

`crates/trimmer-app/src/workspace.rs:250-268`:

```rust
    /// Returns [`AppError::Media`] only when a *present* file cannot be probed; an absent file is
    /// a state, not an error.
...
            let probed = if present {
                self.engine.probe(&path).await.ok()
            } else {
                None
            };
            if let Some(source) = self.project.sources.get_mut(&path) {
                source.available = present;
                if probed.is_some() {
                    source.media = probed;
                }
            }
```

The function can never return `AppError::Media` — every probe error is discarded — and because the assignment is guarded by `is_some()`, a failed probe leaves the *stale* facts in place while `available` is set to `true`. After a master is replaced or truncated (or a NAS mount starts returning EIO), the batch plans against the old frame count and rate with no indication that the re-probe failed: the segments then fail verification or cut the wrong range. Fix: propagate the error (`Some(self.engine.probe(&path).await?)`) and clear `media` (or record the reason) when the file is gone.

#### A6 — minor — an unavailable source with cached facts is still described as present

`crates/trimmer-app/src/workspace.rs:332-336`:

```rust
                let summary = match (&source.media, source.available) {
                    (Some(media), _) => media.summary(),
                    (None, false) => "the file is not on disk".to_owned(),
                    (None, true) => "not probed yet".to_owned(),
                };
```

The `_` arm ignores `available`: a source that was just marked unavailable but whose `media` was retained is still described by its media summary. `diagnose` (`workspace.rs:407-416`) only receives `Option<&MediaInfo>`, so it produces no problem either — `SegmentView.problems` ("Every problem that would stop this segment being cut") stays empty, `WorkspaceSummary.runnable` counts the dead source, and the batch attempts to cut a file that is not there. This is the exact workflow the module advertises ("an editor who is working from a laptop without the drive attached must still be able to open the project", `workspace.rs:219-222`); the tests cover only the `add_source` path, where `media` is `None`. Fix: match on `available` first — `(Some(_), false) => "the file is not on disk"` — and have `diagnose` push a problem when the source is unavailable.

#### A7 — minor — a failed `preview` becomes "no commands"

`crates/trimmer-app/src/workspace.rs:498-503`:

```rust
        let commands = match &plan {
            Some(plan) if problems.is_empty() => self
                .engine
                .preview(&media, &segment, &preset, plan)
                .unwrap_or_default(),
            _ => Vec::new(),
        };
```

`MediaEngine::preview` is documented as returning "the domain's refusal when the segment cannot be cut" (`ports.rs:74-76`), and the module promises that the dry run shows the exact commands through the real argument builders. Here a refusal becomes an empty `commands` list with nothing added to `problems`, so `ttrim preview` and `GET /v1/projects/{id}/preview` present a segment as runnable while showing zero ffmpeg invocations. Fix: `Err(error) => { problems.push(error.to_string()); Vec::new() }`.

#### A8 — minor — three defects in the watch folder

* **Own outputs are re-processable.** `watch.rs:265-276` filters on `is_video` and on the *master's* fingerprint only; nothing excludes files this process wrote, and `WatchPolicy.output_dir` exists precisely so that outputs can land in the watched tree. With `cut_whole_when_unmarked: true` (set unconditionally by the CLI, `commands.rs:680`), an output with no marker list is a fresh name+size, so it is planned as a whole-file cut, which produces another output, and so on — unbounded growth. Today the CLI escapes the loop only because A1 puts the outputs one directory above the watched folder; **fixing A1 without excluding outputs turns this into a live loop**, so the two must be fixed together. Fix: skip anything equal to or under `policy.output_dir`, and remember the fingerprints of files this policy produced.
* **The settle rule does not cover the marker list.** `quiet_seconds` is evaluated only for files that pass `is_video` (`watch.rs:265-276`), so a marker list that is still being written (created a moment after the master, or edited by an assistant) can be read at `watch.rs:379` mid-write; the plan then runs with a partial mark set, and the CLI records only the *master's* fingerprint as processed (`commands.rs:742`) — the marks that arrive later are never cut, and nothing records the loss. Fix: require the marker file's own size to be stable for `settle_seconds`, or include it in the processed fingerprint.
* **Casing is folded before the filesystem is consulted.** `marker_for` (`watch.rs:140-144`) lower-cases the stem and then looks for `take 2.marks.txt` on disk, so on a case-sensitive filesystem `Take 2.MP4` + `Take 2.marks.txt` never pair and the job is blocked forever; the same folding in `fingerprint` (`watch.rs:253`) makes two files that differ only in case share a fingerprint, so the second is never processed. `is_video` (`watch.rs:125-129`) compares case-insensitively, so the asymmetry is unintended. Fix: list the directory and compare lower-cased names while keeping the on-disk spelling for the path.

These do not affect Windows, which is the target of the current build, but the crates are portable and the CLI is the documented automation surface.

#### A9 — nit — `JobState::Done` is never emitted

`queue.rs:60-62` declares `Done` ("Finished, successfully or not"), but the only `QueueEvent::State` events constructed anywhere are `Planning` (`:573`), `Cutting` (`:684`) and `Verifying` (`:723`) — a repo-wide grep finds no other construction, including in the tests. A consumer that tracks a job through `QueueEvent::State` (the CLI's sink prints `state.label()`, `commands.rs:571-573`) shows every job as `checking` and never `done`. Fix: emit `State { state: JobState::Done }` with the `Finished` event, or delete the variant.

#### A10 — nit — a poisoned lock reads as "no events"

`queue.rs:313-318` uses `self.events.lock().map(|guard| guard.clone()).unwrap_or_default()`, so a test whose reporting thread panicked sees an empty event list and passes vacuously; the same pattern is at `transcript.rs:239`, `transcript.rs:361` and `media/process.rs:204-211`. Fix: surface the poison (`expect` in a test, or a `Poisoned` flag) rather than returning an empty collection.

### trimmer-verify

| # | Severity | Location | What is wrong |
|---|---|---|---|
| V1 | major | `check.rs:439` + `queue.rs:774-784`, `commands.rs:391,618` | The expensive checks are never given evidence in production |
| V2 | major | `check.rs:936-951` | `TimescalePreserved` inspects the plan, not the delivered file |
| V3 | minor | `check.rs:913-916` | Two empty "not measured" codecs compare equal and pass |
| V4 | minor | `check.rs:659-698`, `746-772` | Frame alignment passes on a matching prefix (latent: needs V1 fixed) |
| V5 | nit | `facts.rs:70-74`, `facts.rs:5-7`, `76-88` | `has_audio` measures a duration; stale docs; a dead accessor |

#### V1 — major — the frame-hash differential never runs, and "skipped" counts as success

`crates/trimmer-verify/src/check.rs:433-440`:

```rust
pub fn verify_cut(
    plan: &CutPlan,
    facts: &CutFacts,
    source_facts: &CutFacts,
    policy: VerifyPolicy,
) -> VerifyReport {
    verify_cut_with(plan, facts, source_facts, policy, &Evidence::default())
}
```

`Evidence` is the carrier for frame hashes, the head-similarity score and the caption cues. `grep Evidence` over the whole workspace shows it is constructed **only inside `check.rs`'s own test module**; every production call site uses `verify_cut` with `&Evidence::default()`:

* `crates/trimmer-app/src/queue.rs:774, 778, 782, 784` — the batch path used by the CLI and by the daemon (`routes.rs:849` with `skip_verification: false`).
* `crates/trimmer-cli/src/commands.rs:391` and `:618` — `cut` and `verify`.

`verify_cut_with` therefore has no production caller, and `MediaMeasurer::frame_hashes`, `extract_frame` and `ssim` — implemented for real in `trimmer-daemon/src/state.rs:343-370` and `trimmer-app/src/ports.rs:435-464` — are never invoked. With the default `VerifyPolicy::Strict`, `check_frame_alignment` returns `Skipped` ("no source frame hashes were sampled", `check.rs:653-658`); with `Forensic`, `HeadFidelity` and `Captions` join it. Then:

```rust
    /// True when nothing failed. Skips and warnings do not make a report not ok.
    pub fn ok(&self) -> bool {
        !self.results.iter().any(CheckResult::is_failed)
    }
```

(`check.rs:317-321`) — so the verdict is "ok", and `queue.rs:736` turns that into `JobStatus::Succeeded`. The audit sentence for the skipped check ("cut frames hash the same as the source frames at the same sample point", `check.rs:127-130`-style statements) travels into the audit log for a comparison that never happened, and `trimmer-app/src/queue.rs:392`'s `BatchOutcome::clean()` will be true.

This is the most important finding in the report: the product's headline claim is "frame-exact, verified segment cutting", and in every shipping path the frame-level verification is inert while the report says the cut succeeded. The report *does* print `skipped` with a reason (`check.rs:390`), so a human reading the table can see it — but no machine path acts on it.

Fix: (a) build the evidence the policy asks for — the `MediaMeasurer` trait already has the methods and two real implementations, so the queue needs to call `frame_hashes` for the sampled window on both files and `with_captions` where a caption sidecar exists; (b) in the meantime, make the policy accounting explicit: a policy-required check that returns `Skipped` must not count as `ok` — add e.g. `VerifyReport::satisfies(policy)` and have the queue map it to `JobStatus::Unverified` (which already exists and is reported distinctly at `queue.rs:747`, `store.rs:645`). Either change alone fixes the false claim; both are needed for the feature to work as documented.

#### V2 — major — the timescale check cannot see the output

`crates/trimmer-verify/src/check.rs:936-951`:

```rust
fn check_timescale(plan: &CutPlan) -> CheckResult {
    if plan.mode != CutMode::HeadPatch {
        return skipped(
            Check::TimescalePreserved,
            "the plan does not re-encode a head, so there is no timescale to preserve",
        );
    }
    let ticks = plan.video_timescale;
    if ticks > 0 {
```

The function's only input is the plan, and `CutFacts` (`facts.rs:22-46`) has no timescale field, so the delivered file's timescale is never measured. Its doc block claims the opposite:

```rust
/// ADR-001 was written about this bug, and this check is what catches it.
```

and the audit statement (`check.rs:120-126`) says "a re-encoded head carries the source timescale, so the muxer does not rescale the copied body". For any plan that came from `plan_cut`, `video_timescale` is the source's own ticks (`plan.rs:451`), and `violated_invariants` already refuses a non-positive value (`plan.rs:259-264`), so the check can only ever report `Passed` — a tautology. `MediaInfo.timebase` *is* measured on the output by the prober (`probe.rs:152-156`) but is dropped when `ProbeMeasurer` builds `CutFacts` (`trimmer-daemon/src/state.rs:318-333`). The suite's two tests (`check.rs:1674`, `1686`) exercise the plan only, which is why this survives.

The mitigation is that `check_duration` catches the slow-motion *symptom* (its comment at `check.rs:527-534` says so), so this is a defect in the evidence rather than a hole in the outcome. Fix: add `timescale` to `CutFacts`, populate it in every producer, and compare `facts.timescale` against `source_facts.timescale` — or rename the check and its statement so the audit log claims a property of the plan rather than of the file.

#### V3 — minor — two unmeasured codecs compare equal and pass

`crates/trimmer-verify/src/check.rs:913-916` compares `facts.codec` with `source.codec` case-insensitively and returns `Passed` when they match. Both real "nothing is known" producers use an empty string (`trimmer-app/src/queue.rs:798`, `trimmer-cli/src/commands.rs:405`), and `"" == ""`, so the row claims "the delivered codec is the source codec" for two files nobody measured. In the app the accompanying `frame_count: -1` still fails `check_frames`, so the *run* is `Unverified` — but the row is false, and the audit log quotes it. Fix: skip when either codec is empty, as the other checks do for their missing inputs.

#### V4 — minor — frame alignment accepts a matching prefix (latent)

`crates/trimmer-verify/src/check.rs:691-698`:

```rust
    if best.compared > 0 && best.matches == best.compared {
        return outcome(
            Check::FrameAlignment,
            CheckStatus::Passed,
```

`compared` counts only the cut positions that found a counterpart in the source window; a position that falls off the end is skipped without a trace (`check.rs:761-763`). A cut window longer than the sampled source window therefore passes on a matching prefix, and the extra delivered frames — present nowhere in the sampled source — are never examined. The doc at `check.rs:744-745` acknowledges the mechanism without treating it as disqualifying. Today nothing supplies hashes (V1), so this is latent; it becomes live the moment V1 is fixed. Fix: require every cut position to have been compared (allowing only the one boundary frame the winning shift implies), or fail/skip when the windows differ in length, and report the uncompared count in the detail.

#### V5 — nit — names and docs that do not match the code

* `facts.rs:70-74`: `pub const fn has_audio(&self) -> bool { self.audio_duration.is_some() }` — it reports "an audio *duration* was measured". `check_audio_alignment` uses it to decide whether the cut carries audio (`check.rs:574-585`), so a file with an audio stream whose duration ffprobe did not report is failed with the wrong reason ("the source has audio and the cut does not: the audio was dropped"). Carry an explicit presence flag.
* `facts.rs:5-7`: "producing these values is `trimmer-media`'s job, and it does not exist yet" — `crates/trimmer-media` exists and `ProbeMeasurer` is a live adapter.
* `facts.rs:76-88`: `seconds_per_frame`'s doc says "This is the file's own rate, used for tolerances that are expressed in frames", but every frame tolerance comes from the *plan* (`check.rs:1009-1014`), and the accessor has no caller outside its own test. `CutFacts.rate`, `.width`, `.height` and `.size_bytes` are likewise never read by any check — worth noting because no check compares geometry, so a wrongly cropped delivery is caught (if at all) only by the alignment check that V1 keeps switched off.

### trimmer-export

| # | Severity | Location | What is wrong |
|---|---|---|---|
| E1 | major | `xml.rs:93-102` + `core/domain.rs:168-173` | UNC paths are corrupted into local paths |
| E2 | major | `premiere.rs:57-72`, `110-117` | The xmeml sequence has no audio track: audio is silently dropped |
| E3 | minor | `edl.rs:110-122` + `xml.rs:14-16` | Newlines are not stripped from EDL lines, contradicting the doc |
| E4 | minor | `edl.rs:58`, `84-87` | Source stamps and the `FCM` line can disagree on drop-frame |
| E5 | minor | `fcpxml.rs:145-152` | The asset declares media with `@src` rather than a `media-rep` child (unverified against the DTD) |
| E6 | minor | `write.rs:72-77` | Documents are truncated in place: an interrupted write destroys the previous one |
| E7 | nit | `xml.rs:14-16` | The forbidden set is both too wide (legal C1) and too narrow (U+FFFE/U+FFFF) |
| E8 | nit | `premiere.rs:134-136` | `is_ntsc` tests the string "1001" instead of the fractional-rate test |

#### E1 — major — UNC paths are mangled twice on their way into a document

Two defects compound. First, `crates/trimmer-core/src/domain.rs:168-173`:

```rust
fn strip_extended_prefix(path: &Path) -> PathBuf {
    match path.to_string_lossy().strip_prefix(r"\\?\") {
        Some(stripped) => PathBuf::from(stripped),
        None => path.to_path_buf(),
    }
}
```

`std::fs::canonicalize` on a UNC path returns the `\\?\UNC\server\share\...` form, not `\\?\C:\...`. Stripping only the `\\?\` prefix turns `\\nas-01\share\x.mp4` into `UNC\nas-01\share\x.mp4` — a relative path. `Workspace::add_source` canonicalises before storing (`trimmer-app/src/workspace.rs:227`), so a source added from a network share is written into the project database under a path that no longer resolves; the CLI's export resolver canonicalises too (`trimmer-cli/src/project.rs:204`).

Second, even with the path intact, `crates/trimmer-export/src/xml.rs:93-102`:

```rust
pub(crate) fn premiere_url(path: &str) -> String {
    let encoded = url_path(path);
    format!("file://localhost/{}", encoded.trim_start_matches('/'))
}
```

`url_path` maps `\\nas-01\masters\a.mp4` to `//nas-01/masters/a.mp4`; `trim_start_matches('/')` removes **all** leading slashes, so the result is `file://localhost/nas-01/masters/a.mp4` (and `file:///nas-01/...` for FCPXML) — a local path with the host lost. This is the exact workflow the crate's module docs advertise ("A studio whose masters live on `\\nas-01\masters` while its editors work from a mirrored `M:\`", `export/lib.rs:22-27`), and on import the editor gets offline media instead of the share. The two production callers reach it differently and both are affected: the CLI canonicalises first (`trimmer-cli/src/project.rs:204`), so it hits both halves of this finding; the desktop shell passes the path through unchanged (`apps/desktop/src-tauri/src/commands.rs:946`, `let resolve = |source: &MediaPath| source.to_string();`), so it hits the host-loss half only.

Fix: in `strip_extended_prefix`, special-case `\\?\UNC\` → `\\` before the generic strip; in `premiere_url`/`fcpxml_url`, preserve a leading `//` authority (`file://nas-01/...`) and only strip a single leading slash from a rooted POSIX path. Both deserve tests with a UNC input — none exists today.

#### E2 — major — the Premiere sequence is picture-only

`crates/trimmer-export/src/premiere.rs:57-72` writes the sequence's `<media>` with a single `<video>` element and no `<audio>` element; each clip's `<file>` (`premiere.rs:110-117`) likewise carries only `<video><samplecharacteristics>`. Nothing in the document creates an audio track or an audio `clipitem`, so an editor importing the sequence gets silent picture with no warning — and `ExportClip.has_audio` exists for exactly this purpose (`export/clips.rs:190` tracks `media.audio.is_some()`) but is read only by the FCPXML writer (`fcpxml.rs:143`). The EDL writer is the same shape: every event is `V` channel only (`edl.rs:92`), which is defensible for a V-only conform but is not stated in the warnings.

Fix: emit an `<audio>` track with one audio `clipitem` per clip (channel 1/2, sourcetrack `mediatype="audio"`), or, if that is deliberately out of scope, add a warning to `ExportProduct.warnings` whenever a clip has audio, so the omission is visible at export time rather than discovered in the edit.

#### E3 — minor — the EDL "forbidden set" permits newlines

`crates/trimmer-export/src/edl.rs:110-122`:

```rust
/// Text with the characters a line-oriented list cannot carry removed.
///
/// A newline in a title does not produce a bad EDL, it produces two EDLs, and the second one
/// is gibberish. The forbidden set is the same one XML uses, for the same reason.
fn line_text(raw: &str, what: &str, warnings: &mut Vec<String>) -> String {
    if xml::has_forbidden(raw) {
        ...
    }
    xml::strip_forbidden(raw).0.into_owned()
```

But the set it borrows is `xml.rs:14-16`:

```rust
fn is_forbidden(character: char) -> bool {
    character.is_control() && !matches!(character, '\t' | '\n' | '\r')
}
```

which explicitly **permits** `\n`, `\r` and `\t` — deliberately, because XML can carry them. So the doc's premise is false and a newline survives into `TITLE: {title}` (`edl.rs:62`) and `* FROM CLIP NAME: {file_name}` (`edl.rs:105`). A project name is arbitrary text (the daemon accepts any non-blank string, `routes.rs:340-343`), so `TITLE: take 1\n002  AX  ...` injects a fabricated event line into the list.

Fix: give the EDL its own filter (replace `\n`, `\r`, `\t` and other control characters with a space, warning as it does now) rather than reusing the XML rule, and add the test the doc implies.

#### E4 — minor — a mixed-rate timeline can emit one `FCM` line with two stamp styles

`crates/trimmer-export/src/edl.rs:58` derives the list's frame-code mode from the *timeline* rate (`let drop = rate.supports_drop_frame();`) and uses it for `FCM` (`edl.rs:63-67`) and the record stamps (`edl.rs:86-87`), while the source stamps are rendered with `None`:

```rust
        let source_in = format_timecode(clip.source_in, clip.rate, None);
        let source_out = format_timecode(clip.source_out - 1, clip.rate, None);
```

`None` selects *that clip's own* default form (`timecode.rs:730`), so on a 29.97 timeline holding a 29.97 clip and a 25 fps clip the list contains `;` stamps in one event and `:` in another under a single `FCM`. The module's own doc says this must never happen ("A list whose header and whose stamps disagree is read differently by different tools", `edl.rs:11-14`), and the clip-level rate mismatch is explicitly supported and warned about (`export/clips.rs:132-140`). Fix: choose one mode for the whole list (force non-drop when any stamp cannot be written drop-frame) and warn when a clip's rate has to be coerced.

#### E5 — minor — the FCPXML asset declares its media with a deprecated form

`crates/trimmer-export/src/fcpxml.rs:145-152` writes the asset's media as an attribute:

```rust
xml::line(out, 2, &format!("<asset ... src=\"{src}\" ...>"));
```

FCPXML 1.10/1.11 (the version this crate declares) declares an asset's media as a child element, `<media-rep kind="original-media" src="..."/>`; the `@src` attribute on `<asset>` is the pre-1.8 form. If that is right, a strict consumer either rejects the asset or imports it with no media. **I could not verify this against Apple's DTD in this environment** (no network access to the DTD, and no Final Cut/Premiere available), so treat it as a likely defect that needs a check against `FCPXMLv1_11.dtd` before acting. Fix if confirmed: make `<asset>` a container with one `<media-rep>` child carrying the URL, and keep `hasVideo`/`format`/`duration` on the asset.

#### E6 — minor — writing a document destroys the previous one in place

`crates/trimmer-export/src/write.rs:72-77` hands the string straight to `std::fs::write`, which opens with create+truncate. A crash, a full disk or a cancelled process midway leaves a truncated document at the final path and the previously exported document gone. The four wrappers are otherwise correct. Fix: write to a sibling temp file, `sync_all`, then rename over the target (the same pattern `executor.rs:460-470` uses for deliverables).

#### E7 — nit — the XML forbidden set is measured in the wrong currency

`crates/trimmer-core/src/export/../export/xml.rs:14-16` (quoted in E3) approximates XML 1.0's `Char` production with `char::is_control`. That is both too wide — `U+007F` (DEL) and the C1 block `U+0080`–`U+009F` are legal XML 1.0 characters and are stripped (with a warning) from names — and too narrow — `U+FFFE` and `U+FFFF` are **not** control characters but are excluded from `Char`, so a name containing one passes through and produces a document that is not well-formed, contradicting the module's promise (`export/lib.rs:38-40`). Fix: test the production directly (`c == '\t' || c == '\n' || c == '\r' || ('\u{20}'..='\u{D7FF}').contains(&c) || ('\u{E000}'..='\u{FFFD}').contains(&c) || c >= '\u{10000}'`).

#### E8 — nit — `is_ntsc` is a string test where a predicate exists

`crates/trimmer-export/src/premiere.rs:134-136`:

```rust
fn is_ntsc(rate: FrameRate) -> bool {
    rate.as_ffmpeg().contains("1001")
}
```

`FrameRate::is_fractional()` (`timecode.rs:250-253`) is the intended test and is what the doc describes. The string test also answers `true` for a rate whose numerator merely contains "1001" (e.g. `FrameRate::parse("1001")` = 1001 fps). Fix: use the predicate (or `supports_drop_frame()` if only the drop-frame family counts).

### trimmer-store

| # | Severity | Location | What is wrong |
|---|---|---|---|
| S1 | major | `schema.rs:34`, `142-145` | A newer database is opened and written as if it were version 1 |
| S2 | minor | `store.rs:184-187`, `59-63` | No `busy_timeout`; SQLite error codes are erased |
| S3 | minor | `store.rs:464-466`, `487`, `516`, `564` | `load_project` reads four statements without a snapshot |
| S4 | minor | `schema.rs:130-134`, `180-184` | `.ok()` on the version read turns any error into "version 0" |
| S5 | minor | `store.rs:317` | A corrupt run id silently becomes the nil UUID |
| S6 | nit | `store.rs:206-216` | `with_connection`'s doc claims an invariant it does not protect |
| S7 | nit | `store.rs:237` | `started_at + elapsed` can overflow where the clock deliberately saturates |

#### S1 — major — the schema version is never checked

`crates/trimmer-store/src/schema.rs:34` defines `pub const LATEST_VERSION: i64 = 1;` and `migrate` (`schema.rs:142-145`) only ever moves *forward*:

```rust
    for migration in MIGRATIONS {
        if migration.version <= version {
            continue;
        }
```

`version` is whatever the file records (lines 130-140). A file recorded at version 5 applies no migrations and `migrate` returns `Ok(5)`; `LATEST_VERSION` is referenced only by tests, so nothing anywhere refuses it. The consequence is not a crash but a misread: v1-shaped `SELECT`s run against a v5 file (a renamed column is an error, a same-typed semantic change — seconds to ticks, a new enum word reaching `verify_policy` — is read as a valid value), and `save` then writes v1-shaped rows back into the newer file without changing its version. `docs/adr/012-sqlite.md` states the required behaviour ("a file whose version is newer than the binary supports is refused with a sentence saying so rather than opened and misread"). Fix: before the loop, `if version > LATEST_VERSION { return Err(StoreError::Schema { version, reason: ... }) }`.

#### S2 — minor — no busy timeout, and busy errors are unidentifiable

`store.rs:184-187` opens the connection, sets `foreign_keys` and turns on WAL, but never sets `busy_timeout`; `From<rusqlite::Error>` (`store.rs:59-63`) flattens the error into a string, so `SQLITE_BUSY` cannot be recognised by a caller. The `Mutex<Connection>` serialises only this process; the CLI and the daemon both default to the same `trimmer_store::default_store_path()` (`store.rs:121-125`, used by `trimmer-cli/src/context.rs:93` and `trimmer-daemon/src/config.rs:33`), so a `ttrim batch` running while a daemon writes a run record is the expected case, and the second writer fails immediately instead of waiting. (The desktop shell uses a *different* file — see DS7 — so it is not a second writer on this one, though that divergence is itself a defect.) Fix: `conn.busy_timeout(Duration::from_secs(5))?` after opening, and keep the `ErrorCode` in the error type.

#### S3 — minor — `load_project` is not a snapshot

`store.rs:464-466` takes the connection mutex and then issues the header read, then separate statements for sources, segments and presets (lines ~487, 516, 564) with no open transaction. The mutex protects against other threads in this process, but under WAL each statement takes its own snapshot, so a second process committing a `save` in between yields a project assembled from two different states — the "half its segments" outcome the crate docs say must not be observable. Fix: wrap the body in a deferred read transaction (`conn.unchecked_transaction()` in rusqlite 0.32) so all four statements share one snapshot.

#### S4 — minor — a failed version read is treated as "never migrated"

`schema.rs:130-134` (and the identical pattern at `180-184`) uses `.ok()` on a `query_row` whose only expected failure is `QueryReturnedNoRows`. A `schema_version` table of the wrong shape, a locked file or a corrupt page all become `None`, i.e. "version 0", so migration 1 is re-applied to an already-migrated database and fails on its first statement with the misleading reason "table projects already exists". Fix: match on `QueryReturnedNoRows` explicitly and propagate anything else.

#### S5 — minor — a corrupt run id becomes the nil UUID

`store.rs:317`: `id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap_or_default()`. Every other place that decodes a stored UUID uses the fallible `uuid_of` (`store.rs:688-690`). Here a damaged primary key silently yields `Uuid::nil()`, so `list_runs` reports a run that `load_run` says does not exist. Fix: decode outside the row mapper and propagate `StoreError::Decode`.

#### S6 — nit — `with_connection`'s rationale is false

`store.rs:206-216` documents the method as protecting the pragmas and the schema, then hands out `&Connection`, with which a caller can `PRAGMA foreign_keys = OFF` or `DROP TABLE segments`. It is a deliberate test escape hatch; the comment should say so. **Note:** I did not report the `HashMap`/`BTreeMap` case-collision consequence of C6 separately here; it is the same root cause.

#### S7 — nit — `finished_at` can overflow

`store.rs:237`: `let finished_at = started_at + outcome.elapsed_seconds.max(0.0) as i64;`. `SystemClock::now_unix` deliberately saturates to `i64::MAX` (`trimmer-app/src/ports.rs:161-167`), so on a clock past 2262 the addition overflows — a panic in a debug build, a negative timestamp (sorting before the epoch) in release. Fix: `saturating_add`.

### trimmer-daemon

| # | Severity | Location | What is wrong |
|---|---|---|---|
| D1 | major | `routes.rs:423-424`, `493-494`, `510-514` + `store.rs:396-406` | Concurrent edits lose each other: load, mutate, replace |
| D2 | major | `routes.rs:546-547`, `573-575` | The 409 "already running" check is not atomic with the reservation |
| D3 | minor | `routes.rs:604-618` + `state.rs:212-216` | Cancelling marks a run evictable while it is still cutting |
| D4 | minor | `routes.rs:456-486` | Segments added while a source is unprobed skip all range validation |
| D5 | minor | `routes.rs:425`, `432` | A stored canonical path is looked up by the caller's raw spelling |
| D6 | minor | `routes.rs:650-657` | An unreadable caption file is answered as "no hits" |
| D7 | minor | `routes.rs:129-134` | A bad source path is a 500 with the tool's output in the body |
| D8 | minor | `routes.rs:14-17` | axum's own `Json` rejections bypass the documented error shape |
| D9 | minor | `routes.rs:821-856` | A run never re-probes sources, unlike the CLI |
| D10 | nit | `routes.rs:187-195` | The timing comment describes the opposite of the loop bound |
| D11 | nit | `openapi.rs:125-126` | A duplicate JSON key |

#### D1 — major — read-modify-write on a whole project document loses concurrent edits

`routes.rs:423-424` and `493-494` (`add_segment`; `delete_segment` at 510-514 and `add_source` at 381-391 have the same shape):

```rust
    let mut project = load(&state, project_id)?;
...
    project.segments.push(segment);
    state.store.save(&project).map_err(ApiError::internal)?;
```

`SqliteStore::save_project` replaces the document rather than merging it (it deletes the child rows for the project and re-inserts, `store.rs:396-406`), and it locks only for the duration of each call. Two clients that both `load`, mutate their own copy and `save` will have the second save delete the first client's row — and both receive `201`. The daemon's stated purpose is unattended studio tooling where concurrent requests are expected, and the module docs promise a 409 only for a concurrent *run*, not for lost edits. Fix: serialise mutations per project (a `Mutex<HashMap<ProjectId, Arc<tokio::sync::Mutex<()>>>>` in `DaemonState`, or a single edit actor), or add a compare-and-swap on `updated_at` in the store so a stale write is refused.

#### D2 — major — the run guard is a check-then-act

`routes.rs:546-547` and `573-575`:

```rust
    let project = load(&state, project_id)?;
    if state.project_is_running(project_id) {
```

```rust
    state
        .insert_run(record)
        .map_err(|error| ApiError::conflict("registryFull", error))?;
```

`project_is_running` (`state.rs:193-200`) and `insert_run` (`state.rs:209-231`) each take the registry mutex and release it, with clock reads, record construction and a `spawn` in between. Two simultaneous `POST /v1/projects/{id}/run` requests — a double-clicked button, or a client retrying after a timeout — can both observe "not running" and both start a batch over the same segments. The two batches then write the same output paths concurrently, and they share a work directory: `WorkDir::create` (`trimmer-media/src/executor.rs:543-553`) names it `.trimmer-{pid}-{output stem}`, which is identical for two cuts in the same process targeting the same deliverable, so the two batches overwrite each other's `head.mp4`/`body.mp4` as they go. Fix: one method that scans and inserts under a single lock (`start_run_if_idle(record)`), so the 409 decision and the reservation are one critical section; and make the work directory unique per run (include the run id or a counter).

#### D3 — minor — a cancelled run is still cutting but is treated as finished

`routes.rs:604-618` sets `RunState::Cancelled` immediately when a cancel is requested, while the batch keeps running (the API's own documentation says "The segment in flight finishes; the ones after it are not attempted"). Two consequences follow from the filters that read the state:

* `project_is_running` (`state.rs:194-199`) only counts `Queued | Running`, so a new run for the same project is accepted while the first is still finishing.
* `insert_run`'s eviction (`state.rs:212-216`) treats `Cancelled` as evictable, so the record can be dropped while work continues; the batch's final `update_run` then silently does nothing (`state.rs:240-243` returns `Ok(())` for an unknown id) and the client polling that id gets a 404.

Fix: add a `Cancelling` state set by the route and cleared when the batch returns, count it as in flight in both filters, and make the transition conditional on the run actually being `Queued`/`Running`.

#### D4 — minor — a segment added while the source is unprobed is never range-checked

`routes.rs:456` guards everything that validates the range against the source:

```rust
    if let Some(media) = project.media(&source) {
```

Inside the `if let` are the `start_frame > media.last_frame()` check (line 457) and the `KeyframeGrid` + `plan_cut` call (467-485). For a source that is present but unprobed (or offline — a state `add_source` explicitly supports), only `end <= start` is checked (446-455), so `{"start_frame": -5, "end_frame": null}` is accepted with `201` and pushed straight onto the project (line 493, bypassing `Project::add_segment`'s source check). It then fails at cut time. The comment three lines above says the opposite: "A range that is not a range is refused here rather than three processes later". Fix: reject a negative `start_frame` unconditionally, and validate against the media facts when they arrive.

#### D5 — minor — canonicalised keys looked up by raw text

`routes.rs:425` builds `MediaPath::new(text(&body, "source")?)` and line 432 does `project.sources.contains_key(&source)`, while `add_source` stores `path.canonicalised()` (`workspace.rs:227`). A client that adds `..\masters\a.mp4`, a relative path, or (on a case-sensitive platform) a different spelling gets `400 unknownSource` for a source that is in the project. `MediaPath::same_file_as` exists for this comparison (`domain.rs:99-107`). Fix: canonicalise on the way in, or match with `same_file_as`.

#### D6 — minor — an unreadable transcript is reported as "no matches"

`routes.rs:650-657` returns `200` with an empty `hits` array and a `reason` for both `Ok(None)` (no caption file beside the video) and `Err(...)` (a file that exists but cannot be read or holds no cues, `trimmer-app/src/transcript.rs:300-305`). The crate docs justify the `200` only for the first case. A client that looks at `hits` concludes "the phrase is not in this video". Fix: answer `Err` with a 4xx/5xx in the documented error shape.

#### D7 — minor — a client-fixable mistake is a 500 that echoes tool output

`routes.rs:129-134` maps `AppError::Media` to `ApiError::internal(error.to_string())`. `POST /v1/projects/{id}/sources` with a path to a file that exists but is not media (e.g. `C:\Windows\win.ini`) fails in `probe` and returns `500 {"error":"internal","detail":"<ffprobe output>"}` — contradicting the module's own status table (`routes.rs:3-17`, where 500 means "the store or the media layer failed") and the route's documented responses. A render manager reads 500 as "retry" rather than "fix your input". Fix: classify an unprobeable named input as 400/422 and keep the raw tool output for the log.

#### D8 — minor — axum's extraction failures do not use the documented error body

The module docs promise `{"error", "detail"}` for every failure (`routes.rs:14-17`), but a rejected `Json` extractor never reaches a handler: malformed JSON is a `400` with a `text/plain` body, a wrong `Content-Type` is `415`, and a body over axum's 2 MiB default is `413` — none of which the OpenAPI document lists. Fix: extract through a wrapper whose rejection renders `ApiError`, and document 413/415.

#### D9 — minor — a run does not re-probe its sources

`run_batch` (`routes.rs:821-856`) opens the workspace and goes straight to `queue.run`; nothing in the daemon calls `Workspace::refresh_sources`, whose doc says it is "Called when a project is opened and before a batch runs" (`workspace.rs:245-249`) and which the CLI does call (`trimmer-cli/src/commands.rs:469-472`). Because `add_source` supports a file that is not currently on disk, an offline-then-reattached drive means the daemon run skips every segment (`queue.rs:658-670` → `JobStatus::Skipped`) and `GET /preview` plans against stale facts. Fix: call `refresh_sources().await` after opening in both `run_batch` and `preview`.

#### D10 — nit — the timing comment describes the opposite bound

`routes.rs:187-195` loops `for index in 0..left.len().max(right.len())` and the comment says it "compares every byte of the shorter string" and that "the clock is quiet". The comparison is content-constant-time, but the number of iterations is the *longer* length, so the token's length is observable. The comment should say what the code does (or the comparison should use fixed-length digests).

#### D11 — nit — duplicate key in the OpenAPI document

`openapi.rs:125-126` lists `"ffmpeg"` twice in the same object literal; `json!` keeps the last, but strict validators and codegen tools may reject the document. Delete one line.

### trimmer-cli

The crate has **no tests at all** (no `tests/` directory, no `#[cfg(test)]` module), which is why several of these are user-visible defects that survived: the findings marked *reproduced* below were observed by running the built binary against an ffmpeg-generated 10 s / 25 fps / 250-frame clip, in both debug and release profiles.

| # | Severity | Location | What is wrong |
|---|---|---|---|
| L1 | major | `cli.rs:190-193` vs `commands.rs:217-220` | The `--in`/`--out` help promises frames and `MM:SS:FF`; the parser reads bare numbers as **seconds** and three fields as `HH:MM:SS` *(reproduced)* |
| L2 | major | `commands.rs:221-225`, `project.rs:144`,`149` | `out_frame + 1` overflows on a saturating parse: panic (exit 101) in debug, nonsense in release *(reproduced)* |
| L3 | major | `cli.rs:230-238`, `commands.rs:345-353`, `context.rs:115-117` | `--crf` cannot reach the head encode and hard-fails on the default preset *(reproduced)* |
| L4 | major | `cli.rs:449-455` | `verify --plan` documents a producer (`project show --plan`) that does not exist, so `verify` is unusable |
| L5 | major | `commands.rs:834-843` | `watch --apply` ignores `WatchAction::RunProject` and cuts the whole file where the dry run promised a project batch *(reproduced)* |
| L6 | major | `commands.rs:741-744`, `796-810` | A permanently failing watch job is retried every two seconds and inserts a new project each time |
| L7 | major | `commands.rs:303` → executor | A debug build stack-overflows on every cut, contradicting `trimmer-media`'s "measured, not assumed" comment *(reproduced)* |
| L8 | minor | `commands.rs:600-618` | `verify` deserialises a plan and never checks its invariants |
| L9 | minor | `commands.rs:519-521`, `880` | The run's audit record is written and its failure discarded |
| L10 | minor | `commands.rs:431-439` | The SRT sidecar is rewritten with CRLF and no BOM whatever the source used |
| L11 | minor | `commands.rs:76-78`, `context.rs:50-57` | `doctor` exits 2 where its help promises 1; every internal I/O failure also exits 1 |
| L12 | minor | `commands.rs:684-724` | If the notifier cannot be built, the two-second fallback scan hot-spins instead of sleeping |
| L13 | minor | `commands.rs:166-170` | An unreadable transcript prints "(0 cues)" rather than "unreadable" |
| L14 | minor | `commands.rs:327`, `431-433` | A caption failure is reported (exit 2) only after the verified deliverable has been written |
| L15 | minor | `context.rs:199-202` | `quote` leaves shell metacharacters bare, so the "copy it out and run it" line is not copy-safe |
| L16 | minor | `commands.rs:675-680` | `watch` forces `cut_whole_when_unmarked = true` with no flag and no mention in the help |
| L17 | nit | `project.rs:204` | The export resolver inherits E1's UNC corruption |
| L18 | nit | `commands.rs:477-490` | `batch --segments` prefix matching silently picks the first of several matches |
| L19 | nit | `cli.rs:385-386` | `--out-exclusive` is silently ignored when `--out` is absent |
| L20 | nit | `commands.rs:99` vs `144` | The key `timescale` carries ticks in one place and `1/n` in another |
| L21 | nit | `Cargo.toml:3` | The description advertises a `licence` command that does not exist; four declared dependencies are unused |
| L22 | nit | `commands.rs:332-333`, `448`, `cli.rs:99` | Dead parameters and a "the plan is shown and confirmed" claim the non-dry-run path does not honour |

#### L1 — major — the documented `--in`/`--out` grammar is not the implemented one

`crates/trimmer-cli/src/cli.rs:190-193`:

```rust
        long_help = "Accepts `HH:MM:SS:FF`, `MM:SS:FF` or \
                                                       a bare frame count, on the source's own \
                                                       frame rate."
```

The value goes to `parse_timecode` (`commands.rs:217-220`), which reads a bare decimal as **seconds** (`timecode.rs:557-565`) and a three-field value as `HH:MM:SS:00` (`timecode.rs:570-574`) — neither is what the help says. Reproduced: `cut clip.mp4 --in 0 --out 3 --dry-run` on a 25 fps source prints `0..76 (76 frames)` (three *seconds*), and `--in 01:30:00` reads as one and a half hours, sixty times the `MM:SS:FF` a user typed. Nothing warns; the user gets the wrong material or a "past the end of the file" refusal. Fix: make the two agree — either correct the help to the real grammar (`.s`/seconds for a bare number, `HH:MM:SS`) or add explicit `--in-frames`/`--in-seconds`, parse the three-field form as `MM:SS:FF` in the CLI, and pin the behaviour with an integration test (`--out 3` ⇒ 3 frames).

#### L2 — major — `out_frame + 1` overflows on a value the parser can produce

`crates/trimmer-cli/src/commands.rs:221-225`:

```rust
    let end = if args.out_exclusive {
        out_frame
    } else {
        out_frame + 1
    };
```

`parse_timecode` saturates an all-digit decimal to `i64::MAX` (`(seconds * rate).round() as i64`), so `--out 99999999999999999999` reaches this line. Reproduced: a debug build panics with `attempt to add with overflow` at `commands.rs:224` and exits **101**; a release build (overflow checks off) wraps and reports `the out point (frame -9223372036854775808) is not after the in point (frame 0)` — a nonsense message that gives no hint the input was out of range. The same pattern is at `project.rs:144` and, eagerly, at `project.rs:149` (`end.unwrap_or(start + 1)` evaluates `start + 1` even when `--out` was given), both reproduced. Fix: `checked_add(1)` with a refusal naming the field, and validate the parsed frame against the source's length before the arithmetic.

#### L3 — major — `--crf` hard-fails on the default preset and changes the wrong encode elsewhere

`crates/trimmer-cli/src/commands.rs:345-353` refuses unless the *preset* re-encodes the picture:

```rust
    if let Some(crf) = crf {
        if let trimmer_core::VideoTreatment::Encode { quality, .. } = &mut preset.video {
            *quality = crf;
        } else {
            return Err(Failure::refused(format!(
                "the {name} preset copies the picture, so --crf has nothing to change; use a \
                 preset that re-encodes, or a re-encode will happen only in the head"
            )));
```

But the head is always re-encoded in head-patch mode and its CRF comes from `CutConfig.crf` (`trimmer-media/src/executor.rs:627-628`), which the CLI pins to 18 (`context.rs:115-117`) and never wires to `--crf`. Reproduced: with the default `master` preset (a copy preset) `--crf 20` exits 2 with the message above — so the flag's stated purpose, the re-encoded head, is unreachable — while with a re-encoding preset it silently overrides the whole-segment quality instead. The refusal text even concedes the point ("a re-encode will happen only in the head"). Fix: thread the value into `CutConfig.crf`, only touch `preset.video.quality` when the preset encodes, and refuse only when neither applies.

#### L4 — major — the mandatory input to `verify` cannot be produced

`crates/trimmer-cli/src/cli.rs:449-455` documents the plan file as "A `CutPlan` as `project show --plan` writes it, or as a run's audit record holds it". `project show` has no `--plan` flag (its only options are `--store`, `--verbose`, `--help`), `project export` writes the project document rather than a `CutPlan`, and a grep of the crate finds no writer of `CutPlan` JSON. The documented route to the one required argument of `verify` therefore does not exist, so the subcommand is unusable from the CLI as shipped. Fix: add `project show --plan` (or `cut --plan-out <file>`) that serialises the resolved plan in the shape `verify` deserialises, or name a producer that exists — and note that `verify` should also call `plan.verify_invariants()` (L8).

#### L5 — major — `watch --apply` cuts the whole file where its own dry run promised a project batch

`crates/trimmer-cli/src/commands.rs:834-843`:

```rust
    let marks = match plan.trigger {
        WatchTrigger::MarkerList => folder.read_marks(plan, rate).map_err(Failure::refused)?,
        _ => Vec::new(),
    };
    if marks.is_empty() {
        let mut segment = Segment::new(added.clone(), "whole file", 0, frame_count);
```

`plan.action`/`WatchAction` is never consulted, and `crates/trimmer-app/src/watch.rs:289-304` sets `WatchAction::RunProject` for a `.trimmerproj` marker with the description "run the project's batch". Reproduced with `clip.trimmerproj` beside the master: the dry run prints `would cut clip.mp4: run the project's batch`, and `watch --apply` logs `#0 whole file` and delivers all 250 frames — the operator's marks are ignored and the dry run and the apply disagree. Fix: match on `plan.action`; implement `RunProject` or refuse such a plan loudly, and never fall through to a whole-file cut for a trigger the code does not implement.

#### L6 — major — a failing watch job retries forever and creates a project per attempt

`crates/trimmer-cli/src/commands.rs:741-744`:

```rust
            match apply_plan(context, &mut folder, plan, &preset).await {
                Ok(()) => folder.mark_processed(plan, size),
                Err(failure) => eprintln!("{}: {}", plan.master, failure.message),
            }
```

`mark_processed` is only reached on success, and `apply_plan` begins by creating and saving a fresh project (`Project::new(format!("watch {}", ...))`, `commands.rs:796-810`). A master that fails permanently — an unwritable output folder (see A1, which makes this likely), a corrupt marker list, an unsupported codec — is therefore re-attempted every two seconds, each attempt inserting another project row and running the cut again: on the order of 1 800 projects an hour, growing without bound. Fix: mark the fingerprint (or a per-fingerprint attempt count with backoff) on failure as well, and reuse one project per master rather than minting a new one per attempt.

#### L7 — major — a debug build overflows its stack on every cut

Reproduced (more than six runs, three subcommands, all four verify policies): a debug build of `thetrimmer cut clip.mp4 --in 00:00:00:00 --out 00:00:01:00 -y` completes the encode step, then reports `thread 'main' has overflowed its stack` and dies with `0xC00000FD`; the release build of the identical command exits 0 with a full check table. `batch` and `watch --apply` behave the same way, and the CLI has no tests, so nothing in CI catches it. The recursion is not in this crate — the last progress event is `Progress::Finished` (`trimmer-media/src/process.rs:468`) and the probe of the produced file succeeds in debug, so the overflow is somewhere in the executor path — and I could not localise it without a debugger. It directly contradicts the justification written next to the lint exemption it relies on, `crates/trimmer-media/src/lib.rs:70-75`:

```rust
// `large_futures` is a stack-size lint. The futures here are large because the executor's own
// state machine nests four levels deep, not because any single future holds a large buffer;
// boxing them all would trade a stack cost for a heap allocation on every call. The depth is
// bounded and the tests run on the default 2 MiB thread stack, so this is measured rather than
// assumed — see the async tests in `executor.rs`, which run the real nesting.
```

Whatever the recursion turns out to be, that claim is false for the debug profile. Fix: reproduce with a debug build (or a test that runs a real cut in the debug profile, which the crate lacks), find the unbounded recursion or box the future, and either way replace the comment.

#### L8 — minor — `verify` never validates the plan it is given

`commands.rs:601-618` deserialises a `CutPlan` from a file and passes it straight to `verify_cut`. `CutPlan`'s own documentation (`plan.rs:222-227`) says the invariants are re-checked "whenever one is read back from a project file or handed across a process boundary", and `verify_cut_with` never calls `violated_invariants`. A hand-edited plan therefore reaches the checks unvalidated, and the unchecked subtraction in `requested_frames()` (`plan.rs:186-188`) can overflow, with `check_frames`' `delivered >= requested` comparison (`check.rs:501`) passing trivially for a negative request. Fix: `plan.verify_invariants()?` immediately after deserialising, and use saturating arithmetic in `requested_frames`.

#### L9–L16 — minor — user-visible defects in the CLI surface

* **L9** `commands.rs:519-521` and `:880`: `let _ = context.store()?.record_run(...)` discards the result, so a run whose audit record cannot be written (full disk, locked database, a manifest that will not serialise) exits 0 with no message — while the store's own doc says such a failure "is worth a line in a log" (`store.rs:224-235`). Print it.
* **L10** `commands.rs:431-439`: the retimed SRT is written with `caption::write(&target, &retimed.cues, "\r\n", false)` — always CRLF, never a BOM — while `caption::read` records the source's own ending and BOM and `caption::retime_file` passes them through (`caption.rs:535-540`). This contradicts the module's stated reason for preserving the shape ("these files get diffed and re-imported, and a gratuitous reformat shows up as a whole-file change").
* **L11** `commands.rs:76-78` promises `doctor` exits 1 when something is missing, but a missing ffmpeg fails in `Context::new` (`context.rs:89-90`) as `Failure::refused` = 2 (reproduced with a broken `THE_TRIMMER_FFMPEG`), and `Failure::internal` is also 1 (`context.rs:50-57`) for a store write failure, an unopenable database or a missing data directory — so a script is told "the material is not good enough" when the tool could not open its own database. Fix the promise or add a third code, and update `lib.rs`'s table.
* **L12** `commands.rs:684-724`: the notifier closure is `move` and owns the only `Sender`; when `recommended_watcher` or `watch()` fails, the closure is dropped, so `recv_timeout` returns `Disconnected` immediately, every time, and the fallback loop never sleeps. The message promises a two-second scan; the process burns a core instead. (Static: I could not force `notify` to fail here.) Keep a `Sender` alive and treat `Disconnected` as "sleep two seconds".
* **L13** `commands.rs:166-170`: `view.transcript_cues.unwrap_or_default()` renders an unreadable caption file as "(0 cues)", masking the failure that `--json` correctly reports as `null`.
* **L14** `commands.rs:327` with `:431-433`: the caption work runs after the cut is written and verified, and a failure exits 2 — so a typo in `--srt` costs the whole encode and then reports a usage-class failure while a verified deliverable sits on disk.
* **L15** `context.rs:199-202`: `quote` returns a bare string unless it contains a space, tab or quote, so `a&b.mp4`, `100%.mp4`, `it's.mp4`, `a|b.mp4` and `a;b.mp4` are printed unquoted (and the inner escape is POSIX `\"`, which `cmd.exe` does not honour). `render_command` is display-only, so this is not injection — but the module sells the line as something that can be "copied out of the terminal and run", and for those names it runs something else.
* **L16** `commands.rs:675-680`: `policy.cut_whole_when_unmarked = true` is forced, overriding `WatchPolicy::default()` (false, "Cut the master whole when no marker list is present") with no flag and no mention in the `watch` help — so `watch --apply` on a folder of masters silently delivers a full-length cut of every file.

#### L17–L22 — nit — naming, help and dead surface

`project.rs:204` (the UNC-carrying resolver, see E1/L2 above); `commands.rs:477-490` (`batch --segments` uses `.find` on a prefix match and then disables every other segment, so an ambiguous prefix silently cuts the wrong one); `cli.rs:385-386` (`--out-exclusive` without `--out` is ignored — `requires = "out"` would say so); `commands.rs:99` vs `:144` (the key `timescale` means ticks in `--json` and `1/n` in the human output); `Cargo.toml:3` (the description advertises a `licence` subcommand that does not exist, and `tracing`, `tracing-subscriber`, `directories` and `serde` are declared but unused — nothing initialises a subscriber, so `--verbose` is the only diagnostic channel); `commands.rs:332-333` and `:448` (dead parameters `path` and `context`, and `cli.rs:99`'s "Nothing is written until the plan is shown and confirmed" describes a confirmation step the non-dry-run path does not perform).

### apps/desktop (Tauri backend)

The crate is a workspace member and its `commands.rs` is 1 098 lines of IPC surface over the same domain, so it is in scope. Findings here are static (nothing in this tree compiles into an application — see DS1) and were cross-checked against the vendored `tauri-2.11.6` / `tokio-1.53.1` sources where the behaviour depends on them.

| # | Severity | Location | What is wrong |
|---|---|---|---|
| DS1 | major | `lib.rs` (61 lines), `Cargo.toml:11-13` | There is no entry point: not one command is registered, and there is no binary target |
| DS2 | major | `state.rs:215`, `267-273` | The measurer builds a runtime and `block_on`s inside the Tauri runtime: panic, and `panic = "abort"` kills the app |
| DS3 | major | `commands.rs:930-965` | `export_timeline` truncates any path the renderer names and creates its directories |
| DS4 | major | `commands.rs:203-453`, `1082-1085` | Project edits are never persisted; `open_project` drops the open project without saving |
| DS5 | minor | `commands.rs:712-722` | `run_batch` takes the workspace out of shared state and restores it unconditionally |
| DS6 | minor | `commands.rs:604-678` | `cut_segment` bypasses verification and returns a fabricated "succeeded" payload |
| DS7 | minor | `state.rs:167-172` | The desktop reads a different database file from the CLI and the daemon |
| DS8 | minor | `commands.rs:152-159`, `370-410`, `514-576` | Open-ended segments cannot be set (serde swallows `null`); `delete_project`'s check and clear use two locks; `preview` reports no problems |
| DS9 | minor | `commands.rs:72-76`, `875-905` | `doctor` prints the ffmpeg version as ffprobe's; `transcript_lines` invents 25 fps; the transcript cache is rebuilt per call |
| DS10 | minor | `lib.rs:16-23`, `commands.rs:1034-1037` | The module's security claims contradict the capability file and `reveal`'s implementation |
| DS11 | nit | `state.rs:79-85`, `commands.rs:353`, `809`, `1088-1097` | `Debug` for `AppState` takes a non-reentrant lock; `handle_frames` unvalidated; Debug enum names in UI payloads; dead helpers |

#### DS1 — major — the desktop backend is not wired to anything

`apps/desktop/src-tauri/src/` contains exactly `lib.rs`, `state.rs` and `commands.rs` — no `main.rs` — and `Cargo.toml:11-13` declares only `[lib] crate-type = ["staticlib", "cdylib", "rlib"]` with no `[[bin]]`. A search of the whole repository for `tauri::Builder`, `invoke_handler` or `generate_context` matches a single comment in `build.rs:3`; `AppState::bootstrap()` (`state.rs:97`) and `has_workspace()` (`state.rs:156`) have no callers. So nothing constructs the state or the window, none of the 25 commands is registered, and every `invoke(...)` from `apps/web` would reject as unknown — while `lib.rs:8` states that "this crate turns those into typed, named IPC commands the interface can call" and `build.rs:3-5` describes a `generate_context!` invocation that does not exist. Fix: add `src/main.rs` (or `pub fn run()` in `lib.rs`) that boots the state, `.manage`s it and registers every command in `invoke_handler(tauri::generate_handler![…])`, add the binary target, and re-test DS2–DS4 against the real application. Until then those findings are latent by construction, and this is the gate that decides whether they are live.

#### DS2 — major — the desktop measurer panics inside the Tauri runtime

`apps/desktop/src-tauri/src/state.rs:215` calls `block_on(self.prober.probe(...))`, and that helper (`state.rs:267-273`) builds a **new current-thread runtime** and calls `.block_on()` on it:

```rust
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(future)
}
```

Tauri runs command futures on a multi-thread Tokio runtime (`tauri-2.11.6/src/async_runtime.rs:223`, `TokioRuntime::new()`), and `Queue::verify` calls this synchronous trait method from inside `Queue::run_one`, which is awaited by `run_batch`. Entering a runtime while one is already entered makes `Runtime::block_on` panic with "Cannot start a runtime from within a runtime"; with `[profile.release] panic = "abort"` (`Cargo.toml:61`) the process dies mid-batch, and in a dev build the task dies and the `invoke` promise never settles. The queue's fallback (`queue.rs:776`, `let Ok(cut_facts) = … else`) catches `Err`, not a panic. The daemon's sibling implementation does it correctly — `Handle::try_current()`, a multi-thread flavour check, then `block_on` inside `block_in_place` (`trimmer-daemon/src/state.rs:301-317`) — and `trimmer_media::FactsMeasurer` (`probe.rs:374+`) exists for this purpose. The three other trait methods are also wrong in the opposite direction: `frame_hashes`, `extract_frame` and `ssim` return *empty successes* (`state.rs:239-263`) where the daemon returns explicit `Err`/`UNMEASURABLE`, and their comment claims "the check reports itself as skipped rather than pretending to have run" — which is true today only because nothing ever calls them (V1). Fix: reuse the daemon's measurer (or `FactsMeasurer`) and return refusals rather than empty values, so a future evidence-supplying queue cannot mistake "not measured" for "measured clean".

#### DS3 — major — `export_timeline` writes wherever the renderer says

`apps/desktop/src-tauri/src/commands.rs:959-965`:

```rust
        let destination = PathBuf::from(&path);
        if let Some(parent) = destination.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(explain)?;
            }
        }
        std::fs::write(&destination, &product.body).map_err(explain)?;
```

`path` is an unvalidated `String` from the webview: it is never turned into a `MediaPath`, never checked against the project, and never checked for extension or for an existing file. A single `invoke("export_timeline", {format:"edl", path:"H:\\masters\\EP01_master.mp4", …})` truncates the master and replaces it with a few kilobytes of EDL — and because the document is built before the write, the call always succeeds — while `create_dir_all` will happily build an arbitrary tree first. This contradicts the file's own rules ("A path is a string until the domain accepts it. … Nothing joins a user string to a directory", `commands.rs:12-14`) and `lib.rs:16-19` ("**no** filesystem, process or licence operation is reachable from JavaScript"). Fix: choose the destination from Rust (a `tauri_plugin_dialog` save dialog invoked from the command), or at minimum require an absolute path whose extension matches the format, refuse to overwrite without an explicit flag, and drop `create_dir_all`. Note the JS-side `pickSave` (`apps/web/src/ipc/dialog.ts:65`) is a UI affordance, not a boundary: it produces the string the command receives.

#### DS4 — major — the desktop never saves a project except after a cut

Every mutating command edits the workspace and returns without persisting: `add_source` (`commands.rs:203-229`), `refresh_sources` (`:249-259`), `remove_source` (`:266-279`), `add_segment` (`:347-360`), `update_segment` (`:380-428`), `remove_segment` (`:438-441`), `reorder_segment` (`:447-453`) and `set_verify_policy` (`:1082-1085`). Only `cut_segment` (`:651-656`) and `run_batch` (`:720`) call `workspace.save`, and both reduce a failure to a `tracing::warn!`. `save_project` (`:179-184`) exists but has **no caller**: the web IPC layer (`apps/web/src/ipc/commands.ts`) defines no `saveProject` wrapper and no file under `apps/web` invokes one (verified by search). `open_project` then replaces the in-memory project outright (`commands.rs:140`, `state.set_workspace(workspace)`), dropping the previous one unsaved. So an editor can create a project, add sources, mark forty segments and close the window — or simply open a second project — and the store still holds only the empty row `create_project` wrote, with no warning; `apps/web/src/components/ErrorBoundary.tsx:48` tells the user "Your cuts are saved as you make them, so reloading loses nothing." Fix: persist inside `with_workspace` after each mutation (a debounced autosave is fine), save before replacing the workspace in `open_project`, and surface a save failure instead of logging it.

#### DS5 — minor — `run_batch` removes the workspace from shared state

`commands.rs:712-722`:

```rust
    let mut workspace = state
        .workspace
        .lock()
        .take()
        .ok_or_else(|| "no project is open".to_owned())?;

    let outcome = state.queue.run(&mut workspace, &options, sink).await;

    let saved = workspace.save(state.store.as_ref());
    *state.workspace.lock() = Some(workspace);
    *state.running.lock() = None;
```

The comment above it justifies the removal with a renderer-side assumption ("the interface disables the editing controls"), while `lib.rs:11-14` declares the opposite threat model. Tauri commands are concurrently callable, so with the field empty: a concurrent `cut_segment` fails with "no project is open"; a concurrent `open_project(B)` installs B and line 721 overwrites it with A, after which the UI and Rust disagree about which project is open; a concurrent `delete_project(A)` is undone by line 720's upsert, resurrecting the project the user just deleted; and any panic between the `take` and the restore (DS2 is one) loses the open project for the session. Fix: keep the workspace in place and pass the project through the queue (the file's own comment concedes this is the deeper fix), or clone-and-merge under one lock only if the id still matches, plus an explicit "a batch is running" guard on the commands that mutate.

#### DS6 — minor — a single cut skips verification and reports a fabricated success

`commands.rs:604` documents `cut_segment` as going "through the queue machinery, so a single cut and a batch behave identically", but it calls `engine.cut(...)` directly and never touches `state.queue`, so `Queue::verify` never runs, the project's verify policy is ignored, and there is no way to ask for it. The reply then hardcodes the verdict it never computed (`commands.rs:658-678`):

```rust
            "status": {
                "kind": "succeeded",
                ...
                "checks": [],
            },
        }],
        "deliveredFrames": outcome.frame_count,
        "deliveredSeconds": 0.0,
```

Under the queue's own semantics `Succeeded` means "the checks passed or were switched off" (`queue.rs:83-84`), and the default policy is `Strict`. `deliveredSeconds: 0.0` and `cancelled: false` are constants, and `name` carries the *cut-mode label* here while the same wire field carries the *segment name* in `run_batch`, so the proof panel shows a check table headed "Head patch" with no checks. (The panel does caveat an empty check list, so the user is not actively misled — but the file's claim of identical behaviour is false and the verification is skipped.) Fix: route the single cut through the queue with a one-segment workspace, delete the hardcoded fields, and map `name` to the segment name.

#### DS7 — minor — the desktop uses a different project database from the rest of the product

`state.rs:167-172`:

```rust
fn default_store_path() -> PathBuf {
    directories::ProjectDirs::from("com", "thetrimmer", "TheTrimmer").map_or_else(
        || PathBuf::from("thetrimmer.db"),
        |dirs| dirs.data_dir().join("projects.db"),
    )
}
```

`trimmer-store` already publishes this helper with different values (`ProjectDirs::from("com", "TheTrimmer", "TheTrimmer")` and `projects.sqlite`, `store.rs:121-125`), and the CLI and the daemon use it. The private copy changes both the organisation casing and the file name, so the desktop's projects are invisible to `ttrim` and to the daemon and vice versa — in a product whose export and batch surfaces are meant to work on the same projects. The `thetrimmer.db` fallback is also a relative path, i.e. dependent on the working directory. Fix: call `trimmer_store::default_store_path()`.

#### DS8 — minor — three places where the command surface contradicts itself

* **An open-ended segment cannot be set.** `update_segment` takes `end_frame: Option<Option<i64>>` (`commands.rs:370-374`) and only writes when the outer option is `Some` (`:396-410`), but serde maps JSON `null` to the *outer* `None`, so `Some(None)` is unreachable from the wire: no request means "run to the end", even though the frontend types the field as `number | null` and an open-ended segment is a representable domain state (`Segment.end_frame: Option<i64>`). A caller sending `endFrame: null` gets a silent no-op and a success reply. Fix: double-option deserialisation (or an explicit `clear_end_frame` flag).
* **`delete_project` locks twice.** `commands.rs:152-159` tests the workspace under one guard and clears it under a second: between the two, a concurrent `open_project(B)` installs B, which the body then clears — leaving the UI believing B is open while every command answers "no project is open". Fix: hold one guard across the read and the write.
* **The dry run reports no problems and hides a refusal.** `preview` calls `engine.preview(...).unwrap_or_default()` (`commands.rs:514-516`) and hardcodes `"problems": []` (`:530`), so a build refusal becomes `"commands": []` — indistinguishable from "nothing to run" — while `Workspace::preview` computes real problems from `diagnose()`; and `preview_all` turns a failed element into `{"problems":[reason]}` with no `segment` and no `estimatedBytes` (`:573-576`), which the UI then counts as a zero-byte, correctly-planned segment. Fix: carry the error into `problems` (or fail), populate the fields the interface's type requires, and delete the duplicated size estimate in `commands.rs:537-555`, which already disagrees with `workspace.rs:510-526` on the audio term.

#### DS9 — minor — diagnostics and transcripts guess

* `commands.rs:72-76`: `let ffprobe = ffmpeg.clone();` — the panel reports ffmpeg's version line as ffprobe's, and `capabilities()` only ever runs `ffmpeg` flags (`probe.rs:339-359`), so a missing, ancient or mismatched ffprobe — the tool every probe depends on — can never be surfaced. `.unwrap_or_else(|_| "not found")` also discards the reason.
* `commands.rs:905`: `transcript_lines` substitutes `FrameRate::FPS_25` when the source has not been probed, while `parse_timecode` in the same file refuses with "that source has not been probed, so its frame rate is unknown" (`commands.rs:469-471`). One of the two is wrong, and the guess is the failure `Project::rate_for`'s doc names ("guessing a rate is exactly how timecode silently becomes wrong").
* `commands.rs:875-897`: a fresh `TranscriptService` is built per command, so the cache documented as shared between searches (`state.rs:62-63`, "Transcripts, cached across searches") is created and dropped on every call, and the `transcripts` field it would use is never read.

#### DS10 — minor — the security claims in `lib.rs` are not true of the code

`lib.rs:16-23` asserts that "the capability file grants the page a file picker and nothing else", that "**no** filesystem, process or licence operation is reachable from JavaScript", and that every command turns a path string into a `MediaPath` "rather than joining it to anything itself". Three counter-examples in this tree: `export_timeline` (DS3) writes an arbitrary path; `apps/desktop/src-tauri/capabilities/default.json:12-13` grants `opener:allow-reveal-item-in-dir` **and** `opener:allow-open-path`, so page JavaScript can hand any path to the OS to open with its default handler; and `reveal` (`commands.rs:1034-1037`) documents itself as selecting "a file that this application just wrote" while its implementation accepts any existing path. Fix: either remove `opener:allow-open-path` and validate the path arguments (preferred), or rewrite the module docs so they describe the actual trust boundary — an audit that promises a boundary the code does not have is worse than one that documents the boundary it does.

#### DS11 — nit — small hazards and stale surface

`state.rs:79-85`: `Debug for AppState` calls `self.workspace.lock()`, and the lock is not reentrant, so any future `{:?}` of the state from inside a `with_workspace` closure deadlocks every command (latent — no current caller). `commands.rs:353`: `end_frame.unwrap_or(start_frame + 1)` overflows for `start_frame == i64::MAX` even though the next line overwrites `end_frame` unconditionally, so the arithmetic is pure hazard. `commands.rs:809`: `format!("{:?}", result.check)` puts Rust enum names in the UI, against the file's own rule that an error is "never a debug format of a Rust enum", where the domain already provides `Check::label()`. `commands.rs:1088-1097`: `collecting_sink()`/`adapter()` are `#[allow(dead_code)]` helpers kept "so a future command can…", which keep otherwise-unused imports alive. `commands.rs:321-322`: `preservesPicture` re-implements `DeliveryPreset::preserves_picture`; the two agree for every shipped preset but diverge for a geometry with zero width/height and a non-`Native` fit.

## Checked and clean

These are the things I examined and found correct, with the evidence that the check was actually made. I list them because they are the other half of the review: the findings above are exceptions to a codebase that is, in most places, careful.

**trimmer-core**

* Drop-frame conversion is right. `format_timecode`'s ten-minute correction and `parse_timecode`'s label subtraction round-trip for every rate and 1 500 000 frames under `proptest` (`timecode.rs:1135-1169`), and the pinned values at `timecode.rs:843-867` (107 892 for one hour at 29.97, 1 800 = `00:01:00;02`) match the arithmetic by hand.
* `FrameRate` validation itself is sound: zero and negative denominators, negative numerators and rates above 1 MHz are refused (`timecode.rs:148-167`); `Ord` compares by exact `i128` cross-multiplication (`timecode.rs:329-336`); `decimal_to_ratio`/`limit_denominator` do the continued-fraction walk in checked integer arithmetic with a documented 1001 bound, and the `29.97 → 30000/1001` case is pinned (`timecode.rs:1051`).
* `split_timecodes`' byte-index arithmetic is safe: every slice boundary is either a comma (1 byte), the end of an ASCII digit run, or guarded by `starts_with(':')` (`timecode.rs:689-719`).
* `plan_cut` refuses before it clamps: empty ranges, an in point past the last frame, an out point past the frame count and an unsupported codec are all named errors with the numbers attached (`plan.rs:392-432`), and `resolve_range` is called only after that validation.
* `CutPlan::violated_invariants` and `apply_calibration` implement the accounting they describe: calibration moves frames between head and body without changing the total, refuses an offset over the 12-frame bound and refuses one that would empty a part (`plan.rs:554-579`), and `FramesAddUp`/`BodyStartsOnKeyframe`/`HeadIsBounded` are recomputed from the fields rather than trusted from the claims list.
* The SRT parser is robust where it claims to be: BOM, CRLF, a cue number glued to the timing line, out-of-order cues and blank-line runs are all handled (`caption.rs:301-377`), `parse_stamp` scans rather than anchors and the comma/dot quirk is pinned by a test (`caption.rs:953-966`), and `render` preserves the file's own line ending inside cues as well as between them (`caption.rs:411-440`).
* `capture`'s `retime` clamps rather than drops a straddling cue only when at least `MIN_OVERLAP` survives, and the epsilon use at line 479 is in the right direction (a cue exactly on a mark is not treated as straddling).
* `TranscriptIndex`'s fold maps folded offsets back to original byte offsets through a provenance map (`transcript.rs:388-445`), and `highlight` is only ever called with boundaries the map produced, so it cannot slice a multi-byte character.
* `MediaInfo::is_variable_rate` uses a relative threshold and a zero guard (`domain.rs:320-331`); the `Timescale` type genuinely prevents the `1/90000`-as-a-frame-rate mistake the docs describe (its own doc block, `domain.rs:195-207`).

**trimmer-media**

* No shell anywhere: every invocation is an `OsString` argument array (`process.rs:355-364`), and the concat listing escapes an apostrophe in the ffconcat quote style (`executor.rs:918-923`) — which I checked against `av_get_token`'s behaviour: outside quotes a backslash escapes the next byte and inside quotes everything up to the next quote is literal, so `'\''` round-trips to `'`.
* The three V1 hazards are genuinely addressed in the argument builders: `-video_track_timescale` carries the source's timescale (`executor.rs:633-634`), the encoder follows the source codec family with the `hvc1` tag for HEVC (`executor.rs:623-636`, `plan.rs:508`), and the body is copied as two passes — picture with an input seek, sound with an output seek, then a mux (`executor.rs:683-745`) — with tests asserting the `-ss`/`-i` order for both (`executor.rs:1061-1082`).
* `prepare_copy` bounds the copy by time rather than by `-frames:v`, with the decode-order reason recorded (`executor.rs:785-812`).
* Cancellation, heartbeat and timeout are all polled between reads, no lock is held across an `await`, and the child is killed on cancel and on timeout (`process.rs:383-457`). `kill_on_drop` is set.
* `ToolPaths::resolve` treats a broken override as an error rather than a fallback, ignores an empty override, and names the environment variable that would fix a missing tool (`tool.rs:87-118`).
* `parse_listing` handles the three column widths of `-encoders`/`-muxers`/`-filters`, skips legend lines by their `=` token and deduplicates (`tool.rs:278-329`); the fixtures are literal ffmpeg output.

**trimmer-app**

* `Workspace::output_path` is the single place a deliverable path is derived, so the queue, the preview and the interface cannot disagree (`workspace.rs:615-644`). The single-source-of-truth property is correct; the arithmetic inside it is A1.
* `Queue::run` logs a `QueueEvent::Finished` and an audit record for every job it attempts, and `run_one` never returns an error for a per-segment failure (a segment's failure is its status, as documented). The `Pending → Planning → Cutting → Verifying` progression never goes backwards.
* `MediaEngine::preview` is routed through the real argument builders, so the CLI's dry run cannot drift from what will actually run (`ports.rs:300-329`).
* `SystemClock` uses saturating conversions (`ports.rs:161-179`) and a monotonic `Instant` for durations; the preview's size estimate guards `requested_frames() <= 0`, `width == 0` and `rate_numerator.max(1)`, so it cannot divide by zero (`workspace.rs:510-526`), and `summary()`'s `views.len() - runnable.len()` cannot underflow because `runnable ⊆ views`.
* `TranscriptService::load` drops the cache guard before reading the file and rejects a transcript with no readable cues (`transcript.rs:288-305`).

**trimmer-store**

* No SQL is built by string concatenation: every statement binds its values, and the only string-built SQL in the crate is in tests over literal table names. Column/parameter mappings were checked row by row (the 12-column segment insert, the 8-column project upsert, the 7-tuple header read, the 9-column run aggregate query) and none is off by one.
* `save_project` and `record_run` each run inside one transaction, and every `?` before the commit drops the transaction (rolling back), which is what makes `a_failed_save_leaves_the_previous_state_intact` pass.
* No `unwrap`/`expect`/indexing/slicing over stored data anywhere in `src/`; `ordinal_of` saturates, float→int casts saturate, and `uuid_of` reports decode failures.
* `document.rs` is a pure serde JSON mapping with no database access and no findings.

**trimmer-verify**

* The verdict plumbing is consistent: exactly the nine checks in `Check::ALL` order, one result each (`check.rs:455-479`), with `is_failed`/`is_warning`/`is_skipped` and the report table all derived from the status enum; `report()` prints a row per check so what was *not* checked is visible.
* `check_duration` uses the plan's exact numerator/denominator rather than a rounded rate, compares against the frames the file actually holds (with the documented reason), and fails closed on NaN.
* `check_audio_alignment` compares track *ends* with the sign the doc states, tolerates two frames, fails dropped audio and warns about audio the source lacks.
* `check_head_fidelity` gates on `Forensic`, uses an inclusive 0.98 threshold, and keeps "unmeasurable" distinct from a score of 0.0 via the `Similarity` newtype.
* `check_captions` reuses `caption::retime` rather than re-implementing the clamp rules, and its `zip` is length-guarded so both a missing and an invented cue fail.
* `audit.rs`'s canonical form rebuilds every object through `BTreeMap` at every depth (so feature unification cannot change the digest), covers the run id, project, version and machine, and its constant-time comparison folds the length with XOR.
* `measure.rs`: `NoMeasurer` never fails, truncates to the requested window and re-stamps `first_frame`; no rule calls a `MediaMeasurer` method.

**trimmer-export**

* XML escaping is correct: `quick_xml::escape::escape` escapes `&`, `<`, `>`, `"` **and** `'`, including in character data, and the test asserts all five (`xml.rs:43-45`, `xml.rs:122-128`).
* Percent-encoding is correct for the characters it handles: everything outside RFC 3986's unreserved set plus `/` and `:` is encoded byte-wise from the UTF-8 representation, with uppercase hex (`xml.rs:66-90`), so a path cannot inject XML or a query.
* CSV quoting doubles embedded quotes and quotes fields containing commas, quotes or newlines, and the header/row widths line up (`csv.rs`).
* EDL out points are inclusive and cannot underflow, because zero-length clips are skipped before the `- 1` (`clips.rs:167-174`, `edl.rs:84-87`), and event numbers are guarded at 999.
* FCPXML times are exact reduced rationals built in `i128` from the frame count and the rate, with a documented `0s` spelling for zero (`clips.rs:234-283`); the `gcd` divisor cannot be zero because `FrameRate` guarantees a positive numerator.
* Every `write_*` helper is a thin `std::fs::write` with the path and OS reason in the error (the atomicity gap is E6, not correctness of the mapping), and the format-specific names prevent writing XML into a `.csv`.

**trimmer-daemon**

* Authentication covers every route: the middleware is applied with `route_layer` to the whole router before `.with_state` (`routes.rs:231-255`), and `constant_time_eq` plus the non-empty check (`routes.rs:215`) means a missing header cannot match an empty configured token; `validate()` refuses a token under 16 characters and any whitespace (`config.rs:60-72`).
* The loopback-only rule is enforced in the library path (`serve` calls `validate` before binding, `routes.rs:268`) and the bind string is parsed as an `IpAddr` before use, so a hostname cannot resolve to a public address; tested for `0.0.0.0`, `192.168.1.10` and `::`.
* No CORS layer is installed, so there is no wildcard-with-credentials combination.
* Bodies are bounded: every body arrives as `Json<Value>`, and axum applies its 2 MiB default limit even without an explicit layer; the transcript `limit` is clamped to 0..=1000 (`routes.rs:632-634`).
* No path is joined to a user-controlled root anywhere; the only derived path is a sibling caption file. `add_source` canonicalises (its canonicalisation bug is C6/E1, not traversal).
* The run registry is a plain `std::sync::Mutex<HashMap<..>>` whose methods all take the lock in short synchronous bodies with no guard held across an `await`, and no lock result is `unwrap`ed.
* No panic is reachable from request input: ids go through `Uuid::parse_str`, body fields through `as_str`/`as_i64`, and `handle_frames < 0` is refused. `plan_cut`'s `keyframe.expect("checked above")` (`plan.rs:478`) is genuinely guarded by the `unusable` match above it.
* `ProbeMeasurer` refuses rather than panics on a single-threaded runtime (`state.rs:303-317`), and the daemon's runtime is multi-threaded (`main.rs:97-100`).
* The OpenAPI paths and methods match the router exactly (all thirteen, including the `get`+`post` pairs), the documented success codes match the handlers, the documented required fields match the extractor calls, and the enum words match `RunState::word()`.

**trimmer-cli**

* Every failure path returns a non-zero exit code: `Failure` distinguishes refused/internal/checked and `main` maps them (`lib.rs`, `main.rs`), and `verify` exits non-zero when a check fails (`commands.rs:621-628`).
* `watch` validates that the folder exists and that the preset resolves before starting (`commands.rs:668-674`).

**apps/desktop (static; nothing here is wired up — DS1)**

* Every id argument is parsed before use (`open_project`, `delete_project`, `update_segment`, `remove_segment`, `preview`, `cut_segment` all go through `Uuid::parse` and return a sentence on failure), and `reorder_segment`'s indices are validated by the domain *before* `Vec::remove`/`insert` (`domain.rs:795-805`), so no index panic is reachable.
* `format` and `policy` are allowlisted (`commands.rs:936-942`, `:1075-1081`), and the caller-supplied `sequence_name` is cleaned and escaped downstream by `trimmer-export`, so there is no XML/CSV injection through the export.
* `add_source` canonicalises at the boundary (`commands.rs:204`), and the command layer routes through `trimmer-app`'s `Workspace` rather than re-implementing the domain's validation.
* Segment ranges that reach the views are planned first (`Workspace::add_segment` → `plan_cut`, and `update_segment` at `:419`), so the `end - start` / `end - 1` arithmetic in `domain.rs:607-609` and `workspace.rs:370-386` cannot be driven to overflow through the API.
* The cut's output name cannot be traversed by a hostile segment name: `sanitise_name` strips both separators and every illegal character, collapses runs, trims trailing dots and caps the length (`workspace.rs:728-754`).
* `AppState::bootstrap` fails loudly when the tools or the store are unusable (`state.rs:97-100`), and `EmitterSink`'s ignored `emit` results are deliberate and documented (a dead window must not take the cut down with it).
* `get_verify_policy`/`set_verify_policy` round-trip, and the capability is bound to the app's window.

## Summary

Counts by severity for the 90 findings above:

| Severity | Count |
|---|---|
| critical | 0 |
| major | 26 |
| minor | 46 |
| nit | 18 |
| **total** | **90** |

The findings I would act on first:

1. **V1 — the frame-level verification never runs, and a skipped check is reported as success.** Every production path calls `verify_cut`, which supplies an empty `Evidence`, so the frame-hash differential, the head-fidelity comparison and the caption check are `Skipped` under the default `Strict` policy; `VerifyReport::ok()` ignores skips and the queue maps that to `Succeeded` (`trimmer-verify/src/check.rs:439`, `trimmer-app/src/queue.rs:736`). The product's headline claim is not being tested anywhere.
2. **A1/L5 — every deliverable is written to the wrong directory, and its name collides.** `Workspace::output_path` passes a *directory* to `MediaPath::with_suffix`, whose `with_file_name` replaces that directory's last component (so outputs land one level up, not "beside each source"), and whose `with_extension` then eats the timecode's frame field, so two different segments can resolve to the same path and the second silently replaces the first under a `Succeeded` status. Reproduced from the CLI: `cut` without `--output` wrote `<TEMP>\ttrim-review clip 00.00.00.00 00.00.00.mp4`.
3. **M1 — an unreadable frame rate silently becomes 30 fps** (`probe.rs:136-146`). Every frame number, every `-ss`/`-t` argument and every derived frame count is then computed on the wrong grid, which is the silent-wrong-file failure the crate's own docs repeatedly call unacceptable.

Close behind, because each is cheap to fix and embarrassing to ship: **C1** (`format_timecode` panics on a rate the public API accepts, and the release profile is `panic = "abort"`), **C3/M2** (the `wav_split` and `podcast_audio` presets cannot be produced at all, and `preset.audio` is ignored everywhere, so ProRes and broadcast deliveries silently get AAC), **L1/L2** (`--out 3` means three seconds, not three frames, and a large `--out` panics a debug build), **L7** (a debug build stack-overflows on every cut, and the lint comment that relies on "measured" stack depth is false), and **DS4** (the desktop never saves a project except after a cut, while its UI tells the user every edit is saved).

Honest limitations. I did not compile or run the workspace myself; the findings marked *reproduced* were executed by the reviewer of `trimmer-cli`, who built both profiles and ran the real binaries against an ffmpeg-generated fixture, and those results are reported here as evidence rather than as my own observation. Everything else is a reading of the code and its documented contract, not an observed failure — with these specific gaps: the SQLite concurrency findings (S2, S3, D1) are code-level proofs of a missing critical section and were not reproduced with two processes; the FCPXML asset finding (E5) needs a check against Apple's DTD, which this environment cannot fetch; the axum rejection statuses (D8) and the Tauri `block_on` panic (DS2) come from reading the vendored `axum`, `tokio` and `tauri` sources rather than from executing a request; the watch-folder re-cut loop (A8) and the notifier hot-spin (L12) are consequences of ownership and control flow that could not be triggered here; and the exact recursive function behind L7 was not localised, only the crash.

One correction of process worth recording. My first pass at `Workspace::output_path` read `with_suffix` as *appending* to the directory and concluded the output was merely prefixed with the folder name. It is worse than that — the directory's last component is replaced — and the corrected analysis (A1) came from a second reader, then from the CLI reviewer's execution trace, both quoting `Path::with_file_name`'s behaviour against the call site. The file reads were each "correct" in isolation and wrong together, which is worth remembering for the next audit: the second example of the same failure is my claim in S2 that all three front ends share one database, which the desktop's own `default_store_path` disproves (DS7).

## Appendix — review method and what is not covered

* Every finding above was read in the file and line numbers were checked against the working tree; where a claim rests on behaviour I could not execute, the finding says so.
* `crates/*/tests/` were read only to establish what is already pinned (so that a missing test is not reported as a bug on its own) and to check whether a suspicious path is exercised — which is how the untested paths in C1, C4, M3, V2 and E1 were identified.
* Not reviewed: the TypeScript front end under `apps/web` (read only as *evidence* — for which IPC commands the UI actually calls and with what arguments, which is what makes DS3–DS9 reachable), the CI workflows, the `docs/` prose other than the ADRs cited, `apps/desktop/src-tauri/gen/`, and the four other audit documents already in this directory (`api.md`, `security.md`, `performance.md`, `documentation.md`), which I did not read. Where my findings overlap with them — the daemon's non-atomic project edits and the desktop's unvalidated export path are the likely candidates — the overlap is independent, not a reference.
