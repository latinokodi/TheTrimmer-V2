# How each claim is checked

Every statement this product makes about itself is listed here with the mechanism that checks it.
The point is that a claim and its check are the same size: a sentence nobody can test is removed
rather than softened.

Run everything with:

```powershell
cargo test --workspace          # 402 tests over ten crates
cargo clippy --workspace --all-targets -- -D warnings
cd apps/web; npx tsc --noEmit; npm run build
```

---

## The cutting claims

| Claim | Checked by | What the check would catch |
|---|---|---|
| The cut is **frame-exact**: the in point is the first frame kept | `end_to_end.rs::a_head_patch_cut_is_lossless_exact_and_lands_on_the_mark` | The output's first frame is compared by SSIM against the source's frame at the in point *and* against a frame five earlier; the check fails unless the mark is the better match, so a head that lands off by even a few frames is caught |
| The body is **the original packets**, not a re-encode | The same test: 20 decoded frames from the keyframe onward are MD5-hashed in both files and must be identical | A re-encode cannot pass. This is the assertion that distinguishes "lossless" from "looks the same" |
| Only the head is re-encoded | `executor::tests::the_head_encode_pins_the_source_timescale_which_is_what_stops_the_slow_motion_bug`, and `CutPlan::reencode_fraction` in the report | A plan that re-encoded more than the head would show a fraction above the head's share |
| Nothing runs past the out point without saying so | `plan::tests`, `verify::tests`, and the `Overshoot` check | Overshoot is a *warning*, never a failure — a stream copy stops on a packet boundary — but it is always recorded |
| The timescale is the source's own | Two places: `prepare_head` asserts `-video_track_timescale 90000` in the end-to-end test, and `PlanInvariant::HeadMatchesSourceTimebase` is checked on every plan | A wrong timescale rescales the copied body into slow motion **while ffmpeg exits 0**. This is the defect that cost the most time in V1 |
| The frame count is bounded by time, never by `-frames:v` | `executor::tests::the_copy_path_is_bounded_by_time_and_never_by_a_frame_count`, which asserts the absence of `-frames:v`, plus the end-to-end assertion on the real command line | `-frames:v` counts packets in **decode** order, which with B-frames drops a frame that was asked for and keeps one that was not |

## The verification claims

| Claim | Checked by | What the check would catch |
|---|---|---|
| A finished cut is measured against its source | `end_to_end.rs` hashes real frames; `trimmer-verify`'s 74 tests cover every check's pass, fail and skip path | A cut that shifted would fail the frame comparison and be reported rather than delivered |
| A cut is only called **verified** when every check its policy asked for actually ran | `application.rs::a_cut_with_real_evidence_is_certified` and `…did_not_run_is_uncertified_not_verified` | This is the check that caught the worst bug found in this codebase: the queue called the verifier with no evidence, every strict check reported `Skipped`, `ok()` returned true because nothing had *failed*, and the batch called it verified. The headline claim was untrue while the report said everything was fine |
| A check that cannot apply to a plan is not counted as a gap | `application.rs::a_check_that_cannot_apply_to_this_plan_is_not_counted_as_a_gap`, and `check_applies` | Counting it would mark every whole-segment re-encode uncertified for a check that could never have run |
| Switching verification off is recorded, not hidden | `application.rs::switching_verification_off_is_recorded_rather_than_hidden` | All nine checks must still appear, each skipped with the reason `verification is off` |
| The run is auditable afterwards | `trimmer-verify`'s manifest tests: digest stability, per-field sensitivity, signature accept/reject on a tampered manifest and a wrong key | A log edited after the fact does not verify |

## The timecode claims

| Claim | Checked by | What the check would catch |
|---|---|---|
| Drop-frame is read and written correctly at every rate | 100 unit tests plus `proptest` over 1.5M frames at eight rates, asserting a round trip and that a skipped label is never rendered | The 108-frame-per-hour error that a naive reading of `01:00:00:00` produces on 29.97 |
| The implementation agrees with the V1 engine | `tests/oracle.rs` and `tools/oracle/run_oracle.py`: 626 cases compared field by field | Any divergence from the engine that was validated on real broadcast material. The V1 engine is imported from its own checkout, never vendored, because a vendored copy is a fork and a fork drifts |
| `29.97` means `30000/1001`, not `30` | `timecode::tests::rate_parsing_agrees_with_ffprobe_spellings`, and the oracle | A continued-fraction search cannot distinguish them under a sane bound; the table is what makes it exact |
| A container timescale is not a frame rate | `Timescale` is its own type, so the mistake is unrepresentable. `domain::tests` plus the end-to-end assertion | Modelling it as a `FrameRate` silently yielded `1` instead of `90000` — the slow-motion bug, reintroduced by a type choice |

