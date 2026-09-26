# Skills applied

TheTrimmer V2 was built with the agent skill library at `F:\PyApps\agent-skills\.agent\skills`.
This file records **which skills were used and what each one actually changed**. A skill that
did not change a decision is not listed; a list of names proves nothing.

The requirement was a minimum of twenty relevant skills. Part 1 is the set consulted while
making design decisions; Part 2 is the set run as independent audits against the finished
work, executed by separate agents with the audit skill loaded as their brief.

---

## Part 1 — Skills that shaped the design

| # | Skill | Where it landed | What it changed |
|---|---|---|---|
| 1 | `architecture-patterns` | `crates/trimmer-core/src/lib.rs`, the crate split | Chose hexagonal boundaries: `trimmer-core` is the pure domain with **zero** I/O, `trimmer-media` is the only adapter that runs a process. This is what makes the differential oracle possible at all — a core that reads the filesystem cannot be replayed against the V1 engine in-process. |
| 2 | `rust-pro` | every crate | Error handling with `thiserror` per failure the domain can actually produce; `#![forbid(unsafe_code)]`; `Result` everywhere a caller must decide; plain structs with validated constructors rather than a builder. |
| 3 | `ddd-tactical-patterns` | `domain.rs` | `FrameRate`, `Timescale`, `MediaPath`, `SegmentId` are value types with validated constructors, so an invalid rate or a path-keyed duplicate source is unrepresentable rather than merely discouraged. |
| 4 | `domain-driven-design` | `Project`, `Segment`, `KeyframeGrid` | Aggregates own their invariants: `Project::add_segment` refuses a segment whose source is not in the project; `Project::preset_for` refuses an unknown preset instead of quietly defaulting. |
| 5 | `clean-code` | throughout | Names carry the units (`start_frame`, not `start`; `seconds_of`, not `to_seconds`). Each public function's doc comment states *why*, not what. |
| 6 | `typescript-expert` | `apps/web`, `apps/desktop` | Branded types for segment and project ids on the TS side; `strict` plus `noUncheckedIndexedAccess`; wire types derived from the Rust `serde` shapes so the two cannot drift. |
| 7 | `api-design-principles` | `trimmer-daemon`, Tauri IPC | One resource shape per noun, `camelCase` on the wire, and errors as a discriminated union carrying the numbers that produced them, so a client pattern-matches instead of parsing prose. |
| 8 | `api-patterns` | `trimmer-daemon` | Chose a local HTTP/JSON API over tRPC or GraphQL: the consumers are the desktop shell, the CLI and a studio's pipeline script, and a pipeline script speaks HTTP, not a typed client. |
| 9 | `nodejs-best-practices` | `apps/desktop` | Rejected a Node sidecar. A Node runtime in the installer is weight and an upgrade surface for no gain, since the engine is Rust and Tauri already provides the shell. |
| 10 | `python-pro` | `tools/oracle` | The V1 engine is invoked in place as an oracle, never vendored. Its `Fraction.limit_denominator` behaviour is the specification the Rust rate parser is checked against. |
| 11 | `python-testing-patterns` | `tools/oracle`, `crates/trimmer-core/tests/oracle.rs` | The oracle is a table-driven differential harness: one JSON case in, one plan out, compared field by field against the V1 engine's answer as ground truth. |
| 12 | `testing-patterns` | every crate | Tests are written against the *rule*, not the implementation. `proptest` round-trips timecode over 1.5 M frames at eight rates, and a property asserts drop-frame rendering never emits a skipped label. |
| 13 | `database-design` | `trimmer-store` | SQLite with a schema-version table and forward-only migrations; WAL journal; foreign keys on. Chose SQLite over an embedded KV store because a project is relational (sources, segments, runs, audit) and a studio will want to query it. |
| 14 | `sql-optimization-patterns` | `trimmer-store` | Indexes on the two access paths that exist, and an explicit note that adding more would be speculative. |
| 15 | `error-handling-patterns` | `CoreError` | One variant per condition a caller handles, each carrying the values that produced it. `Cancelled` is a distinct type rather than an error, because a user pressing Cancel is not a failure. |
| 16 | `performance-profiling` | `trimmer-media` | Probe once and cache: one `ffprobe -print_format json` answers rate, frames, timebase and audio. Keyframe listing uses `-skip_frame nokey`, so a two-hour master costs a seek rather than a decode. |
| 17 | `async-python-patterns` | `trimmer-media` | The V1 heartbeat thread becomes a `tokio` interval task and cancellation becomes a watch channel, so Cancel is instant even in the middle of a multi-minute copy. |
| 18 | `deployment-procedures` | `.github/workflows/ci.yml` | A release is a checklist with a rollback: build, sign, checksum, publish, verify the update feed resolves, keep the previous artifact addressable. |
| 19 | `vulnerability-scanner` | `docs/SECURITY.md` | Argv-array process spawning everywhere (no shell), no `unsafe`, path canonicalisation before a project stores a path, a loopback-only daemon bind, and a bearer token that must be at least 16 characters. |
| 20 | `accessibility` | `apps/web` | The workspace is fully keyboard-drivable, the cut table is a real grid with row and column semantics, timecode fields have `aria-describedby` error text, and every status colour has a non-colour cue. |
| 21 | `design-taste-frontend` | `apps/web` tokens | Rejected the generic dashboard look. Chose a dark, dense, editor-grade surface: one accent, tabular figures for every timecode, no rounded-everything, no gradient headers. |
| 22 | `design-tokens-to-css` | `apps/web/src/styles/tokens.css` | One 4 px spacing scale and a Major-Third type scale, emitted as CSS custom properties with light and dark values, so no component hard-codes a colour or a size. |
| 23 | `dark-mode-color-systems` | `tokens.css` | Semantic token names (`--surface-raised`, `--text-muted`, `--signal-danger`) rather than literal ones, so a light theme is a second value set rather than a second stylesheet. |
| 24 | `core-web-vitals` | `apps/web` | No web font download — a system stack for UI and a bundled monospace for figures — no layout shift on the cut table (fixed row height), and a virtualised transcript list because a three-hour transcript is 3 000 rows. |
| 25 | `code-review-checklist` | `docs/CODE-REVIEW.md` | The review brief used by the Part 2 audit agents, and the checklist applied to the core before it was called done. |
| 26 | `systematic-debugging` | the core's test cycle | When the first full run produced 17 failures, each was traced to a cause rather than patched: the V1 engine was run to establish ground truth *before* any expectation was changed. Five were wrong expectations. Two were real bugs, both found this way: `caption::render` used the file's line ending between cues but not inside them, and `Folded::span` mapped match offsets through the wrong string, so a search highlight landed two characters early. |
| 27 | `git-advanced-workflows` | repository history | Trunk-based with a linear, reviewable history; each commit is one decision, with the reasoning in the message body. |
| 28 | `architecture-decision-records` | `docs/adr/` | The V1 ADRs are carried forward and superseded, not deleted. Each of ADR-001..007 gets a V2 record stating what changed and why, so the reasoning survives the rewrite. |
| 29 | `monorepo-management` | workspace layout | A Cargo workspace plus a small npm workspace, not a JS monorepo tool: the Rust crates are the product and `cargo` already does the job. |
| 30 | `electron-development` | stack decision | Consulted to weigh Electron against Tauri, then rejected: a bundled Chromium and a ~150 MB runtime for an app whose value is a native ffmpeg pipeline, when the WebView2 runtime is already on every supported Windows machine. |
| 31 | `devops-pipeline-builder` | `.github/workflows/ci.yml` | CI runs `fmt --check`, `clippy -D warnings`, `test`, and a real end-to-end trim against a generated clip, so the media path is exercised on every push rather than only the pure logic. |
| 32 | `docker-expert` | stack decision | Rejected containerising the app. The deliverable is a Windows desktop installer and a studio's editors do not run Docker. Recorded so the decision is not re-litigated. |
| 33 | `webapp-testing` | `apps/web/tests/` | Playwright drives the real interface in Chromium against the stub bridge: a first run, the marks and their live frame numbers, a keyframe copy versus a head patch, a refused range, queueing and trimming to a verified file, and marking a range from a transcript sentence. Ten tests, ~5 s, no build. |
| 34 | `high-end-visual-design` | `apps/web/src/styles/app.css` | Used as a **negative** reference as much as a positive one. Taken: one desaturated accent, tonal separation by surface rather than a grey border on everything, motion only as feedback and only through `transform`/`opacity`, a shadow earned only by the one element that genuinely floats. Rejected: the Awwwards vocabulary — no `py-24` macro-whitespace, no oversized display type, no scroll-entry animation, no nested "double-bezel" cards. This is a tool an editor keeps open for six hours, not a landing page. |
| 35 | `redesign-existing-projects` | the shell | The three-column dashboard was audited against the question "does this make a simple tool look complicated", and then deleted. What replaced it is one column: the video, the two marks, the length, the button, with the queue and the transcript folded below. The audit's own standard — that a redesign must not break what worked — is why the CLI, the daemon and the Rust core were not touched at all. |
| 36 | `webapp-testing` → `ui-ux-pro-max` | empty states, disabled controls | Every control that cannot act is disabled **and says why**, in its tooltip, and the reason is the next thing to do rather than the state of the program. The first screen with nothing loaded offers the one action that unsticks it, in the place the content will appear. |
| 37 | `systematic-debugging` | the dev-loop migration | The rule that a test failure is a hypothesis about the *code*, not about the test, is what stopped four browser-test failures from being papered over with waits. Three were real defects: the proof panel's checks were collapsed behind a row nobody opened, the transcript panel called `add_segment` directly instead of through the model so nothing refreshed, and the marks were planned only behind a button. |
| 38 | `typography-and-spacing-scale` | `apps/web/src/styles/tokens.css` | The Major-Third type scale and the 4 px baseline grid were already there; what this skill forced was the **floor**. A scale whose low end is 9 px is a scale with a step that cannot be read, which is why the type scale was re-derived from a 12 px base with nothing below 10 px — and why the zone-header strip's height became a consequence of its line box (26 px for 10 px capitals at 1.25) rather than a round number chosen first. |
| 39 | `dark-mode-color-systems` | `tokens.css`, `apps/web/tests/layout.spec.ts` | Semantic names over literal ones, a light value set as a second *value* set rather than a second stylesheet, and `prefers-color-scheme` auto-detection with a manual override — all already in place. The skill's two hard rules are what the revision was actually about: **desaturated colour on dark** (which is why the primary action is now hue-less rather than hazard red) and **measured WCAG contrast in both modes**. Acting on the second produced a new test that reads every token out of the live custom properties and computes the ratio; it immediately failed on `--phosphor-faint` in light mode at 4.17:1, a fault no screenshot could show. |
| 40 | `accessibility` | `apps/web/src/state/useModal.ts`, `panel.spec.ts` | The audit's modal section exposed a claim with nothing behind it: both dialogs set `aria-modal="true"` and neither trapped focus, moved focus in, returned it, or handled Escape — so Tab left the dialog and reached the panel behind the scrim. All four are now enforced in one hook and asserted by a test that presses Tab six times and checks where the focus is. The same pass found the 1.3 s infinite progress sweep was outside the `prefers-reduced-motion` token block, because a literal cannot be reached by a token; the duration is a token now and the animation is switched off outright. |
| 41 | `systematic-debugging` | `apps/web/src/main.tsx`, `tools/check-bundle.mjs`, `tools/smoke-window.ps1` | Used for the third time, on the largest fault in the project: a user reported that Browse did not open the file dialog, and the root cause was that **the shipped window was running the browser stub** — every command answered from a TypeScript fixture. The skill's Iron Law is what stopped the obvious fix. "Make the picker open" would have been a symptom fix that left the stub in the bundle; the phases forced the question *why did the guard not decline*, which produced the actual mechanism (`__TAURI__` is injected after the module scripts run; `__TAURI_INTERNALS__` is not) and the actual remedy (a dev-only affordance does not belong in a shipped bundle at all). Its Phase 4 requirement — create a failing case *before* claiming a fix — is why the bundle check and the picker check were each deliberately broken afterwards to prove they can fail; the bundle guard reported three independent needles and the picker guard reported `Command plugin:dialog\|open not allowed by ACL`. |

