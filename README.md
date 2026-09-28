# TheTrimmer

Frame-exact, verified segment cutting for professional post-production. Give it a master and two
Premiere timecodes and it gives back the segment — **with the original packets, not a transcode** —
then measures what it produced against the source and tells you, with numbers, whether it is right.
Windows desktop, offline, no account.

---

## Quick start

Double-click **`start.bat`**. That is the whole of it.

It works that way on a Windows PC with **nothing installed on it**. `start.bat` finds or installs
everything the application needs, and then opens the window:

| It needs | What happens |
|---|---|
| **Python 3.10+** | used if the machine has a suitable one, otherwise winget installs it, otherwise the official installer runs quietly for the current user |
| **Node.js 18+** | used if present, otherwise winget, otherwise the official archive is unpacked into `.tools\node` |
| **ffmpeg and ffprobe** | used from `PATH` if present, otherwise a portable build is unpacked into `.tools\ffmpeg\bin` |
| **aiohttp and PyAV** | installed into the project's `venv` from `backend/requirements.txt` |
| **npm's trees, and Electron** | installed into `node_modules` and `frontend/node_modules` |
| **the interface** | built, whenever a source file is newer than the bundle |

No administrator is needed at any point, and nothing is written outside this folder and
`%LOCALAPPDATA%`. The first run downloads about 250 MB and takes a few minutes; every run after
that skips what is already done and takes a couple of seconds. The downloads are cached under
`.tools\downloads`, so even a re-provision does not fetch them again.

It needs an internet connection the first time. After that it does not.

To check the provisioning itself — including the download, unpack and locate paths, which cannot be
reached on a machine that already has everything:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-bootstrap.ps1
```

To work on the interface with hot reload:

```powershell
npm run dev          # Vite on 5173, plus Electron pointed at it
```

---

## Naming a segment

The output is named for the range it holds — `reel 00.00.10.00-00.01.00.00.mp4` — unless you name
it. The **Name** field in Options takes any name the filesystem accepts, and the path it will be
written to is shown underneath it, read back from the engine rather than guessed.

The transcript follows the segment: name a segment `Interview wide` and its captions are written as
`Interview wide.srt`, beside it, because a caption file is only a sidecar if it shares the
segment's name and folder.

The **Folder** toggle puts both inside a folder named after the segment, so a cut arrives as one
thing rather than as two files loose among the footage:

```
footage/
├── master.mp4
├── master.srt
└── Interview wide/
    ├── Interview wide.mp4
    └── Interview wide.srt
