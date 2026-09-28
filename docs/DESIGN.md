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

## 17. A frame's time comes from the file, not from the rate it claims

The defect that survived the longest, because it only shows on real masters and only by a frame.

`r_frame_rate` is what a file *claims*. `avg_frame_rate` is what it *is*. The reference master
reports `30/1` and averages `156630000/5221099` — 29.99943. The two rates are 0.0019% apart, which
passes every sanity check anyone would write, and across 52210 frames they diverge by **exactly one
frame**. The two grids cross somewhere in the middle of the file, so a mark is a frame out in one
part and exactly right in another. No threshold on the rate ratio catches that; the quantity that
matters is the **accumulated** drift, and it has to be measured over the file rather than per second.

**Decision.** `MediaInfo.grid_rate` is the file's own grid — the container's exact `avg_frame_rate`,
or the frame count over the file's own span when the container will not say — and every
frame-to-time and time-to-frame conversion uses it. `rate` survives for what it is actually for:
timecode labels, which count at the nominal rate because that is what a timecode is.

**Consequences.** A constant-rate file has the two rates identical and nothing changes. A drifting
one is converted on the grid it has, so the accumulated error goes to zero. And because the drift is
now measured (`grid_drift`, in frames) rather than merely suspected, the plan can say *"its
timestamps are 0.99 frame(s) away from that grid by the end"* instead of the vague warning it used
to print — which is the difference between an operator knowing a failed check means the cut moved
and knowing the file has no grid to be exact against.

The same measurement found two more faults in the same area:

* A frame's presentation time is `start_time + n / grid_rate`. The bare `n / rate` ignored a
  `start_time` of 0.021 s — 0.63 of a frame — so the re-encoded head began one frame early.
* A body copy aimed at a keyframe's own timestamp makes ffmpeg take the keyframe **before** it. The
  copy came out a whole GOP long (8.33 s on this master) with its content a whole GOP early, which
  is why the picture ran ahead of the sound. `body_seek` aims inside the GOP instead and takes the
  preroll — now a known quantity — off the copied length.

## 17a. The body copy reads packets, because ffmpeg's own copy cannot count

A stream copy bounded by `ffmpeg -ss … -t … -c copy` stops when the timestamp it is watching passes
the length it was given, and its answer moves in steps of a whole packet group. That was measured
rather than assumed:

* On the 60 fps master, shortening the request by **5 ms** moved the stop by **15 frames** — where a
  frame is 16.7 ms, so five milliseconds should have moved it by a third of one.
* A request for 189 frames came back with **191**; a request for 19250 came back 19252, then 19249,
  then 19251 as the length was corrected. It never lands on the number asked for, so no arithmetic
  can make it. The delivered file was a frame long and the seam against the re-encoded tail was a
  frame out.

**Decision.** The body — the one region that must be the source's own packets — is written by
reading the container with PyAV and muxing exactly the frames the plan names. Measured on the same
range: 189 packets in, 189 frames out, every one byte-identical to the source.

This is the architecture the two reference implementations use, and for this reason. `avcut`
re-encodes the GOP either side of a cut point; `smartcut` cuts at the packet level and re-encodes
only around the cut points. Both are doing what the head and tail patches do, and both had to reach
below the command line to do it.

**Consequences, stated plainly.** The engine now depends on PyAV, which brings its own ffmpeg
libraries (27.6 MB wheel, no compiler, wheels for all three platforms) alongside the system `ffmpeg`
binary the re-encodes still use. Two copies of the same libraries on disk is the price of an exact
frame count. The corpus is no longer "one dependency"; it is one dependency and one binary.

## 17b. A frame's time comes from the container, not from the average rate

`seconds_of(frame)` computes `start_time + frame / grid_rate`, and `grid_rate` is the container's
*average* rate. On a master whose frames do not sit exactly on that average — the reference source's
frames step by 0.0333 s while its average rate, stretched over 1872.633 s, implies a very slightly
larger step — the computed time drifts ahead of the frame it names. By frame 8596 it was 0.28 ms
past, which is enough for `-ss` aimed there to land **inside the next frame**.

The symptom was a head that started exactly one frame late on a 31-minute master and was exact on a
10-minute one: the error grows with the frame number, so only a long file shows it. The product had
been reporting the drift as a warning on the plan and then seeking by the computed time anyway.

**Decision.** A seek is aimed at the time the container states for the frame, read once per file by
`FrameTimes` and kept. The computed time remains the fallback for a source that reports no
timestamps, so nothing that worked before stops working. The verifier's own window extraction uses
the same reader: it had the same drift, and it was reporting a correct cut as one frame late —
a checker failing the file it was checking.

**And the reader was wrong for longer than the writer.** This was found by asking it, on the
reference master, for frames whose time the container's own packet index already knew — and
comparing the two answers. For frame 162241 it answered 5400.700 where the container states
5408.033: **222 frames and 7.3 s out**. Frame 162900 came back 131 frames early, frame 8596
forty-two early. Only the luckiest frames got a usable answer.

