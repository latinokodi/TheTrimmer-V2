# Specification

What TheTrimmer is required to do, and how each requirement is verified. This is the source of
truth for behaviour: code that does not serve a requirement here is either a defect or a
requirement nobody wrote down, and the second is the more expensive of the two.

**How to read it.** Each requirement has an id, a statement a person can judge, and the checks
that decide whether it holds. The checks are executable — the scenarios in `specs/features/` run
under `pytest-bdd`, and the Python tests beside them — so a requirement cannot quietly stop being
true.

```
venv\Scripts\python.exe -m pytest backend\tests -q     # unit tests + every scenario
npm --prefix frontend test                             # the interface's own scenarios and units
```

**What this document is not.** It does not describe how any of it is done. `docs/DESIGN.md` holds
the decisions and the failures behind them; this holds what the product promises.

---

## 1. Cutting

The product exists because a stream copy cannot start on an arbitrary frame. Everything in this
section follows from that one fact.

| Id | Requirement | Verified by |
|---|---|---|
| **R1.1** | A range whose in point is a keyframe copies its body with nothing re-encoded at the front. | `cutting.feature:a range that begins on a keyframe re-encodes only its far end` |
| **R1.2** | Both ends of a range are re-encoded — the run to the keyframe that opens the body, and the run from the last keyframe before the out point — and every frame between them is copied untouched. | `cutting.feature:a range that begins between keyframes re-encodes both ends` |
| **R1.3** | A range containing no keyframe is re-encoded whole, and the plan says so **before** anything is written. | `cutting.feature:a range with no keyframe in it is re-encoded whole, and the plan says so first` |
| **R1.4** | The copied body holds **exactly** the frames the plan names, taken by frame index from the container's packets, and the copy is not bounded by a span of time. | `cutting.feature:a body copy takes exactly the frames the plan names, by index` |
| **R1.5** | The output's video timescale is the source's own. | `cutting.feature:the output is written on the source's own timescale` |
| **R1.6** | A pass that copies the source's packets is never bounded by a frame count; a pass that re-encodes is bounded by the frame count the plan names. | `cutting.feature:a re-encoded piece pins its own length, because nothing is dropped by counting` |
| **R1.7** | A seek is aimed at the time the container states for the frame, so a long source does not start its re-encoded end one frame late. | `cutting.feature:the ends are seeked to the time the container states, not to a computed one` |
| **R1.8** | The re-encoded ends cannot be hashed, so they are compared by picture against the frame the mark names **and the two frames beside it**, and the offset found is reported. | `verify.head_within_frame`, `verify.tail_frame_offset` |
| **R1.9** | The output is written beside the source, named for the range it holds, with no character Windows refuses. | `cutting.feature:the finished file is named for the range it holds` |
| **R1.10** | A frame's stated time is read from the container over a window wide enough to contain that frame, and **nothing** is returned when it does not — the computed grid time is used instead. | `cutting.feature:a frame's stated time comes from a window wide enough to contain it`, `cutting.feature:the container is never asked to guess which frame was meant` |

## 2. Rates and frame grids

| Id | Requirement | Verified by |
|---|---|---|
| **R2.1** | Frames are counted on the grid the file is **actually** on, not the rate it claims, so a mark is exact at any frame rate. | `rates.feature:frames are counted on the grid the file is on` |
| **R2.2** | A file whose timestamps accumulate a frame or more of drift from its claimed rate says so, with the measured figure. | `rates.feature:a file that drifts from the rate it claims says so, with the number` |
| **R2.3** | A frame's presentation time includes the source's own first timestamp. | `rates.feature:a frame's time is measured from where the picture begins` |
| **R2.4** | Drop-frame timecode is read and written correctly at every rate that has one, and refused at every rate that does not. | `rates.feature:drop-frame is the reading the separator asks for` |
| **R2.5** | A timecode naming a frame its rate cannot have is refused with the reason, rather than read as the next second. | `rates.feature:a timecode that names a frame the rate cannot have is refused` |

## 3. Verification

A cut nobody measured is a cut nobody can vouch for.