## The transcript claims

| Claim | Checked by | What the check would catch |
|---|---|---|
| A caption that crosses a mark is clamped, and dropped when too little survives | `caption::tests` (14) and the oracle's caption cases | A caption that flashes for forty milliseconds, or a word that is still audible going uncaptioned |
| The written file keeps the source's shape | `caption::tests::rendering_renumbers_from_one_and_keeps_the_shape` | V1 used the file's line ending between cues but not *inside* one, so a CRLF transcript came back with LF inside every multi-line cue and `git diff` showed every cue as changed. Found and fixed here |
| Search never crosses a pause | `transcript::tests::a_phrase_is_not_matched_across_a_pause_between_cues` | A hit that spans a cut point, which is the one thing the feature exists to avoid |
| The index is built once and kept | `application.rs::the_transcript_cache_holds_one_index_per_file`, and the desktop and daemon both read the state's service | Building one per request re-read the SRT, re-parsed three thousand cues and re-folded all of them on every polled search — the worst hot path an audit found |

## The engineering claims

| Claim | Checked by | What the check would catch |
|---|---|---|
| No shell is ever used | All four spawn sites go through `trimmer-media`'s `ProcessRunner`, which takes an argv array. `process::tests` runs real processes through it | A file name becoming syntax. The security audit verified all four sites independently |
| No `unsafe` | `#![forbid(unsafe_code)]` at the root of every library crate | The compiler refuses it |
| A path is a string until the domain accepts it | Every command converts to `MediaPath` and checks existence; `MediaPath::canonicalised` before the store records one | Nothing joins a user string to a directory |
| The daemon cannot be reached off the machine | `DaemonConfig::validate` refuses a non-loopback bind, with a test per address; the token must be at least 16 characters | An API that can cut files and delete projects being exposed to a network |
| The webview holds no capability | `capabilities/default.json` grants a file picker and four dialog permissions, and nothing else. The audit that found `opener:allow-open-path` in an earlier revision is why it is gone | Anything the page can do, something will eventually make it do |
| The `doctor` report is truthful | `end_to_end.rs::the_capability_report_is_believed_by_the_ffmpeg_that_is_actually_installed` asserts against the ffmpeg on the machine | The report was wrong twice — it claimed a good build had no `mp4` muxer and no `loudnorm` — and a fixture that does not match reality is exactly what hid it |
| The engine works end to end on real media | `end_to_end.rs`, run in CI with ffmpeg installed | Every class of defect that exits 0 while producing the wrong file |

---

## What is not checked, and why

Stating these is the point of the document.

* **Variable-frame-rate sources.** The method assumes a constant frame grid. Such a source is
  detected (`MediaInfo::is_variable_rate`), warned about in the plan's notes and in the interface,
  and trimmed anyway — but the marks are approximate and no check can make them exact, because
  there is no frame grid to be exact against.
* **Frame hashing costs a decode pass.** The engine hashes exactly the sample window
  (`CutExecutor::frame_hashes`), and the queue asks for it only when the policy does — `Strict` and
  `Forensic` hash, `Standard` does not. A cut made under `Strict` therefore runs two extra ffmpeg
  passes over 24 frames each. That is the cost of the strongest check that does not decode the whole
  segment, and it is why the policy is a choice rather than a constant.
* **The follow-up audit findings** in `docs/audit/` are recorded, triaged and not all closed. The
  three that mattered most — the unverified-verification bug, the per-request transcript index, and
  the unused `opener` permission — are fixed and each has a regression test. The rest are listed
  there with a severity and are honest outstanding work.
* **The installer is unsigned.** The bundle job stops rather than producing one, because a
  certificate is a secret this repository should not hold.
