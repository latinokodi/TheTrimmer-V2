# Design decisions

The decisions that were not obvious, and the failures that produced them. Every one of these was
measured on a real machine rather than reasoned about in the abstract; the numbers are in the text.

---

## 1. Cut with a re-encoded head and a copied body

`ffmpeg -ss S -to E -c copy` cannot start on an arbitrary frame. H.264 and HEVC frames are deltas
against earlier frames, so a stream copy begins at the last keyframe at or before the in point. On
the masters this was built for — a 250-frame GOP — that is up to ten seconds early, and the operator
gets a file that starts before the mark with no error printed anywhere.

**Decision.** Split the range at the first keyframe at or after the in point. Re-encode the head, then
splice in the original packets from that keyframe to the out point.

**Consequence.** A two-hour master loses a couple of seconds of generation instead of two hours, and
the job takes seconds instead of an hour. The cost is one extra ffmpeg pass and a temporary file.

## 2. The body's picture and sound are copied in separate passes

A single `-c copy` over both streams lets the muxer choose an interleaving, and the result can drift
against the head by a frame or two.

**Decision.** Copy the picture and the sound as two passes and join them with the concat demuxer.

**Consequence.** Two more passes over the body, and a `concat_offset` knob for the cases where a
source's own timestamps carry drift.

## 3. The timescale is pinned to the source's

The worst defect in V1, and the one that cost the most time: ffmpeg rescales a copied body when the
output's timescale differs from the source's, turning it into slow motion **while exiting 0**. Nothing
in the output says anything is wrong; the file is simply wrong.

**Decision.** Pass `-video_track_timescale` set to the source's own value on every encode, and put the
exact command line in the log so the pin is visible after the fact.

## 4. The copy is bounded by time, never by `-frames:v`

`-frames:v` counts packets in **decode** order. With B-frames that drops a frame the operator asked
for and keeps one they did not, and the count comes out right — which is what makes it so hard to see.

**Decision.** Bound every pass with `-t`, and let the count be a measurement rather than a control.

## 5. Overshoot is a warning; a missing frame is a failure

A stream copy can only end on its own packet boundary, so a cut can run a frame or two past the out
point. Treating that as an error would make the product refuse correct work.

**Decision.** Record it, show it in the proof panel with the number, and never fail on it. A *frame
count that is short* is still a failure.

## 6. Alignment is measured with frame hashes, on the file's own frame grid

"Lossless" and "looks the same" are different claims, and only one of them can be checked by eye.

**Decision.** Hash frames of the copied range in the output and the same frames in the source; they
must be identical. When the segment had to be re-encoded there is nothing to hash against, so the
check becomes a frame-for-frame SSIM comparison and the report says which of the two it did — and
why.

## 7. The domain core is the V1 engine, kept whole

V2 began as a rewrite of the engine into a pure, separately tested core, verified against V1 as an
oracle. That work is gone from this tree — see decision 11.

**Decision.** The engine is the V1 Python, adopted verbatim. It *is* the oracle the rewrite was being
checked against, so adopting it removes the entire class of "the new engine disagrees with the old
one" risk rather than managing it.

**Consequence.** The engine's behaviour is byte-for-byte V1. The only changes are the progress
plumbing: a `Ticks` record and a `progress` callback threaded through each pass.

## 8. Python and Electron, not Rust and Tauri

The Rust rewrite was measured against the operator's own working practice and lost. It could not be
run without building an executable, a compile sat between an edit and a test, and the interface could
not be looked at while the core compiled.

**Decision.** The stack the rest of the shop uses: an aiohttp backend in Python, an Electron window,
a React interface built by Vite.

**Consequence.** Editing a Python file and pressing `npm run dev` is the whole loop. `start.bat` needs
no compiler on the machine.

## 9. The engine serves the interface

Loading the built page with `loadFile` is the obvious thing and it cannot work. A Vite build is an ES
module, and Chromium refuses a module script from `file://` — the origin is opaque, so it fails CORS
before a line of the application runs. Even had it loaded, every call to the engine would have been
`file://` → `http://127.0.0.1:8765`, which is cross-origin too, and the page would have needed CORS
headers on every response to talk to its own backend.

**Decision.** One origin: the engine serves `frontend/dist`, Electron waits for `/api/health` and then
loads that URL.

