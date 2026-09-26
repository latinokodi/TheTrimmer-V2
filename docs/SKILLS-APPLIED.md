# Skills applied

The practices this build was held to, and what each one actually changed. A skill that left no mark
on the tree is not listed, because a list of names is not evidence of anything.

Thirteen were loaded for the migration from the Rust/Tauri tree to the Python/Electron tree.

---

## Python

| Skill | What it changed |
|---|---|
| **python-pro** | Type hints on every public signature; `from __future__ import annotations` throughout; dataclasses for the engine's records; `asyncio.to_thread` for the engine calls that block (probing, planning), so the HTTP loop is never held by ffprobe |
| **python-patterns** | The framework decision was made from the workload rather than by habit: a loopback HTTP API with a streaming response needs an async server and nothing else, so `aiohttp` and not FastAPI — there is no routing tree, no auth, no schema to validate. One job at a time is a lock and a slot, not a queue and a worker pool |
| **async-python-patterns** | Server-sent events as the progress channel; a bounded `queue.Queue` per subscriber with `put_nowait` everywhere, so a window that has stopped reading can never block a cut. Cancellation is a token the worker checks, not a task cancellation across a thread boundary |
| **python-testing-patterns** | 34 tests in about a second. `monkeypatch` replaces the one process launch in the plan tests, so the planning logic is tested without a fixture on disk and without ffmpeg. `aiohttp.test_utils.TestClient` for the contract, driven with `asyncio.run` rather than a plugin, because a plugin is a dependency the runtime does not need |

## Robustness and validation

| Skill | What it changed |
|---|---|
| **clean-code** | Every function does one thing and is named for it. `_level`, `_ticks`, `_describe`, `_plan_json`, `_check_json` each map one thing to one representation. The seven-field hand-copied `Ticks` → dict → JSON round trip was removed once it was noticed, because a second spelling of the same fields is a thing that drifts |
| **lint-and-validate** | `python -m py_compile` on the backend, `npx tsc --noEmit` on the interface, and a Vite build, after every change. Every defect in this migration was caught by one of those three or by reading computed styles out of the running window |
| **tdd-workflow** | The tests were written against the engine's declared behaviour and *two of them were wrong* — the first assertion about `parse_rate("29.97")` and the first about the output file name. Both were corrected to state what the code actually guarantees, and the reason is in the test. A test bent to pass is worse than no test |
| **powershell-windows** | `start.bat` uses `EnableDelayedExpansion` and `!BUNDLE_TIME!` correctly; the doubled `%%` in `for` loops is commented, because a single one is a parameter substitution even inside a `REM` line and that is how the file first failed |
| **documentation-templates** | The README is Quick Start → what it does → how it is built → tests → docs. `docs/TRUTH.md` pairs every claim with its check; `docs/DESIGN.md` records the decisions and the failures behind them. Bad comments were deleted rather than rewritten |

## Interface

| Skill | What it changed |
|---|---|
| **design-taste-frontend** | The window's single action is the highest-contrast object on the panel. Disabled is a state and has to be legible as one — a disabled control keeps its edge and its fill and loses only its contrast, so "not pressable" and "not a button" stop looking the same |
| **typography-and-spacing-scale** | A 4 px baseline grid for every structural padding and margin, and a fixed type scale from a 10 px floor to a 27 px product name. Timecodes are the one control above body size, because eleven characters of `HH:MM:SS:FF` are what the product turns on |
| **dark-mode-color-systems** | One dark palette with every text level measured against the panel it actually sits on (`--phosphor-faint` is 4.8:1 on `--substrate-200`) rather than against the page background, and `color-scheme: dark` so native controls — the `<select>` popups and the scrollbars — are dark too and do not flash white. The light value set this skill first produced was removed at the operator's decision: a bright surface beside a video monitor defeats the operator's adaptation to the picture, so a light theme is a way to judge the picture wrong rather than a preference. `forced-colors: active` remains, because that is the operating system taking the palette away and it must be honoured |

## Process

| Skill | What it changed |
|---|---|
| **plan-writing** (specification) | `docs/SPEC.md` states what the product is required to do as numbered requirements, each with the check that decides whether it holds, and a traceability table from requirement to scenario to test to code. Nothing in it describes how anything is done — that is `docs/DESIGN.md`. A requirement nothing can check is either a gap or a defect, and both are worth seeing |
| **tdd-workflow** | Where the defect was known before the fix, the test came first and was watched to fail: the grid-rate behaviour, the GOP-preroll arithmetic, the keyframe index measured from `start_time`, and the drift warning. Where a test disagreed with the engine, the *test* was wrong twice — `parse_rate("29.97")` and the output file name — and both were corrected to state what the code actually guarantees rather than bent to pass |
| **behaviour-driven development** | `specs/features/*.feature` are executable: `cutting`, `rates`, `verification` and `engine_api` run under `pytest-bdd` (31 scenarios, in the same second as the unit tests), and `window` runs under a small vitest runner because the interface has no Gherkin plugin here. A scenario with no step definition fails rather than being skipped, so the feature files and the steps cannot drift apart. Every defect found on real footage is now a named scenario — aiming a copy inside the GOP, counting frames on the file's own grid, measuring the alignment window from inside the frame |
| **git-commit** | Conventional commits, one logical change each, with the body explaining the failure the change prevents. The two defects reported from real masters are recorded with the before and after measurements |
