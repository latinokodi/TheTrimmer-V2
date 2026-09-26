# ADR-021: A fixture that cannot express the state a fault lives in will not find it

**Status:** accepted
**Date:** the day a source list was added to a window that had never had one

## Context

> *"When I select a file with browse, then try to select a different one, the first file gets stuck and
> cannot change."*

The picker was working. `add_source` was working. The project was working. What was missing was one line.

`browse()` added the master to the project and never selected it:

```ts
if (picked.kind === "picked") {
  await model.addSource(picked.path);   // the project now holds two masters
  return;                               // …and the window goes on showing the first
}
```

`add_source` **upserts into a map keyed by path** — `Project::upsert_source` is `sources.insert(...)` —
so a second video is a second source rather than a replacement. `useSegmentDraft` then picks
`sources[0]` and holds it until that source disappears:

```ts
if (!usable.some((item) => item.path === source)) {
  setSource(usable[0]?.path ?? "");
}
```

And `draft.setSource` was **called from nowhere in the interface**. Grep for it and the only match was
its own declaration. So the panel showed the first master for ever. Picking a fifth file added a fifth
master and changed nothing on screen. Two files in the project, one file in the window, and no way to
reach the other.

### Why it was invisible

The browser stub's `sources` command was:

```ts
sources: () => (state.hasSource ? [sourceView()] : []),
```

One fixture behind a boolean. **The stub could not represent two masters in a session**, so the exact
state the fault lives in could not be constructed in a browser — and in every state that *could* be
constructed, the panel showing "the" source was correct. `chooseMaster` clicked Browse, one source
appeared, one path was displayed, the test passed. Pressing Browse again in a browser would have
produced the same single source and the same passing test.

This is the third instance of one family of fault in this project, and the three are worth reading
together:

| | The proxy that was asked | What it described instead |
|---|---|---|
| [ADR-019](019-the-stub-never-ships.md) | the browser stub | a *real* bridge, so `doctor` passed on a hard-coded string |
| [ADR-020](020-the-window-has-a-titlebar.md) | Tauri's window API | a *decorated* window, so a window with no titlebar certified as healthy |
| this one | the browser stub again | a *single-source* session, so a single-source panel was correct |

The first two are "a check that a fixture can satisfy is not a check". This one is the other half:
**a fixture that cannot express the fault's state cannot fail because of it.** A test can only be as
sharp as the world its doubles can describe.

## Decision

### 1. The stub holds the session's masters as a list

```ts
interface StubSource { path: string; name: string; transcript: string | null; transcriptCues: number }

add_source:  upsert into `state.sources` by path, return that source's view
sources:     state.sources.map(sourceView)
remove_source: drop the source *and* the segments marked against it
```

The two masters are made distinguishable in the two ways the interface reads: the first has a caption
file beside it and the second does not, so the Caption file zone and the transcript panel behave
differently for each. A fixture whose entries are identical cannot show that a list is a list.

The stub's new claims are asserted against the real thing in
`ipc_contract.rs::a_project_holds_several_masters_and_removing_one_takes_its_segments` — two sources
listed, two different resolutions, exactly one with a transcript, and a removal that takes its segments
with it. **A fixture that describes the real system wrongly is worse than no fixture**, which is
ADR-019's lesson from the other side. `remove_source` had no behavioural test before this.

### 2. The interface selects the master it just added

One callback, used by all three ways a source can arrive — the picker, the in-app dialog, and a typed
path:

```ts
const addSource = useCallback(async (path: string) => {
  await model.addSource(path);
  draft.setSource(path);
}, [draft, model]);
```

If the add is refused, `setSource` names a path that is not in the session and the draft's own effect
falls back to the first one that is, so a refusal cannot strand the panel either.

### 3. The session is reachable, and a master can be removed

Selecting the new master without a way back would have replaced one dead end with another: the first
file would have been stranded instead of the second. So the Video row grows two controls, and only when
there is more than one master:

```
SOURCE  [ B014C003_250401_R2QK ▾ ]  [ H:\masters\reel 3\B014C003_250401_R2QK.mov ]  [Browse]  [×]
```

* the **select** moves between the session's masters, and marks one that is not on disk;
* the **×** removes the chosen master, and the segments marked against it, which is what the command
  already did.

Both sit in the row that was already there, so they cost **no height**. That matters: the window is
maximized rather than fullscreen since ADR-020, which is about 80 logical pixels tighter than it was,
and the vertical budget at 1440x960 has no room for a new row. The select yields width before the path
does, because the path is the thing being read.

## Consequences

**Good.**

* Changing the video changes the video, and the previous one is still there to go back to.
* `remove_source` — a command with no UI and no test — now has both. An operator who picks the wrong
  file can undo it without leaving sources in the project that affect the summary and the export.
* The browser suite can finally construct a two-master session. The new test fails on the old code with
  the user's own symptom: `Received string: "H:\masters\reel 2\A007C012_250312_R1QK.mov"` after asking
  for `B014C003_250401_R2QK.mov`.
* A clipping probe now covers the tightest the Video row ever gets — four controls on one line at the
  window's minimum width — and `screens/session.png` is the two-master panel, committed as evidence.

**Costs, stated plainly.**

* The stub is now a slightly larger lie. It reports every fixture master with the same mock media, so a
  test cannot distinguish two sources by their frame counts; the *contract* test can, and does.
* Browsing twice now leaves two masters in the project where it previously left one in the interface and
  two in the project. The second was always there; now it is visible and removable, which is a change in
  appearance as well as in behaviour.

## The general rule

Three faults in a row have had the same cause, and it is not "not enough tests". It is that **every
double in the system was asked about the thing it stands in for**:

* the stub, about the bridge;
* Tauri's window API, about the window;
* the stub again, about the project's shape.

The rule that follows is narrow enough to be checkable: **when a fault is found, first ask whether the
test doubles could have represented the state it lives in. If they could not, fixing the double is part
of the fix.** The failing test comes before the change, and it has to fail for the reported reason — the
first run of this test reported the first master's path where the second was asked for, which is the
user's sentence exactly.
