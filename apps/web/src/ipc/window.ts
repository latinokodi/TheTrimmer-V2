/**
 * The window itself: reading and setting fullscreen, and nothing else.
 *
 * ## Why this exists at all
 *
 * The window opens **maximized**, with the operating system's own titlebar — so restore, minimize,
 * close, move, snap and resize are all the operating system's job and need no code here. That is
 * deliberate: those are the controls a user cannot do without, and the only way to be certain they
 * exist is to let Windows draw them.
 *
 * The window *was* borderless fullscreen, and the consequence was measurable. `GetWindowLong` on the
 * real window returned `0x14000000` — `WS_VISIBLE | WS_CLIPCHILDREN` and nothing else: no `WS_CAPTION`,
 * no `WS_SYSMENU`, no `WS_MINIMIZEBOX`, no `WS_MAXIMIZEBOX`, no `WS_THICKFRAME`. There was no titlebar
 * to restore, minimize or close from, and the page could not do it either — `minimize` and
 * `set_fullscreen` were both refused by the capability file. The window was a dead end.
 *
 * Fullscreen is still worth having: a colourist comparing a frame against a monitor does not want a
 * taskbar glowing underneath it. So it is offered as an **explicit, reversible** action rather than as
 * the state the window starts in, and it is the only window operation the page is permitted.
 *
 * ## Why the state is read rather than tracked
 *
 * `F11`, the button, the operating system's own shortcut and the maximise button in the titlebar can
 * each change whether the window is fullscreen, and only the last is ours. Asking the window what it is
 * — instead of remembering what we set it to — is the difference between a button that lies after
 * somebody presses `Win+Up` and one that does not.
 */

/** What a fullscreen change came back with. */
export type WindowResult =
  | { readonly kind: "ok"; readonly fullscreen: boolean }
  | { readonly kind: "unavailable"; readonly reason: string };

interface TauriWindow {
  isFullscreen(): Promise<boolean>;
  setFullscreen(value: boolean): Promise<void>;
}

interface TauriWithWindow {
  readonly window?: { getCurrentWindow(): TauriWindow };
}

/**
 * The window handle, or `null` with the sentence that says why not.
 *
 * Read at every call rather than cached, for the same reason the picker bridge is: `withGlobalTauri`
 * injects `window.__TAURI__` after the page's module scripts run, so anything captured at module load
 * would be `undefined` in the window and the feature would silently do nothing — which is precisely the
 * fault that put the browser stub into a shipped build.
 */
function currentWindow(): { readonly handle: TauriWindow } | { readonly reason: string } {
  const globals = window as unknown as {
    readonly __TAURI__?: TauriWithWindow;
    readonly __TAURI_INTERNALS__?: unknown;
  };
  const handle = globals.__TAURI__?.window?.getCurrentWindow();
  if (handle !== undefined) {
    return { handle };
  }
  return {
    reason:
      globals.__TAURI_INTERNALS__ === undefined
        ? "There is no window to change: this page has no window behind it."
        : "The window plugin did not install, so the window's fullscreen state cannot be read or set.",
  };
}

/** Whether the window is fullscreen right now, or `null` when there is no window to ask. */
export async function readFullscreen(): Promise<boolean | null> {
  const found = currentWindow();
  if ("reason" in found) {
    return null;
  }
  try {
    return await found.handle.isFullscreen();
  } catch {
    // A refusal here is not worth a message: the button simply keeps showing the state it can prove.
    return null;
  }
}

/** Put the window in or out of fullscreen. */
export async function setFullscreen(value: boolean): Promise<WindowResult> {
  const found = currentWindow();
  if ("reason" in found) {
    return { kind: "unavailable", reason: found.reason };
  }
  try {
    await found.handle.setFullscreen(value);
    return { kind: "ok", fullscreen: await found.handle.isFullscreen() };
  } catch (error) {
    return {
      kind: "unavailable",
      reason: `The window refused to change fullscreen state: ${String(error)}`,
    };
  }
}