| Id | Requirement | Verified by |
|---|---|---|
| **R3.1** | The copied body is compared with the source frame for frame, by hash; anything that is not the source's own packet is reported **with the frame it is at**. | `test_verify.py:test_a_body_that_is_the_source_packets_passes`, `test_verify.py:test_a_body_that_is_not_is_reported_with_the_frame` |
| **R3.2** | A re-encoded end cannot hash-match its original, so it is checked by picture against the frame the mark names **and the two frames beside it**, one frame wide because that is the measured drift; the offset found is named in the report and is not itself a failure. | `verify.head_within_frame` |
| **R3.3** | Every check reports **passed**, **failed** or **not checked**, and the interface never collapses those into two. | `verification.feature:a verdict never means two things at once` |
| **R3.4** | A file that holds fewer frames than its own head and body need is reported as short rather than compared into nonsense. | `test_verify.py:test_a_file_shorter_than_its_own_head_and_body_is_reported` |
| **R3.5** | The engine cuts once. It does not measure its own result and re-cut on what it finds. | `docs/DESIGN.md §17b` |
| **R3.6** | Captions are moved onto the segment, clamped at the marks, and the file written is named in the report. | `verification.feature:captions move with the segment and are clamped at the marks` |

## 4. The engine's interface

| Id | Requirement | Verified by |
|---|---|---|
| **R4.1** | The engine reports what it can do without saying everything it knows. | `engine_api.feature:the capability report is small and to the point` |
| **R4.2** | Every refusal carries a sentence naming the file or the reason. | `engine_api.feature:a refusal names what is wrong` |
| **R4.3** | One cut at a time; a second request is refused rather than queued. | `engine_api.feature:one cut at a time` |
| **R4.4** | Cancelling nothing is an answer, not an error. | `engine_api.feature:cancelling nothing is not an error` |
| **R4.5** | The engine is reachable only from this machine, and serves nothing outside its own interface. | `docs/TRUTH.md §The engineering claims` |

## 5. The window

| Id | Requirement | Verified by |
|---|---|---|
| **R5.1** | While a cut runs, the bar means the pass ffmpeg is running, and says so. | `window.feature:the bar is the pass ffmpeg is running` |
| **R5.2** | When a pass does not know its own length the bar does not invent a fraction. | `window.feature:a pass of unknown length does not invent a fraction` |
| **R5.3** | The clock keeps moving while the run is quiet, so a silent pass does not look frozen. | `window.feature:the clock keeps moving while the run is quiet` |
| **R5.4** | Cancelling is reported as a decision, not as a failure. | `window.feature:cancelling is a decision, not a failure` |
| **R5.5** | The log shows the newest line first, so what just happened is where a reader looks. | `window.feature:the log shows the newest line first` |
| **R5.6** | No label is ever drawn over the value beside it, and a file path is not truncated to nothing. | Measured in the running window: the label's right edge against the value's left edge, for the real markup with a path as long as the panel shows. See `docs/TRUTH.md §The interface claims` |
| **R5.7** | Every control is on screen without scrolling, and the frame never scrolls. | `docs/TRUTH.md §The interface claims` |
| **R5.8** | The window is restorable, minimizable and closable, and opens maximized rather than fullscreen. | `docs/TRUTH.md §The interface claims` |
| **R5.9** | The Name field takes a name for the segment, and the path the segment will be written to is shown beneath it. | `docs/TRUTH.md §The naming claims` |
| **R5.10** | The Folder toggle puts the segment and its transcript inside a folder named after the segment. | `cutting.feature:a named segment can arrive in a folder of its own` |

---

## 6. Starting, and the dependencies

