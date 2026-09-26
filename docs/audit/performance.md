# TheTrimmer V2 — Performance Audit of the Hot Paths

## Scope

I read, in full or in the cited regions, the following files (paths relative to the repository root;
`target/` and `node_modules/` were ignored as generated), and I did not modify anything outside this
directory:

- `crates/trimmer-media/src/probe.rs`, `process.rs`, `executor.rs` — the probe, keyframe and cut paths.
- `crates/trimmer-app/src/queue.rs`, `ports.rs`, `workspace.rs`, `transcript.rs` — the batch queue, the media adapter, the
  workspace views and the transcript service.
- `crates/trimmer-core/src/transcript.rs`, `caption.rs`, `plan.rs`, `domain.rs` — the index and search, SRT reading, the
  planner and `KeyframeGrid`.
- `crates/trimmer-store/src/store.rs`, `schema.rs` — project save/load and the run record.
- `crates/trimmer-verify/src/measure.rs`, `check.rs` (the `verify_cut` entry point and its call graph) — what a verification pass actually
  costs.
- `crates/trimmer-daemon/src/routes.rs`, `state.rs` and `crates/trimmer-cli/src/commands.rs`, `context.rs` — the second and third
  front ends, checked because they share the same application layer.
- `apps/desktop/src-tauri/src/commands.rs`, `state.rs` and `apps/web/src/**` (in particular `components/TranscriptPanel.tsx`, `components/CutTable.tsx`,
  `state/useAppModel.ts`, `ipc/commands.ts`, `App.tsx`, `components/SourceList.tsx`).

No profiler was run, per the brief: every number below is a complexity argument read off the source.
The one thing I could not verify by reading is what ffprobe actually costs on this machine's 6 GB
master — where that matters I say so and reason from the flag set instead. Where a claim is inferred
rather than read, the wording says "inferred".

## Findings

Severity is about the effect on the stated path, not on correctness. `High` = a per-item process
launch or a per-keystroke rebuild; `Medium` = repeated work that is bounded but avoidable; `Low` =
constant-factor waste.

