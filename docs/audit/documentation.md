# Documentation audit — TheTrimmer V2

> **This is a record of a review, not a list of open work.** It was written against the tree as it
> stood when it was run, and the findings that mattered have since been fixed — the fix for each is a
> commit, and the current state of the product is [docs/TRUTH.md](../TRUTH.md) and the test suite.
> Line numbers below refer to the revision that was read and will have moved.
>
## Scope

I read, in full: `README.md`; `docs/DESIGN.md`; `docs/SKILLS-APPLIED.md`; all fifteen records in
`docs/adr/` (`001-head-patch.md` … `015-licensing.md`); and the module-level doc comments of every
crate source file in the workspace — `crates/trimmer-core/src/{lib,timecode,domain,plan,caption,transcript,delivery,error}.rs`,
`crates/trimmer-media/src/{lib,executor,probe,process,tool}.rs`,
`crates/trimmer-verify/src/{lib,check,facts,measure,audit}.rs`,
`crates/trimmer-export/src/{lib,request,clips,premiere,fcpxml,edl,csv,xml,write}.rs`,
`crates/trimmer-app/src/{lib,workspace,transcript,queue,watch,ports}.rs`,
`crates/trimmer-store/src/{lib,store,schema,document}.rs`,
`crates/trimmer-daemon/src/{lib,config,openapi,routes,state}.rs`,
`crates/trimmer-cli/src/{lib,cli,commands,project,context}.rs`, and
`apps/desktop/src-tauri/src/lib.rs`. To check the claims rather than the prose I also read
`Cargo.toml`, `rust-toolchain.toml`, `.github/workflows/ci.yml`, `apps/web/package.json`,
`apps/web/tsconfig.json`, `apps/desktop/src-tauri/tauri.conf.json`,
`apps/desktop/src-tauri/capabilities/default.json`, and the test sources
`crates/trimmer-core/tests/oracle.rs`, `crates/trimmer-media/tests/end_to_end.rs`,
`crates/trimmer-media/src/executor.rs` (test module) and `tools/oracle/run_oracle.py`. Nothing was
changed outside `docs/audit/`. `target/`, `node_modules/`, `apps/web/dist/` and `.git/` were
ignored as generated.

The single most useful fact for a reader of this report: `git log` shows the last commit that
touched `README.md` and `docs/` is `d462a58` ("media, verify, export, app"), and four further
commits landed afterwards — the oracle (`101481e`), the web interface (`dc3284a`), the desktop shell
(`0b0f341`) and the CLI/daemon/store (`2cf9543`). The documentation describes the repository as it
stood four commits before the end — just before any of the store, the CLI, the daemon, the interface
or the oracle existed. The working tree then holds an uncommitted, partial correction
(`git status`: `README.md`, `docs/DESIGN.md`, `docs/SKILLS-APPLIED.md`, `docs/adr/015-licensing.md`)
that removed the licensing claims but did not re-read the rest. Almost every finding below is a
stale-status claim, not a design disagreement.

## Findings

### F1 — `README.md:9-15` — the workspace status block is false in every particular — **High**

> "Four crates are real workspace members and compile … A fifth, `trimmer-app` … is in the tree but
> is a commented-out workspace member while its modules are written."

`Cargo.toml:3-14` lists ten members, none commented out:
`trimmer-core`, `trimmer-media`, `trimmer-verify`, `trimmer-export`, `trimmer-app`,
`trimmer-store`, `trimmer-daemon`, `trimmer-cli`, `apps/desktop/src-tauri` — nine crates plus the
Tauri app. `crates/trimmer-app/src/` holds all five modules the README says are missing
(`ports.rs`, `queue.rs`, `transcript.rs`, `watch.rs`, `workspace.rs`) and an integration test,
`crates/trimmer-app/tests/application.rs`. The paragraph then contradicts itself two sentences
later by saying the store, command line, daemon and desktop shell exist. **Fix:** replace the whole
status block with the actual member list and drop the "fifth crate" sentence; the block is the
first thing a reader sees and it currently mis-describes the tree it sits in.

### F2 — `README.md:216-282` — the documented command line does not match `trimmer-cli` — **High**

The README declares the CLI "**Not yet implemented**" and then documents a flag surface that
partly does not exist. `crates/trimmer-cli/src/cli.rs` is the authority.

* Flags documented but absent from `cli.rs` entirely: `--out-exclusive` exists
  (`cli.rs:208-209`) but `--no-drop-frame`, `--calibrate`, `--no-calibrate`, `--concat-offset N`
  and `--jobs` do not exist anywhere in the crate (grep for `no-drop-frame|calibrate|concat_offset|jobs`
  returns only `outcome.jobs` in `commands.rs:536`). `README.md:265-267` and `README.md:272-277`
  promise all five.
* `thetrimmer batch --preset master --verify --continue-on-error` (`README.md:253`) — `BatchArgs`
  (`cli.rs:266-297`) has no `--preset` and no `--verify`; it has the inverse `--no-verify`
  (`cli.rs:287-288`) and `--stop-on-error` (`cli.rs:279-284`), which defaults to off, so
  `--continue-on-error` has no spelling at all.