| Id | Requirement | Verified by |
|---|---|---|
| **R6.1** | Double-clicking `start.bat` on a Windows PC with nothing installed results in the window, with no administrator and no step left to the person. | `scripts/check-bootstrap.ps1` installers: a Node and an ffmpeg are downloaded, unpacked, run, and found again on a second run |
| **R6.2** | A dependency is used when the machine has a usable one; installed only when it does not. | `scripts/check-bootstrap.ps1` locators: a Python below 3.10, a Python without `venv`, the Microsoft Store stub, a Node below 18 and an ffmpeg without `ffprobe` are each refused |
| **R6.3** | Nothing is installed outside the project's own folder and `%LOCALAPPDATA%`. | No administrator is requested: the Python installer runs with `InstallAllUsers=0`, and Node and ffmpeg are archives unpacked into `.tools` |
| **R6.4** | A second run downloads nothing it already has. | `scripts/check-bootstrap.ps1`: the locators find what the installers unpacked |
| **R6.5** | The engine finds a portable build beside the application, not only one on `PATH`. | `test_plan.py::test_a_portable_ffmpeg_beside_the_application_is_found` |
| **R6.6** | An override that is set and points at nothing is an error, not a silent fallback to another build. | `test_plan.py::test_an_override_pointing_at_nothing_is_an_error_not_a_fallback` |

---

## 7. Naming a segment

| Id | Requirement | Verified by |
|---|---|---|
| **R7.1** | With no name given, the segment is named for the range it holds, exactly as before. | `test_plan.py::test_no_name_still_names_the_segment_for_its_range` |
| **R7.2** | A name that is given becomes the segment's file name, in the source's own folder and container. | `cutting.feature:a segment is named after what the operator calls it` |
| **R7.3** | The transcript carries the segment's name and sits in the segment's folder. | `test_naming.py:test_the_transcript_is_named_after_the_segment_and_lands_with_it` |
| **R7.4** | The folder toggle puts both in a folder named after the segment, and the folder is created. | `test_naming.py:test_a_named_segment_in_a_folder_gets_its_captions_there` |
| **R7.5** | A name the filesystem would refuse is refused before the source is read, and the reason names what was wrong. | `test_server.py:test_a_name_windows_refuses_is_refused_before_anything_is_read` |
| **R7.6** | A name that is merely unusual — punctuation, accents, dots inside it, a non-Latin script — is accepted. | `test_plan.py::test_a_name_that_is_merely_unusual_is_accepted` |
| **R7.7** | Nothing is created for a name that was refused. | `test_naming.py:test_captions_are_not_written_for_a_name_that_was_refused` |
| **R7.8** | A name is resolved to its path without reading the source, so the field works before a range is marked and on a source that is not reachable. | `cutting.feature:a name is resolved before a range has been marked` |
| **R7.9** | A refused name is tagged as being about the name, so the window shows it beside the field rather than among the run's errors. | `cutting.feature:a refusal says which field it is about` |
| **R7.10** | An empty name is not a refusal: it means the range name, which is what clearing the box asks for. | `test_server.py:test_an_empty_name_is_not_a_refusal` |
| **R7.11** | The row says nothing about a name the engine has not answered about. It never reports a name as unusable on its own account. | `nameRow.test.ts`, `cutting.feature` |

---

## Traceability

| Area | Spec | Scenarios | Steps / tests | Code |
|---|---|---|---|---|
| Cutting | §1 | `cutting.feature` | `backend/tests/test_behaviour.py` | `trimmer/trim.py` |
| Rates | §2 | `rates.feature` | `backend/tests/test_behaviour.py` | `trimmer/ffmpeg.py`, `trimmer/timecode.py` |
| Verification | §3 | `verification.feature` | `backend/tests/test_behaviour.py` | `trimmer/verify.py` |
| Engine API | §4 | `engine_api.feature` | `backend/tests/test_behaviour.py` | `server.py` |
| Window | §5 | `window.feature` | `frontend/src/state/window_steps.test.ts` | `frontend/src/state/useRunLog.ts`, `frontend/src/components/ProgressLog.tsx` |
| Starting | §6 | — | `scripts/check-bootstrap.ps1` | `start.bat`, `scripts/bootstrap.ps1` |
| Naming a segment | §7 | `cutting.feature` | `backend/tests/test_naming.py`, `backend/tests/test_server.py` | `trimmer/trim.py`, `server.py`, `frontend/src/App.tsx` |

Requirements with no scenario are verified by the audit in `docs/TRUTH.md`, which pairs every
claim the product makes about itself with the mechanism that decides it. Where a requirement
cannot be checked — whether a cut is right on real footage — `docs/TRUTH.md` says so rather than
leaving the gap unstated.
