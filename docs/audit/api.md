# API design audit — TheTrimmer-V2 public surfaces

> **This is a record of a review, not a list of open work.** It was written against the tree as it
> stood when it was run, and the findings that mattered have since been fixed — the fix for each is a
> commit, and the current state of the product is [docs/TRUTH.md](../TRUTH.md) and the test suite.
> Line numbers below refer to the revision that was read and will have moved.
>
## Scope

This audit covers the two surfaces the product exposes to a caller: the local HTTP/JSON API in
`crates/trimmer-daemon` and the Tauri IPC commands in `apps/desktop/src-tauri/src/commands.rs`.
Files read in full or in the parts cited below: `crates/trimmer-daemon/src/routes.rs`,
`crates/trimmer-daemon/src/openapi.rs`, `crates/trimmer-daemon/src/state.rs`,
`crates/trimmer-daemon/src/config.rs`, `crates/trimmer-daemon/src/main.rs`,
`crates/trimmer-daemon/src/lib.rs`, `crates/trimmer-daemon/Cargo.toml`,
`crates/trimmer-daemon/tests/api.rs`, `crates/trimmer-store/src/document.rs`,
`crates/trimmer-store/src/store.rs` (the `list_projects`/`list_runs` SQL only),
`crates/trimmer-core/src/domain.rs` (`MediaPath`, `SegmentSource`, `Segment`, `VerifyPolicy`,
`Project`, `CutMode`), `crates/trimmer-core/src/transcript.rs` (`Hit`),
`crates/trimmer-app/src/workspace.rs` (`SourceView`, `SegmentView`, `QueuePreview`,
`WorkspaceSummary`, `preview_all`), `crates/trimmer-app/src/queue.rs` (declarations only),
`crates/trimmer-app/src/transcript.rs` (declarations only),
`crates/trimmer-media/src/tool.rs` (tool resolution only),
`apps/desktop/src-tauri/src/commands.rs`, `apps/desktop/src-tauri/src/state.rs`,
`apps/desktop/src-tauri/src/lib.rs`, `apps/web/src/ipc/types.ts`,
`apps/web/src/ipc/commands.ts`, `crates/trimmer-cli/src/cli.rs` and
`crates/trimmer-cli/src/commands.rs` (the `daemon` subcommand only, to check for a CLI client).
`target/` and `node_modules/` were ignored. Behaviour was cross-checked against the prebuilt test
binary `target/debug/deps/api-97c47e0e16c12d01.exe`, which was executed and passed 36 tests; that
binary may predate the current working tree, and the limits of what it proves are stated in
**Could not verify**.

The brief used as the standard is the `api-design-principles` skill and its
`resources/implementation-playbook.md` (resource-oriented routes, correct method and status
semantics, consistent error bodies, versioning, pagination, documented contracts).

## Findings

### Critical

**C1 — An error body carries no machine-readable cause, so no script can act on a failure.**
`crates/trimmer-daemon/src/routes.rs:103` (`IntoResponse for ApiError`) fixes every refusal as
exactly `{"error": <word>, "detail": <prose>}`; `crates/trimmer-daemon/src/openapi.rs:57-64`
declares the same two fields and nothing else. Every handler-visible failure is therefore a word
that is reused across unrelated causes: `"internal"` is returned both for a missing ffmpeg
(`routes.rs:691-696`) and for a store failure (`routes.rs:131`, `routes.rs:324`, `routes.rs:494`),
and `"refused"` is returned for anything the domain rejects (`routes.rs:132`). A pipeline cannot
distinguish "retry, ffmpeg is not on `PATH` yet" from "the database is corrupt", and there is no
`details` object carrying the offending field, the id that was not found, or the frame range that
was refused — the fix has to be recovered by parsing the English sentence. Fix: add an optional
`details` object and an `error` vocabulary that is one code per cause (for example `toolMissing`,
`storeFailure`, `badRange` with `{"startFrame","endFrame"}`), extend the `Error` schema in
`openapi.rs:57-64`, and have `ApiError::internal` take a code so callers stop seeing the same word
for two different problems.

### High

