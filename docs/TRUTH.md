# How each claim is checked

Every statement the product makes about itself is listed here with the mechanism that checks it. The
point is that a claim and its check are the same size: a sentence nobody can test is removed rather
than softened.

```powershell
venv\Scripts\python.exe -m pytest backend\tests -q     # 34 tests, about a second
npm --prefix frontend test                             # 12 tests, about a second
npm --prefix frontend run build                        # types, then the bundle
```

---

## The cutting claims

| Claim | Checked by | What the check would catch |
|---|---|---|
| The body of a head-patch cut is **the original packets, not a re-encode** | `verify.py`'s alignment check: frame MD5s of the copied range in the output are compared against the same frames in the source | A re-encode cannot pass. This is the assertion that separates "lossless" from "looks the same" |
| Only the head is re-encoded | `plan_trim`'s split arithmetic, and `TrimPlan.describe` in the log | A plan that re-encoded more than the head shows up as a `headpatch` split wider than the distance to the keyframe |
| A range with no keyframe in it is re-encoded **whole**, and said so before it starts | `test_plan.py::test_a_range_with_no_keyframe_in_it_is_re_encoded_whole`, plus the note carried on the plan and shown in the range line and the proof panel | Silently re-encoding a segment the operator believed was a copy |
| An impossible request is refused before ffmpeg is launched | `test_plan.py`: backwards range, negative in point, out point past the end, source codec that cannot be patched | A cut that starts, runs for a minute, and then fails |
| Nothing runs past the out point without saying so | The `overshoot` figure in the outcome, and the note in the proof panel | Overshoot is a *warning* and never a failure — a stream copy ends on its own packet boundary — but it is always recorded |
| The timescale is the source's own | `-video_track_timescale` is pinned to the source's in `trim.py`, and the exact command line is in the log | A wrong timescale rescales the copied body into slow motion **while ffmpeg exits 0**. This is the defect that cost the most time in V1 |
| Frames are counted on the grid the file is **actually** on, at any rate | `MediaInfo.grid_rate`, which is the container's own `avg_frame_rate`; `test_plan.py::test_frames_are_counted_on_the_grid_the_file_is_actually_on` | The reference master claims `30/1` and averages `156630000/5221099`. Counting its frames at 30 puts them a whole frame from where they are by the end, so a mark is a frame out in one part of the file and exact in another — and the alignment check failed on cuts that were right. Verified on that master: every alignment sample frame-exact, both ranges |
| A body copy begins on the keyframe it was aimed at | `body_seek`, which aims inside the keyframe's GOP and takes the preroll off the copied length | Aimed at a keyframe's own timestamp, ffmpeg takes the keyframe *before* it: the copy comes out a whole GOP — 8.33 s on the reference master — too long, its content a whole GOP early, and the picture 8.33 s ahead of the sound |
| A file whose timestamps accumulate a frame of drift says so | `MediaInfo.grid_drift`, and the plan's note, which carries the measured figure | A rate comparison misses it: the reference master's rates are 0.0019% apart. The note says "0.99 frame(s) away from that grid by the end", which is what tells an operator whether a failed check means the cut moved or the file has no grid to be exact against |
| The alignment check reports the offset the content sits at | `measure_offset` scans outward from the expected position | A window of near-identical frames — a locked-off shot, a title card — matches at several offsets, and taking the earliest hit biased every ambiguous measurement negative: a frame of drift reported for a cut that was frame-exact |
| The frame count is bounded by time, never by `-frames:v` | The command lines in `trim.py`, which pass `-t` and never a frame count | `-frames:v` counts packets in **decode** order, which with B-frames drops a frame that was asked for and keeps one that was not |

## The verification claims

| Claim | Checked by | What the check would catch |
|---|---|---|
| A finished cut is measured against its source | `verify.verify` returns a `VerifyResult` the interface draws as a table; the outcome carries `verified` | A cut that shifted is reported rather than delivered |
| The verdict is three-valued, and the interface never collapses it | `ProofPanel`: **passed**, **failed**, **not checked** — each with a word as well as a colour | "We did not look" and "we looked and it was right" must not look alike |
| Switching verification off is visible | The `Verify: Off` option leaves `verified: null`, which the panel shows as *not measured* | A run that measured nothing being presented as a verified one |
| A variable-rate source is planned anyway and warned about | `test_plan.py::test_a_variable_rate_file_is_planned_anyway_and_says_so` | No check can make a mark exact against a frame grid that does not exist, so the honest answer is a warning beside a plan that still works |

## The timecode claims

| Claim | Checked by | What the check would catch |
|---|---|---|
| Drop-frame is read and written correctly at every rate | `test_timecode.py`: the 108-frame case, the skipped labels, and a round trip at five rates over eleven frame numbers each | The 108-frame-per-hour error that a naive reading of `01:00:00:00` produces on 29.97 |
| `29.97` is not silently treated as `30000/1001` | `parse_rate`'s fraction branch, since ffprobe writes 29.97 material as the fraction; `test_timecode.py` asserts both spellings and documents why the decimal one stays a decimal | Precision the text does not carry. The fraction branch is what makes a real file exact |
| A timecode naming a frame its rate does not have is refused | `test_timecode.py::test_a_frame_that_this_rate_cannot_name_is_refused` | `00:00:00:25` at 25 fps silently resolving to the next second |
| Drop-frame on a rate that has no drop-frame form is refused | The same file, `::test_drop_frame_on_a_rate_that_has_no_drop_frame_form_is_refused` | Rendering `00:00:00;00` at 25 fps would shift every stamp by two labels a minute while looking perfectly plausible |