* `thetrimmer batch --markers "M:\markers\*.csv" --preset youtube_1080 --jobs 1`
  (`README.md:254`) — `batch` takes a *project id* as its first positional (`cli.rs:269-270`);
  there is no `--markers` flag. Marker files are read by `watch` (`trimmer-app/src/watch.rs`),
  not by `batch`.
* `thetrimmer project new --name "Documentary" --source "M:\masters\interview.mp4"`
  (`README.md:257`) — `ProjectNewArgs` (`cli.rs:327-336`) has `--name` and `--by` only; there is no
  `--source`. `project add-source <PROJECT> <PATH>` is the real command (`cli.rs:346-359`).
* `thetrimmer project export … --format json` (`README.md:260`) — `ProjectExportArgs`
  (`cli.rs:410-419`) has `-o/--output` only; `project export` always writes JSON
  (`crates/trimmer-cli/src/project.rs:183-192`). `--format` belongs to the separate `export`
  command (`cli.rs:421-435`).
* `README.md:275` — "`--json` on any command makes the output machine-readable". `--json` exists on
  `probe` (`cli.rs:165-170`) and `batch` (`cli.rs:294-296`) only. `cut --dry-run --json`
  (`README.md:250`) prints the human block at `commands.rs:255-281`; the flag is not accepted.
* `README.md:253` — "one item's failure does not stop the rest" is correct behaviour
  (`trimmer-app/src/queue.rs:21-25`), but it is reached by *not passing* `--stop-on-error`, not by
  `--continue-on-error`.

**Fix:** delete the "Not yet implemented" banner (the binary is real — `crates/trimmer-cli/src/bin/ttrim.rs`,
two binaries `thetrimmer` and `ttrim`, per `crates/trimmer-cli/src/lib.rs:1-3`), and rewrite the
example block from `cli.rs` rather than from the design.

### F3 — `README.md:279-304` — the API table is not the daemon's route table — **High**

`README.md:281-282` says "**Not yet implemented.** … the daemon crate does not exist."
`crates/trimmer-daemon/` is a workspace member with five source modules, a router and an
integration test (`crates/trimmer-daemon/tests/api.rs`). The documented endpoints also differ from
the real ones in `crates/trimmer-daemon/src/routes.rs:230-255`:

| README (line) | Actual route |
|---|---|
| `POST /v1/probe` (291) | does not exist |
| `POST /v1/plans` (292) | `GET /v1/projects/{id}/preview` (245) |
| `POST /v1/cuts` (293) | `POST /v1/projects/{id}/run` (246) |
| `GET /v1/cuts/{id}`, `DELETE /v1/cuts/{id}`, `GET /v1/cuts/{id}/report` (294-296) | `GET /v1/runs/{run_id}` (247), `POST /v1/runs/{run_id}/cancel` (248); no report route |
| `GET /v1/checks` (297) | does not exist |
| `POST /v1/exports` (298) | does not exist |
| `GET /v1/projects` (299) | exists (234), plus health, capabilities, sources, segments, transcripts/search, openapi.json |

**Fix:** replace the block with the routes from `routes.rs` and say that `/v1/openapi.json`
(`routes.rs:250`) is the machine-readable contract; the crate's own docs
(`crates/trimmer-daemon/src/lib.rs:36-43`) already flag the dry-run coupling honestly.

### F4 — `README.md:345-355` — the "What is not yet true" list is now mostly untrue — **High**

This section is the README's credibility anchor and it is the most inaccurate part of the file.

* **345-348:** "The oracle harness is not in the repository. … there is no `tools/oracle` directory."
  `tools/oracle/run_oracle.py` exists (234 lines), and `crates/trimmer-core/tests/oracle.rs` exists
  (484 lines) with four differential tests and an availability guard. The sentence also contradicts
  the README's own `README.md:318`, which cites `caption::render`'s line-ending bug as *found by the
  oracle*, and `crates/trimmer-core/src/lib.rs:36` which names `tests/oracle.rs` as the mechanism.
* **349-352:** "There is no end-to-end trim in the suite — no generated clip cut and measured."
  `crates/trimmer-media/tests/end_to_end.rs:1-23` documents exactly that, and
  `:240 a_head_patch_cut_is_lossless_exact_and_lands_on_the_mark` and
  `:497 a_cut_that_lands_on_a_keyframe_copies_everything` are the tests. `.github/workflows/ci.yml:90-95`
  runs `cargo test -p trimmer-media --test end_to_end -- --nocapture` in a job named "A real cut".
* **353:** "There is no continuous integration workflow: `.github/workflows` does not exist."
  `.github/workflows/ci.yml` exists — 176 lines, three jobs (`rust`, `web`, `bundle`) — and
  `docs/SKILLS-APPLIED.md:34` cites it by name.