**H1 — A missing ffmpeg is a `500`, and the same code as a crash.** `routes.rs:690-701`
(`engine`) maps "ffmpeg and ffprobe could not be found" to `StatusCode::INTERNAL_SERVER_ERROR` with
`error: "internal"`, and `run_batch` (`routes.rs:810-814`) puts the same sentence in `record.error`
after returning `202`. The condition is an environmental prerequisite, not a server defect: the
correct answer is `503 Service Unavailable` with a distinct code and the sentence naming
`THE_TRIMMER_FFMPEG` / `THE_TRIMMER_FFPROBE`, which is what an operator needs to see in a log.
Fix: return `503` with `error: "toolMissing"` from `engine`, and document `503` on
`/v1/projects/{id}/sources`, `/v1/projects/{id}/preview`, `/v1/projects/{id}/run` and
`/v1/capabilities` in `openapi.rs`.

**H2 — `/v1/health` reports `"status": "ok"` on a machine that cannot cut.** `routes.rs:290-297`
always emits `"status":"ok"` and carries the truth only in the separate `ffmpeg` boolean. A
supervisor that checks the HTTP status or the `status` field — the two things the field invites —
reports a healthy daemon that will fail every `POST /v1/projects/{id}/run`. Fix: report
`"status":"degraded"` (and keep `ffmpeg:false`) when `state.has_ffmpeg()` is false, or document
`status` as liveness-only and add a `ready` boolean that a probe is told to use.

**H3 — The published OpenAPI document names a `Project` and a `Run` it does not define, so codegen
produces an untyped client.** `openapi.rs:65-81` declares `sources` as `{"type":"object"}` and
`segments` as `{"type":"array"}` with no `items`, and gives `presets` no schema at all, although
every one of them is always present in a real response (`crates/trimmer-core/src/domain.rs:665-689`
and the response asserted at `crates/trimmer-daemon/tests/api.rs:249-256`). `openapi.rs:97-109`
declares `Run.outcome` as `{"type":["object","null"]}`, so the `RunOutcome` fields a poller actually
needs — `succeeded`, `unverified`, `failed`, `skipped`, `items`, `report`
(`crates/trimmer-daemon/src/state.rs:78-99`) — are invisible. Fix: define `SegmentSource`,
`MediaInfo`, `RunOutcome` and `RunItem` schemas and reference them from `Project` and `Run`.

**H4 — No schema in the document declares a `required` field, so every generated client treats
every field as optional.** `openapi.rs:56-110` and every inline `200`/`201` schema
(for example `openapi.rs:119-128`, `openapi.rs:330-339`) omit `required` entirely. Generators such
as `openapi-typescript` and `openapi-generator` will emit `report?: string`, `state?: string`,
`hits?: unknown[]`, which defeats the point of publishing a schema and pushes the null checks back
into every consumer. Fix: add `required` to each schema, and add a document test that every
`$ref`'d object schema has one.

**H5 — `/v1/health`'s response schema declares `ffmpeg` twice.** `openapi.rs:125-126` contains
`"ffmpeg": { "type": "boolean" }` twice in the same object literal. In JSON two keys with one name
are legal but the second silently wins, and strict validators and several generators reject the
document outright. Fix: delete the duplicate line and add a test that the document has no
duplicate keys (for example by parsing with a duplicate-key-rejecting deserializer).

**H6 — The two surfaces disagree on what a "source" is, and one endpoint's response matches
neither.** `POST /v1/projects/{id}/sources` answers with `{path, available, label, probed}`
(`routes.rs:392-404`), while `GET /v1/projects/{id}` answers with a `sources` map whose entries are
`SegmentSource` — `{path, media, available, label}` (`crates/trimmer-core/src/domain.rs:518-530`) —
and the IPC command `add_source` answers with `{path, name, present, media, summary, transcript,
transcriptCues, label, variableRate}` (`apps/desktop/src-tauri/src/commands.rs:218-228`), mirrored
by `SourceView` (`crates/trimmer-app/src/workspace.rs:31-52`). The same resource therefore has three
field sets; `apps/web/src/ipc/types.ts:25-35` types `add_source` as `SourceView`, which is only true
of the IPC surface, and `openapi.rs:207` types the HTTP response as an opaque object. Fix: one
canonical source representation per surface, produced by one serializer and used by every endpoint
that returns a source; document it (see H3).

