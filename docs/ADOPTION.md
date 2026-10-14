# Adoption review: what two reference projects offer this one

Two open-source projects were read for ideas worth taking into TheTrimmer-V2. This is the
result: what to adopt, in what order, what it becomes in this codebase, and — the part that
matters more — what to refuse and why.

| Source | What it is | Read |
|---|---|---|
| [`browser-use/video-use`](https://github.com/browser-use/video-use) | An agent-driven editor: transcribe, pick cuts on word boundaries, per-segment extract, lossless concat, self-evaluate | All 36 files, in full |
| [`heygen-com/hyperframes`](https://github.com/heygen-com/hyperframes) | An HTML→video rendering framework: seekable page capture, FFmpeg splice, caption/audio pipeline, and an unusually large set of repository gates | ~8,600 files surveyed; the FFmpeg, seeking, media, caption, cache, process-supervision and gate machinery read in depth |

**The headline.** Neither project is a frame-accurate cutter, so nothing here is a design to
copy wholesale. What they do offer is a small number of hard-won specific facts — and, in two
places, an attractive idea that this codebase has already *measured as wrong*. The most
valuable outcome of this review is not a feature list; it is three correctness gaps of our own
that the reading exposed, plus two popular ideas that must not be adopted.

---

## 1. Measured here, before recommending anything

Claims below marked *measured* were run on this machine against a synthetic 1920×1080/30 fps
30 s H.264 master built with `lavfi` — no user media was touched. FFmpeg 8.0.1 (gyan
essentials), the system build, because `.tools\ffmpeg` is not populated in this checkout until
`start.bat` provisions it.

| Measurement | Result |
|---|---|
| One `-ss … -frames:v 1` frame extraction to a 320 px JPEG | **120 ms** (116–147 ms single; 115 and 120 ms/seek over loops of five) |
| 20 tiles as 20 separate extractions (the `video-use` pattern) | **≈2.4 s**, 20 processes — extrapolated from the 115–120 ms per seek measured above |
| 20 tiles in one pass, `fps=5,scale=320:-1,tile=5x4` | **213–251 ms**, 1 process |
| The same pass plus a 1600×120 waveform (`showwavespic`) in the same invocation | **394 ms**, 1 process, two outputs |
| One full-resolution PNG extraction (as `ff.frame_png` does) | **90 ms** |
| Three SSIM comparisons as three processes (as `ff.ssim` does) | **236 ms** |
| The same three comparisons as one process, three chains, `stats_file=` per chain | **153 ms** |
| Four PNG extractions + three SSIM comparisons (today's head check) | **≈645 ms**, 7 processes |
| The whole head check as one graph, selecting frames by index | **994 ms**, 1 process — *slower* |
| `ffmpeg -version` | 24 ms (the floor cost of every process) |

The 994 ms row is why §4.2 exists.

Two Windows traps found while measuring, both worth recording because they fail as a *parse
error*, not as a wrong number:

* A path inside a filtergraph needs its colon escaped (`stats_file='H\:/x.log'`); an
  unescaped `H:\…` is an `Error parsing a filterchain`.
* The escape-free form is to pass a **bare filename** and set the working directory — the
  pattern `hyperframes` uses for its own stats files
  (`packages/producer/src/regression-harness.ts:669`).

---

## 2. Adopt, tier 1 — correctness gaps in this codebase, found by the reading

These are not features borrowed from the references. They are places where reading the
references showed that our own engine can produce a deliverable that is subtly wrong, or can
lose a file to a crash. They come first because a paid product's promise is frame-exact
*fidelity*, and these are the cases where fidelity is silently not delivered.

### 2.1 Colour and bit depth are not carried across the splice, and never checked

`PASSTHROUGH_PIX_FMTS` (`backend/trimmer/trim.py:93-95`) holds six formats; **anything else
falls back to 8-bit `yuv420p`** (`trim.py:674`, `:721`, `:1019`). A 10-bit master whose decoder
reports `p010le`, `p012le` or `yuv420p12le` — none of which are in that set — therefore gets an
**8-bit head and tail spliced onto a 10-bit copied body**: a change of profile in the middle of
one file. (An 8-bit `nv12` source also converts, to `yuv420p`; that one is harmless at the
bitstream level — both are 4:2:0 8-bit — and costs a conversion.) The colour description
(`color_primaries`, `color_transfer`, `color_space`, `color_range`) is never read and never
passed. ffmpeg usually propagates a decoded frame's colour properties into the encoder's VUI, so
this is *often* right by accident; it is never checked, and the bit-depth fallback above is
unconditional. On an HLG or PQ master the re-encoded ends can therefore carry a different
description from the copied middle. `video-use` documents this failure from the other direction
(`helpers/render.py:110-132`: 8-bit output still carrying HLG metadata plays oversaturated).

Why it is invisible today: the verification compares **pictures** (SSIM on PNGs, which
converts to RGB at 8 bits) and **frame counts**. Neither can see a bit-depth or colour
mismatch. `docs/TRUTH.md` should not be claiming fidelity we do not measure.

**Adopt.**
* `ff.probe` reads what is already in the same ffprobe JSON it fetches — `ffmpeg.py:613-623`
  already returns every stream field and simply discards these. Add `bits_per_raw_sample`,
  `profile`, and the four `color_*` fields to `MediaInfo`; add nothing to the process count.
* Read the source's **coded** bit depth rather than pattern-matching a pix_fmt allowlist, and
  pass the colour description explicitly on the two re-encoded pieces
  (`-color_primaries`, `-color_trc`, `-colorspace`, `-color_range`) so the join cannot depend on
  what ffmpeg happened to propagate.
* Refuse, or say so before cutting, when a re-encoded piece cannot keep the body's bit depth.
* Add one free check to `verify.py`: `FileFacts` already comes from an `inspect()` that reads
  the delivered file's streams, so the delivered `pix_fmt` and `profile` can be compared with
  the source's at **zero extra process cost**. Report it as a check that can say *not checked*
  (R3.3), never as a silent pass.
* SPEC: new §8, `R8.1`–`R8.4`.

**Calibration against real footage.** The masters still in the folder beside this repo were
probed (header only, no decode): both are 8-bit `yuv420p` Main-profile H.264, so the bit-depth
fallback above is a **latent path, not an observed defect** in that library. The colour half is
not hypothetical, though: `Sergey-full.mp4` carries **no colour description at all** —
`color_primaries`, `color_transfer` and `color_space` are all empty — while
`THE ROLLUP SERGEY V1.mp4` is fully tagged bt709. The honest statement is therefore that we
cannot currently tell a good splice from a bad one on colour, not that these files are wrong.

### 2.2 Multi-track audio is silent about itself

`_audio_map` hardcodes `-map 0:a:0` (`trim.py:657-661`) and `probe` keeps only the first audio
stream (`ffmpeg.py:662`, `:674-680`). On the masters this product is for — OBS recordings, where
the microphone is very often track 2 or 3 — the app cuts track 1 and says nothing. `video-use`
has this exact fact written down (`helpers/transcribe.py:213-220`: "OBS writes the game on track
0 and the mic on track 1; without this ffmpeg applies its default audio stream selection, which
picks the track with the most channels"), and guards against the silent-track case before paying
for an upload (`transcribe.py:62-71`, `peak_dbfs` < −60 dBFS ⇒ refuse).

**Adopt.** List every audio stream in the source zone (index, codec, channels, sample rate,
language, title), mark the one being cut, allow choosing it, and warn when the chosen track
measures silent over a short window at the in point. The media line at `frontend/src/App.tsx:519-521`
already reports the first track's codec and channels; this widens it.

Layout cost, measured against the record already in `docs/TRUTH.md`: an option row is 36 px
(`--field-height` 30 + `--space-3` 6) against a content floor of 558 px inside a 960 px frame
minimum, so a fifth option row fits with ~400 px to spare. `R5.7` still holds.

SPEC: new §9, `R9.1`–`R9.3`.

**Calibration against real footage.** Both masters probed above have exactly **one** audio
stream, so `-map 0:a:0` is right for them today. One of the two stores it at absolute index
`a:1` with no `a:0` present, which looks alarming and is not: `0:a:0` means *the first audio
stream*, not *stream number zero*, so it selects the same stream `probe()` does. The risk this
item addresses is the multi-track OBS case, not the current library.

### 2.3 A cancelled or crashed run leaves a truncated file at the deliverable's name

`_mux` writes straight to `spec.output` (`trim.py:993-1001`), and `trim` returns only after
`ff.inspect(spec.output)` has read it back. A kill between those two points leaves a
partial MP4 wearing the final name. `hyperframes` extracts every segment to a temp name and
publishes with an atomic rename (`skills/media-use/scripts/transcript-cut.mjs:120-137`, and
`packages/engine/src/services/extractionCache.ts` does the same for cache entries).

**Adopt.** Write to `<name>.part.<ext>` in the destination folder, rename on success, remove on
cancel. The extension must stay last for the muxer; that is why the reference writes
`${outPath}.part${extname(outPath)}` rather than replacing the suffix.

### 2.4 A one-line escaping hole in the concat list

`_concat_list` writes `file '<path>'` (`trim.py:961`). A single quote in the path breaks the
list — and the paths are in `%TEMP%`, which on Windows is
`C:\Users\<name>\AppData\Local\Temp\…`, so a profile named `O'Brien` is enough. `video-use`
does not escape either (`helpers/render.py:389`); `hyperframes` does
(`transcript-cut.mjs:254-256`, `chunkEncoder.ts:575-588`) and also makes the list name unique so
two jobs cannot read each other's list — a failure that produces a silently wrong video rather
than an error.

**Adopt.** `path.replace("'", "'\\''")` plus a unit test on `_concat_list`, which is already a
pure function.

---

## 3. Adopt, tier 2 — the segment's edges, and a record of the cut

### 3.1 30 ms audio ramps on the segment's own edges

Both references treat a hard cut through a waveform as a defect: `video-use` makes it Hard Rule
3 (`SKILL.md:24`, `helpers/render.py:284-286`), and `hyperframes` refines it — scale the ramp to
`min(0.03, duration/4)`, and **skip it entirely at an export's true start and end**, where there
is no splice to smooth (`skills/media-use/scripts/lib/transcriptCutFade.mjs:9-21`).

Our segment is standalone, so by that last rule it needs no ramp at all — until you consider
what these segments are *for*. They are cut to be joined again. A 30 ms ramp is what makes two
of them butt-join in an NLE without a click, and it costs one filter on a pass that already
exists (`trim.py:771` is `-af asetpts=PTS-STARTPTS` today).

**Adopt**, as an option, on by default, stated in the plan line — with the honest note that it
alters the first and last 30 ms of the sound, which is precisely why it is an option.

### 3.2 A cut record beside the segment

`video-use`'s whole session state is one versioned `edl.json` carrying *why* each range was
chosen (`SKILL.md:286-307`), and it caches derived artifacts against the source they came from —
"immutable outputs of immutable inputs" (`SKILL.md:30`, `:341`). Our keyframe cache already
follows that rule (`ffmpeg.py`, keyed on size + `mtime_ns`).

**Adopt.** Write `<name>.cut.json` beside the segment: source path and size/mtime, in and out
frames, the grid rate, the keyframes the plan used, encoder, CRF, preset, audio settings, the
offered and chosen audio track, the caption outcome, and the verification verdicts. Three
things fall out of it: a buyer can see what was done and reproduce it, the app can say "this
range was already cut from this source", and the numbers in `docs/TRUTH.md` get a per-run
anchor instead of a remembered one.

---

## 4. Reject — both with evidence

These two are the most attractive ideas in the references. Both are wrong for this codebase, and
saying so is the point of the review.

### 4.1 "Copy the audio instead of re-encoding it" — measured wrong here already

`hyperframes` keeps audio untouched with `-map 0:a -c:a copy` while compositing video
(`skills/embedded-captions/scripts/render-and-composite.sh:412`, `:423`, `:464`), and the
independent read of its media pipeline ranked this its **number one** finding: it would remove
our continuous audio pass entirely.

It must not be adopted. `_copy_body`'s own docstring (`backend/trimmer/trim.py:819-825`, "Picture
only.") records the measurement: asking ffmpeg for a stream copy of **both** streams at once
makes it seek the *audio* to its own sync point — up to four AAC frames before the keyframe —
and `-avoid_negative_ts make_zero` then rebases the file on that earlier audio packet, so **the
body's picture began 80 ms into its own file and the segment's content landed about three frames
behind the mark**. The single audio pass exists to make that failure unrepresentable. The
reference's advice is correct for its own tool (which cuts on word boundaries with fades, and
rebuilds the timeline) and wrong for a frame-exact splicer.

Keep the record as a *deliberate* rejection, with the measurement attached: this is the shape of
advice that arrives from outside as an obvious improvement, and the docstring is what stops it
being tried again.

### 4.2 "Do the whole check in one FFmpeg graph" — measured slower

`hyperframes` verifies many frames at once by selecting them in one filtergraph and pairing by
decode index (`packages/producer/src/regression-harness.ts:587-717`: `select='eq(n\,a)+…'`,
then `settb=1/1,setpts=N` so framesync pairs by index rather than timestamp, with the output row
count asserted). It looks like an obvious win for our head check, which is seven processes.

Measured here: **994 ms** as one graph versus **645 ms** as the seven processes — because
`select=eq(n,300)` makes the demuxer **decode from frame 0 to the frame wanted**, while
`-ss` (which `ff.frame_png` uses, `ffmpeg.py:1017-1023`) seeks to the keyframe first. On a
two-hour master the one-graph form is minutes, not milliseconds.

Keep the shape; take only the cheap half: three `ssim` comparisons in one process with a
`stats_file` per chain (measured **153 ms vs 236 ms**), with bare filenames and a working
directory. That is a ~90 ms saving on a ~650 ms check — real, but not worth restructuring
`verify.py` for on its own.

### 4.3 The rest of what was considered and refused

| Idea | Source | Why not |
|---|---|---|
| In-page video preview via a WebCodecs/mediabunny decoder | `packages/studio/src/player/lib/thumbnailVideoDecoder.ts`, `thumbnailScheduler.ts` | H.265 and ProRes in Chromium is exactly the codec set these masters use; the file the app must always read is the master. Tiles from the ffmpeg we already install give the same information for one process (measured above). |
| Proxy-transcode the master for preview | `packages/studio-server/src/helpers/proxyTranscoder.ts:327-439` | ~Real-time for a multi-GB file, 2 concurrent, 15-minute ceiling — and it contradicts "lossless, no re-encode". |
| Hosted or local transcription | `helpers/transcribe.py`; `packages/cli/src/whisper/manager.ts:160-162` | Requires an API key, or on Windows "build with cmake" plus 75 MB–3.1 GB of model. Contradicts `R6.1`, and we already *find* the caption file instead of inventing one. |
| Loudness normalisation | `helpers/render.py:534-627`; `packages/studio-server/src/helpers/loudness.ts` | Applying gain re-encodes and breaks the promise. Measured reporting only, inside a pass that already decodes (see §6). |
| Caption burn-in, ASS, overlay compositing | `helpers/render.py:633-706`; most of `packages/shader-transitions` | A different product. Our contract is a sidecar `.srt` beside the picture. |
| The whole Chrome/CDP capture engine, the browser-lease pool, the parallel worker model | `packages/engine` | Nothing here screenshots a page, and the worker model exists to shrink a streaming encoder's reorder window — a problem sequential splice passes do not have. |
| Comment-share ratchet, five agent manifests, 1,409-line CI, ADOPTERS/TRIAGE taxonomy | `scripts/comment-ratchet.mjs`, `plugin.json` + four variants, `.github/workflows/ci.yml` | Affordable at a company's headcount, not at one maintainer's. |

---

## 5. Adopt, tier 3 — what a buyer can see and feel

### 5.1 "Proof of the cut": a filmstrip and waveform for the planned range

`video-use`'s `timeline_view` is the best idea in either repository: a filmstrip, a waveform
with the silences shaded, and the words from the transcript laid over it, used at *decision
points* rather than as a scan (`helpers/timeline_view.py:1-11`, `:184-331`). `hyperframes`
drafts the same surface as a UI contract with a name band above each frame
(`docs/contracts/2026-10-01-timeline-filmstrip.html`).

Implement it the opposite way from `video-use`, which extracts one frame per process
(`timeline_view.py:49-61`) — ≈2.4 s for 20 tiles by the 115–120 ms per seek measured in §1,
against **213–251 ms for one process** producing the same 20 tiles as a `tile=5x4` contact
sheet, or **394 ms** for the sheet *and* a `showwavespic` waveform from the same invocation.
`ffmpeg` will not even need a new dependency.

Add to it what is free: the caption file we already read holds the pauses, so gaps between cues
can be shaded as silence bands without decoding audio at all. This is the visible difference
between "trust me" and "look".

### 5.2 Silence-aware marks

`video-use`'s editorial rules are all about where *not* to cut: never inside a word, pad the
edge, prefer gaps ≥ 400 ms, and pad 30–200 ms to absorb timestamp drift (`SKILL.md:27-28`,
`:112-113`). Ours has no opinion at all about where a mark lands.

Adopt the cheapest half: `ffmpeg silencedetect` over a **bounded window** at a mark (a few
seconds either side — a decode of a window, not of the master) to offer "snap to the nearest
silence", and the caption file's own cue gaps as the free version of the same signal. Report
when a mark lands mid-cue rather than moving it.

---

## 6. One measurement worth folding into a pass that already runs

`video-use` insists on measuring rather than assuming — "you cannot listen: say so, and report
the numbers" (`SKILL.md:99`) — and uses EBU R128 (`helpers/render.py:534-565`). Loudness *gain*
is out of scope (§4.3), but the *number* is not: the audio pass already decodes the whole
segment, so `ebur128` in that filter chain costs CPU only, no extra decode and no extra process.
Report integrated LUFS and true peak in the log and in the cut record.

The reference also documents a trap for exactly this measurement: with `-f null` there is no
output to bound, so a *windowed* audio measurement must put `-ss`/`-t` **before** `-i`
(`packages/studio-server/src/helpers/loudness.ts:450-457`, measured there as −33.8 LUFS against
a true −61.8). If a windowed measurement is ever added, that ordering is part of the
requirement, not an implementation detail.

---

## 7. What both references confirm we already do better

Worth recording so it is not copied away later.

| Our behaviour | Theirs |
|---|---|
| Marks snapped on `avg_frame_rate` via `MediaInfo.grid_rate`, with a drift measurement (`ffmpeg.py:527-556`) | `video-use` prefers `avg_frame_rate` but has one nominal fallback (`render.py:215-242`); `hyperframes` trusts `avgFps \|\| rFps` with a 10% VFR flag (`engine/src/utils/ffprobe.ts:716-720`) |
| Exact `Fraction` arithmetic everywhere time becomes frames, and six-decimal spans in the concat list (`timecode.py`, `_concat_list`) | `hyperframes` reaches the same conclusion with `BigInt` and rational strings after documenting a float bug of its own (`packages/engine/src/services/videoFrameExtractor.ts:131-194`) |
| The keyframe cache is keyed on size + `mtime_ns` | `video-use`'s transcript cache tests *existence only* (`transcribe.py:145-148`), so `SKILL.md:30`'s "unless the source file itself changed" is not actually enforced |
| Captions: straddling cues clamped or dropped by a measured minimum, renumbered from 1, BOM and line endings preserved (`subtitles.py`) | The same rules, arrived at independently (`packages/cli/src/whisper/normalize.ts:255-321`, `:411-447`) — including right-padding a short millisecond field, which we do too |
| Every check can answer *not checked* as distinct from *failed* (`R3.3`) | `video-use` has pass/fail; `hyperframes` has the three-state idea in its lint severities but not in its render gates |

---

## 8. Engineering practice worth taking, and the bulk worth leaving

`hyperframes` carries more gate machinery than any paid desktop app needs, and it is candid
about which of its own gates are heuristics
(`CONTRIBUTING.md:94` demotes two rules to warnings "because both are heuristics that measured
too many false positives on this repo to fail a build"). Three items fit this project.

1. **A citation check for the documentation.**
   `scripts/check-comment-citations.mjs` extracts backticked paths, `path:line` references and
   symbols from added comment lines and resolves them against `git ls-files`, failing when a
   cited line no longer exists. Our `docs/SPEC.md` requirement IDs are cited from tests and code
   and are exactly the kind of claim that rots when a line moves. Becomes
   `scripts/check_citations.py` (Python 3, stdlib only) run by hand and from a hook.
2. **A gate that proves it can fail.**
   The same checker first grades a fixture whose citations are dead by construction and exits
   non-zero if the resolver stays quiet (`check-comment-citations.mjs:939-966`). Nine lines per
   checker, and it separates a gate from a rubber stamp.
3. **A hook that refuses what `.gitignore` cannot.**
   `.gitignore` refuses media at any depth, but cannot stop `git add -f`.
   `scripts/check-large-files.sh` reads the staged index itself (not lefthook's `{staged_files}`,
   which splits paths with spaces), skips text files of any size, and is installed with
   `git config core.hooksPath .githooks` — zero install, and it never blocks `start.bat`.

Left behind, deliberately: the comment-share ratchet, the acceptance-budget and
package-cycle checkers, the 20-shard regression jobs, the pinned headless-Chrome image, and the
four duplicate agent manifests. Every one of them is a real cost paid for a contributor base
this project does not have.

---

## 9. Order of work

| # | Item | Why here |
|---|---|---|
| 1 | `§2.3` atomic output + `§2.4` concat escaping | Smallest changes in the list; they protect the file the user is waiting for |
| 2 | `§2.1` colour and bit depth | The only gap that can ship a wrong deliverable while every current check passes |
| 3 | `§2.2` multi-track audio | The masters in the folder next to this repo are the case |
| 4 | `§3.1` ramps + `§3.2` cut record | Cheap, and they are what the segment is *for* |
| 5 | `§5.1` proof strip + `§5.2` silence-aware marks | The visible justification for the price |
| 6 | `§6` loudness in the existing pass | Free once the pass is touched anyway |
| 7 | `§8` citation check, gate fixture, hook | Keeps all of the above from rotting |

Each item lands the way the rest of this project lands: a requirement in `docs/SPEC.md` with a
number, a scenario in `specs/features/`, a unit test for the pure part, and the measured reason
recorded in `docs/DESIGN.md`.
