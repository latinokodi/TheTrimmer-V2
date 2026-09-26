# ADR-020: The window opens maximized, not fullscreen, and the check asks Windows

**Status:** accepted
**Date:** the day a user reported being unable to restore, minimize or close the application

## Context

> *"Im unable to restore, minimize or close the app window"*

A borderless window with no titlebar has no restore, no minimize and no close. That is what the window
was. `tauri.conf.json` asked for `"fullscreen": true` with `"decorations": true`, and on Windows a
fullscreen window is a borderless one: the decorations are not hidden by the application, they are
removed from the window by the operating system, along with the frame it can be resized from.

Measured on the real window, from the process, with `GetWindowLong(hwnd, GWL_STYLE)`:

```
GWL_STYLE      : 0x14000000
WS_CAPTION     : ABSENT      no titlebar to drag, double-click or restore from
WS_SYSMENU     : ABSENT      no system menu, which is where Close lives
WS_MINIMIZEBOX : ABSENT      no minimize button
WS_MAXIMIZEBOX : ABSENT      no maximize or restore button
WS_THICKFRAME  : ABSENT      no resizable, snappable frame
```

`0x14000000` is `WS_VISIBLE | WS_CLIPCHILDREN` and nothing else. The window filled the screen and could
not be done anything else with.

**And the page could not do it either.** The interface could not have offered a way out even if it had
tried, because the capability file grants `core:default` — which contains only *read* window permissions
— so both operations that would have helped were refused:

```
minimize       → DENIED: Command plugin:window|minimize not allowed by ACL
setFullscreen  → DENIED: Command plugin:window|set_fullscreen not allowed by ACL
```

The window was a dead end from both directions: no chrome to click, and no permission to act.

### Why every test was green

`tools/smoke-window.ps1` had just been rewritten to ask the window question it could not previously ask,
and it reported a healthy window throughout. The reason is the same shape as the fault in
[ADR-019](019-the-stub-never-ships.md), which is why it is worth stating twice:

**Tauri's answers were about a different window from the one that existed.**

```
isFullscreen    : true
isDecorated     : true     ← while WS_CAPTION was ABSENT
isMinimizable   : true     ← while WS_MINIMIZEBOX was ABSENT
isMaximizable   : true     ← while WS_MAXIMIZEBOX was ABSENT
isClosable      : true     ← while WS_SYSMENU was ABSENT
```

Every one of those is the *requested* configuration, reported as fact. A check built on them would have
certified a window that could not be closed. `isFullscreen: true` was the only honest answer in the set,
and it was the one thing nobody thought to assert.

Both faults in this session have the same root: **a component was asked about the system it fronts, and
it described its own intentions instead.** The browser stub described a real bridge; Tauri described a
real window. A test that asks a proxy is testing the proxy.

## Decision

### 1. The window opens maximized, with the operating system's titlebar

```json
"maximized": true,
"decorations": true,
"resizable": true,
"minWidth": 1440,
"minHeight": 960
```

Measured on the rebuilt window: `GWL_STYLE = 0x15CF0000` — `WS_CAPTION`, `WS_SYSMENU`,
`WS_MINIMIZEBOX`, `WS_MAXIMIZEBOX` and `WS_THICKFRAME` all present, `WS_MAXIMIZE` set, and no
`WS_DISABLED`.

It still fills the screen, which is what "opens fullscreen" was for. What it gains is everything the
operating system provides for a normal window and a hand-rolled titlebar has to reimplement badly:

* minimise, restore, close, move, resize;
* **Snap Layouts** — the Windows 11 maximise-button hover that offers a layout, which only exists for a
  window with a real maximise box;
* the system menu, and therefore `Alt+Space`;
* `Win+Up` / `Win+Down` / `Win+Left` / `Win+Right` behaving as they do everywhere else;
* correct behaviour under per-monitor DPI, high contrast, and the user's own titlebar preferences.

The cost is about 80 logical pixels of client area on a 1080p display — a 32 px titlebar and the
taskbar. Measured after the change: **1920x1009**, against the layout's declared 1440x960 minimum.

### 2. Fullscreen is an explicit, reversible action rather than the state the window starts in

Fullscreen is still wanted: nothing is more distracting beside a calibrated monitor than a taskbar
glowing under it. So it is offered — `F11`, and a *Fullscreen* button in the header — and it is
reversible by the same means, which is what makes offering it safe at all.

This is the only window operation the page is permitted, and the capability file says so:

```json
"core:default",
"dialog:allow-open", "dialog:allow-save", "dialog:allow-message", "dialog:allow-confirm",
"core:window:allow-set-fullscreen"
```

Still **no** `minimize`, `maximize`, `close`, `start-dragging`, `set-position` or `set-size`. A custom
titlebar would have required four of those, which is the second reason the chrome belongs to Windows:
the page keeps holding one capability it cannot be tricked into misusing, and the window controls work
even if the interface is broken.

`apps/web/src/ipc/window.ts` holds the two calls, wrapped in the same
`ok`/`unavailable`-with-a-reason shape as the file picker. The state is **read from the window** rather
than remembered, because `F11`, the button and the titlebar's own maximise button can each change it,
and a button showing what we last set rather than what is true is a button that lies after `Win+Up`.

### 3. The check asks Windows, and it is proven able to fail

`tools/smoke-window.ps1` reads `GWL_STYLE` from the process's main window and fails if any of the five
bits is absent, then minimises and restores the window with `ShowWindow` and confirms with `IsIconic`,
then round-trips fullscreen through the interface's own button, then closes the window with `WM_CLOSE`
and waits for the process to exit. That last one is the check for the user's exact complaint: the
window closes when asked to close, the way the titlebar's X asks.

Reinstating `"fullscreen": true` and rebuilding produces:

```
FAILED  the window has no WS_CAPTION (GWL_STYLE is 0x14000000), so there is a titlebar to drag,
        double-click and restore from. The window must open maximized with decorations rather than
        fullscreen; see ADR-020.
```

The same `0x14000000` the broken window had. The check cannot pass on the window the user could not
close.

## Consequences

**Good.**

* The window can be restored, minimized, closed, moved, resized and snapped, for the reason that
  matters: Windows draws the controls, so they exist regardless of what the interface does.
* Fullscreen survives as a deliberate choice, and the key that enters it is the key that leaves it.
* The style bits are now asserted, which closes the class of fault rather than the instance. Any future
  configuration that removes the frame — `decorations: false`, a kiosk mode, a fullscreen default —
  fails the smoke test with the bit that went missing.
* The capability file grew by exactly one permission, for one user-initiated action.

**Costs, stated plainly.**

* About 80 logical pixels less client area. The layout's minimum was left at 1440x960 because the
  window still clears it, and the browser suite proves the panel is not squeezed at that size.
* The application now has a native titlebar **and** its own header strip, which both say "TheTrimmer".
  That is normal for a Windows application with a header — Photoshop and Audition both do it — but it is
  a deliberate acceptance rather than an oversight.
* The smoke test compiles a P/Invoke block with `Add-Type`, so it now depends on a working C# compiler
  in the runner. That is a real new dependency for a check, and it is the only way to ask the question
  that Tauri answers wrongly.
* `WM_CLOSE` at the end of the smoke test means the window is closed by the check itself rather than
  killed by the harness. If `WM_CLOSE` ever stops working, the harness still kills the process, so the
  failure is reported rather than hanging.

## Alternatives considered

**Rejected — keep fullscreen and put window controls in the app's own header.** The header already looks
like a titlebar, and this would have produced a single, tidier bar. It was rejected on two grounds.
First, it needs `core:window:allow-minimize`, `allow-toggle-maximize`, `allow-close` and
`allow-start-dragging` — four new permissions and four new ways for the page to drive the window, in an
application whose security stance is that the page holds one capability. Second, and worse: the controls
would live in JavaScript. A window whose only means of closing is a script is a window that cannot be
closed when the script fails, and the failure mode of a broken interface would become an unclosable
fullscreen window — precisely the fault being fixed, reachable by a new route. Snap Layouts, `Alt+Space`
and per-monitor DPI correctness would also have to be reimplemented or given up.

**Rejected — keep fullscreen and add only an `F11` escape.** The cheapest possible fix, and it fails the
requirement: the user asked to *restore* and *minimize*, not only to close. Fullscreen hides the taskbar,
so minimizing is not even meaningful from it; the only real exit is out, and "exit fullscreen" is not
the same affordance as a titlebar.

**Rejected — trust `isDecorated()` and the other Tauri booleans in the smoke test.** They were all
`true` on the broken window. Any check built on them certifies the fault. This is the same mistake as
the smoke test's ffmpeg fixture string in ADR-019, and it is now a stated rule: **a check must ask the
thing that has the property, not the thing that describes it.**