**H7 — A batch outcome has two incompatible shapes across the two surfaces.**
`RunOutcome` (`state.rs:78-99`) is `{succeeded, unverified, failed, skipped, deliveredFrames,
deliveredSeconds, elapsedSeconds, items, report}`, where each item's `status` is a prose line from
`JobStatus::summary()` (`routes.rs:868-877`). `BatchOutcomeWire`
(`apps/web/src/ipc/types.ts:170-178`, produced at `commands.rs:728-740`) is `{jobs, deliveredFrames,
deliveredSeconds, elapsedSeconds, cancelled, digest}` with a discriminated `status.kind`, and it
carries `cancelled` and `digest` that the daemon omits while the daemon carries the counters and
`report` that the IPC omits. A studio with both a dashboard and the desktop app cannot share a
client type, and the daemon's `status` string cannot be branched on at all. Fix: pick one outcome
schema (the discriminated one is the better of the two), publish it in `openapi.rs`, and derive both
responses from it.

**H8 — `preview` fails differently depending on which door the caller came through, and the
document does not say so.** `GET /v1/projects/{id}/preview` calls `Workspace::preview_all`, which
returns on the first hard failure (`crates/trimmer-app/src/workspace.rs:559-572`) — so the daemon
answers `400 refused` and the client gets *no* previews at all — while the Tauri
`preview_all` (`commands.rs:569-579`) catches each segment's failure and substitutes
`{"problems":[reason]}`. `openapi.rs:268` says only "One entry per enabled segment". Fix: decide the
contract (per-segment problems in both, or fail-fast in both), and state it in the operation
description.

**H9 — `preview_all` over IPC returns an entry that is not a `QueuePreview` and does not name its
segment.** `commands.rs:575` pushes `serde_json::json!({ "problems": [reason] })`, which omits
`segment`, `plan`, `preset`, `forcesFullEncode`, `reencodeFraction`, `estimatedBytes`, `commands`
and `notes` — every field `QueuePreview` declares (`apps/web/src/ipc/types.ts:70-80`). A caller
iterating the array has no way to tell which segment failed (the id is only inside the prose), and a
field access on the failed entry yields `undefined` rather than an error. Fix: emit the full
`QueuePreview` shape with the failure in `problems`, as the daemon's own per-item rendering does
(`routes.rs:730-753`).

**H10 — `cut_segment` reports success unconditionally.** `commands.rs:658-678` hardcodes
`"kind": "succeeded"`, `"checks": []`, `"deliveredSeconds": 0.0` and `"digest": ""` regardless of
what the run actually did. The sibling `run_batch` reports the real `Succeeded`/`Unverified`/`Failed`
verdict through `status_json` (`commands.rs:744-787`) under the same declared TypeScript type
`BatchOutcomeWire`. A caller cannot tell a verified cut from an unverified one, and the batch
outcome's `unverified` state — a state the whole `trimmer-verify` design exists to express — is
unreachable from the single-cut path. Fix: build this body with `status_json` from the cut's own
outcome and compute `deliveredSeconds` from the frames actually written.

### Medium

**M1 — The router has no fallback, so some refusals are not the documented error shape.** The
router registers only the listed routes (`routes.rs:231-255`), so an unknown path returns an empty
`404` — asserted, and accepted, at `crates/trimmer-daemon/tests/api.rs:801-808` — and a body the
`Json<Value>` extractor rejects produces axum's own plain-text `400`/`415`/`422`. Both contradict
the crate's own promise at `routes.rs:14-17` that "There is no case where the API answers with a
bare string". A client that does `body["error"]` gets a parse failure on those paths. Fix: add a
fallback handler and a custom JSON rejection handler that render the `Error` schema, and add a test
for a bad content type and for a non-object body.

**M2 — `POST /v1/runs/{id}/cancel` moves the state before the run stops, and that lets a second run
start.** `routes.rs:612-615` sets `RunState::Cancelled` immediately; the batch is still finishing its
in-flight segment (documented at `openapi.rs:306`). `DaemonState::project_is_running`
(`state.rs:193-200`) treats only `Queued` and `Running` as in flight, so a second
`POST /v1/projects/{id}/run` is accepted while the first run is still writing. Fix: keep the record
`Running` and add a `Cancelling` state set by the route, cleared by `spawn_run` when the batch
returns; `project_is_running` should then treat `Cancelling` as in flight.

