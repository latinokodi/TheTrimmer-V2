# ADR-019: The browser stub never ships, and a check a fixture can satisfy is not a check

**Status:** accepted
**Date:** the day a user reported "clicking the browse button should open the windows native file
selection modal", and the answer was that nothing in the application had ever done anything

## Context

The report was about one button. What it exposed was that **the shipped window had been running
entirely on the browser stub** — the fake bridge in `apps/web/src/ipc/stub.ts` that exists so the
interface can be developed and tested in a browser with no Rust, no WebView2 and no build. Every
command the application makes was being answered from a fixture in TypeScript. Nothing in the window
had touched Rust for several revisions.

The evidence, in the order it arrived:

| Probe | Result | What it meant |
|---|---|---|
| `Object.keys(window.__TAURI__)` | `mocks, core, dialog` | the stub's marker was present in the real window |
| `window.__TAURI__.dialog.open({…})` | rejected: *"the browser harness has no plugin:dialog\|open; add it to src/ipc/stub.ts"* | the invoke handler was the stub's, so **every** command was |
| `window.__TAURI__` keys after the fix | `app,core,dpi,event,image,menu,mocks,path,tray,webview,webviewWindow,window,dialog` | the real Tauri global — and note `mocks`, which is Tauri's own namespace and the reason the first detector was wrong |
| `window.__TAURI_INTERNALS__.invoke('doctor')` | real ffmpeg output | the actual IPC bridge had been there the whole time |

**Why the guard failed.** `installStub()` tested `window.__TAURI__` before installing itself:

```ts
const existing = (window as unknown as { __TAURI__?: unknown }).__TAURI__;
if (existing !== undefined && (existing as { mocks?: unknown }).mocks === undefined) {
  return false;
}
```

`window.__TAURI__` is the *convenience* global that `withGlobalTauri` injects, and it is injected
**after** the page's own module scripts have run. `window.__TAURI_INTERNALS__` — the IPC bridge itself
— is there from the first script. So at the moment `main.tsx` called `installStub()`, `__TAURI__` was
`undefined`, the guard's first condition was false, the stub installed, and Tauri's own global was
clobbered when it finally arrived.

That the guard *would* have declined had the global been present is provable from the fix's own
measurement: the real `__TAURI__.mocks` is a namespace **object**, not `undefined`, so
`existing.mocks === undefined` was false. The condition only ever failed on `existing === undefined`,
which means the global was genuinely not there yet. There was no third possibility.

**Why nothing caught it.** Each test did its job and each job was the wrong one:

* **The browser suite (28 tests)** runs in a browser, where the stub is *supposed* to be the bridge.
  It agreed with itself by construction.
* **`ipc_contract.rs`** drives the real commands through Tauri's real invoke handler and proves the
  shapes — but it never started the window, so it never asked which handler the window had.
* **`tools/smoke-window.ps1`** did start the window and did assert a round trip through `invoke`. It
  checked that the footer reported the ffmpeg build string. The stub hard-codes
  `"ffmpeg version 8.0.1-essentials_build-www.gyan.dev"` — copied from this very machine when the
  fixture was written — so the assertion passed on a constant.

That is the generalisable fault, and it is the reason this ADR exists:

> **A check that a fixture can satisfy is not a check.** The smoke test's claim — "it cannot pass
> without the bridge and without the command being registered" — was false in the way that matters.
> It could pass without *either*, because a stub implements both.

**Why it was possible at all.** `installStub()` was called unconditionally, on the argument that it
refuses to replace a real bridge and is therefore safe. That argument made the stub's presence in a
production bundle a matter of *runtime ordering* — a race the application could lose silently, with no
visible symptom other than an application that appeared to work.

## Decision

### 1. The stub is not in a production bundle, and the build proves it

`main.tsx` calls it only under `import.meta.env.DEV`:

```ts
if (import.meta.env.DEV) {
  installStub();
}
```

Vite folds that to `false` for a production build, the branch goes, and Rollup drops the module.
That is the guarantee. It is checked by **`apps/web/tools/check-bundle.mjs`**, which runs as the last
step of `npm run build` and opens the built files looking for anything that could only have come from
the stub — its refusal sentence, its diagnostic marker, its minified `mocks` flag, and its fixture
path read out of the stub's own source so the check follows the stub instead of drifting from it.

A build step rather than a test, because this has to be a property of the **artefact**. A test can be
run a different way, skipped by a filter, or invalidated by a refactor of the test setup; `npm run
build` is the thing that produces what ships.

The guard stays as a second line of defence, and now tests `window.__TAURI_INTERNALS__` — the real
signal — and leaves `window.__TAURI_STUB__` behind saying which condition fired. A future reader
should not have to re-derive this.

### 2. A missing bridge is loud, and "cancelled" is not "unavailable"

`pickFile` and `pickSave` answered `string | null`, where `null` meant both *the operator cancelled*
and *there is no picker here*. Collapsing those two is what made the dead Browse button look like a
working one: it did nothing, twice, identically.

They now answer a discriminated result — `picked` | `cancelled` | `unavailable`, the last carrying the
sentence that says which fault it is — and the two globals are distinguished in the message, because
"nothing has finished loading" and "this build has no dialog plugin" are different problems.

### 3. Browse opens the operating system's dialog, on the first click

The user's actual request. `Browse` called `setSourceOpen(true)` and opened an in-app dialog containing
a second button labelled *Choose a file…* which opened the picker. So the operator's one-click job was
two clicks, and the click that mattered was behind a dialog that hid the one file chooser on Windows
that has their favourites, recent folders, mapped drives and search in it.

Now:

```
Browse ──▶ pickFile ──▶ the Windows dialog ──▶ add the source
                   └──▶ unavailable ──▶ the in-app dialog, with the reason in it
```

The in-app dialog is not deleted, because the fallback is a real situation and it does two things the
panel cannot: take a typed or pasted path as its *primary* action, and list the masters already in the
session. It is also still reachable from inside itself (`Try the Windows file dialog`), because "no
picker" can be transient.

A **cancel says nothing**. Cancelling is a decision and it is respected.

### 4. One command list, and a test that reads the interface's own names

`main.rs` registered the handler and `ipc_contract.rs` kept a hand-written copy, with a comment
arguing the duplication was deliberate — that a shared list "would still pass if the binary forgot to
register one of them". That is backwards: two lists drift, and the direction that matters is a command
present in the test and absent from the binary, which passes every test in the crate and is a dead
button in the shipped window.

There is now one list, `trimmer_commands!()` in `lib.rs`, used by both. And a new test,
`every_command_the_interface_names_is_registered`, reads `COMMAND_NAMES` **out of
`apps/web/src/ipc/commands.ts`** and asserts each name is answerable. That test could not have been
written before this fix and would have found the gap immediately: with the stub present, a missing
registration was answered by the stub.

### 5. The state tests no longer share a process-global

Running the ignored media tests together, in parallel, failed —
`the store path is not the one the environment named`. Each test set `THE_TRIMMER_STORE` before
constructing its state, and the variable is process-global, so one test's workspace could open
another's database. The symptom was not a crash: it was `storePath` naming the wrong scratch
directory, which reads as a broken assertion rather than as shared state.

`AppState::bootstrap_at(path)` takes the store path now, and CI no longer needs `--test-threads=1`.
Serialising the tests would have hidden the next race instead of removing this one.

## Consequences

**Good.**

* The shipped window does real work. `tools/smoke-window.ps1` now asserts the absence of the fixture
  and the presence of the picker, neither of which a stub can fake, and it verified the Windows dialog
  actually opens — the promise stays pending, which means a modal is on screen.
* The whole class is closed at the artefact level. `npm run build` cannot produce a bundle containing
  the stub, and the check is proven able to fail: reinstating the unconditional call makes it exit 1
  naming three independent needles.
* A dead control now says so. `picked`/`cancelled`/`unavailable` is the contract for both pickers, and
  the export dialog's *Browse…* got the same treatment without being asked for.
* The bundle got smaller: **201.1 kB → 191.6 kB** of JavaScript, because 9.5 kB of stub and fixtures
  are no longer shipped to customers.
* A command missing from the handler is now a test failure naming the command, rather than a button
  that fails silently the first time someone clicks it in a release build.

**Costs, stated plainly.**

* `npm run build` is now `tsc && vite build && node tools/check-bundle.mjs`, so a build can fail for a
  reason that is not a compile error. That is the point, and the message says what to do about it.
* Opening `apps/web/dist/index.html` in a browser now shows an application with no bridge rather than a
  working demo. Correct and honest — `dist` is the shipped artefact, not the demo — but it is a change
  in behaviour for anyone who used it that way. `npm run dev` is the demo, and still is.
* The picker check in the smoke test opens a real modal and leaves it on screen for two seconds before
  the window is killed. `-SkipPicker` exists for a machine with no desktop session; the failure message
  says so, and also says to be sure that is really the reason.
* The fixture path is no longer used as a placeholder in the source dialog. It was, and it made the
  first bundle check fail on legitimate copy — which is a useful reminder that a string chosen as a
  canary has to be unique, and the fix was to derive it from the stub's source rather than to delete it.

## What this says about the test suite as a whole

Every job in CI was green while the product did nothing. Not because a job was broken, but because
every job tested a *seam* and none tested the *artefact*:

| Test | What it proves | What it could not see |
|---|---|---|
| Rust unit and oracle tests | the engine is correct | whether the window calls it |
| `ipc_contract.rs` | the commands' shapes | whether the window registers them |
| Browser suite | the interface behaves | whether its bridge is real |
| `smoke-window.ps1`, as it was | *that the footer had text in it* | that the text was true |
| Browser suite + smoke test, now | the interface behaves, against a bridge that is proven real | — |
| `check-bundle.mjs` | the shipped bundle has no fixture in it | — |

The lesson is not "add more tests". It is that **a fixture must never be able to satisfy an assertion
about the real system**, and the cheapest way to guarantee that is to make the fixture's absence a
property of the build.

## Alternatives considered

**Rejected — keep the stub unconditional and fix the guard.** Checking `__TAURI_INTERNALS__` would
probably have worked. It was rejected as the *primary* fix because it leaves a race in the shipping
path: the stub's absence would still depend on the order in which two scripts run, and the failure
mode of losing that race is an application that looks like it works. A dev-only affordance should not
be in the bundle at all, and then there is no race to lose. The guard remains, as defence in depth.

**Rejected — assert a value that only Rust can produce, and leave the stub in.** This is what the
smoke test tried to do with the ffmpeg build string, and it failed because the fixture had been copied
from real output. Any such value is one `cargo run` away from being faked by accident. Absence is the
only thing a fixture cannot imitate.

**Rejected — delete the stub entirely and use a mock service in the browser instead.** It is the same
thing with more machinery: the interface would still need a bridge in a browser, and the value of
`stub.ts` is precisely that it is the *same* `commands.ts` and the same components with one function
replaced. Being in the dev bundle and not the shipped one is the whole requirement.

**Rejected — make `pickFile` throw when there is no picker.** A dialog that cannot open is a fact
about the environment, not a programming error, and the interface has somewhere useful to send the
operator. A discriminated result carries the same information to a caller that can act on it, and
`unavailable` is not an exception in any language.
