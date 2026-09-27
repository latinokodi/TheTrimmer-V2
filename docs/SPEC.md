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
| **R1.8** | The far end of a segment is a re-encode, so it is checked against the frame the out point names rather than by hashing, and no alignment sample is taken where a hash comparison could not be honest. | `verification.feature:alignment samples stay inside the copied body, which is the only part that can match` |
| **R1.9** | The output is written beside the source, named for the range it holds, with no character Windows refuses. | `cutting.feature:the finished file is named for the range it holds` |

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
| **R3.1** | The copied body is compared with the source frame for frame, by hash; a re-encoded frame cannot pass. | `verification.feature:the copied body is compared with the source by frame hash` |
| **R3.2** | The re-encoded head is measured differently — it cannot hash-match — and the report says which of the two was done and why. | `verification.feature:the head is measured by similarity, and the report says so` |
| **R3.3** | Every check reports **passed**, **failed** or **not checked**, and the interface never collapses those into two. | `verification.feature:a verdict never means two things at once` |
| **R3.4** | Alignment samples avoid the re-encoded head and stay inside the range that was asked for. | `verification.feature:alignment samples avoid the head and the overshoot` |
| **R3.5** | Two samples that disagree mean the timeline is not simply shifted, so nothing corrects on them. | `verification.feature:disagreeing samples are not corrected on` |
| **R3.6** | A cut where the body landed off the mark is re-cut once with the measurement applied, before it is reported. | `verification.feature:a body off the mark is re-cut from the measurement` |
| **R3.7** | Captions are moved onto the segment, clamped at the marks, and the file written is named in the report. | `verification.feature:captions move with the segment and are clamped at the marks` |

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

---

## Traceability

| Area | Spec | Scenarios | Steps / tests | Code |
|---|---|---|---|---|
| Cutting | §1 | `cutting.feature` | `backend/tests/test_behaviour.py` | `trimmer/trim.py` |
| Rates | §2 | `rates.feature` | `backend/tests/test_behaviour.py` | `trimmer/ffmpeg.py`, `trimmer/timecode.py` |
| Verification | §3 | `verification.feature` | `backend/tests/test_behaviour.py` | `trimmer/verify.py` |
| Engine API | §4 | `engine_api.feature` | `backend/tests/test_behaviour.py` | `server.py` |
| Window | §5 | `window.feature` | `frontend/src/state/window_steps.test.ts` | `frontend/src/state/useRunLog.ts`, `frontend/src/components/ProgressLog.tsx` |

Requirements with no scenario are verified by the audit in `docs/TRUTH.md`, which pairs every
claim the product makes about itself with the mechanism that decides it. Where a requirement
cannot be checked — whether a cut is right on real footage — `docs/TRUTH.md` says so rather than
leaving the gap unstated.