**M3 — Two collection endpoints are unbounded, and the OpenAPI document admits no paging
parameters anywhere.** `GET /v1/projects` returns every row with no `limit`/`offset`/cursor
(`routes.rs:323-333`), `GET /v1/projects/{id}/segments` returns every segment
(`routes.rs:408-414`), and `GET /v1/projects/{id}/preview` returns every enabled segment with the
full ffmpeg argument vectors (`routes.rs:521-538`, `routes.rs:730-753`). `GET /v1/capabilities`
also returns the full `encoders`, `muxers` and `filters` lists (`routes.rs:311-313`), and
`POST /v1/transcripts/search` clamps `limit` to `0..=1000` (`routes.rs:632-634`). There is no
pagination parameter in `openapi.rs` at all. Fix: add `limit`/`offset` (or `cursor`) to the two list
endpoints with a documented default and maximum, return a collection envelope with `total` — adding
the envelope now is a breaking change, so if the bare array must stay, add the parameters and an
`X-Total-Count` header; give `capabilities` a `?detail=` switch or move the lists behind their own
path.

**M4 — Nothing tells a client how to poll `GET /v1/runs/{run_id}`.** `routes.rs:895-898` defines
`SUGGESTED_POLL` (500 ms) and its doc comment says it is published "because an API that does not say
is an API every client polls at a different rate" — but it is never emitted in a response, header or
document. `POST /v1/projects/{id}/run` returns `202` with no `Retry-After` and no `Location`
(`routes.rs:585-588`). Fix: either send `Retry-After: 1` on the `202`, or add the interval to the
`202` body, or both, and describe it in `openapi.rs:273-290`.

**M5 — Tauri commands fail with a bare string, so the client has to guess the category.**
`commands.rs:38` aliases `Reply<T> = Result<T, String>` and every command returns a sentence;
`apps/web/src/ipc/types.ts:209-212` nevertheless declares `IpcError` as
`{message, kind: "domain"|"media"|"store"|"internal"}`. `normaliseError`
(`apps/web/src/ipc/commands.ts:82-103`) papers over the mismatch by classifying *every* string as
`domain` and therefore user-fixable (`commands.ts:65-67`), which means a store failure or a missing
`ffmpeg` is presented to the user as something they can fix by editing a field. `tsconfig` cannot
catch this because both sides are hand-written. Fix: return a structured error from the Rust side
(`{message, kind, details}` via a `serde`-able error type) so the declared `IpcError` is real, and
classify from the code rather than from the fact that the payload was a string.

**M6 — `doctor` over IPC wedges on a machine without ffmpeg, while the daemon deliberately does
not.** `commands.rs:68-71` resolves the tools with `?` and returns a string error, so the doctor
panel and every other diagnostic is unavailable precisely when a user most needs it;
`DaemonState::new` (`state.rs:142-158`) documents the opposite decision and `GET /v1/health`
answers `200` with `ffmpeg:false`. Fix: render the doctor report with `"not found"` strings and
`ffmpeg:false`, as `GET /v1/capabilities` already does (`routes.rs:300-320`), and reserve the error
for a genuine failure to run the tools.

**M7 — The `verify` policy cannot be read or changed from the web client, and cannot be changed at
all over HTTP.** `get_verify_policy` and `set_verify_policy` (`commands.rs:1067-1086`) are registered
commands with no counterpart in `apps/web/src/ipc/commands.ts` and no type in
`apps/web/src/ipc/types.ts`, so the policy the API documents as a `Project` field
(`openapi.rs:75`) is unreachable from the interface. Fix: add both to `commands.ts` with a union type,
and add a `PATCH /v1/projects/{id}` that accepts `{verify, defaultPreset, name, outputDir}`.

**M8 — `plan_watch_folder` leaks Rust variant names as wire values.**
`commands.rs:1020-1021` formats `plan.trigger` and `plan.action` with `{:?}`, so the contract is
whatever the enum is called in Rust (`"Settled"`, and the `PlanAction` variants), with no documented
vocabulary and no stability guarantee. Fix: give the enums a `serde` representation or map them to
explicit words, and add the shape to `types.ts`.