**Consequence.** No CORS anywhere, no second API in the shell, and the window keeps only what a page
genuinely cannot do — the file dialog, revealing a file in Explorer, and fullscreen.

## 10. Colour is state, never decoration

**Decision.** One near-white primary control with no hue, and four semantic hues that each mean
exactly one thing (`--verified`, `--caution`, `--hazard`, `--select`). Inter for language, JetBrains
Mono for data, both bundled under SIL OFL 1.1 so the window renders identically on a machine with no
fonts installed. No viewport media queries: this is a desktop application with a minimum window size,
not a responsive page. The frame never scrolls; only the log and the proof panel do, because only
those hold an unbounded number of rows.

**One theme, and it is dark.** A light value set was built, with every semantic colour re-derived
rather than inverted, and reachable from a switch in the title bar. It was removed. A grading suite is
dim, and a bright surface beside a video monitor destroys the operator's adaptation to the picture —
so a light theme is not a preference, it is a way to judge the picture wrong. It also doubled the
argument for every one of the four status hues, because contrast is not symmetric and each colour
needed a second value and a second set of figures.

**`color-scheme: dark` is part of the palette.** Without it the native `<select>` popups and the
scrollbars render light against a dark window, and the dropdown is the one control that leaves the
page's own styling behind.

## 11. A length token and a colour token must never share a name

This is the most instructive failure in the migration, because nothing reported it and no test
noticed it. The token file declared `--rule: 1px` in its geometry section and `--rule: #2b333b` in its
colour section. The later declaration won, so every `border: var(--rule) solid …` in the stylesheet
resolved to `border: <a colour> solid <a colour>` — invalid at computed-value time and therefore
dropped. `border-style` then fell back to `none`, **which forces the used width to zero**, so
twenty-six declarations across the interface quietly rendered no edge at all.

What a person saw was the window's single action — `Trim` — with no border and a dark fill on a dark
panel, reading as text lying on the background, and the report was exactly that: *"the Trim button
just looks like floating text."* The primary button was never wrong; the state they were looking at
was the disabled one, and the disabled state is the default state of a window with nothing loaded.

**Decision.** `--hairline` is the only width token. The colour family is `--rule`, `--rule-hair`,
`--rule-strong`. The two families are named apart, and the reason is written next to both.

**The lesson, stated generally:** a check a fixture can satisfy is not a check, and a *stylesheet* a
browser can parse is not a stylesheet it can apply. The only thing that found this was reading
computed styles out of the running window and seeing `0px none` where a border was meant to be.

## 12. The window subscribes to a run once

`useRunLog` returned a fresh object literal on every render. The window's effect that opens the event
stream depended on that object, so every render closed the stream and opened a new one — several
times a second during a cut. Each new subscriber is replayed the backend's recent history, so the log
filled with its own past, and the backend was left holding hundreds of half-closed sockets. The
backend's log was a wall of `ConnectionResetError`.

**Decision.** The returned object is memoised, and every callback in it is stable.

**The lesson:** an identity that changes every render is a subscription that re-subscribes. React
effects key on identity, and "it is the same data" is not "it is the same object".

## 13. A plan describes one range

The plan arrives about a quarter of a second after the marks stop changing, so for that quarter
second there is an answer on screen to a question that is no longer being asked: an old plan beside
new marks, and an enabled `Trim` that would cut against it.

**Decision.** Each plan carries a signature of the marks it was asked about. The range line and the
button both require that signature to match what is in the fields.

## 14. The window opens maximized, not fullscreen

A borderless fullscreen window has no titlebar, and a window with no titlebar cannot be restored,
minimized or closed. It shipped that way once: `GetWindowLong` on it returned
`WS_VISIBLE | WS_CLIPCHILDREN` and nothing else — no caption, no system menu, no minimize box, no
maximize box.

**Decision.** `maximize()`, which fills the screen and keeps all of them, and F11 as an explicit,
reversible alternative.

## 15. No licensing feature, no installer, no updater

All three were considered and declined. There is no licence check, no activation, and no automatic
update. `start.bat` is the distribution for now; `npm run dist` exists but is unexercised.

## 16. Nothing is claimed that is not checkable

Every module in this tree carries a docstring explaining *why* it exists in the terms of the failures
it prevents, and `docs/TRUTH.md` pairs each claim with its check. A claim nobody can test is removed
rather than softened.
