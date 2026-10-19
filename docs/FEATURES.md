# Thirteen features worth taking from seven reference projects

Each feature names the mechanism it comes from, what it becomes in this codebase, and what it
costs.

**The spine: the picture is never re-encoded.** The body of a cut is copied packet for packet;
only the runs from each mark to the nearest keyframe are re-encoded, because a stream copy cannot
start or stop on an arbitrary frame. Everything below is judged against that. A feature that would
re-encode the picture to add something to it is not an improvement to this app — it is a
different app — and the two that were proposed and cut for exactly that reason are in §17.

The other three filters, unchanged: **no cloud or API key, no administrator, and the cut itself
must not get slower.**

| Project | What it is | What it contributed |
|---|---|---|
| [openshorts](https://github.com/mutonby/openshorts) | FastAPI service turning long videos into vertical shorts | Word snapping, the audio envelope, batch ffmpeg discipline, caption filter syntax |
| [autoclip](https://github.com/zhouxiaoka/autoclip) | Tauri + Python desktop auto-clipper (the closest sibling) | Process ownership, cancel semantics, crash reconciliation, shadow QA, output identity |
| [OpenCreator](https://github.com/krillinai/OpenCreator) | Go + Electron AI production app | Pre-flight findings, progress parsing, the character→time caption map |
| [fragua](https://github.com/pedrovelasquez9/fragua) | Small agent-driven shorts editor | Delivery recipes as data, the delivery gate, Windows PATH traps |
| [reelmimic](https://github.com/edenfunf/reelmimic) | Reference-reel style cloner | Shot detection, the seam integrity scan, proof strips, cheap cache keys |
| [hyperframes](https://github.com/heygen-com/hyperframes) | HTML→video renderer | Typed findings, failure diagnosis |
| [video-use](https://github.com/browser-use/video-use) | Agent-driven editor | Cut-list algebra, the EDL model, editorial cut rules |

## Where these land now

`/api/probe · parse · plan · name · cut · cancel · events` (`backend/server.py:683-690`); zones
Video / Caption file / Range / Options / What came out / Progress (`frontend/src/App.tsx`); engine
`trim.py` (plan, head/body/tail, sound, mux), `ffmpeg.py` (probe, keyframes, frames, SSIM),
`verify.py`, `subtitles.py`; requirements `R1`–`R7` in `docs/SPEC.md`.

## Measured here, before the list

Synthetic 1920×1080/30 fps H.264 clip, FFmpeg 8.0.1, timing read from ffmpeg's own `-benchmark`
clock rather than the shell's:

| Test | Result |
|---|---|
| ffmpeg-only shot scan, `select='gt(scene,0.3)'`, 1,800 frames | **0.95 s** → **19 of 19** boundaries, at exactly 3.00 s … 57.00 s → a 2-hour 30 fps master in **≈1.9 min** |
| 30-tile filmstrip, one pass (`fps=0.5,scale=320:-1,tile=6x5`) | **0.90 s** |
| the same 30 tiles as 30 separate seeks | **≈3.6 s** (120 ms per seek, measured earlier) |
| a filter option given a Windows path (`file=H:\…`) | does not parse — use a **bare filename** and set the working directory |

The synthetic clip decodes faster than a real long-GOP master, so read 1.9 min as a lower bound.

## The list

| # | Feature | From | Tier | Encodes a frame? | Effort |
|---|---|---|---|---|---|
| F1 | The app owns its children | autoclip | Protect | no | S |
| F2 | Pre-flight findings | autoclip, OpenCreator | Protect | no | S |
| F3 | Verification that cannot slow the cut | reelmimic, autoclip | Protect | no — decodes windows | M |
| F4 | Failure diagnosis and capability memory | openshorts, hyperframes, OpenCreator | Protect | no | S |
| F5 | Transcript drawer with pause snapping | openshorts | Feel | no — audio decode only | M |
| F6 | Energy map and silence snapping | openshorts, autoclip | Feel | no — audio decode only | S |
| F7 | Proof strip of the seam | reelmimic, video-use | Feel | no — decodes frames | S |
| F8 | Cut list import and export | video-use | Scale | no — data | S |
| F9 | Batch queue: one source, many ranges | autoclip | Scale | no — schedules the same cut | M |
| F10 | Jobs that survive | autoclip | Scale | no | M |
| F11 | Delivery recipes | fragua | Scale | **only the edge patch and the audio** | S |
| F12 | Shot boundaries and chapter list | reelmimic | Reach | no — decodes pixels | S |
| F13 | Take cleaner: a cut list from silence | video-use | Reach | no — many segments, each still three pieces | M |

Not one of the thirteen encodes a frame that a copy could have carried. **F11 is the only feature
that changes an encoder setting at all**, and it changes it only for `_encode_head`,
`_encode_tail` and `_encode_sound` — never for the body, and never on a delivered file that has
already been cut. F13 multiplies *segments*, not re-encoded content per segment: thirty ranges are
thirty short edge patches and thirty copied bodies.

---

# Tier 1 · Protect the promise

Small features in the planning and verification path. Together they turn "it finished" into "it
is correct", and none of them touches the cut's speed.

## F1 · The app owns its children

**Source project:** autoclip — `backend/core/parent_watch.py:69` watches a PID handed in
`AUTOCLIP_PARENT_PID` (consumed from the environment so children never inherit it, `:25-39`) and
compares `psutil` process create times so a recycled PID cannot be mistaken for the parent
(`:42-66`); on parent death it kills every descendant (`:97`) and exits. A Windows **Job Object**
with `KILL_ON_JOB_CLOSE` backs it up (`services/child_guard.py:35,70`), and the shell passes its
own PID (`src-tauri/src/backend_manager.rs:79`).

**The Value Add:** close the window mid-cut today and ffmpeg keeps encoding a multi-gigabyte
master, holding a file lock, invisible — and the next attempt at that source fails or collides.
This is not a nicety; it is the class of bug that produces a refund request. It also makes the
cancel button honest: cancelling must kill the *tree*, not the one process the backend happens to
know about.

**This is not hypothetical in this codebase.** `electron/main.cjs:86-92` handles `before-quit`
with `backend.kill()` — no request, no wait — and Windows terminates only that process. The engine
kills its ffmpeg child *when a cancel token is set* (`backend/trimmer/ffmpeg.py:208-219,
385-396`), and a quit never sets one. So quitting mid-cut orphans the encoder: it keeps writing,
keeps the file locked, and the job is left in no state at all.

**Implementation Insight:** the Python side is shell-agnostic, so Electron needs no native addon.
Start the backend with `THE_TRIMMER_PARENT_PID=<electron pid>`; in `backend/server.py`, run a
2-second watcher thread that exits the server when the parent is gone — and have `before-quit`
set the cancel token and wait up to ~2 s first, so the normal path is a clean stop and the watcher
is only the backstop. Funnel every `subprocess` through one `backend/trimmer/processes.py` wrapper
that records the `Popen`, and kill with `taskkill /F /T /PID` (the tree flag is the part that
matters). Two details from the reference worth copying: compare start times before killing a PID,
and hand the child `CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW` so a console window never
flashes. The shell already passes the port through the environment (`main.cjs:67`), so the
reference's `PORT=` stdout handshake is a small step from here and removes the fixed-port
collision when 8765 is taken.

## F2 · Pre-flight findings

**Source project:** autoclip — `backend/pipeline/media_precheck.py:1-26` runs the checks whose
answers are known as soon as ffprobe can read the file, and fails with a *stable code* plus a
sentence that says what to do, so a doomed source never reaches the expensive work; codes that
describe the user's input are explicitly not bug reports. OpenCreator supplies the shape worth
copying: every check resolves to **blocked / warning / ready** with a fix action, and *ready*
entries are emitted too so the panel reads as a checklist (`apps/daemon/src/creator/preflight.ts:257-273, :539-560`).

**The Value Add:** a ten-minute cut that fails at the end is a refund; one that should never have
started is an insult. This is the cheapest feature here and the one that most changes how
professional the app feels: before the Trim button is live it says the source is readable, ffmpeg
and the encoder are present, there is disk for the expected output, audio is present and not
silent, and the caption file was found.

**Implementation Insight:** `/api/plan` already returns notes; widen them into
`findings: [{code, severity, message, fix}]` with `severity ∈ {blocked, warning, ready}` — a pure
function over facts the app already has, so it is a table test. Keep the blocking set small:
unreadable source, missing ffmpeg or encoder, no free disk, output locked. Everything recoverable
stays a warning, because a false block is worse than a missed warning. Disk estimate: `frames ×
bitrate` from the profile, times 1.2.

## F3 · Verification that cannot slow the cut

**Source project:** two mechanisms, one feature. reelmimic's `motion_check.py` streams every
frame to 160×96 grey rawvideo over an ffmpeg pipe (`:23,33-38`) and flags hard cuts (grid-cell
change in >50% of an 8×8 grid, `:64-66`), freezes (whole-frame difference < 0.0015 for ≥1 s,
`:69-73`), black (<0.04) and white (>0.92) frames, and a clip ending on a hard cut to black
(`:76-80`). autoclip's shadow QA supplies the *scheduling*: enqueued only after the result is
committed, on a 1-thread pool, in a separate low-priority interpreter, hard-killed at 12 s
(`services/studio/qa/report.py:16`), and a checker that misses its budget is recorded
`skip/timeout` — never a failure (`:97-119`). Its `avsync.py:108` cross-correlates 1 s PCM windows
at 15%, 50% and 80% of the output against the mapped source time and requires the median offset
and spread ≤40 ms (`:12-13`), with a container `start_time` match explicitly not a pass (`:160-167`).

**The Value Add:** this is the one thing the current verification structurally cannot see. SSIM
answers "is this the right frame, roughly"; it cannot answer "did the splice land without a frozen
or duplicated frame, a one-frame black gap, or a 30 ms audio slip". Those are exactly the failure
modes a three-piece splice produces — and the audio ones are inaudible until a client hears a
click. Just as important: because it runs *after* the deliverable is committed, it cannot make the
cut slower or fail a good job, which is the constraint this project has held throughout.

**Implementation Insight:** run on windows, never the master — ±0.5 s around head→body,
body→tail, and the mux point for the freeze/black/flash scan, and three 1 s windows for the
sync cross-correlation. Reuse `active_speaker.decode_activity`'s pipe shape
(`active_speaker.py:258-286`) and the existing `ff.frame_png` for evidence frames. Rows go in
"What came out" beside the SSIM checks, each naming its frame. The three answers are `passed`,
`failed`, `not checked` — never collapse them (that is already `R3.3`). Loudness belongs here too:
integrated LUFS and true peak measured in the audio pass that already decodes, reported, never
applied.

## F4 · Failure diagnosis and capability memory

**Source project:** openshorts — `ffmpeg_utils.cut_clip()` makes the **file** the verdict (exit 0
*and* ≥1024 bytes), retries the identical command at 3 s and 9 s, and remembers per source that a
hardware path failed so it does not repeat it thirty-nine more times (`:339-415`);
`finished_before_crash()` (`:232-250`) accepts an ffmpeg that segfaults after writing everything.
hyperframes decodes the Windows exit-code class by name and carries the last ~15 stderr lines into
the error (`packages/engine/src/utils/runFfmpeg.ts:57-112`). OpenCreator parses `-progress
pipe:2` for real ETA and retries once with the software encoder, with a message
(`apps/daemon/src/creator/clip/executor.ts:311-316, :134-152`).

**The Value Add:** in a batch of thirteen cuts, one transient failure should not leave a red row
and a manual rerun, and a real failure should say *why* in words the user can act on. The
file-is-the-verdict rule also kills a specific silent failure: exit 0 with a zero-byte piece.

**Implementation Insight:** three changes inside `ff.run()` (a) require `size > 1024` after every
piece and the expected frame count after every picture piece — the app already inspects the
deliverable; (b) on a transient-looking failure (encoder init, allocation) retry once after a
pause, then once with a software encoder, and record the fallback on the piece; (c) put the stderr
tail in the message the UI shows. Never retry a content failure — a bad seek fails identically.
Copy autoclip's progress rule too: fixed stage weights, and the bar never reaches 100 until the
job is `done` (`services/simple_progress.py:189-216`).

---

# Tier 2 · Change how it feels

The bottleneck in a trimming session is not the cut. It is deciding where to cut, in footage you
have not memorised.

## F5 · Transcript drawer with pause snapping

**Source project:** openshorts — `clip_selection.snap_clip_to_words()`
(`clip_selection.py:303-347`) on word timestamps from `transcribe_backends.transcribe_media()`
(`transcribe_backends.py:650-677`, faster-whisper, int8, CPU, fully local). The mark is moved onto
the nearest word boundary within 1.5 s, then extended into the measured pause by
`min(0.35, gap/2)` at the head and `min(0.45, gap/2)` at the tail.

**The Value Add:** today both marks are numbers. The person cutting a two-hour interview knows the
moment by *what was said*. A transcript pane turns "cut from the last 40 seconds of the intro"
into Ctrl-F and a click. The snapping half is the more valuable half: the cut lands in the silence
*after* the last word instead of clipping its final consonant — a defect invisible in a
frame-accurate render and immediately audible.

**Implementation Insight:** two stages, so the first ships with no new dependency. *Stage 1:* the
app already parses the caption file into cues (`subtitles.py:97-118`); snap the marks to the
nearest **cue** start or end with the same "extend into the gap by half" rule. A pure
`snap_to_cue(frame, cues, rate)` in `backend/trimmer/`, tested without media. *Stage 2:* word
timings from faster-whisper `small` int8 (~250 MB, CPU wheels, no admin), cached per source keyed
exactly like the keyframe cache (size + `mtime_ns`) — the reference's own transcript cache tests
only for the file's existence and therefore never invalidates when the source changes, which is
the mistake to avoid. `GET /api/sources/{id}/transcript` → `{w, s, e}` list; the drawer is rows of
cue text with *set in* / *set out*. Never run automatically: an ASR pass over two hours is 10–25
minutes and must be an explicit action with progress and cancel.

## F6 · Energy map and silence snapping

**Source project:** openshorts — `active_speaker.audio_envelope()` (`active_speaker.py:73-101`):
`ffmpeg -vn -ac 1 -ar 8000 -f s16le -` piped into NumPy, per-0.4 s RMS normalised to the peak,
with `AUDIO_FLOOR = 0.18` (`:37`) as the "nobody is speaking" threshold. autoclip corroborates the
placement use: `boundaries.audio_silences()` runs `silencedetect` over a **bounded window**, not
the whole file (`services/studio/boundaries.py:298`).

**The Value Add:** 28 lines and no new dependency, answering the first question anyone asks of
unfamiliar footage — where are the takes, and where is the dead air? On a two-hour master it is a
strip under the marks showing speech as blocks and silence as gaps, plus "snap to the quietest
point within 2 s" on each mark. It works on music-only and screen-recording material where a
transcript cannot, and it is the cheap half of F12: one audio decode instead of a video decode.

**Implementation Insight:** background task on load, cached beside the keyframe cache; ~18,000
floats for two hours (72 KB), drawn on a canvas. Silence bands are runs below the floor longer
than 0.25 s. Two honest details: an empty envelope means *no audio track*, not *silence* (the
reference returns `[]` for exactly this), and the normalisation is per-source, so it says "quiet
for this file", not "quiet in absolute terms".

## F7 · Proof strip of the seam

**Source project:** reelmimic — `clip_strip.py`: 12 fps, 240 px wide, the time burned under each
frame, `--vs` stacking a REF row above an OURS row, and a refusal to build a comparison against a
>80%-black reference (`:62-81`). video-use supplies the layout: filmstrip, waveform, silence bands
(`helpers/timeline_view.py:184-331`).

**The Value Add:** users argue about one-frame differences and a number does not settle an
argument. One PNG — the frames either side of each mark, with `HH:MM:SS:FF` and the frame index
burned under each — settles it in a look, and is the best possible support artifact: "send me the
strip" beats "send me the 40 GB master" by a factor of a thousand.

**Implementation Insight:** one ffmpeg pass per seam, measured here at **0.90 s for 30 tiles**
against **≈3.6 s** for 30 separate seeks. Sparse sampling is the seek's job, dense sampling is the
pass's: `-ss` before `-i` for a handful of frames, `fps=…,scale=…,tile=…` for a span. Do **not**
copy the reference's one-process-per-frame extraction (`timeline_view.py:49-61`) — it is its
slowest idea. Write the PNG into the job folder, name it after the segment, and link it from the
verification panel.

---

# Tier 3 · Change what it is for

These only pay off together, and not one of them is worth much alone.

## F8 · Cut list import and export

**Source project:** video-use — the `edl.json` document (`SKILL.md:286-307`): a version, a source
map, ranges carrying `{source, start, end, beat, quote, reason}`, and optional overlays and
subtitles.

**The Value Add:** the workflow around this app is not self-contained. A client sends six
timecodes; an editor wants the twelve segments as markers in their NLE; a previous session's
decisions should be revisitable. One file format serves all three, and it is the cheapest
integration with Premiere and Resolve that does not involve writing an importer.

**Implementation Insight:** the model is the reference's; the formats are ours.
`{version: 1, source: {path, size, mtime_ns}, rate: "30000/1001", ranges: [{inFrame, outFrame,
name, note}], recipe: {…}}` — **frames, not seconds**, because seconds would round away the
property this product sells. Import accepts that JSON, a `HH:MM:SS:FF` CSV, and an FCP XML/EDL for
markers; export emits the same. `timecode.py` already reads drop-frame correctly — reuse it rather
than accepting decimal seconds. Validate on import through F2's findings.

## F9 · Batch queue: one source, many ranges

**Source project:** autoclip — a durable domain model (`backend/models/project.py`, `clip.py`,
`collection.py`, `task.py`, `backend/repositories/*`) with a task per unit of work and a collection
over clips, and an in-process `ThreadPoolExecutor(1)` for rendering (`services/studio/jobs.py:19,27`).

**The Value Add:** the hinge. A cut list, a take-cleaned session, a set of client timecodes, and
thirteen segments all want to be *queued*. It also changes how the tool is used: start a session,
walk away, return to a folder of named segments instead of babysitting thirteen runs. Nothing else
on this list scales without it.

**Implementation Insight:** one worker, not Celery (the reference runs Celery because it is also a
service; its `tasks/scheduler.py` is periodic maintenance we do not need). The engine is already
sequential by design (`R4.3`), so the queue is a table plus a loop:
`jobs(id, source, spec_json, state, stage, started, finished, error)`, one worker task, each job
calling the existing `trim()` unchanged. The part that makes it trustworthy is not the queue: per-job
stop, a queue-wide pause, and the guarantee that job *n+1* on the same source cannot start until
job *n* has released its file handles.

## F10 · Jobs that survive

**Source project:** autoclip — an instance UUID per process (`services/studio/store.py:18`), recorded
on every queued and running job (`services/studio/jobs.py:53`); on the next read, any job whose
instance is not the current one is rewritten as a terminal `service_restarted` failure
(`store.py:360, 393-416`), and writes are temp-file + `os.replace` with a Windows
`PermissionError` winerror 5/32/33 retry (`:336-347`). Its publishing rule is the other half:
render to `.part.mp4`, probe-validate, then `os.replace` into place (`services/studio/render.py:142,
302-309`), with a deterministic slug over inputs + version tokens so an identical request returns
`cached: True` without rendering (`services/publish_export.py:288, 302-308`).

**The Value Add:** two promises a paid tool must keep. First, a crash or a quit never leaves a
spinner running forever or a half-written file wearing the deliverable's name — it leaves an
interrupted job that says "interrupted, retry". Second, re-cutting the same marks with the same
settings is instant and produces the same bytes, which is what makes a cut list reproducible.
Note what this deliberately does *not* promise: **resume**. An interrupted AAC pass cannot be
resumed without reintroducing the priming-delay problem the single audio pass exists to avoid, so
the honest UI is retry, not continue.

**Implementation Insight:** `%LOCALAPPDATA%\TheTrimmer\jobs\<id>\` holds `spec.json`, the pieces,
and `state.json` written atomically with a process UUID. On launch, any job from a dead instance
becomes `interrupted` with the plan pre-filled. The identity key is
`(size, mtime_ns, hash of first and last 1 MB)` — never a whole-file hash of a 20 GB master — plus
encoder settings in the version token, or a changed CRF silently reuses the old cut. This composes
with the overwrite dialog already built: internal artifacts are content-addressed and never
prompt; only the final user-facing name does.

## F11 · Delivery recipes

**Source project:** fragua — `presets.json`: four named recipes (`youtube_long`, `tiktok`,
`reels`, `youtube_short`), each binding dimensions, fps, `crf`, x264 preset, audio bitrate, a
loudness target, a full ASS subtitle style and a card theme (`presets.json:3-93`), with
documentation carried *inside* the file as `_`-prefixed keys stripped by the loader
(`scripts/common.py:97-104`).

**The Value Add:** a paid tool sells consistency — but here the thing that must be consistent is
the *edge patch*, not the picture. A recipe pins the encoder, CRF, preset and pixel format used for
the runs at the marks, the sound settings for the one audio pass, the naming and folder rule, and
the verification tolerance. That is what makes two operators cutting the same master produce
byte-identical edges, "it looks soft" a one-file diff, and a support question answerable by asking
which recipe ran. It also puts a name on the only place the app has any choice left.

**Implementation Insight:** one versioned JSON validated with pydantic *and ranges* — the
reference's weakest point is that a missing key becomes a `KeyError` mid-render
(`scripts/subtitles.py:258-264`), so validate at load and refuse with a sentence naming the field.
A recipe is applied in exactly three places and nowhere else: `_encode_head`, `_encode_tail`
(`-c:v`, `-crf`, `-preset`, `-pix_fmt`), and `_encode_sound` (`-c:a`, `-b:a`). It cannot touch the
body, because the body is a packet copy — and the schema should say so, with a comment, so nobody
adds an "output quality" field later that implies otherwise. Built-ins ship with the app;
user recipes live in `%APPDATA%\TheTrimmer\recipes\` so an update cannot erase them (the
reference learned this: config that must survive lives in `~/.fragua/`, `common.py:35-46`).

---

# Tier 4 · Reach, without touching a packet

## F12 · Shot boundaries and chapter list

**Source project:** reelmimic — `analyze.shots()` (`analyze.py:112-119`) uses PySceneDetect
`ContentDetector(threshold=22)`, run on a proxy when the source is long (`:379`).

**The Value Add:** when the master is a rollup of takes, or a screen recording with distinct
scenes, the *visual* cut is a better landing point than a timecode, and the boundary list is a
chapter menu for a file too long to scrub. It also gives the app something honest to say about a
master it just opened: how many shots, how long the average is, where they are.

**Implementation Insight:** ffmpeg alone, no new dependency — measured above at **0.95 s per 1,800
frames**, exact on every boundary: `-vf "select='gt(scene,0.3)',metadata=print:file=scenes.txt"
-an -f null -`, with a **bare filename** in a working directory (a Windows path inside a filter
option does not parse). Parse `pts_time` into frames on the source's grid and cache per source.
PySceneDetect remains the optional upgrade if real footage proves the simple score too noisy —
it is more robust to flashes and gradual transitions. Draw the boundaries as ticks and let a mark
snap to the nearest.

## F13 · Take cleaner: a cut list from silence

**Source project:** video-use — `skills/media-use/scripts/lib/cutlist.mjs`, a pure algebra over a
transcript: removal sources (explicit ranges, word-index ranges, a filler set, silence gaps),
merged, inverted, filtered to a minimum length, with silence edges padded (`:3-4`, `:121-180`).

**The Value Add:** the single biggest time saving in this document. A 40-minute take with four
minutes of dead air becomes a 36-minute cut list in one action — and because the output is a
*list*, the operator reviews it, deletes what they disagree with, and only then cuts. It is the
difference between "I trim" and "I hand it a rough cut".

**Implementation Insight:** no transcript in the first version. `ffmpeg -af
silencedetect=noise=-35dB:d=0.4` gives the gaps; the algebra is a ~60-line pure function
`kept_ranges(removals, duration)`, and the result is exactly what F9's queue consumes. Guard rails
that matter: a minimum kept length (1.0 s is a sane floor for talking heads, not 0.2 s), a cap on
how many ranges one action may create, and the existing plan forecast (`R5.11`) shown before
anything is cut. Filler-word removal needs F5 stage 2, so it is a later switch on the same screen.

## Two candidates that did not survive the spine

Burned-in captions and a vertical reframe were both in the first draft of this list, and both
are in §17 now. Not because they are bad ideas — they are the two most commercially obvious —
but because each one requires encoding the whole picture to add something to it, which is the
one thing this app exists not to do. If they are ever built, they belong to an export step that
takes a finished segment as an input and is honest about being a different operation.

# 17. What not to take, and why

| Tempting | From | Why not |
|---|---|---|
| **Burn-in captions on the delivered segment** | openshorts `subtitles.py:640-646`, fragua | Requires re-encoding the whole picture to add text to it. The app's answer is the retimed `.srt` beside the file, which travels with it and costs nothing. If burning is ever sold, it is a separate export step that names its own cost. |
| **Vertical reframe of the master** | openshorts `reframe_v2.py`, `active_speaker.py:204-218` | Same objection, plus it changes the frame: a 9:16 crop of a 16:9 master is a different picture, not a trim. Belongs beside the app, not inside it. |
| Single-pass `-ss/-t` full re-encode as the cut | OpenCreator `clip/executor.ts:191-222` | Contradicts lossless and frame accuracy; no keyframe boundary, no packet copy, no concat. |
| A whole-master analysis proxy (`libx264 -g 12`) | reelmimic `analyze.py:103-109,379` | A full re-encode before analysis. Take the *idea* of cheap pixels, bounded to a window. |
| One `-ss` process per filmstrip frame | video-use `timeline_view.py:49-61` | Measured 4× slower than one pass, with 20–40 processes. |
| PySceneDetect + TransNetV2 + torch (~2.5 GB) | openshorts, reelmimic | A single-camera talking head yields one scene; the ffmpeg scene score is measured exact and free. |
| LLM moment scoring, layout pickers, translation repair loops | openshorts, OpenCreator | Paid API, non-deterministic, and inside the cut path where determinism is the product. |
| Celery, stage leases, claim/renew schedulers | autoclip, OpenCreator | Service machinery for a single-user desktop app; one worker and a lock is the right size. |
| `loudnorm` applied to the deliverable | openshorts, fragua | Alters the master's audio — the opposite of the promise. Measure it, never apply it. |
| A transcript cache keyed on the file's existence | autoclip `speech_recognizer.py:462` | Never invalidates when the source changes. Key on size + `mtime_ns`, as the keyframe cache does. |
| Rendering a waveform for an hour-long file in the UI thread | OpenCreator `SubtitleTimeline.tsx:21-24` | Its own correct answer: refuse above a size and keep the timing editable. |
| Hand-maintained agent manifests and 80 KB skill manuals | fragua, reelmimic | Prose that must stay in sync with code, for no user benefit. |

---

# 18. Order of work

**Tier 1 — protect** (F1–F4). All small, all outside the cut path, none of them encoding anything.
F1 is a crash bug and should not wait for anything.

**Tier 2 — feel** (F5–F7). The transcript and the energy map are what make a two-hour master
navigable; the proof strip is what makes the result arguable. All three only ever decode.

**Tier 3 — scale** (F8–F11). The cut list, the queue and the job state are one coherent step. F11
is the only feature in the document that sets an encoder flag, and it sets it for the edge patches
and the audio pass — the two places the app already has to encode.

**Tier 4 — reach** (F12–F13), and both are still just marks and lists: one decodes pixels to find
shot boundaries, the other decodes audio to find silence. Neither touches the cut.

The rule to apply to anything added later, including anything from a future reference project:
**does it require encoding a frame that a copy could have carried?** If yes, it is not a feature of
this app. The app already tells the operator when it has to encode — the plan line names the head
patch, and `R5.11` makes it a forecast — and the roadmap should not quietly contradict that line.

Each feature lands the way this project already lands: a numbered requirement in `docs/SPEC.md`, a
scenario in `specs/features/`, a unit test on the pure part, and the measured reason recorded in
`docs/DESIGN.md`. The two items already agreed in `docs/ADOPTION.md` that these depend on are the
atomic publish (§2.3) and the cut record beside the segment (§3.2); F10 and F11 are the same
substance approached from the job side and the recipe side.