**M9 — The transcript search shape and the "no captions" answer differ between the surfaces.**
`POST /v1/transcripts/search` returns `{video, phrase, rate, hits[, reason]}` and a `200` with a
`reason` when there is no caption file (`routes.rs:626-668`, correct and well reasoned), while
`search_transcript` over IPC returns a bare array and an empty array for both "no captions" and "no
matches", with no rate (`commands.rs:860-884`). Fix: return the daemon's envelope from the IPC
command too, so the two surfaces share one contract and the client can tell the two empties apart.

**M10 — A new daemon connection requires a fresh request-per-command.** `SUGGESTED_POLL` aside, the
only way to observe a run is to poll `GET /v1/runs/{id}`, which returns a `RunRecord` whose
`outcome.items` is the entire batch at once (`state.rs:78-99`). There is no Server-Sent Events or
long-poll option, and the `202` carries no `Location`. For a ninety-segment batch the difference
between 500 ms polling and a stream is a large amount of JSON. Fix (lower priority than M4): return
`Location: /v1/runs/{run_id}` on the `202`, and consider `GET /v1/runs/{run_id}/events` or
`?wait=<seconds>` long-polling.

### Low

**L1 — Bodies are not consistent with 204 or with the 202 envelope.** `delete_project`
(`routes.rs:369`) and `delete_segment` (`routes.rs:515`) return `200` with `{"deleted": id}` where
`204 No Content` is the conventional answer for an idempotent delete, and
`delete_segment`'s body echoes the caller's own input. Fix: `204` with no body, or keep `200` and
document the body as intentional. Minor, but a generator will type it as a body the client must
discard.

**L2 — The bearer scheme is matched case-sensitively and a leading space in the token is not
tolerated.** `routes.rs:198-206` requires the exact prefix `"Bearer "` and trims the remainder;
`bearer` therefore returns `None` for `bearer <token>` (legal per RFC 7235, which makes the scheme
case-insensitive) and a double space yields an empty token and a `401`. Fix: split the header on
whitespace and compare the scheme case-insensitively.

**L3 — `trimmer-export` is a declared dependency the daemon never uses.**
`crates/trimmer-daemon/Cargo.toml:15` lists it and no daemon source file mentions
`trimmer_export`. Either remove it, or — the more useful reading — add the export route the
dependency implies (see S1).

**L4 — `GET /v1/health` duplicates the service identity instead of using a header.** `routes.rs:290-296`
returns `service` and `version` in the body with no `Server` header; that is harmless, but a client
that must parse the body to learn which product answered could have read a header. Informational.

## Checked and clean

These were examined and found correct; they are listed because they are the evidence that the check
was performed.

- **Route naming and method semantics.** Every HTTP path is a plural noun collection with
  sub-resources (`routes.rs:231-255`); `GET`/`POST`/`DELETE` are used for their defined meanings,
  and no path is verb-shaped. The one action-ish path, `POST /v1/runs/{id}/cancel`, is a legitimate
  non-idempotent transition that cannot be expressed as a resource edit.
- **Status codes that are correct.** `201` for `create_project` (`routes.rs:347`) and for
  `add_source` and `add_segment` (`routes.rs:404`, `routes.rs:500`); `202` for an accepted
  asynchronous run (`routes.rs:585-588`); `409 runInProgress` for a second run on the same project
  (`routes.rs:547-552`) and `409 registryFull` when every registry slot is a run in flight
  (`routes.rs:574-575`); `404` for a missing project, segment or run
  (`routes.rs:355`, `routes.rs:513`, `routes.rs:600`); `400 badId` for a non-UUID path segment
  rather than a misleading `404` (`routes.rs:109-126`); `400 badRange` for a reversed or out-of-range
  cut (`routes.rs:446-486`); `400 unknownSource` for a source not in the project
  (`routes.rs:432-437`). All of these are asserted by `crates/trimmer-daemon/tests/api.rs`
  (lines 264-290, 293-300, 302-317, 361-408, 476-489, 555-605), and the prebuilt suite passes.
- **Error body shape is consistent for every failure that reaches a handler.** `ApiError` is the
  single refusal type (`routes.rs:48-106`) and `{"error","detail"}` is produced in exactly one
  place; the gap in C1 and M1 is the content and the uncovered paths, not an accidental second
  shape.
- **A missing project is read before a mutation, so it is a `404` rather than a `500` that leaks
  the store's error.** The intent is written down and implemented at `routes.rs:365-367`,
  `routes.rs:379-381` and `routes.rs:526-527`.
