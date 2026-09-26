# TheTrimmer V2 — Security Audit

> **This is a record of a review, not a list of open work.** It was written against the tree as it
> stood when it was run, and the findings that mattered have since been fixed — the fix for each is a
> commit, and the current state of the product is [docs/TRUTH.md](../TRUTH.md) and the test suite.
> Line numbers below refer to the revision that was read and will have moved.
>
## Scope

This audit covers the Rust workspace at `H:\THEROLLUPFILES\TheTrimmer-V2` against the threat model
of a Windows desktop application that **spawns `ffmpeg`/`ffprobe`, reads media and subtitle files
chosen by the user, stores projects in SQLite, and runs a loopback-only HTTP API with a bearer
token**. Generated trees (`target/`, `node_modules/`, `apps/desktop/src-tauri/gen/`) were not
treated as source. The files read in full were: `Cargo.toml`, `rust-toolchain.toml`, `.github/workflows/ci.yml`,
`crates/trimmer-media/src/{process.rs,tool.rs,executor.rs,probe.rs}`,
`crates/trimmer-store/src/{store.rs,schema.rs}`,
`crates/trimmer-daemon/src/{config.rs,main.rs,lib.rs,state.rs,routes.rs}`,
`crates/trimmer-core/src/{caption.rs,domain.rs,transcript.rs,audit}`-adjacent code in
`crates/trimmer-core/src/plan.rs` (planner region), `crates/trimmer-verify/src/audit.rs`,
`crates/trimmer-app/src/{workspace.rs,watch.rs,transcript.rs}` (relevant regions),
`crates/trimmer-export/src/xml.rs`, `apps/desktop/src-tauri/src/{commands.rs,state.rs}`,
`apps/desktop/src-tauri/{tauri.conf.json,capabilities/default.json}`,
`apps/desktop/src-tauri/gen/schemas/acl-manifests.json` (the `opener` plugin entry only), and
`apps/web/src/components/TranscriptPanel.tsx`. Everything else in the repository was searched with
`grep` for the specific patterns named in the brief (process spawning, `unsafe`, SQL construction,
`unwrap`/`expect`/`panic!`, filesystem mutation, `dangerouslySetInnerHTML`, secret-shaped strings),
and where a search hit landed in non-test code the surrounding function was read. Files under
`crates/*/tests/` and `apps/desktop/src-tauri/icons/`, `tools/oracle/run_oracle.py`, and the
`assets`-free `apps/web` shell were consulted only as supporting evidence and are not audited as
attack surface. Two claims in the brief could not be verified from source alone and are called out
as such rather than asserted: the behaviour of the third-party `tauri-plugin-opener` crate's
implementation (only its generated ACL manifest and the upstream permission semantics were read),
and the runtime behaviour of the installed `ffmpeg` build when handed adversarial input (no fuzzing
or live media experiments were run).

## Summary of findings

| # | Severity | Location | Issue |
|---|---|---|---|
| 1 | Medium | `apps/desktop/src-tauri/capabilities/default.json:13` | Unscoped `opener:allow-open-path` grants the webview OS-level "open this path" over any path |
| 2 | Low | `crates/trimmer-media/src/process.rs:373-376`, `:295-310` | Child stdout/stderr buffered into unbounded `Vec<u8>` |
| 3 | Low | `crates/trimmer-media/src/tool.rs:145-169` | `which()` resolves `.BAT`/`.CMD` from `PATHEXT`, which Windows executes through `cmd.exe` |
| 4 | Low | `crates/trimmer-core/src/caption.rs:204-221`, `:384-401` | `parse_stamp` re-scans the remainder of the buffer from every digit position — quadratic on crafted input |
| 5 | Low | `rust-toolchain.toml:2`, `.github/workflows/ci.yml:42` | No dependency vulnerability scanning; toolchain and CI actions not pinned to an immutable revision |

**Medium: 1 · Low: 4 · High: 0 · Critical: 0**