* **354-355:** "`trimmer-app` is not a workspace member, so its modules are not compiled or run by
  `cargo test`." It is (`Cargo.toml:9`), and `crates/trimmer-app/tests/application.rs` compiles
  against it.
* **357:** "The suites that do run are `cargo test` over the four workspace members." There are
  nine crates plus the Tauri app in the workspace.

**Fix:** rewrite the section from the test tree. Where a claim has become true, say so; the section
is worth keeping *only* if it is read from the tree each time.

### F5 — `docs/SKILLS-APPLIED.md:63-77` — all seven audit reports are declared and none exist — **High**

The table lists seven "independent audits … executed by separate agents", each with a report path
under `docs/audit/`: `code-review.md`, `security.md`, `accessibility.md`, `performance.md`,
`documentation.md`, `api.md`, `web-quality.md`. `docs/audit/` did not exist before this report was
written, so six of the seven are still missing and the seventh is this file. The prose above the
table (`:64-67`) states the reports "live in `docs/audit/`" as a fact. A reader — or a procurement
reviewer — takes this as evidence of seven completed reviews. **Fix:** either write the reports or
mark the table "planned, not yet run"; do not leave a claim of completed audit work with nothing
behind it. (This is the same failure mode `docs/adr/015-licensing.md:23-25` praises the daemon tests
for avoiding: "a claim with nothing behind it is worse than no claim".)

### F6 — `README.md` in four places — the desktop shell is described as absent while it is in the tree — **High**

* `README.md:56-59`: "**The desktop shell does not exist yet** — there is no `apps/` directory."
  `apps/web/` (React + Vite, 17 source files) and `apps/desktop/src-tauri/` (`lib.rs`,
  `commands.rs`, `state.rs`, `tauri.conf.json`, icons, capabilities) both exist.
* `README.md:361-365` ("Requirements") describes an installer and a WebView2 fetch with no note
  that neither is built; `README.md:36-40` already says the installer does not exist, so the two
  sections disagree with each other.
* `docs/adr/009-desktop-stack.md:55-58`: "there is no `apps/` directory, and
  `apps/desktop/src-tauri` is present in `Cargo.toml` only as a commented-out workspace member."
  `Cargo.toml:13` lists `apps/desktop/src-tauri` as a live member.
* `docs/adr/012-sqlite.md:52-57`: "Nothing. `rusqlite` is declared … but `crates/trimmer-store` is a
  commented-out workspace member and the crate does not exist." `crates/trimmer-store/` holds
  `lib.rs`, `store.rs`, `schema.rs`, `document.rs` and `tests/store.rs`; `Cargo.toml:10` lists it.

**Fix:** the ADR "What exists today" sections need to be updated at the moment the thing starts
existing, or the ADR needs a dated status line so a reader can tell it is a historical snapshot.

### F7 — `README.md:11-13, 391` — "the kind of small lie this README is trying not to tell" does not hold — **High**

`README.md:390-391` says "linking a document that is not there is the kind of small lie this README
is trying not to tell", and `README.md:14-15` says "Nothing here is claimed to be shipped that is
not in the tree." Both statements are the correct standard, and F1–F6 are cases where the document
fails its own standard in the same file. **Fix:** keep the sentences, but only after F1, F2, F3, F4
and F6 are corrected; as written they invite the reader to lower their guard over the sections that
are wrong.

### F8 — `crates/trimmer-cli/src/commands.rs:438-440` — a documented caption behaviour is not the coded one — **Medium**

`README.md:127-128` and `docs/adr/007-caption-clamp.md:20-21` both say the retimed file "keeps its
own shape, BOM, line endings and decimal separator included". The library function that does this
is `trimmer_core::caption::retime_file`
(`crates/trimmer-core/src/caption.rs:523-542`), which passes `&transcript.newline` and
`transcript.bom` to `write`. **The CLI does not call it.** `write_captions` calls `caption::retime`
and then `caption::write(&target, &retimed.cues, "\r\n", false)` — a hard-coded CRLF and no BOM, for
every source file, on every `thetrimmer cut`. The claim is therefore false for the only code path
that writes a caption sidecar today. **Fix:** call `caption::retime_file` (or pass
`transcript.newline` / `transcript.bom`) in `commands.rs:438`. A related behavioural gap on the same
lines: `README.md:129-130` says "Nothing inside the window writes no file and logs why", but
`retime_file`'s early return on an empty result (`caption.rs:532-534`) is bypassed by this path, so
an empty `.srt` is written instead. I verified this by reading the source; I did not execute a cut.

### F9 — `docs/adr/011-canonical-rates.md:63-67` — the ADR's own correction has been applied and is now stale — **Medium**

> "One inconsistency to record while it is visible: the doc comment on `FrameRate::parse` in
> `timecode.rs` still says decimal forms map through the continued-fraction expansion to exactly
> `2997/100`."