- **Auth.** A token shorter than 16 characters or containing whitespace is refused at startup
  (`config.rs:59-72`, `MIN_TOKEN_LEN` at `config.rs:13`); `bind` must be a loopback address and the
  refusal names the address (`config.rs:73-85`); the comparison is constant-time and mixes the length
  difference into the result (`routes.rs:187-195`), and is tested against equal/unequal/short/long
  inputs (`tests/api.rs:181-188`); an empty supplied token cannot authenticate even against an empty
  configured token (`routes.rs:215`); `Authorization` is read as `Bearer <token>`
  (`routes.rs:198-206`). The OpenAPI document declares `securitySchemes.bearer` and a document-level
  `security` requirement (`openapi.rs:53-54`, `openapi.rs:112`), and a `401` is documented on every
  operation. `/v1/openapi.json` itself is behind the token, which is consistent.
- **Versioning.** The API is versioned in the path (`/v1/...` everywhere), the version is reported
  in `GET /v1/health` and `GET /v1/capabilities` (`routes.rs:294`, `routes.rs:302`), and a future
  `/v2` can be added without renaming anything. No header- or query-based versioning is mixed in.
- **`POST /v1/transcripts/search` answering `200` with a `reason` for a video with no caption file**
  (`routes.rs:621-668`): this is the right call and it is documented in the operation description
  (`openapi.rs:317-318`) and the crate docs (`lib.rs:41-43`). "There is no transcript here" is a
  property of the file, not a missing resource. Pinned by `tests/api.rs:708-726`.
- **The run registry is bounded.** `MAX_RUNS = 100` (`state.rs:19`), eviction takes the oldest
  *finished* run (`state.rs:209-231`), and a registry full of runs still going is refused rather
  than silently dropped. Tested at `tests/api.rs:632-704`.
- **`GET /v1/projects` ordering matches its documentation.** `openapi.rs:145` says "newest first"
  and the store's SQL is `ORDER BY updated_at DESC, id` (`crates/trimmer-store/src/store.rs:590-593`).
- **The OpenAPI document's path list cannot drift from the router silently.**
  `openapi::PATHS` (`openapi.rs:18-32`) is compared against the document's own keys
  (`tests/api.rs:764-777`) and each path is then driven through the router
  (`tests/api.rs:779-799`). The mechanism is right; the schemas inside the document are where the
  defects are (H2-H5).
- **The `verify` policy vocabulary agrees across the layers.** `openapi.rs:75` lists
  `off|standard|strict|forensic`, and `set_verify_policy` accepts exactly those four words
  (`commands.rs:1075-1081`), which match the `VerifyPolicy` variants
  (`crates/trimmer-core/src/domain.rs:632-647`) serialised `camelCase`. The defect is reachability
  (M7), not vocabulary.