| # | Severity | File : line | What is wrong | Fix and expected effect |
|---|----------|-------------|---------------|-------------------------|
| 1 | High | `crates/trimmer-app/src/queue.rs` : 762–785 (`Queue::verify`), called once per job from : 725 | Every job's verification probes **both** the freshly written output **and the source master again**. `self.measurer.facts(&request.media.path)` (line 780) re-reads the source's header for every segment. Over 100 segments that is 100 extra ffprobe launches against the 6 GB master — the exact "repeated process launch" the brief asks about — to re-derive facts the project already holds in `SegmentCutRequest::media` (the same `MediaInfo` the cut was planned from). Inferred cost: the source probe itself is cheap per launch (headers only, see "Checked and clean"), so the waste is launch overhead plus 100 concurrent-free header reads, not 100 full-file scans. | Build the source's `CutFacts` **once** before the loop in `Queue::run` (from the `MediaInfo` already in `Project.sources`, or one probe) and pass it into `verify`, which then probes only the output. Effect: 200 probes → 101 for a 100-segment batch; the source is opened once instead of 101 times. |
| 2 | High | `apps/desktop/src-tauri/src/commands.rs` : 875–877 and `crates/trimmer-daemon/src/routes.rs` : 636 | A **new `TranscriptService` is constructed on every search**, and its cache is a `Mutex<BTreeMap>` field of that service (`crates/trimmer-app/src/transcript.rs` : 232). A fresh service starts with an empty cache, so every debounced keystroke re-reads the SRT, re-parses all cues, re-builds every `Folded` provenance map for 3 000 cues, and re-groups sentences (`TranscriptIndex::new`, `crates/trimmer-core/src/transcript.rs` : 138–149). The module documentation at `crates/trimmer-app/src/transcript.rs` : 9–15 claims exactly the opposite behaviour — "built once and kept" — but the only construction site is per call. At a 140 ms debounce (`TranscriptPanel.tsx` : 88–93) this is O(cues × characters) per keystroke. | Hold one `Arc<TranscriptService>` in `AppState` (the engine and store already live there) and call `load` on it. Effect: the first search pays the build; every later one is the fold of the query plus an O(cues) scan. This is the single largest win in the transcript path. |
| 3 | High | `crates/trimmer-core/src/transcript.rs` : 193–201 with : 165–169 (`sentence_of_cue`) | Inside the per-cue hit loop, each hit calls `sentence_of_cue`, which does `self.sentences.iter().position(...)` — a **linear scan of every sentence for every hit**, so a search for a common word over 3 000 cues is O(hits × sentences), ~10⁶ comparisons for `"the"`, on the UI thread that answers the search box. | Make the lookup O(1) or O(log n): since both `sentences` and `cues` are sorted by time, binary-search `sentences` for the group containing `cue` (or store a `Vec<u32>` cue→sentence map built once in `TranscriptIndex::new`). Effect: the loop becomes O(cues + hits); the 3 000-cue search stops being the slowest thing in the typing path. |
| 4 | Medium | `crates/trimmer-app/src/workspace.rs` : 324–331 (`Workspace::sources`) | For every source, the view re-reads and re-parses the whole caption file **just to report a cue count**: `self.transcripts.read(&found).ok().map(|t| t.cues.len())`. `FileTranscripts::read` goes to `caption::read` (`crates/trimmer-core/src/caption.rs` : 384–401), which reads the file, lossy-decodes it, normalises line endings and parses every cue. This runs on every `refresh()` — i.e. after every mutation of any kind (`useAppModel.ts` : 135–157 issues four commands including `sources()`, and every mutation calls `refresh`, : 160–176) — and it never consults `TranscriptService`'s cache. Additionally, when the `.srt` is language-tagged, `caption::find_for` (: 551–583) does a full `read_dir` of the folder per call. | Serve `transcript_cues` from a cached `TranscriptView` (via `TranscriptService::load`) or store the count when the source is added. Effect: a refresh after toggling one checkbox stops parsing tens of thousands of caption characters, and the folder listing disappears on the common `clip.srt` path (which is already short-circuited by the `is_file()` check at : 556). |
| 5 | Medium | `crates/trimmer-app/src/ports.rs` : 250–265 (`MediaAdapter::plan`) | The plan asks `prober().keyframes(media, segment.start_frame, to)` with `to = segment.end_frame.unwrap_or(frame_count)` — the **whole segment** — but the planner only ever asks `first_at_or_after(start_frame)` (`crates/trimmer-core/src/plan.rs` : 446) and compares it with `end_frame` (: 463–466); it never looks past `start + MAX_HEAD_SECONDS` (`plan.rs` : 58, `MAX_HEAD_SECONDS = 30.0`). The executor's own path already knows this and bounds the window (`crates/trimmer-media/src/executor.rs` : 254–258). The two paths disagree, so the adapter scans a 30-minute segment for a keyframe it only needs within 30 s of the in point. Secondarily, the CLI plans through the adapter (`crates/trimmer-cli/src/commands.rs` : 240–243) and then cuts; because `plan: None` is passed, the executor's `cut` re-lists keyframes over its own window (`executor.rs` : 234–238) — two keyframe listings for one segment. | Bound the adapter's window to `min(start + frames(MAX_HEAD_SECONDS), to)`, the same expression the executor uses, and have the CLI pass the plan it already made into the cut request (`SegmentCutRequest { plan: Some(plan), .. }`) as the desktop `run_one` does (`commands.rs` : 632–639). Effect: the keyframe listing becomes a bounded seek rather than a whole-segment scan, and a CLI cut stops paying for a second listing. |
| 6 | Medium | `crates/trimmer-app/src/queue.rs` : 593–611 and : 230–238 | Per job, `status.summary()` is called for the audit entry (line 600) and the `JobStatus` is cloned into the event (line 610) and again into `jobs` (line 620) — and each `JobStatus::Succeeded`/`Unverified` owns a boxed `CutPlan` and a boxed `VerifyReport`, so every clone copies the plan (including `notes: Vec<String>`) and the whole verification report. `summary()` is called a second time per job in `BatchOutcome::report` (: 432–434), and `report()` calls `self.succeeded()`, `unverified()`, `failed()` and `skipped()` (: 356–387), each a fresh pass over the job list — O(jobs) four times for one string. Small per item, but it is the batch's own bookkeeping, and at 100 segments with audits on it is 100 plan+report copies for data that is only ever read. | Emit `summary()` once into the event and reuse the string; count the four categories in one pass of `run` as the statuses are pushed (or compute them in `BatchOutcome::report` in a single fold). Effect: removes ~2 whole-report clones per job; keeps the audit output identical. |
| 7 | Low | `crates/trimmer-core/src/domain.rs` : 449–454 (`KeyframeGrid::first_at_or_after`) | A linear scan of the grid rather than a binary search of a vector that `KeyframeGrid::new` has just sorted (: 437–439). It is called once per plan, and the grid only spans the search window, so the absolute cost is tiny (tens of entries in the normal case). | `self.keyframes.binary_search_by(...)` or `partition_point`. Effect: matters only for a very wide window — which finding 5 is the real fix for. Listed for completeness, not because it is a bottleneck today. |
| 8 | Low | `apps/web/src/components/TranscriptPanel.tsx` : 75 (limit 200) with : 147–152 and : 170 | The search command's result cap is 200 (`search_transcript` clamps to 1..=500, `apps/desktop/src-tauri/src/commands.rs` : 881), and the panel renders `hits.length` rows into a virtualised, fixed-height list. That cap is what keeps the rendering bounded — the list itself is in good shape (see "Checked and clean") — but it also means the panel can never show a full 3 000-row list and reports the count as `200+`. I found **no** caller of the `transcript_lines` command (`apps/desktop/src-tauri/src/commands.rs` : 888–922) anywhere in `apps/web`, so the "3000-row transcript list" of the brief is not currently a rendered surface at all; the heaviest transcript surface is the 200-row hit list. | No rendering fix is needed for the list as it exists. If `transcript_lines` is ever wired to a 3 000-row pane, reuse the same slice-and-overscan technique the hits list uses (windowed rows, fixed `ROW_HEIGHT`) rather than adding a table library. Flagged so the question "how does the 3 000-row list render" has an explicit answer rather than an assumption. |