It does not. `crates/trimmer-core/src/timecode.rs:178-184` now reads "A decimal form is resolved
from [`CANONICAL_RATES`] first, so `29.97` becomes exactly `30000/1001`", and `CANONICAL_RATES` is
at `timecode.rs:346`. The ADR declines to name a line ("a few lines below it"), so a reader cannot
even check. **Fix:** replace the final paragraph with a short note that the comment was corrected,
or delete it; an ADR whose stated defect no longer exists reads as an unresolved problem.

### F10 — `README.md:105-107` and `docs/adr/013-presets.md:38-39` — calibration is described with flags that do not exist — **Medium**

Both say a whole-frame concat drift is measured and folded into the head, "re-cut once", and that a
correction larger than twelve frames is refused. The *pure* half is real and correct:
`trimmer_core::plan::apply_calibration` with `MAX_CONCAT_OFFSET_FRAMES = 12`
(`crates/trimmer-core/src/plan.rs:64, 553-560`). The *flag* half is not: `README.md:272` offers
`--calibrate` / `--no-calibrate` and `--concat-offset N`, none of which exist in `cli.rs` (see F2),
and nothing in `trimmer-cli` calls `apply_calibration` (grep for `calibrat` across
`crates/trimmer-cli/src` returns nothing). So the README describes the measurement-and-re-cut pass
as an available option when it is a library function with no caller outside the tests. **Fix:** say
that calibration is implemented as a pure plan-level operation and is not yet wired to the command
line.

### F11 — `docs/adr/009-desktop-stack.md:19` — an unverifiable machine fact used as evidence — **Low**

> "the machine this decision was made on reports **version 153**"

No WebView2 runtime version 153 exists (the Edge/WebView2 evergreen line is a three-digit build
number in the low hundreds), and nothing in the repository records the measurement, so the claim
cannot be checked by a reader. The rest of the paragraph — WebView2 ships with Windows 11 and
updated Windows 10, and the installer fetches it as a fallback — is correct and is what
`apps/desktop/src-tauri/tauri.conf.json` and `README.md:363-365` rely on. **Fix:** drop the version
number or attribute it to a named dated note; an unsourced number weakens the sourcing of the
paragraphs around it.

### F12 — `Cargo.toml:6` / `README.md:354` — a stale inline comment — **Low**

`Cargo.toml:5` reads "# Added as they are built, so the workspace always compiles:" above a list in
which every member is already present and live. It is harmless to the build, but it is the comment
that makes F1's "commented-out member" claim look plausible to a reader who trusts it instead of
reading the list. **Fix:** delete the comment.

### F13 — `apps/desktop/src-tauri/src/lib.rs:17` — residual "licence" mention — **Low**

"**no** filesystem, process or licence operation is reachable from JavaScript". The same stale word
survives in `apps/desktop/src-tauri/capabilities/default.json`'s `description` ("Everything that
touches a file, a process or the licence goes through a typed Rust command"). The uncommitted
README/ADR-015 edits (`git diff`) removed licensing from the README and from ADR-015, so these two
strings are leftovers of that pass. The *claim* is true — no licence operation exists
(ADR-015, and `crates/trimmer-daemon/tests/api.rs:225,748` assert the absence of the fields) — but
the word now names a thing the build does not have. **Fix:** drop "or the licence" from both.

### F14 — `crates/trimmer-app/src/watch.rs:9` — the docs say the watching lives elsewhere than it does — **Low**

> "The filesystem watching itself is `notify`'s job and lives in the daemon."

`notify` is used by `crates/trimmer-cli/src/commands.rs:684-703` (`watch`), not by
`trimmer-daemon`, whose `routes.rs` contains no watcher. The architecture is fine; the sentence
sends a reader to the wrong crate. **Fix:** say "lives in the `watch` command".

### F15 — `README.md:255` vs `README.md:277` — two ADRs are cited for the queue's design note — **Low**

`README.md:255` on batch: the design note "is in ADR-013 and in `trimmer-app`"; `README.md:277` on
`--jobs`: "the queue's design note is in ADR-013 and in `trimmer-app`". ADR-013 is *presets* —
`docs/adr/013-presets.md:1` is titled "Presets are data, and a preset that reshapes the frame
forfeits passthrough" and says nothing about queue concurrency. The sequential-queue reasoning is in
`crates/trimmer-app/src/queue.rs:3-9` and `crates/trimmer-app/src/lib.rs:31-37`. **Fix:** cite the
module, or add the ADR the claim needs.

### F16 — `README.md:96-213` — no Quick Start, and no build/run instructions for the parts that exist — **Medium** (missing documentation)

The README's structure is otherwise good — one-liner, what-it-does, features, command line, API,
correctness, requirements, documentation map, licence — but it has no "Quick Start" section and no
"Crate/Workspace" or "Building" section that a reader can follow. `README.md:373-375` mentions
`cargo test` in one sentence inside Requirements, and nothing anywhere says how to run the desktop
application, which is the product: `apps/web` needs `npm ci && npm run build`, and
`apps/desktop/src-tauri` needs `npx @tauri-apps/cli build`, exactly as
`.github/workflows/ci.yml:109-119` and `:147-165` do it. **Fix:** add a Quick Start that gives the
CLI path (`cargo run -p trimmer-cli --bin thetrimmer -- --help`), the test command, and the two
steps for the desktop build, and link the crate map so the ten workspace members are named in one
place.