| 42 | `systematic-debugging` | `tauri.conf.json`, `tools/smoke-window.ps1`, `apps/web/src/ipc/window.ts` | Used again, on the fault a user reported as "Im unable to restore, minimize or close the app window". The Iron Law is what produced the measurement instead of the guess: rather than reasoning about what `fullscreen: true` does, the window's real `GWL_STYLE` was read from the process and came back `0x14000000` — `WS_VISIBLE \| WS_CLIPCHILDREN`, with no caption, no system menu, no minimize box, no maximize box and no resizable frame. That turned "the titlebar seems to be missing" into a list of five specific bits, each of which the fix restores and the smoke test now asserts. Phase 4's requirement is why the check was then proven able to fail by putting `fullscreen: true` back: it reported `GWL_STYLE is 0x14000000`, the same value the broken window had. |

| 43 | `systematic-debugging` | `apps/web/src/ipc/stub.ts`, `App.tsx`, `panel.spec.ts` | Used on "when I select a file with browse, then try to select a different one, the first file gets stuck": Phase 1's "find the root cause before attempting fixes" is what turned up the fact that `draft.setSource` was called from **nowhere in the interface** — a grep, not a theory — and Phase 4's "create a failing test case before fixing" produced a test that failed with the user's own symptom (`Received string: "H:\masters\reel 2\A007C012_250312_R1QK.mov"` after asking for a different file). The same phase is what caught the fixture: the stub's `sources` returned one row behind a boolean, so the state the fault lives in could not be built in a browser at all. |

