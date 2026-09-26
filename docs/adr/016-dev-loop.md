# ADR-016: The interface is developed in a browser, not in the window

**Status:** accepted
**Date:** the day the loop was measured at two minutes and then at five seconds

## Context

The desktop shell is Tauri v2: a Rust core, a WebView2 window, and a React interface loading from
`dist`. For most of the build, every interface change was checked by running

```text
cargo build --release -p thetrimmer-desktop     # 1m44s – 2m10s
target/release/thetrimmer-desktop.exe
```

and looking at the window, or by driving it through the Chrome DevTools Protocol from a PowerShell
harness that also had to find the native file picker through UI Automation and answer it.

Two things were wrong with that, and only one of them was the two minutes.

**The loop was slow.** Every CSS change, every label, every layout fix cost a full Rust relink plus a
window launch plus a screenshot. A session that needed twenty interface changes spent forty minutes
compiling to make twenty one-line edits.

**The harness was fragile, and its fragility looked like product defects.** That CI harness was built
against the assumption that a test could type into the window. It cannot, reliably:

* `SendInput` returns error 5 on a locked-down session, and the window test stopped dead with
  `SendInput refused the keystroke`, which says nothing about TheTrimmer.
* The native file picker is a real Win32 dialog owned by the application process. It is not part of
  the page, so the DevTools protocol cannot reach it, and UI Automation found the taskbar and an
  unrelated Explorer window instead of the picker.
* The debug port (`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`) works, but a test harness written against
  it is a second program with its own bugs — and it *had* its own bugs. A PowerShell function that
  returned a one-element collection as a scalar made `.Count` throw, and the harness reported a
  product defect that was not there.

Worse, the seam that harness was testing is where the real defects were. Three of them, all found by
the Rust-side contract test rather than by the harness:

1. `add_source` answered a hand-written JSON object with a **different shape** from the `sources`
   command, so the source rail could render a row of blanks for a file the next refresh described in
   full.
2. `get_verify_policy` **refused to answer before a project was open**, on a window that reads it on
   mount — a status-bar error on a first run that is behaving perfectly.
3. An in/out range marked in the interface produced a segment **one frame longer** than the dialog
   promised, because the interface's half-open end convention was converted on both sides of the
   boundary.

## Decision

**The interface runs, and is tested, in a plain browser, against a stub of the one function it needs
from outside.**

`apps/web/src/ipc/stub.ts` installs a `window.__TAURI__` of the same shape the window provides —
`{ core: { invoke }, dialog: { open, save } }` — and answers the commands with a realistic fixture
project. Nothing else changes:

* `apps/web/src/ipc/commands.ts` is the same file, calling the same `invoke`.
* Every component is the same component.
* The stylesheet is the stylesheet the window loads.
* The stub **refuses to replace a bridge that is already there**, so `installStub()` is called
  unconditionally from the entry point and is a no-op inside the real window.

The loop becomes:

| Change | Before | Now |
|---|---|---|
| CSS, label, layout | 1m44s + launch + screenshot | **< 100 ms**, hot-reloaded in the tab |
| Interface behaviour | a Rust rebuild and a fragile harness | **`npm run e2e` — 10 tests, 5.6 s** |
| Rust core | 1m44s release relink | `cargo test`, or `tauri dev` (debug, ~25 s) |
| Shipping | `--release` build | unchanged |

## The division of testing, and why neither half is sufficient

The stub agrees with the interface **by construction**, so it can never catch a *shape* mismatch: if
`add_source` stopped returning `summary`, the stub would keep returning it and every browser test would
keep passing. What the stub does catch is that a command added to `COMMAND_NAMES` and not stubbed is a
**compile error** — the failure mode being prevented is a control that works in the window and does
nothing in every browser test.

So the contract has two tests, of two different things:

| | `apps/web/tests/*.spec.ts` | `apps/desktop/src-tauri/tests/ipc_contract.rs` |
|---|---|---|
| Drives | the real interface, in Chromium | the real commands, through Tauri's real invoke handler |
| Answers come from | the stub | Rust |
| Catches | behaviour, states, labels, arithmetic **as the interface reads it** | the **shape** of what Rust sends, and that every command is registered |
| Runs in | 5.6 seconds | 2.3 seconds, `--ignored` |

Two of the three defects listed above were found by the second, and the browser suite immediately
found two more of a kind the Rust side cannot see at all: a proof panel whose checks were collapsed
behind a row the test never opened, and a transcript panel that called `add_segment` **directly**
instead of through the model, so the new segment was written to the project and the screen kept
showing the state from before — a button that visibly did nothing.

## Consequences

**Good.**

* Interface work costs no compilation. The window is built once at the end, to confirm it still opens.
* The tests are reviewable. A browser test says `getByRole("button", { name: "Trim this segment" })`
  and a reader knows what is being asserted.
* The interface is now runnable on any machine with Node, including a machine with no WebView2 and no
  Rust toolchain, which is what made it possible to review the design at all.
* A screenshot of the real interface, at a real size, comes out of a test (`apps/web/screens/`).

**Costs, stated plainly.**

* The stub is a second implementation of thirty commands and has to be kept in step. The
  `Record<CommandName, …>` type is what makes that mechanical rather than a matter of discipline.
* A browser is not WebView2. Chromium is the engine WebView2 uses, so the rendering is the same, but
  the *window* is not: the native file picker, the title bar, `Start` menu integration and the
  installer are not exercised by any of this. They are exercised by launching the built binary, which
  is now the last step rather than every step.
* The stub cannot test the parts of the interface that depend on a real project on disk — a source
  that has gone missing, a master with a variable frame rate, a caption file that has been retimed.
  Those live in `ipc_contract.rs` and in the CLI's own suite.

## Alternatives considered

**Rejected — migrate to Python and Qt.** Python is the only one of the candidates where the UI loop
needs no compiler at all. It was rejected on distribution: this is a product that will be sold, and
shipping a Python runtime means a ~40 MB frozen installer with the antivirus false-positive tax, or
asking a customer to install Python. The dev-loop problem is real; it is not worth that.

**Rejected — migrate to `egui`.** One language, the IPC seam deleted, a ~6 MB binary, no runtime. It
was rejected because the loop is 5–15 s rather than instant — `egui` still compiles — and because
`egui` publishes essentially one canvas to a screen reader, which would throw away the accessibility
work the web layer already has: a skip link, `role="grid"` on the cut table, `aria-describedby` on
every mark, and forced-colours support.

**Rejected — keep the CDP harness and fix it.** It could have been made to work: drive the page
through the DevTools protocol, answer the picker through UI Automation. It was rejected because it is
strictly worse than the stub at the same job — a second program, written in a second language, with
its own failure modes that present as product defects — and because it still required the window to be
built to test anything.