### F17 — `README.md` and `docs/` — environment variables and the store location are undocumented — **Medium** (missing documentation)

The code reads at least six environment variables that no document lists: `THE_TRIMMER_FFMPEG` and
`THE_TRIMMER_FFPROBE` (`crates/trimmer-media/src/tool.rs:66`; the README mentions these two at
`:51` and `:367`), `THE_TRIMMER_STORE` (`crates/trimmer-cli/src/cli.rs:54`; the module docs at
`crates/trimmer-cli/src/lib.rs:37-40` do list all three), `THE_TRIMMER_TOKEN`
(`crates/trimmer-cli/src/cli.rs:472`), `THE_TRIMMER_V1_ROOT`
(`crates/trimmer-core/tests/oracle.rs:44`; `tools/oracle/run_oracle.py:32`), and `RUST_LOG`
(`crates/trimmer-daemon/src/lib.rs:82`). The documentation-templates structure calls for a
Configuration table in the README; here the information exists but only in Rust doc comments and in
`--help`. **Fix:** add a Configuration table to the README with the variable, its effect and its
default, and point at the oracle's variable from the correctness section.

### F18 — `docs/` — the correctness evidence exists but is not documented as runnable — **Low** (missing documentation)

`docs/DESIGN.md:1-5` promises "Each was measured on real material rather than reasoned about in the
abstract; the numbers are in the text", and several ADRs name tests by name (ADR-001:36, ADR-002:29,
ADR-004:25). Nothing tells a reader *how to run the oracle*: it needs Python and a V1 checkout,
honours `THE_TRIMMER_V1_ROOT`, and skips loudly when either is missing
(`crates/trimmer-core/tests/oracle.rs:26-31, 59-73`). The instructions exist only inside
`tools/oracle/run_oracle.py:13-21`. **Fix:** add a short "Reproducing the checks" subsection to the
README (or restore `docs/TRUTH.md`, which the README's map at `:384` already reserves for exactly
this).

## Checked and clean

These are the claims I verified against the source and found accurate. I am listing them because
they are the evidence the audit was performed rather than inferred.

**Links and structure**

* Every relative link in `README.md` resolves: `docs/adr/015-licensing.md` (49, 387, 409),
  `docs/adr/006-no-preview.md` (92), `docs/adr/008-pure-core-oracle.md` (317), `docs/DESIGN.md`
  (381), `docs/adr/` (382), `docs/SKILLS-APPLIED.md` (383). The one doc path that does not exist,
  `docs/PRICING.md`, is *not* linked and the README explains why (`:387`, `:390-391`).
* All fifteen links in `docs/DESIGN.md:11-25` resolve to existing files, and the ADR numbering is
  contiguous 001–015 with no gaps or duplicates. The carried-forward / superseded / new-in-V2
  labels match each ADR's own `**Status:**` line, including `006-no-preview.md:3` ("superseded by
  ADR-009") and `015-licensing.md:3`.
* `crates/trimmer-core/src/lib.rs:52` — `[`docs/DESIGN.md`]: ../../docs/DESIGN.md` resolves
  correctly from `crates/trimmer-core/src/`, so the intra-doc link is not broken in `cargo doc`.
* The README's own documentation map (`:377-391`) correctly marks `docs/TRUTH.md`, `SECURITY.md`,
  `RELEASE.md` and `CODE-REVIEW.md` as not written — none of the four exists, and none is linked.

**Cutting**

* Head patch, `CutMode` variants (`Copy`, `HeadPatch`, `Reencode`) and the refusal of a
  non-keyframe-bodied copy: `crates/trimmer-core/src/plan.rs`, `crates/trimmer-core/src/domain.rs`.
* `-video_track_timescale` written from the plan's `video_timescale` — `crates/trimmer-media/src/executor.rs:633, 867`,
  asserted at `:985`; the test `the_body_picture_is_seeked_as_an_input_and_the_sound_as_an_output`
  (`executor.rs:1062`) and `the_copy_path_is_bounded_by_time_and_never_by_a_frame_count`
  (`executor.rs:1096`) exist under exactly the names ADR-002:29 and ADR-004:25 give.
* The body's two passes (picture by input seek, sound by output seek, then mux) — `executor.rs`,
  `prepare_body`, as ADR-002 describes.
* `-frames:v` absent from the copy path and `-t` present — `executor.rs:1096` test.
* Handles per segment, clamped to the source; inclusive out points and `--out-exclusive` —
  `crates/trimmer-cli/src/cli.rs:196-209`, `crates/trimmer-cli/src/commands.rs:219-230`.
* Variable-rate warning — `crates/trimmer-core/src/domain.rs:317-320`,
  `crates/trimmer-core/src/plan.rs:436`.