Two mistakes, both in the same ten lines. `-read_intervals` **seeks, and a seek lands on the
keyframe at or before the time asked for**, not on the time — so a 32-frame window opened after
that keyframe and closed long before the frame wanted, whose GOP is 219 frames on this master. And
a frame is **not** the one whose stated time is nearest the computed guess: on a master off its
average-rate grid a B-frame one position along has a later presentation time, so a request for
frame 162000 was answered with the time of the row two along. The reader then returned the nearest
thing it had, with no way for the caller to tell it was not the frame asked about.

**Decision.** The window is wide enough to survive the seek (480 frames), the answer is selected by
the numbering the rest of the engine uses — the frame whose time is `round((t - start) * rate)` —
and when the window holds no such frame the reader returns **nothing** rather than something close.
Nothing is lost by refusing: the caller falls back to the computed time, which is exactly what it
did when the probe failed before. A wrong answer passed off as a right one is the one outcome that
is not survivable, and it is the one that was happening.

## 18. The log is read at its top, so the newest line is at its top

The log is the record of a cut, and the line being read is the one that just happened. It used to
append downwards and scroll itself to the end, which meant the newest line sat at the bottom of a
box taller than the window, and the top — where a reader looks first — held the oldest line in the
run. Scrolling it into view is not the same as putting it where it is looked for.

**Decision.** The lines are rendered newest first, the scroll position is pinned to the top, and the
ordering is a pure function (`newestFirst`) with a scenario behind it rather than a `.reverse()` in a
component. The clock column therefore counts down the list, which is the right way round for a record
of what just happened.

## 19. A grid track sized to its content will collapse the one beside it

The proof panel's facts rows are a two-column grid. It was written as
`grid-template-columns: minmax(0, 1fr) auto` — label, then value — and that is the wrong way round
for the values this panel holds.

An `auto` track will not go below its content's **min-content** width, and a file path is one long
unbreakable word. So the value track grew to the path's full width, the label's `minmax(0, 1fr)`
track was squeezed to nothing, and `MEASURED AGAINST` was painted across the top of the path beside
it. Nothing reports that: both elements are exactly where the layout put them.

**Decision.** The label column is `max-content` and the value column takes the remainder, so a label
always has room for its own words and `text-overflow` ellipsises the path as intended. Paths get
their own modifier that lifts the 340 px cap the numeric rows use, because a number and its label
should not be a screen apart but a path is read left to right and is not a number.

**The lesson, and it is the same one as §11:** a stylesheet a browser can parse is not a stylesheet
it can apply, and a layout that overlaps is not an error to anything. Both faults were found by
measuring the running window — `0px none` where a border should have been, and a label's right edge
past a value's left edge — and by nothing else.

## 20. Nothing is claimed that is not checkable

Every module in this tree carries a docstring explaining *why* it exists in the terms of the failures
it prevents, and `docs/TRUTH.md` pairs each claim with its check. A claim nobody can test is removed
rather than softened.

## The two frame grids, and how to tell which one a number is on

The container numbers frames by **packet order**: the Nth video packet is frame N. A mark is
numbered on the rate the file claims. These are different grids and their offset is not constant
-- measured, +1 on Joseph Chalom and +2 on TY Gellasch and Andy Ross -- so neither can be derived
from the other by arithmetic.

Everything that *reports* a span now reports it in rows, and says so, because the two grids in one
log read as a fault that is not there:

    head  re-encoding frames 7514..7741     <- frame numbers
    body  copying frames 7743..9492         <- rows
    tail  re-encoding frames 9493..9523

Row 7742 is in neither, which reads as a missing picture. The same cut in rows joins end to end:

    head  rows 7515..7742
    body  rows 7743..9492
    tail  rows 9493..9523

`TrimPlan.rows` is the single source for those spans. When reading a log, check the units before
believing a gap.

## The sound is level with the picture, and how that was settled

An earlier reading of the delivered files showed the sound 15.7 ms from the picture. That was a
comparison of declared stream start times, which include AAC encoder priming samples, and it was
recorded here as a defect on the strength of that number alone.

Correlating the *content* settles it. Taking a window from the delivered file, finding it inside
the source, and comparing where the sound landed against where the picture landed:

    delivered 10 s -> +0.0 ms      delivered 30 s -> +0.0 ms      delivered 55 s -> +0.0 ms

There is no offset to fix. A declared stream start time is not a synchronisation measurement; a
seek, an encode, or a container's edit list each move it without moving a single sample of audio.

## A segment's name is not a second naming rule