---

## Findings

### 1. Unscoped `opener:allow-open-path` in the desktop capability set — Medium

**File:** `apps/desktop/src-tauri/capabilities/default.json`, line 13 (with the plugin's own
manifest at `apps/desktop/src-tauri/gen/schemas/acl-manifests.json`).

**What is wrong.** The single capability granted to the `main` window includes:

```json
"permissions": [
  "core:default",
  "dialog:allow-open", "dialog:allow-save", "dialog:allow-message", "dialog:allow-confirm",
  "opener:allow-reveal-item-in-dir",
  "opener:allow-open-path"        // line 13
]
```

The `opener` plugin's own ACL manifest (read from the generated `acl-manifests.json`) documents
`allow-open-path` as *"Enables the `open_path` command without any pre-configured scope"* and
declares a `global_scope_schema` with `path` and `app` properties — i.e. the plugin **supports**
constraining which paths and which applications may be opened, and this configuration supplies
neither. The effect is that any JavaScript running in the webview can call the IPC
`plugin:opener|open_path` with an arbitrary filesystem path and an arbitrary `openWith` program,
and the OS will launch it — which is arbitrary local code execution on the user's machine.

Two things make this a real, if conditional, exposure rather than a theoretical one:

* The capability file's own description (line 4) claims the page "can open a file picker and reveal
  a file in Explorer, and nothing else", and says "the webview never holds a capability it could be
  tricked into using". Both statements are false of this line: `allow-open-path` is precisely a
  capability that can be tricked into use, and it is broader than `reveal-item-in-dir`, which is the
  only related operation the application actually needs.
* `apps/web/src` never calls `openPath` — a grep for `opener|openPath|open_path|revealItem` over
  `apps/web/src` returns no matches, and the Rust side wires its own `reveal` command
  (`apps/desktop/src-tauri/src/commands.rs:1038-1064`). The permission is therefore unused surface
  today. The `openWith` argument makes the difference material: `reveal-item-in-dir` shows a file in
  Explorer, whereas `open_path` with an attacker-named program runs that program.

The delivery vector into the webview would be a script-injection or supply-chain compromise of the
frontend bundle — not reachable through the file formats this audit examined, which is why this is
Medium and not High. The consequence if it were reached is code execution, which is why it is not
informational.

**Fix.** Remove `"opener:allow-open-path"` from the permission list. The application's own Rust
`reveal` command already covers the "show me where that went" use case, and it authenticates the
path itself (`target.exists()` at line 1041). If the frontend genuinely needs `open_path` later,
grant it with an explicit scope instead of unscoped:

```json
{ "identifier": "opener:allow-open-path", "allow": [{ "path": "$APPDATA/TheTrimmer/**" }] }
```

and constrain the application via the same scope entry's `app` property, so `openWith` cannot name
an arbitrary executable. Also add `"opener:deny-open-url"` unless `open_url` is wanted, since
`allow-default-urls` is part of the upstream default set and is not currently granted here — that
much the configuration already gets right.

### 2. Child stdout and stderr are buffered without a bound — Low

**File:** `crates/trimmer-media/src/process.rs`, buffers declared at lines 373-376 and appended at
lines 295-310 (`pump`).

**What is wrong.** `ProcessRunner::run` accumulates the entire stdout and stderr of every child into
`Vec<u8>` with no cap:

```rust
let mut stdout = Vec::new();            // line 373
let mut stderr = Vec::new();            // line 374
...
into.extend_from_slice(&chunk[..read]); // line 304, in pump()
```

`pump` loops on a 1 ms timeout until EOF, so it will keep appending for as long as the child keeps
writing. Nothing in the crate limits the volume. The executor's cut passes `-v error`
(`executor.rs:584`), which suppresses most chatter, but ffmpeg's diagnostic volume is a function of
the input: certain malformed or damaged containers cause ffmpeg to emit one error per packet or per
frame, and the relevant `PollPolicy` for a cut is `PollPolicy::long()`
(`process.rs:73-79`), which deliberately sets `timeout: None`. A crafted media file can therefore
trade disk for process memory in the trimming host. The reachable paths are the daemon's
`POST /v1/projects/{id}/run` (`routes.rs:541`, after a token check) and any local cut.