* Cancellation polled rather than awaited to completion — `crates/trimmer-media/src/lib.rs:42-47`,
  `crates/trimmer-media/src/process.rs`.
* Only `trimmer-media` starts a process in non-test source: grep for
  `Command::new|std::process::Command|tokio::process::Command` across `crates/` matches only
  `trimmer-media/src/process.rs:355` and `trimmer-media/src/tool.rs:142` (plus the two test files
  and the oracle). The README's "the only crate that starts a process" (`:10`) is literally true.
* `THE_TRIMMER_FFMPEG` / `THE_TRIMMER_FFPROBE` overrides and the `doctor` report —
  `crates/trimmer-media/src/tool.rs:66`, `crates/trimmer-cli/src/commands.rs:23-63`.
* No network call anywhere in the product: the daemon binds only to loopback by validated
  configuration (`crates/trimmer-daemon/src/config.rs:73-86`), and there is no HTTP client
  dependency in the workspace table (`Cargo.toml:25-55`). The README's offline claim (`:45-49`) holds.

**Delivery presets**

* The eleven-row preset table at `README.md:137-149` matches `standard_presets()` exactly —
  `crates/trimmer-core/src/delivery.rs:478-680` — including container, codec, geometry and loudness
  per preset (`master` and `master_faststart` copy picture and audio; `prores_master` is
  `prores_ks`/PCM/MOV; `youtube_1080` is libx264 CRF 18 / AAC 320k / `HD_1080`; `wav_split` is
  48 kHz 24-bit PCM in WAV; `mxf_op1a` is XDCAM HD422).
* The loudness target table at `README.md:154-159` matches the four constructors:
  `STREAMING` −14/−1 (`delivery.rs:196-197`), `PODCAST` −16/−1 (`:202-203`), `EBU_R128` −23/−1
  (`:208-209`), `ATSC_A85` −24/−2 (`:214-215`).
* `preserves_picture` vs `is_passthrough` vs `head_patch`, and the deliberate asymmetry the README
  does not claim but ADR-013:48-53 states, are consistent between `delivery.rs` and `plan.rs`.
* `batch_safe` false for `prores_master`, `broadcast_r128`, `mxf_op1a`
  (`delivery.rs:528, 653, 677`) — as ADR-013:40-42 says.
* The geometry each preset claims is implemented as data, not prose:
  `Geometry::VERTICAL_1080.filters()` builds `force_original_aspect_ratio=increase` +
  `crop=1080:1920`, and `Geometry::VERTICAL_BLUR.filters()` builds a `gblur` + `overlay` chain
  (`crates/trimmer-core/src/delivery.rs:336-358`), asserted at `:746-759`. So `vertical` is
  "centre-cropped" and `vertical_blur` really is "the whole wide frame on a blurred bed"
  (`README.md:143-144`).
* `master_faststart`'s stated difference is delivered by cut configuration rather than by the
  preset row: `CutConfig::faststart` defaults to `true` (`crates/trimmer-media/src/executor.rs:68, 81`)
  and every mux that supports it emits `-movflags +faststart` (`executor.rs:655-656, 813-814, 894-895`),
  asserted at `:1156`. The README's claim is therefore true of the output, though a reader should
  know the behaviour is the default for all MP4 presets and not unique to this row.

**Verification**

* Exactly nine checks, and the nine names in the README table (`:187-195`) match `Check::ALL`
  (`crates/trimmer-verify/src/check.rs:87-97`) and `Check::statement`
  (`check.rs:101-132`) sentence for sentence.
* `HEAD_FIDELITY_MIN = 0.98` (`check.rs:40`) — as ADR-003:28 says.
* `ALIGNMENT_OFFSETS = [0, -1, 1]`, zero first (`check.rs:32`) — as ADR-003:20 says.
* `VerifyPolicy` default is `Strict` (`crates/trimmer-core/src/domain.rs:640-643`), the CLI default
  is `VerifyArg::Strict` (`cli.rs:245, 458`), `forensic` adds head SSIM and caption checking
  (`check.rs:779-783, 821-825`), and `Off` skips every check with `VERIFICATION_OFF`
  (`check.rs:455`, `:49`) — exactly what `README.md:197-201` claims.
* `Frames` fails on short, `Overshoot` warns, `Duration` compares against the delivered count
  (ADR-014:31-46) — reflected in `check.rs` and in the doc comment at `check.rs:9-19`.
* `trimmer-verify` starts no process, and `NoMeasurer` (`crates/trimmer-verify/src/measure.rs:74`)
  is the test double the README's "pure functions of measured facts" claim (`:182-183`) rests on.
* Caption clamp threshold `MIN_OVERLAP = 0.25` (`crates/trimmer-core/src/caption.rs:36`) with the
  drop branch at `< min_overlap` (`caption.rs:480`), so "at least 0.25 s survives" is accurate.
  Caption tolerance is a millisecond (`check.rs:55`) — as ADR-007:51 says.