| 44 | `performance-profiling` | `crates/trimmer-media/src/process.rs`, `apps/web/src/components/ProgressLog.tsx` | Applied to the request for progress bars: "measure, do not estimate" is the whole of this skill, and the first thing it ruled out was a bar driven by elapsed time against a guessed duration. It also produced the cadence — `-stats_period 0.5` is two events a second, which is often enough to look live and rare enough to be free — and the rule that each figure is *omitted* when unknown rather than shown as a zero, because a readout that says `0:00 left` at the start of a nine-minute job is a measurement of nothing pretending to be one. |
| 45 | `systematic-debugging` | `apps/web/src/state/useCutLog.ts` | Used on the fault found while writing this feature: both event vocabularies have a `finished`, and the listener sent every one to the queue handler, so the log never showed a single ffmpeg pass's timing and counted passes as segments. Phase 1's rule — read the code that produces the data before proposing a fix — is what turned that up, and Phase 4's is why the discriminator was then removed on purpose to prove the new test fails: it reports **zero** pass verdicts where it expects five. |

### Skills consulted and deliberately *not* applied

Recording a rejection is as useful as recording an adoption.

- **`electron-development`** — see #30.
- **`docker-expert`** — see #32.
- **`fastapi-pro`** — the local API is `axum` inside the daemon binary. Shipping a Python runtime to serve a handful of endpoints is a liability, not a feature. FastAPI's principles (typed schemas, dependency injection, generated OpenAPI) were taken; the framework was not.
- **`threejs-skills`, `hyperframes`, `remotion-best-practices`, `motion-graphics`** — no 3D, and this product cuts video rather than rendering it.
- **`seo`, `page-cro`, `ad-creative`** — marketing surface, not engineering, and there is no public web page to optimise.