A name that is typed has to reach three things: the segment, the caption file, and the folder they
may be asked to arrive in. The temptation is to give each one its own rule — replace the stem for
the video, replace the suffix for the captions, make a folder and join the paths — and that is
three chances for the three to disagree. It is also exactly how a caption file comes out called
after the source: the video path is rebuilt correctly, and the `.srt` is written by a different pass
from the source's own name because nobody told it otherwise.

**Decision.** There is one function, `output_for`, and one rule: the transcript is
`output.with_suffix(".srt")`. Naming the segment names the captions because the captions have never
had a name of their own. The folder toggle changes the output's parent, and that single change moves
both files, because both are derived from the same path.

The rule is checked as a relationship rather than as two expectations. `test_naming.py` writes real
subtitle files and reads them back, and asserts that the caption `stem` equals the segment `stem` and
the caption `parent` equals the segment `parent` — in a plain folder and in a named one. A test that
asserted two literal paths would pass while the relationship was broken in the third case nobody
thought of.

**A name Windows would refuse is refused, not repaired.** Trimming `Take 1/2` to `Take 12` produces
a file that exists, under a name nobody chose and nobody will look for — and the person is not
looking for it, because they typed something else. So the character is named and the request
refused. A trailing *space* is trimmed instead, because Windows drops it anyway and `Take 1 ` is a
slip rather than a different name; a trailing *dot* is refused, because Windows also drops that, and
accepting it would write a file whose name is not the one that was asked for. That asymmetry is
deliberate and is the only subtle thing in the feature.

The refusal happens in `_spec_from`, before the source is probed: a person who mistyped a colon
should not wait for an 11 GB master to be read to be told about the colon.

### A state nobody named is a state nobody handled

The first version of the row derived the path from the **plan**. The plan does not run until both
marks are set. So typing a name with no range yet left the path empty, and the row read an empty
path as *"the engine refused this name"* — reporting every name as unusable, next to a message that
said "see the reason below" and a reason that was printed nowhere.

Two faults, and only one of them was the false refusal. The other was that the window was deciding,
on the engine's behalf, that something had been refused. It is not entitled to: it knows what it
asked and what came back, and "no answer yet" is a third thing.

**Decision.** Three states, in `frontend/src/state/nameRow.ts` rather than in the markup, with tests
of its own: nothing typed, answered, or refused. Only a refusal the engine *tagged* — `reason:
"name"`, from a `NameRefused` — is drawn as a problem, so the window cannot blame a name for an
engine that is simply down. And the path is resolved by its own endpoint, `POST /api/name`, which
reads nothing: a named segment's path is the source's folder and the name, so it needs neither the
marks nor a probe. That is what makes the field answer while somebody is typing, and on a master
sitting on a disconnected drive.

`NameRefused` is a subclass of `TrimError`, so every caller that catches the general refusal keeps
working; the tag exists only so the window can put this one in the right place.

## Provisioning is a script, and `start.bat` is a launcher

The first version of `start.bat` did the work itself, and it did the one thing that makes a start
script useless on a machine nobody has seen: it checked for Python and Node, and when they were
missing it printed a link and exited. It also never checked for ffmpeg at all, which the engine
shells out to for every probe and every encode.

**Decision.** The work moves to `scripts/bootstrap.ps1`, and `start.bat` becomes a launcher that
starts PowerShell with `-ExecutionPolicy Bypass`. Batch cannot download over TLS, cannot unpack an
archive, and cannot compare version numbers — three things this job is made of. PowerShell 5.1 is
part of every supported Windows, so requiring it costs nothing; requiring PowerShell 7 would have
repeated the original mistake in a new costume.

Four things are looked for and installed only when absent: Python 3.10+ (winget, else the official
installer per-user), Node 18+ (winget, else the official archive unpacked into `.tools\node`),
ffmpeg and ffprobe (a portable build unpacked into `.tools\ffmpeg\bin`), and then the project's own
packages. Nothing needs an administrator and nothing is written outside this folder and
`%LOCALAPPDATA%`.

Two details are worth more than they look. A running process never observes a `PATH` change made
while it runs, so everything installed here is located *by path* afterwards rather than by name —
which is also why the engine reads `THE_TRIMMER_FFMPEG` before it reads `PATH`. And the installers
report the binary they unpacked rather than asking the locator about it: the locator is also the
function a caller turns to when it has found *nothing*, so an installer reporting through it can
answer "nothing" about a job it completed perfectly. That is not hypothetical — it made the script
fetch a second 190 MB ffmpeg archive for a tool it had already installed.

The script is dot-sourceable and `scripts/check-bootstrap.ps1` uses that: it replaces the locators
with stubs that report nothing found — the state a clean PC is in — and calls the installers for
real, then puts the locators back and requires them to find what was installed. It checks that the
unpacked ffmpeg has every encoder this engine asks for, because a build missing `libx264` or the
concat demuxer fails on a real cut rather than at startup, and it carries the licences with the
binaries. Every one of those checks has already caught a real defect in this file.