Context noted while reading, outside this audit's remit but visible from the same files: `formatFrames` in
`TranscriptPanel.tsx` : 250–256 hardcodes 25 fps, so a hit's displayed time is wrong on a 29.97 or
23.976 source even though `startFrame` is correct; and the `MediaAdapter`'s `frame_hashes`, `extract_frame`
and `ssim` (`crates/trimmer-app/src/ports.rs` : 435–465) are stubs, so the frame-alignment and
head-fidelity checks are always skipped in the desktop path. Both are correctness matters, not
performance ones, and neither is a finding here.

## Checked and clean

These were examined specifically for accidental quadratic behaviour, repeated process launches and
repeated allocation, and are correct as written:

- **Probing a 6 GB master is already right.** `Prober::probe_json` (`crates/trimmer-media/src/probe.rs` : 283–310) makes exactly one ffprobe call
  with `-show_streams -show_format` and derives every domain field from that one JSON —
  the module doc at : 3–11 states the reason (each field asked for separately would cost
  a process launch). Nothing buffers the media: only stdout, which is the probe report.
  The result is persisted on the source (`Store`'s `sources.media_json`, `crates/trimmer-store/src/store.rs` : 412–424, : 491–511) and the desktop
  commands read it back from the project rather than re-probing per command (`commands.rs` : 499–503, : 618–622),
  so opening a project and editing segments does not touch the master at all. Only
  `refresh_sources` re-probes, once per source, deliberately (`workspace.rs` : 254–272).
- **Keyframe listing uses the right flags.** `Prober::keyframes` (`probe.rs` : 224–276) passes `-skip_frame nokey`, so non-keyframes are
  not decoded, and `-read_intervals` with a one-second slack either side, so the demuxer seeks
  rather than scans. `KeyframeGrid::new` sorts and dedups once (`domain.rs` : 437–445), so the planner's
  grid handling is not quadratic.
- **`plan_cut` is pure.** `crates/trimmer-core/src/plan.rs` : 374–387 launches nothing and reads nothing; every decision is a function of
  `MediaInfo`, `Segment` and the grid. That is why the plan is cheap to re-evaluate, and it is the
  reason finding 1's fix is possible without touching the planner.
- **The batch is sequential on purpose and the process count is what the method requires.**
  `Queue::run` (`queue.rs` : 519–643) runs one job at a time; the module doc at : 3–9 gives the disk-bound
  reason. A head patch is 3–4 ffmpeg invocations (head, body picture, body sound, join —
  `executor.rs` : 412–451) plus one probe of the output; none of that is redundant work.
- **Verification is cheap by construction.** `Queue::verify` calls only `MediaMeasurer::facts`
  (`queue.rs` : 776–784); the trait's `frame_hashes`, `extract_frame` and `ssim` (`crates/trimmer-verify/src/measure.rs` : 25–61) are never reached from
  the queue, and `MediaAdapter` returns empty values for all three (`ports.rs` : 435–465). So a batch does not
  silently run per-frame ffmpeg decode passes for verification, and `verify_cut` itself is a pure
  rule engine over facts (`crates/trimmer-verify/src/check.rs` : 433–456).
- **The store is linear, indexed and transactional.** `save_project` (`store.rs` : 375–461) is one transaction with one
  prepared statement per table; `load_project` (: 463–588) is three queries. The schema creates exactly
  the two indexes the reads need — `segments(project_id, ordinal)` and `runs(project_id, started_at)`
  (`schema.rs` : 116–117) — and says in the module doc why there are no speculative ones (: 8–18).
  A save of 100 segments is 3 deletes plus 100 inserts inside one commit, not 100 transactions, and
  `record_run` (`store.rs` : 258–287) likewise writes the run and all its items in one transaction.
  The delete-and-reinsert on save (: 399–410) costs a full rewrite of the project, but that is a
  deliberate correctness choice documented at : 396–398, and at 100 segments it is a rewrite of 100
  rows, not a scan of the database.
- **The small read paths avoid the master and the database.** `--read_intervals` is used for keyframes but
  not for the probe (correctly — a probe needs the header only); `list_runs` aggregates in SQL rather
  than in Rust (`store.rs` : 296–331); `BatchOutcome`'s counts are in-memory passes over at most 100 items.
- **The interface's list rendering is already virtualised by hand.** `TranscriptPanel` computes a window from
  `scrollTop` with a fixed `ROW_HEIGHT = 26` and `OVERSCAN = 6` and slices `hits` to roughly 25 rows
  (`TranscriptPanel.tsx` : 27–29, : 95–97), giving a total-height spacer for an honest scrollbar (: 170) — so
  rendering time is independent of the hit count, and the panel's comment at : 7–12 explains why. The
  search input is debounced at 140 ms (: 88–93) so typing does not fire a command per keystroke.
  `CutTable` keys rows by segment id, builds one `Map` for plan lookup (`CutTable.tsx` : 46, : 104) and uses a
  fixed row height for the reason given at : 11–15.
- **The command surface bounds its own outputs.** `search_transcript` clamps the caller's limit to 1..=500
  (`commands.rs` : 881); `ProcessRunner::run` reads stdout and stderr concurrently in 16 KiB chunks
  (`crates/trimmer-media/src/process.rs` : 371–400) so a chatty child cannot deadlock or balloon memory; polling sleeps
  1 ms only while data is arriving and the policy interval otherwise (: 449–456), so a probe does not spin.
- **The release profile is tuned for this workload** — `lto = "thin"`, `codegen-units = 1`, `strip`,
  `panic = "abort"` (`Cargo.toml` : 57–61) — and the batch is a single sequential pass, so there is no
  lock convoy: the store's connection mutex is held per statement, not across a cut (`store.rs` : 133–135
  and the lock scopes at : 258–287, : 376–459).

## Summary

| Severity | Count |
|----------|-------|
| High | 3 |
| Medium | 3 |
| Low | 2 |
| **Total** | **8** |

The three High findings are one theme seen from three angles: the transcript index is rebuilt per
keystroke (finding 2), the hit loop scans the sentence list per hit (finding 3), and the batch
re-probes the 6 GB master per segment (finding 1). The first two are in the same code path and the
same fix shape — cache the derived structure and index into it. The store, the probe, the planner,
the process runner and the list rendering all measured clean; the two Low findings are recorded for
completeness and neither is worth a change on its own.