* The captions check reuses `caption::retime` rather than reimplementing it (`check.rs:821-825`) —
  as ADR-007:47-49 says.

**Transcript, queue, watch, automation**

* `<video>.srt` / `<video>.<lang>.srt` discovery — `crates/trimmer-core/src/caption.rs:551`
  (`find_for`), and the "pointed at explicitly, or switched off" halves are `--srt` / `--no-srt`
  (`cli.rs:248-254`, `commands.rs:424-427`).
* Cues renumbered from 1 and shifted to zero; `retime` is pure and takes the window as an argument
  — `crates/trimmer-core/src/caption.rs`.
* Word-level index, search, hit → segment, sentence/pause snapping —
  `crates/trimmer-core/src/transcript.rs`, `crates/trimmer-app/src/transcript.rs`.
* Sequential batch queue with per-item outcomes and "Cancel means finish this one, stop" —
  `crates/trimmer-app/src/queue.rs:1-25`.
* `notify`-based watch rules with settle, fingerprint-based de-duplication and the four marker-list
  forms — `crates/trimmer-app/src/watch.rs:7-45`.
* Audit manifest appended in order and hashed over a canonical form so re-serialising does not
  change the digest — `crates/trimmer-verify/src/audit.rs:1-11`, with `constant_time_eq` at
  `routes.rs:187` and the `AuditEntry`/`AuditManifest` shapes at `audit.rs:26, 69`.
* ADR-015's three "absence" claims are exact and *tested*: `GET /v1/health` has no `licensed` field
  (`crates/trimmer-daemon/tests/api.rs:225`), `GET /v1/capabilities` has no `licence` object
  (`api.rs:748`), no `THE_TRIMMER_LICENCE` exists, no `licence` subcommand exists in `Command`
  (`cli.rs:71-155`), and no `trimmer-license` crate is in `Cargo.toml`. The only remaining hits for
  the word are the two stale comments in F13 and the OpenAPI `LicenseRef-Proprietary` identifier
  (`crates/trimmer-daemon/src/openapi.rs:47`), which is the *software* licence the README
  distinguishes correctly at `:397-410`.

**Timeline export**

* Four formats from one `ExportRequest` (`crates/trimmer-export/src/lib.rs:106-113`); FCPXML times
  built by integer arithmetic from numerator/denominator (`lib.rs:41-44`); EDL inclusive out points
  (`lib.rs:54`); RFC 4180 CSV (`lib.rs:55`); control characters stripped with the caller told
  (`lib.rs:38-40`, `crates/trimmer-export/src/xml.rs:27`); a rate-mismatched segment written on its
  own rate with a warning and an unprobed source skipped with a warning rather than failing the
  export (`lib.rs:31-37`, warned at `crates/trimmer-cli/src/project.rs:221-223`).
* The module map at `README.md:169-174` and `crates/trimmer-export/src/lib.rs:46-57` agree.

**Design records**

* `docs/DESIGN.md` correctly labels 001–007 as carried forward/superseded and 008–015 as new in V2,
  and `docs/DESIGN.md:7-9`'s promise ("superseded … never deleted") is honoured: ADR-006 keeps its
  original decision text and then states what V2 overturned (`006-no-preview.md:17-43`).
* ADR-005's `MAX_CONCAT_OFFSET_FRAMES` (12) and the "this is not drift, it is a symptom" wording
  match `crates/trimmer-core/src/plan.rs:64, 553-560`, and the bounds are tested at `plan.rs:835-836`.
* ADR-010's `Timescale` design (own type, `ticks_per_second`, `from_ffprobe_time_base` taking the
  denominator and refusing a numerator that is not 1) matches `crates/trimmer-core/src/domain.rs`.
* ADR-008's purity claim is checkable and true: `trimmer-core` has no I/O dependency, and
  `Cargo.toml:65-66` keeps `panic = "unwind"` in dev for the oracle, exactly as ADR-008:63-65 says.
* The README's `:322-327` description of eight named plan invariants and re-checking on read-back
  matches `PlanInvariant::ALL: [Self; 8]` (`plan.rs:95`) and `CutPlan::verify_invariants`.
* The README's `:329-334` description of the property tests matches `timecode.rs:1135-1168`
  (`0..1_500_000` at eight rates; the skipped-label property).

**Other**

* `README.md:172`'s FCPXML examples (`4/1s` for 100 frames at 25 fps, `1001/30000s` for one frame
  at 29.97) are consistent with the integer construction described in `trimmer-export/src/lib.rs:41-44`.
* `rust-toolchain.toml` and `Cargo.toml:19` (`rust-version = "1.85"`) agree with `README.md:373`.
  Note: I did not run `cargo test`, `cargo doc` or the binary, so claims about *compilation* are
  verified by reading manifests and source, not by execution.
* `docs/adr/015-licensing.md`'s claim that the removed crate passed "54 tests" and
  `docs/SKILLS-APPLIED.md:83-89`'s account of the same removal are consistent with each other, and
  I could not check the test count against the tree because `crates/trimmer-license` is not in it —
  **not verified**, and honestly presented as history by both documents.