## The interface claims

| Claim | Checked by | What the check would catch |
|---|---|---|
| Every control is on screen without scrolling | Measured in the running window: the document's `scrollHeight` never exceeds its `clientHeight` | A PC-only application whose frame scrolls is content falling off the bottom of the window |
| The frame never scrolls; the log and the proof panel do | `.app` is a fixed grid; `.log` and the panel body carry `overflow: auto` | An unbounded list pushing the controls off the screen |
| No label is clipped | Measured in the running window: for every note, `scrollWidth <= clientWidth` | A sentence cut mid-word — "applies to the head o…" |
| No label is drawn over the value beside it | Measured in the running window: the fact rows' real markup is injected with a path as long as the panel shows, and each label's right edge is compared with the value's left edge | A facts grid whose value track is `auto` grows to a long path's full width, collapses the label's track to nothing, and paints `MEASURED AGAINST` across the path. Nothing reports an overlap: both elements are where the layout put them |
| The log shows the newest line first | `window.feature:the log shows the newest line first`, over the pure `newestFirst` | A log that appends downwards puts the line being read below the fold and the oldest line where the eye lands |
| Colour is state, never decoration | One near-white primary control with no hue, and four semantic hues | A status colour competing with the button that starts the work |
| A length token is never a colour token | `--hairline` is the only width token; the colour family is `--rule*`. Measured in the window: every control reports a non-zero border | `border: var(--rule) solid …` where `--rule` is a colour is invalid at computed-value time, so `border-style` falls back to `none`, which forces the used width to zero. Twenty-six declarations rendered no edge at all, including the primary button's entire default state |
| The status of the run is legible while it works | The progress strip draws ffmpeg's own `out_time_us` against the pass's expected length, its throughput, and a Cancel button | A long cut with nothing moving on screen |
| The window is restorable, minimizable and closable | `BrowserWindow.maximize()`, **not** `setFullScreen`. F11 toggles fullscreen and is reversible | A borderless fullscreen window cannot be restored, minimized or closed. It was shipped that way once |

## The engineering claims

| Claim | Checked by | What the check would catch |
|---|---|---|
| The page and the engine are one origin | The engine serves `frontend/dist`; Electron waits for `/api/health` and then loads that URL | Loading the built page from `file://` cannot work: Chromium refuses a module script from an opaque origin, and every call would be cross-origin besides |
| The window subscribes to the run once | `useRunLog` returns a memoised object, so the effect that opens the event stream does not re-run on every render | A fresh object per render tore the stream down and rebuilt it several times a second; each new subscriber was replayed the recent history, so the log filled with its own past |
| No shell is ever used | Every ffmpeg invocation is an argv list built in `ffmpeg.py` | A file name becoming syntax |
| The engine is reachable only from this machine | `web.run_app(..., host="127.0.0.1")`; the static handler refuses any path outside the build | An API that cuts files being exposed to a network, or serving a file outside `frontend/dist` |
| Nothing is downloaded at run time | Fonts are bundled; the page's CSP is `default-src 'self'`; `connect-src` names only the loopback engine | A silent remote dependency in an offline product |
| The event stream survives a slow window | `Hub` uses `put_nowait` on a bounded queue per subscriber and drops for a dead one | A closed browser tab blocking ffmpeg behind it |
| Exactly one cut at a time | `CURRENT` under a lock; a second request is a `409`, checked in `test_server.py` | Two ffmpeg processes fighting for one disk |
| A cancel is a decision, not a fault | The worker catches `ff.Cancelled` and publishes `cancelled`; the interface logs a sentence rather than an error | A cancelled run being reported as a failure |
| The tests run anywhere | `test_server.py` uses no media at all, and the plan tests replace the one keyframe lookup with a fixture | A suite that only passes on a machine with a fixture on disk |

---

## What is not checked, and why

Stating these is the point of the document.

* **Whether a cut is right on real footage.** The test suite never runs a cut — by design, and at the
  operator's request. A test that drove one would take seconds, need a fixture on disk, and still not
  be what finds a wrong frame. That judgement belongs to the proof panel and to a person looking at
  the result.
* **Variable-frame-rate sources.** The method assumes a constant frame grid. Such a source is detected
  (`MediaInfo.variable`), warned about in the plan's notes and in the range line, and trimmed anyway —
  but the marks are approximate and no check can make them exact, because there is no grid to be
  exact against.
* **The interface's pixel layout beyond the measurements above.** There is no screenshot regression
  suite. The layout was confirmed in the running window during the migration; a later stylesheet
  change is reviewed by looking at it.
* **`npm run dist` has not been run.** There is no installer and no automatic updater, both by
  decision, so the packaging configuration in `package.json` is unexercised.
* **Windows only.** The engine is portable and the shell would need a small change for another
  platform, but nothing else has been tried.