This is bounded in practice by the fact that even an error-per-frame file produces a few hundred
bytes per frame, so exhausting several gigabytes of RAM requires an implausibly large and
specifically crafted file. It is listed because the brief asked specifically about unbounded
allocation from untrusted input, and because the mitigation is cheap.

A second, smaller note on the same code: `Output::stderr` is converted with
`String::from_utf8_lossy(...).into_owned()` (lines 460-461), so the full buffer is copied twice at
peak. Fixing the cap also fixes that.

**Fix.** Cap the retained bytes in `pump`, keeping the head (for `ffprobe` JSON, which fails if
truncated and is therefore read on the `quick()` policy anyway) and a bounded tail for ffmpeg
diagnostics. For example, add a `max_buffered: usize` to `RunOptions` (default 4 MiB) and inside
`pump` stop appending once `into.len() >= max_buffered`, setting a `truncated` flag that
`Output::stderr_tail` can report. Do not simply drop the read — the pipe must still be drained or
the child deadlocks (the module's own comment at lines 326-328 says exactly this) — so keep reading
and discarding past the cap.

### 3. Tool resolution accepts `.BAT` and `.CMD` from `PATHEXT` — Low

**File:** `crates/trimmer-media/src/tool.rs`, lines 145-169 (the `which` function), specifically the
extension list at 147-152 and the extension probe at 161-166.

**What is wrong.** The module documentation of `process.rs` (lines 6-9) makes a strong, load-bearing
claim: *"**No shell.** `std::process::Command` is given an argument array. Nothing is ever
concatenated into a command line, so no file name can become syntax."* The claim is true of every
call site — see "Checked and clean" below — but it is not true of the *program* that gets resolved
here. `which` builds its extension list from `PATHEXT`, defaulting to
`".COM;.EXE;.BAT;.CMD"`, and returns the first `is_file()` hit:

```rust
for extension in &extensions {
    let with_extension = folder.join(format!("{program}{extension}"));
    if with_extension.is_file() {
        return Some(with_extension);   // line 164
    }
}
```

On Windows a `Command::new()` whose program is a `.bat` or `.cmd` file is not executed directly by
the kernel; it is handed to `cmd.exe` for interpretation. `cmd.exe` does not use `CreateProcess`
argument quoting — it re-parses the command line with its own rules and performs `%VAR%` expansion
inside double quotes. So an argument that contains a percent-delimited token can be rewritten before
the batch file sees it, which is exactly the class of "a file name becomes syntax" transformation the
documentation says cannot happen. Exploiting it requires an attacker to place `ffmpeg.bat` (or
`ffprobe.cmd`) in a directory that precedes the legitimate build on the user's `PATH`, or to set
`THE_TRIMMER_FFMPEG` — both of which are already-privileged positions, which is why this is Low and
not higher. It is reported because the brief explicitly asked whether the "argv arrays, never a
shell" claim holds, and the honest answer is "holds for the spawn, with this one documented
exception".

**Fix.** Restrict the search to directly executable images. Either drop `.BAT`/`.CMD` from the
extension list entirely (`[".COM", ".EXE"]`), or keep them but refuse to use them for the resolved
tool path and report a clear error naming the file, so a user who has a wrapper script learns to
point `THE_TRIMMER_FFMPEG` at the real `ffmpeg.exe`. Either way, amend the `process.rs` module
documentation to state the actual guarantee — "no argument is ever passed to a shell; the program
itself is a resolved executable image" — so the next reader is not misled.

### 4. `parse_stamp` is quadratic in the size of a crafted line — Low

**File:** `crates/trimmer-core/src/caption.rs`, lines 204-221 (`parse_stamp`), reachable from
`read` at lines 384-401 and therefore from `caption::parse` (line 302).

**What is wrong.** `parse_stamp` scans the input for the first byte that is an ASCII digit and, at
each such byte, attempts a full parse by calling `stamp_at`:

```rust
while index < bytes.len() {
    if !bytes[index].is_ascii_digit() { index += 1; continue; }
    if let Some((seconds, _consumed)) = stamp_at(bytes, index) { return Ok(seconds); }
    index += 1;                                    // line 215: advance by ONE
}
```

`stamp_at` is written to scan its fields with a bounded lookahead, so a *successful* parse is
linear. The problem is the failure path: a crafted line of the form
`1:0:0.1:0:0.1:0:0.` … repeated can be arranged so that each digit position begins a parse that
gets several fields deep and then fails, and because the loop advances one byte at a time the same
suffix is re-examined from every subsequent digit. That is O(n²) in the length of the line. The
input is a user-supplied `.srt` file, read whole into memory by `read()` (line 385,
`std::fs::read`) with no size limit, and the reachable trigger is
`POST /v1/transcripts/search` (`crates/trimmer-daemon/src/routes.rs:626`), which calls
`TranscriptService::load` → `caption::read` → `parse` → `parse_stamp`. A multi-megabyte crafted SRT
therefore pins a worker thread for a long time; the daemon has no request timeout and no rate limit.
Note that `retime_file` (line 523), which would have been a second route to this parser, has no
callers outside its own tests, so it is not currently reachable.

This is Low rather than Medium because it requires the operator to point the tool at a hostile
subtitle file (or a hostile file to be placed beside a video in a watched folder), and the outcome
is CPU exhaustion rather than disclosure or execution. A hang is a denial of service all the same,
and the fix is three lines.

**Fix.** Advance past the bytes a failed attempt actually consumed instead of by one: have
`stamp_at` return the furthest offset it reached (or a `Result<(f64, usize), usize>`), and set
`index` to that offset on failure, with `index + 1` as the fallback only when the offset did not
advance. Independently, bound the input: refuse a caption file larger than a few megabytes in
`read()` with a named error, since a legitimate SRT is a few hundred kilobytes at the very most, and
the same bound protects the `Vec<Cue>` allocation.

### 5. Supply-chain: no dependency scanning and no immutable pins — Low

**File:** `rust-toolchain.toml` line 2 (`channel = "stable"`),
`.github/workflows/ci.yml` lines 39-45, 62, 105, 139 (`actions/checkout@v4`,
`actions/cache@v4`, `actions/setup-node@v4`, `dtolnay/rust-toolchain@stable`).

**What is wrong.** There is no `cargo-deny`, `cargo-audit`, or RustSec advisory check anywhere in
the pipeline or the repository — a grep for `deny|audit|advisory|vet|rustsec` over every `*.toml`
finds only two hits, both the word "audit" inside a crate description. `Cargo.lock` is committed,
which is the right call and is what makes builds reproducible, but nothing verifies that the pinned
set is free of known advisories. Separately, the toolchain is pinned to the floating `stable`
channel rather than a dated release, and every GitHub Action is referenced by a mutable major-version
tag rather than a commit SHA. A compromised or force-moved tag, or an upstream `stable` regression,
enters the build unreviewed.

This is a conventional finding rather than an exotic one, and it is included because the brief
explicitly asked for a clean/not-clean statement on every axis and supply chain is an axis the
reviewer is expected to cover.

**Fix.** Add a `cargo deny check advisories bans licenses sources` job (with a `deny.toml`) and a
`cargo audit` job to `ci.yml`, failing the build on a new advisory. Pin the toolchain to an explicit
version in `rust-toolchain.toml` (`channel = "1.85.0"`, matching `rust-version.workspace = true` in
the root manifest) and pin each action to a full commit SHA with the version in a trailing comment.
The `bundle` job already refuses to produce an unsigned installer when
`TAURI_SIGNING_PRIVATE_KEY` is absent, which is the correct behaviour for A08 and should be kept.

---

## Checked and clean

Each item below was verified by reading the named code, not by assuming the documentation. Where a
claim in the code's own comments was tested against the code, that is stated.

**Command and argument injection — clean at every spawn site.** A repository-wide grep for
`Command::new`, `.arg(`, `.args(`, `.raw_arg`, `sh -c`, `cmd /c` returns exactly four non-test spawn
points, and each passes an argument array to `tokio::process::Command` or `std::process::Command`
with no shell:

* `crates/trimmer-media/src/process.rs:355-364` — the single choke point through which every
  `ffmpeg` and `ffprobe` invocation goes (`ProcessRunner::run`). `Command::new(program)` where
  `program: &Path`, then `.args(args)` where `args: &[OsString]`. `.raw_arg` is never used, so no
  argument is ever appended to a raw command line. `display_command` (line 498) builds a string for
  the *log only* and is never fed back into execution — verified by reading its only call site at
  line 342, whose result is used solely in `Progress::Command` and in error text.
* `crates/trimmer-media/src/probe.rs:239-253` and `:284-293` — `ffprobe` argument vectors built by
  `argv(&[...])` with the media path pushed as a single `OsString`; `-read_intervals` is formatted
  from two `f64`s that were themselves derived from frame counts, not from strings.
* `apps/desktop/src-tauri/src/commands.rs:1052-1053` — `reveal`, `explorer` with one
  `format!("/select,{}", target.display())` argument.
* `crates/trimmer-media/tests/end_to_end.rs` and `crates/trimmer-core/tests/oracle.rs` — test-only.

The concatenated-`ffmpeg`-argument vectors in `executor.rs` (`prepare_head`, `prepare_body`,
`prepare_join`, `prepare_copy`, `prepare_reencode`, lines 583-901) take every value from
`CutPlan`/`MediaInfo`/`DeliveryPreset` fields, never from a raw user string in a position ffmpeg
would re-parse as an option: the media path and output path each occupy a fixed positional slot
after `-i` and at the end, and nothing prepends a leading `-` to either. `head_extra_args`
(`executor.rs:636`, `:873`) is the one place an argument vector is extended from plan data, and I
read its only producer (`plan.rs:447`) — it comes from `VideoTreatment` encoder metadata, not from
user input. The `-vf`/`-af` filtergraph strings are likewise assembled from the built-in preset
table, not from typed text.

**XML/document injection in exports — clean.** `crates/trimmer-export/src/xml.rs` escapes all five
XML metacharacters everywhere via `quick_xml::escape::escape` (line 44) and removes characters XML
1.0 cannot represent at all (lines 14-16, 27-36), with the removal reported as a warning rather than
silent. `url_path` percent-encodes every byte outside RFC 3986's unreserved set plus `/` and `:`
(lines 66-90). The crate never *parses* XML — `quick-xml`'s `serialize` feature is enabled but no
deserialisation path exists — so XXE and entity-expansion attacks have no surface here.

**SQL injection in the store — clean, and fully parameterised.** Every statement in
`crates/trimmer-store/src/store.rs` and `schema.rs` was read. All twelve query-executing call sites
(lines 187, 264-286, 301-328, 343-347, 363-368, 379-394, 399-410, 413-445, 448-457, 465-483,
487-511, 516-573, 592-606, 611) use `?1`-style placeholders with `params!`/array bindings. A grep
for `format!` adjacent to `SELECT|INSERT|UPDATE|DELETE` returns zero matches. The only two
`execute_batch` calls (schema.rs:128, :149) pass compile-time constant strings from the
`MIGRATIONS` table; `migrate` iterates `MIGRATIONS` in ascending version order and applies each
block inside its own transaction, so a recorded version greater than `LATEST_VERSION` results in no
statements being applied rather than a downgrade. `PRAGMA foreign_keys = ON` and WAL are set on
every connection (store.rs:184-187), which is what makes the `ON DELETE CASCADE` clauses real.

**`unsafe` code — clean, and the claim verifies.** All nine library crates carry
`#![forbid(unsafe_code)]` at the top of their `lib.rs`: `trimmer-core:54`, `trimmer-media:49`,
`trimmer-verify:62`, `trimmer-export:59`, `trimmer-app:39`, `trimmer-store:49`, `trimmer-daemon:45`,
`trimmer-cli:42`, and `apps/desktop/src-tauri/src/lib.rs:34`. `forbid` (as opposed to `deny`) cannot
be overridden by an inner attribute, so an `unsafe` block in any of these crates is a compile error,
not a lint. A full-repository grep for the token `unsafe` returns exactly those nine attribute lines
and no other occurrence — every `unsafe` in the tree is the word inside a `forbid` attribute. The
binary entry points (`main.rs`, `bin/ttrim.rs`) contain no `unsafe` either.

**Path traversal — clean.** Every user-supplied suffix that reaches a filename goes through
`trimmer_app::workspace::sanitise_name` (`crates/trimmer-app/src/workspace.rs:728-755`), which
replaces `< > : " / \ | ? *` and all control characters with a space, collapses whitespace runs,
trims trailing dots, caps the result at 120 characters, and substitutes `segment` for an empty
result. `/` and `\` are both in the illegal set, so a segment named `../../etc/passwd` cannot escape
the output directory. `Workspace::output_path` (`workspace.rs:621-644`) is the only function that
composes an output name and is used by the GUI cut path, the batch queue and the daemon run. Caption
sidecar discovery (`caption.rs:551-583`) joins a lowercased *stem* of the video file name, so it
cannot introduce a separator. The CLI's `--output` and the desktop's `export_timeline` destination
are written exactly where the operator asked, which is the documented contract for a desktop tool
and not a traversal bug. `MediaPath::canonicalised` (`domain.rs:40-45`) canonicalises when the file
exists and falls back to the lexical path when it does not, deliberately, so a project still opens
with an offline source; the fallback is what makes an offline project usable and does not by itself
grant any filesystem access.

**TOCTOU and symlinks — no exploitable pattern found.** No code re-checks a path after validating
it; checks are `is_file()` immediately before use and the consequences of a swap are "ffmpeg fails
on a different file", not a privilege change. Every path used by this application is already
readable and writable by the user running it, so a symlink swap buys an attacker nothing they did
not already have. The one place a race has a visible consequence is
`WorkDir::create` (`executor.rs:543-559`), which composes `.trimmer-<pid>-<output stem>` in the
output's parent and `create_dir_all`s it, with `Drop` (line 566) removing it recursively — the name
includes the PID and is inside a directory the user chose, so a pre-created directory is a nuisance
rather than a vector. `export_timeline` (`commands.rs:960-965`) `create_dir_all`s the destination's
parent, which is again the user's own choice of directory.

**Unbounded resource consumption — mostly clean, with finding 2 and finding 4 as the exceptions.**
Bounded on purpose, and verified: the keyframe window is clamped to the segment plus `MAX_HEAD_SECONDS`
(`executor.rs:248-260`) rather than the whole file; probe calls use `PollPolicy::quick()` with a
60-second timeout (`process.rs:62-69`); long encodes use `PollPolicy::long()` with a heartbeat and an
explicit decision to have no timeout (`process.rs:71-79`), which is correct for a legitimately
hours-long copy; the daemon's run registry is capped at `MAX_RUNS = 100` with oldest-finished
eviction and a refusal when every record is in flight (`state.rs:19`, `:209-231`); a second run for
the same project is refused `409` (`routes.rs:547-552`); transcript search results are clamped
(`routes.rs:632-634`, `commands.rs:881`, and `TranscriptView::hits` itself returns early at
`transcript.rs:202-204`). The keyframe listing accumulates every frame timestamp in the window into
a `Vec<i64>` with no cap (`probe.rs:269-275`), but the window is bounded by `MAX_HEAD_SECONDS` and by
two seconds of slack, so that vector is proportional to a segment the user selected, not to the
master. Caption parsing has no file-size cap — that is finding 4.

**Denial of service from a malformed media file — no panic path found.** No `unwrap()`, `expect()`,
or `panic!` in non-test code is reachable from untrusted input. The only three such calls in library
code are: `caption.rs:341`, whose `expect("the line contains an arrow")` is guarded by the
`line.contains("-->")` filter that produced the line two statements earlier; `plan.rs:478`, whose
`expect("checked above")` is guarded by the `unusable` match at 463-467; and `audit.rs:144`, `:158`,
`:240`, `:253`, all of which are `expect`s on serialising plain data to a `String`. `2^31`-scale
inputs are handled by saturating conversions rather than wrapping — `ordinal_of`
(`store.rs:696-698`) uses `i64::try_from(...).unwrap_or(i64::MAX)`, and the probe fields all parse
into `Option` with defined fallbacks (`probe.rs:136-212`), so a container that lies about its own
metadata produces a refusal or a conservative default, not a panic. `serde_json::from_str` on
ffprobe output is the one place attacker-influenced bytes enter a parser, and its error is mapped to
`MediaError::BadProbe` rather than unwrapped (`probe.rs:308-310`). The `panic = "abort"` release
profile (root `Cargo.toml:61`) means an unexpected panic is a process death rather than a poisoned
half-state, which is the fail-closed choice.

**Secret handling — clean.** No credential, key, or token literal exists anywhere in the source: a
grep for `THE_TRIMMER_[A-Z_]*KEY|secret|password|api_key` finds only documentation prose and the
word in test names. The daemon requires a token of at least 16 characters and refuses whitespace in
it (`config.rs:59-72`), refuses any bind address that is not loopback by parsing it to `IpAddr` and
calling `is_loopback` (`config.rs:73-85`), and compares the supplied bearer token in constant time
with the empty-token case explicitly rejected (`routes.rs:187-225`). The token is read from
`THE_TRIMMER_TOKEN` or `--token`, and the CLI marks the argument `hide_env_values = true`
(`cli.rs:470-478`); the daemon's own help text recommends the environment variable as the way to
"pass them without putting a secret in a process listing" (`main.rs:12-14`). If an operator chooses
`--token` instead, the value is in that process's command line — worth knowing, but it is the
operator's explicit choice and the safer route is documented at the point of use. The audit
manifest's HMAC key is a parameter of `sign`/`verify_signature` (`audit.rs:156-175`) and is never
stored, generated, or defaulted anywhere in the tree; `verify_signature` uses `constant_time_eq`
(`audit.rs:225-234`) and normalises case and surrounding whitespace, and a truncated signature does
not verify.

**The daemon's HTTP surface — clean.** Every route is behind `route_layer(middleware::from_fn_with_state(...))`
(`routes.rs:251-254`) including `/v1/health`, `/v1/capabilities` and `/v1/openapi.json`; the layer is
applied to the `Router` that already has `.with_state(state)`, so there is no route registered after
it that escapes the check. Path parameters are parsed as UUIDs and a malformed one is a `400`, not a
`404` (`routes.rs:109-126`). A segment is refused when its out point is not after its in point and
when its in point is past the end of a probed source (`routes.rs:438-486`), so the domain's planner
is consulted before any process is spawned. Error bodies are `{"error","detail"}` and internal
failure text is not distinguishable from a refusal by status alone. CORS is not enabled
(`tower-http`'s `cors` feature is declared in the workspace but no `CorsLayer` is constructed —
grep confirms), which is the right default for a loopback control surface; note that a permissive
CORS header is not required for this API to be reachable from a browser page, since a bearer token
cannot be attached cross-origin without CORS, so the absent CORS layer is the load-bearing control
here together with the token. There is no request body size limit configured — `axum`'s default
body limit of 2 MiB applies, and every route takes a small JSON object, so this was not raised as a
finding.

**Desktop IPC and frontend — clean apart from finding 1.** All twelve `#[tauri::command]` functions
in `commands.rs` take owned JSON-shaped arguments and return `Result<_, String>`; none takes a
closure, a program name, or a command line, and none can be steered into running an arbitrary
program. `apps/web/src` contains no `dangerouslySetInnerHTML`, no `innerHTML`, no `eval`, and no
`new Function` — a grep for all five returns nothing — so transcript text and media metadata, which
are attacker-influenceable through a malicious `.srt` or container, are rendered through React's
default escaping. `TranscriptPanel.tsx` renders hits as `{plain(hit.highlighted)}` (lines 186, 197)
inside JSX text nodes. The Tauri CSP
(`tauri.conf.json:29`) sets `default-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none';
frame-ancestors 'none'` with `connect-src 'self' ipc: http://ipc.localhost`; `style-src` allows
`'unsafe-inline'`, which is a common and low-impact relaxation for a bundled React app and was not
raised as a finding. `dragDropEnabled` is on, which is needed for the file-drop workflow. The `main`
window is the only window in `windows[]` and the only entry in the capability's `windows` list, so
the capability cannot leak to a second webview. One cosmetic inconsistency: the CSP's `img-src`
allows the `asset:` protocol, but the asset protocol is not enabled in `tauri.conf.json` (no
`assetProtocol` block), so that origin is advertised without a provider — the effective policy is
narrower than the string suggests, which is harmless, but removing the dead keyword would keep the
policy honest.

**Workspace-level build settings — checked.** `Cargo.lock` is committed. The release profile sets
`lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`, and `panic = "abort"`; the dev profile
deliberately uses `panic = "unwind"` so the differential oracle can catch a panic per case rather
than aborting the run, and the comment at `Cargo.toml:63-64` states that reason. No build script
(`apps/desktop/src-tauri/build.rs`, 355 bytes) or CI step fetches and executes remote content: the
one `git clone` in `ci.yml` (line 82) targets a separate repository into `$RUNNER_TEMP` and its
result is consumed only as a subprocess input by a test that skips loudly when the clone fails.

---

## Summary

Five findings: **one Medium** and **four Low**; no High or Critical. The Medium is
`opener:allow-open-path` in `apps/desktop/src-tauri/capabilities/default.json:13`, an unscoped
capability that would let any script in the webview launch an arbitrary local program, and which is
unused by the current frontend — deleting the line is the whole fix. The four Low findings are the
unbounded child-output buffers in `process.rs`, the `PATHEXT` `.BAT`/`.CMD` resolution in `tool.rs`
that qualifies the code's own "never a shell" guarantee, the quadratic failure path in
`parse_stamp`, and the absence of dependency scanning and immutable pins in the build.

Six of the checks the brief named came back clean and are documented above with file and line
evidence rather than assertion: no shell is used at any of the four real spawn sites and no argument
is ever re-parsed as an option; every SQL statement in the store is parameterised; all nine library
crates `forbid(unsafe_code)` and the tree contains no `unsafe` block; segment names are sanitised of
both path separators so a composed output path cannot escape its directory; no panic path in
non-test code is reachable from untrusted input; and no secret, key, or token literal exists in the
source, with the daemon's token compared in constant time behind a loopback-only bind. Two things
could not be verified from source alone and are recorded as limits rather than conclusions: the
runtime behaviour of the third-party `tauri-plugin-opener` implementation (only its generated ACL
manifest was read, which is what finding 1 rests on), and the behaviour of the installed `ffmpeg`
build under adversarial input, for which no fuzzing or live experiment was run.