## Summary

| Severity | Count | Findings |
|---|---|---|
| High | 7 | F1, F2, F3, F4, F5, F6, F7 |
| Medium | 5 | F8, F9, F10, F16, F17 |
| Low | 6 | F11, F12, F13, F14, F15, F18 |
| **Total** | **18** | |

Counting method, so the totals are checkable. **High** (7): each is a statement of implementation
status or behaviour that the code contradicts and that a reader would act on — a non-existent CLI
flag surface (F2), a non-existent API route table (F3), four crates and the desktop shell declared
absent while present (F1, F6), an end-to-end test suite, a CI workflow and a differential oracle
declared absent while present (F4), seven review reports claimed and not delivered (F5), and the
document failing its own stated honesty standard (F7). **Medium** (5): F8, F9 and F10 mis-state the
behaviour of code that exists (a caption sidecar's encoding, a doc-comment defect that has been
fixed, a calibration feature wired to no flag); F16 and F17 are missing essential documentation
(Quick Start / build instructions, and a Configuration table for six environment variables).
**Low** (6): minor inaccuracies, stale comments, a wrong crate attribution and one missing
reproduction note.

The single most important finding is **F7 read together with F1–F6**: the README and three ADRs
assert that four crates, the desktop shell, the project store, the command line, the headless
daemon, the end-to-end test suite, the CI workflow and the differential oracle do not exist, while
all of them are in the tree and in `Cargo.toml`. The document states its own standard — "Nothing
here is claimed to be shipped that is not in the tree", "linking a document that is not there is the
kind of small lie this README is trying not to tell" — and then fails it in the two sections a
reader is most likely to trust: the status banner at the top and the "What is not yet true" list
near the bottom. The remedy is not more prose but re-reading the tree: the last commit that touched
any documentation is two commits behind the code.

## What is genuinely good

The documentation in this repository is, on the whole, well above the standard of a project at this
stage, and the findings above are the cost of that ambition rather than evidence against it.

* **The ADRs are real decision records.** Each states context, decision and consequences, then a
  "Change in V2" section that says what the rewrite altered and why. ADR-014 is the best of them: it
  records a *contradiction* that appeared when two rules were implemented as one, explains why the
  split into `Frames`/`Overshoot`/`Duration` is the fix, and refuses to hide the mistake
  (`014-overshoot-verdict.md:22-29`). ADR-008's "What is verified today, and what is not"
  (`:59-69`) is the honest-status pattern the README should copy — ironically, it is now itself out
  of date, which is the whole of F4.
* **The module doc comments are genuinely explanatory, not decorative.** `trimmer-core/src/lib.rs`
  opens with *why purity is the product* and lists the three silent failures the type system now
  prevents; `trimmer-media/src/lib.rs` explains `CREATE_NO_WINDOW` and the no-shell rule;
  `trimmer-daemon/src/lib.rs` explains why the bind address is not configurable; `trimmer-app/src/queue.rs`
  and `trimmer-media/src/probe.rs` both explain why a design choice is *not* the obvious one. The
  `trimmer-cli/src/cli.rs` header states the rule that every flag carries `long_about` because
  "`--help` is the only documentation a person has while they are standing in front of the tool",
  and the file obeys it.
* **Where the docs claim a test proves something, the test usually exists under the named
  identifier.** ADR-001:36, ADR-002:29 and ADR-004:25 name tests; I found all three with those exact
  names. The README's "the executor's argument vectors are asserted directly, so dropping
  `-video_track_timescale`, adding `-frames:v` back to the copy path … fails a test rather than
  shipping" (`:332-334`) is true.
* **The record of reversal is exemplary.** ADR-015 and `docs/SKILLS-APPLIED.md:81-90` both record
  that a licensing crate was built, tested and then removed, and why — instead of deleting the
  history. `015-licensing.md:23-25` argues for asserting the *absence* of API fields rather than
  merely not mentioning them, and the tests do exactly that (`trimmer-daemon/tests/api.rs:225,748`).
  This is the single best piece of documentation reasoning in the repository.
* **`docs/SKILLS-APPLIED.md` takes the unusual and correct position that naming skills proves
  nothing** (`:4-5`) and that a rejection is worth recording (`:53`), and it lists which skills
  were rejected and why. Its defect (F5) is that the Part 2 table overshoots: it presents seven
  audits as done when they are not.
* **`docs/DESIGN.md`'s "superseded, never deleted" rule is actually followed**, so a reader can see
  what V1 decided, why V2 changed it, and what is still refused.

In short: the *reasoning* documentation is strong, honest and specific; the *status* documentation
is stale. The fix is a re-read of the tree against `README.md`, `docs/adr/009`, `docs/adr/011`,
`docs/adr/012` and `docs/SKILLS-APPLIED.md`, plus the three line-level code/doc mismatches in
F8–F10.