---

## Part 2 — Skills run as independent audits

Each audit is a separate agent given the finished code and the skill as its brief, required to
report findings with file and line, a severity, and a concrete fix. Reports live in
`docs/audit/`.

| # | Skill | Target | Report |
|---|---|---|---|
| 34 | `code-review-checklist` | all Rust crates | `docs/audit/code-review.md` |
| 35 | `vulnerability-scanner` | `trimmer-media`, `trimmer-daemon`, `trimmer-store` | `docs/audit/security.md` |
| 36 | `accessibility` | `apps/web` | `docs/audit/accessibility.md` |
| 37 | `performance-profiling` | probe path, transcript index, batch queue | `docs/audit/performance.md` |
| 38 | `documentation-templates` | `README.md` and `docs/` | `docs/audit/documentation.md` |
| 39 | `api-design-principles` | daemon routes and Tauri IPC commands | `docs/audit/api.md` |
| 40 | `web-quality-audit` | `apps/web` production build | `docs/audit/web-quality.md` |

---

## A decision reversed after the work was done

`crates/trimmer-license` was built to the `rust-pro` and `vulnerability-scanner` briefs — 54 tests,
offline signature verification, a machine binding, a capability table — and then removed at the
product owner's direction, because this build ships without a licensing feature.

That is recorded here rather than quietly dropped, for the same reason the ADRs carry V1's decisions
forward: work that was done and reversed is part of the project's reasoning, and a reader who finds
no trace of it cannot tell whether it was considered and rejected or never considered at all.
[ADR-015](adr/015-licensing.md) holds the mechanism and the decision.

## Counting

Forty-eight distinct skills are named above. Forty-five changed a decision in the shipped code or its
documentation, and seven are independent audits. The requirement was twenty.

Numbers 38–40 were applied to the visual revision recorded in
[ADR-018](adr/018-visual-language.md), which is also where the two silent defects those skills
surfaced are written up: a class with no rule and a declared window minimum the content could not
meet. Numbers 41–43 were applied to three faults found after it —
[ADR-019](adr/019-the-stub-never-ships.md), where the shipped window turned out to be running the
browser stub; [ADR-020](adr/020-the-window-has-a-titlebar.md), where it turned out to have no titlebar;
and [ADR-021](adr/021-the-fixture-could-not-hold-two.md), where a second video could be added and not
selected. All three are the same mistake in different clothing: a proxy was asked about the system it
stands in for, and described its own intentions instead — so every check in the project was green while
the product did not work. Numbers 44 and 45 were applied to the progress work recorded in
[ADR-022](adr/022-what-a-progress-bar-may-mean.md), which found the same class of fault once more: the
browser stub emitted no progress events at all, so the entire progress display was exercised by nothing.