```

A name Windows would refuse — a colon, a slash, a trailing dot, `CON` — is **refused, not
repaired**. Trimming `Take 1/2` to `Take 12` would write a file that exists under a name nobody
chose and nobody will look for; the reason says which character was the problem instead.

---

## What it does that `ffmpeg -ss S -to E -c copy` cannot

H.264 and HEVC frames are deltas against earlier frames, so a stream copy can only begin at a
keyframe. On a master with a 250-frame GOP, an in point that is not on a keyframe makes `-c copy`
start up to ten seconds early.

TheTrimmer splits the range at the **first keyframe at or after the in point**:

| Part | What happens to it |
|---|---|
| The head — in point to that keyframe | re-encoded at CRF 18, then spliced in front |
| The body — that keyframe to the out point | **the original packets, copied untouched** |

So a two-hour master loses a couple of seconds of generation instead of two hours, and the job takes
seconds instead of an hour. When the in point already lands on a keyframe, the whole segment is
copied and nothing is re-encoded at all. Only when there is no keyframe anywhere inside the range
does it re-encode the segment — and it says so before it starts.

## All-intra sources are copied in full

An editing codec has no such problem to solve: every frame of ProRes or DNxHD stands on its own, so
there is no keyframe to wait for and nothing to patch at either end. Those sources are copied from
the first wanted packet to the last one and **nothing is re-encoded at all** — the segment is
lossless in the strict sense, not "the original packets except at the ends", and their uncompressed
sound stays uncompressed rather than picking up a generation of AAC on the way through.

| Source | What a cut does |
|---|---|
| H.264, H.265/HEVC | re-encodes the run to the opening keyframe and the run from the last keyframe before the out point; copies everything between |
| ProRes, DNxHD | copies the whole range; re-encodes nothing |
| anything else | refused, with a sentence naming the codecs that are supported |

## Verification

A finished cut is measured against its own source rather than assumed correct. Each run reports:

* the frame count and duration against what was asked for, including any trailing hold;
* the audio's length against the picture's;
* whether the copied body is **byte-identical** to the source (frame MD5s) — every copied frame, not
  a sample of them — or, when the segment had to be re-encoded, that the picture shows the frame the
  mark names, with the SSIM of the two frames compared;
* a verdict for every check, shown in the interface beside the number that was measured. A check
  that could not be made says **not checked**, which is a different answer from passing and from
  failing.

Measuring is a setting, not a cost imposed on every cut: with `Verify` set to *Off* the cut is made
and nothing is measured, and a 2009-frame range takes about seven seconds instead of twenty-six.

`docs/TRUTH.md` is the record of how each claim is checked, and of what is deliberately not checked.

---

## How it is built

| Part | What it is | Where |
|---|---|---|
| Engine | Python 3: the cutting, the planning, the verification | `backend/trimmer/` |
| Backend | aiohttp on `127.0.0.1:8765`, one job at a time, progress over server-sent events | `backend/server.py` |
| Window | Electron 33, maximized, native titlebar | `electron/` |
| Interface | React + TypeScript + Vite, built to `frontend/dist` | `frontend/` |

The **engine serves the interface**. The page and the API are therefore one origin, which is why
there is no CORS configuration anywhere and why the window needs no second API of its own. Electron
starts the Python process, waits for `/api/health` to answer, and then loads the page from it.

The window is deliberately thin. It answers only the questions a page cannot answer for itself: the
file dialog, showing a finished file in Explorer, and fullscreen. Everything else is HTTP.

### Colour, type, and the frame

Colour is **state, never decoration**: one near-white primary control with no hue, and four semantic
hues that each mean something. Inter for language and JetBrains Mono for data, both bundled, so the
window renders identically on a machine with no fonts installed. The frame does not scroll — only
the log and the proof panel scroll inside themselves, because only those can hold an unbounded
number of rows. There are no viewport media queries; this is a desktop application and its window
has a minimum size.

---

## Tests

```powershell
venv\Scripts\python.exe -m pytest backend\tests -q      # 131 tests: units + every scenario, ~2s
npm --prefix frontend test                              # 20 tests: units + the interface's scenarios
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-bootstrap.ps1
                                                        # 10 checks: installs Node and ffmpeg for real
```

The engine's scenarios are Gherkin in `specs/features/` and run under `pytest-bdd`; the
interface's are the same, under a small vitest runner. Each covers a place where being wrong is
silent and expensive — drop-frame timecode at every rate, which of the three cutting methods a
range gets and why, counting frames on the grid a file is *actually* on, the verdict rules the
proof panel draws, the HTTP contract's refusals, and that a named segment's captions carry its
name.

`scripts/check-bootstrap.ps1` is the exception to the rule below, and has to be: it downloads a real
Node and a real ffmpeg, unpacks them, and runs them. The provisioning path is the one thing in this
project that a test *must* exercise for real, because it is the only thing that runs on a machine
nobody has seen.

**No test runs a cut.** The engine's behaviour on real footage is verified by using the
application, which is what the proof panel is for.

---

## Documentation

* [`docs/SPEC.md`](docs/SPEC.md) — what the product is required to do, as numbered requirements,
  each with the scenario that decides whether it holds, and a traceability table.
* [`docs/TRUTH.md`](docs/TRUTH.md) — every claim, and the mechanism that checks it.
* [`docs/DESIGN.md`](docs/DESIGN.md) — the decisions that were not obvious, and the failures behind
  them.
* [`docs/SKILLS-APPLIED.md`](docs/SKILLS-APPLIED.md) — the practices this build was held to.
* [`specs/features/`](specs/features/) — the executable behaviour specification.

## Licence

The bundled fonts are SIL OFL 1.1; see `frontend/src/fonts/LICENCE.md`.