- **The Tauri argument-name boundary is correctly factored.** Rust commands declare `snake_case`
  parameters (`created_by` at `commands.rs:110`, `start_frame`/`end_frame`/`handle_frames` at
  `commands.rs:342-345`, `stop_on_error`/`skip_verification` at `commands.rs:686-687`,
  `sequence_name` at `commands.rs:934`) and the web layer calls them with the `camelCase` names the
  Tauri JS bridge maps onto those parameters (`apps/web/src/ipc/commands.ts:141-142`,
  `172-179`, `217-221`, `227-231`). The mapping is consistent across all of them, and none of the
  commands sets `rename_all`, so the bridge's default applies: `WrapperAttributes::parse` initialises
  `argument_case: ArgumentCase::Camel` (`tauri-macros-2.6.3/src/command/wrapper.rs:51`), which is
  why the `snake_case` Rust parameters answer to `camelCase` keys on the JS side. Verified in the
  vendored macro source at
  `C:\Users\ferna\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\tauri-macros-2.6.3\`.
- **The IPC command list and the web command list agree on names**, and the signature gaps I found
  (M7) are absence, not mis-naming: `doctor`, `list_projects`, `create_project`, `open_project`,
  `delete_project`, `add_source`, `refresh_sources`, `remove_source`, `sources`, `segments`,
  `summary`, `presets`, `add_segment`, `update_segment`, `remove_segment`, `reorder_segment`,
  `preview`, `preview_all`, `parse_timecode`, `search_transcript`, `cut_segment`, `run_batch`,
  `cancel_batch`, `export_timeline` and `reveal` all appear in `commands.ts:132-235`.
- **Capability hygiene on the IPC surface.** The commands take named scalars, validate paths through
  `MediaPath`, and never expose a file handle or a command line; `reveal` is deliberately not a
  general "open this" (`commands.rs:1034-1037`), and `export_timeline` is the only command that
  writes to a caller-named path (`commands.rs:959-965`), which is documented as intentional in
  `lib.rs:12-23`. This is a design choice I am recording, not a finding.

## Could not verify

- I did not compile a test harness of my own. The prebuilt binaries in `target/debug/deps/api-*.exe`
  were executed (36 passed) but may predate the working tree, so the only claims resting on them are
  the ones the checked-in test source asserts independently, which I also read.
- **M1's extractor claim is reasoned, not observed.** I did not exercise a malformed request body,
  a missing `Content-Type`, or a non-UTF-8 `Authorization` header against a live route, so I cannot
  state the exact axum status and body for those cases — only that they never pass through
  `ApiError::into_response` (`routes.rs:101-106`), which is where the documented shape is produced.
  The unknown-route half of M1 *is* observed (`tests/api.rs:801-808`, an empty `404`).
- I did not run `serde_json` on a live `VerifyPolicy` or `RunState` to confirm the exact casing of
  each enum variant. The routing and command code agrees with the document as read, so I have
  reported the vocabulary as checked-and-clean rather than as a finding, but a generated-client test
  would be needed to call it verified.
- I did not measure the size of a real `GET /v1/projects/{id}/preview` response; the unbounded claim
  in M3 rests on the code path (`preview_all` over every enabled segment, each with its full argument
  vector), not on a measurement.
- I did not audit `crates/trimmer-cli` as an API surface. I read only its `daemon` subcommand
  (`cli.rs:462-479`, `commands.rs:643-656`) to establish that no CLI command is an HTTP client of the
  daemon, which is the one fact this report needed from it.

## Gaps in the surface itself

**S1 — Nothing reads back what the daemon writes.** `run_batch` persists an audit record through
`state.store.record_run(...)` (`routes.rs:861-866`) and the store has `list_runs`
(`crates/trimmer-store/src/store.rs:296-311`) and `SELECT manifest_json FROM runs`
(`store.rs:344`), but no route exposes them. The only way to see a run's history is the bounded
in-memory registry, and after eviction (`state.rs:209-231`) it is gone from the API even though it is
on disk. A pipeline cannot ask "what did this project do last week".

**S2 — The daemon has no export route.** The desktop app writes Premiere XML, FCPXML, EDL and CSV
through `trimmer-export` (`commands.rs:936-969`), the CLI has `export` (`cli.rs:127`, `cli.rs:423`),
and `trimmer-daemon/Cargo.toml:15` declares the dependency — but `routes.rs` has no route for it,
so a headless pipeline must shell out to the CLI and write the file itself. Given that
`trimmer-export` is already a dependency, `POST /v1/projects/{id}/exports` returning the body (or a
path) would close the gap.

**S3 — A project cannot be updated, only replaced.** The API can create, read and delete a project
and its segments, but not rename one, change its `defaultPreset`, or change its `verify` policy
(see M7). A client that wants to change any of those must reconstruct and re-`POST` a whole project,
which no route accepts either.

## Summary

| Severity | Count |
|---|---|
| Critical | 1 |
| High | 10 |
| Medium | 10 |
| Low | 4 |
| **Total findings** | **25** |

Plus three surface gaps (S1-S3) recorded separately because they are missing capability rather than
a defect in what exists.

The single most consequential problem is **C1**: the error body is a word and a sentence, so nothing
downstream of this API can retry, branch or route on a failure without parsing English. The OpenAPI
defects (H3-H5) and the shape divergences between the HTTP surface and the IPC surface (H6, H7, M5)
follow from the same root — the contract is written twice, by hand, in three places, with no
generated binding to keep the copies honest.

If only one fix is made before this API is handed to a pipeline, make it C1: give the error body a
code and a `details` object. Every other finding here can be worked around by a caller who is
willing to read English and diff two schemas by hand; C1 cannot.
