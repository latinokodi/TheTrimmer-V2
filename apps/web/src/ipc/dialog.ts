/**
 * The file picker, and nothing else.
 *
 * The Tauri capability grants the page a dialog and nothing else, so this is the only part of the
 * interface that reaches outside itself — and it can only ask the operating system to show a picker.
 * It cannot read, write or list anything.
 *
 * ## Three outcomes, not two, and why that distinction is the whole file
 *
 * This used to answer `string | null`, where `null` meant "the user cancelled". It also meant "there is
 * no picker here", and those two are not the same thing at all: cancelling is a decision, and having no
 * picker is a fault. Collapsing them is what made a dead Browse button look like a button that worked —
 * it silently did nothing, twice, and looked identical both times.
 *
 * So a call answers one of:
 *
 * * `picked` — a path, from the operating system's own dialog;
 * * `cancelled` — the operator closed the picker, which is a normal thing to do and calls for no
 *   message;
 * * `unavailable` — there is no picker to show, with the reason. The caller decides what to offer
 *   instead, and the interface has a real answer for it: the in-app dialog, where a path can be typed.
 *
 * There is deliberately no `hasPicker()` predicate. Asking a question about a capability and then acting
 * on the answer is two code paths that can disagree; the call itself reports what happened.
 */

/** What a call to the picker came back with. */
export type PickResult =
  | { readonly kind: "picked"; readonly path: string }
  | { readonly kind: "cancelled" }
  | { readonly kind: "unavailable"; readonly reason: string };

/** One filter row, as Tauri's dialog plugin takes it. */
export interface PickFilter {
  readonly name: string;
  readonly extensions: readonly string[];
}

/** The video containers the engine can read. Shared, so the picker and the dialog cannot drift. */
export const VIDEO_FILTERS: readonly PickFilter[] = [
  {
    name: "Video",
    extensions: ["mp4", "mov", "mkv", "m4v", "mxf", "avi", "webm", "mts", "m2ts"],
  },
];

interface DialogBridge {
  open(options: {
    readonly title?: string;
    readonly multiple?: boolean;
    readonly directory?: boolean;
    readonly filters?: readonly PickFilter[];
  }): Promise<string | string[] | null>;
  save(options: {
    readonly title?: string;
    readonly defaultPath?: string;
    readonly filters?: readonly PickFilter[];
  }): Promise<string | null>;
}

interface TauriWithDialog {
  readonly dialog?: DialogBridge;
}

/**
 * The bridge, or `null` with the sentence that says why not.
 *
 * The distinction between the two globals matters and is the reason this is one function rather than a
 * property read at each call site. `__TAURI__` is the convenience API that `withGlobalTauri` injects and
 * it can arrive late; `__TAURI_INTERNALS__` is the IPC bridge and is present from the first script. A
 * picker that is missing because nothing has finished loading yet is a *different* fault from a build
 * with no dialog plugin in it, and the message says which one this is.
 */
function picker(): { readonly bridge: DialogBridge } | { readonly reason: string } {
  const globals = window as unknown as {
    readonly __TAURI__?: TauriWithDialog;
    readonly __TAURI_INTERNALS__?: unknown;
  };
  const dialog = globals.__TAURI__?.dialog;
  if (dialog !== undefined) {
    return { bridge: dialog };
  }
  if (globals.__TAURI_INTERNALS__ === undefined) {
    return {
      reason:
        "There is no file picker here: this page has no window behind it. Open TheTrimmer's window to " +
        "pick a file, or type the path.",
    };
  }
  return {
    reason:
      "There is no file picker here: the window is up but its dialog plugin did not install. Type the " +
      "path instead, or report this — nothing else in the application is affected.",
  };
}

/** Ask for one existing file. */
export async function pickFile(options: {
  readonly title: string;
  readonly filters?: readonly PickFilter[];
}): Promise<PickResult> {
  const available = picker();
  if ("reason" in available) {
    return { kind: "unavailable", reason: available.reason };
  }
  const chosen = await available.bridge.open({
    title: options.title,
    multiple: false,
    directory: false,
    ...(options.filters === undefined ? {} : { filters: options.filters }),
  });
  if (typeof chosen === "string" && chosen !== "") {
    return { kind: "picked", path: chosen };
  }
  return { kind: "cancelled" };
}

/** Ask where to write a file. */
export async function pickSave(options: {
  readonly title: string;
  readonly defaultPath?: string;
  readonly filters?: readonly PickFilter[];
}): Promise<PickResult> {
  const available = picker();
  if ("reason" in available) {
    return { kind: "unavailable", reason: available.reason };
  }
  const chosen = await available.bridge.save({
    title: options.title,
    ...(options.defaultPath === undefined ? {} : { defaultPath: options.defaultPath }),
    ...(options.filters === undefined ? {} : { filters: options.filters }),
  });
  if (typeof chosen === "string" && chosen !== "") {
    return { kind: "picked", path: chosen };
  }
  return { kind: "cancelled" };
}
