/**
 * The file picker, and nothing else.
 *
 * The Tauri capability grants the page a dialog and nothing else, so this is the only part of the
 * interface that reaches outside itself — and it can only ask the operating system to show a picker.
 * It cannot read, write or list anything.
 *
 * Wrapped in a function with a `null` return rather than throwing on cancel, because cancelling a
 * picker is a normal thing to do and a component should not need a try/catch to handle it.
 */

interface DialogBridge {
  open(options: {
    readonly title?: string;
    readonly multiple?: boolean;
    readonly directory?: boolean;
    readonly filters?: readonly { readonly name: string; readonly extensions: readonly string[] }[];
  }): Promise<string | string[] | null>;
  save(options: {
    readonly title?: string;
    readonly defaultPath?: string;
    readonly filters?: readonly { readonly name: string; readonly extensions: readonly string[] }[];
  }): Promise<string | null>;
}

interface TauriWithDialog {
  readonly dialog?: DialogBridge;
}

/**
 * Ask for one existing file. Returns `null` when the user cancels.
 *
 * There is no `canPick` companion: a caller that needs to know whether a picker exists asks
 * {@link pickFile} and gets `null`, and a caller that wants to offer a typed path instead offers one
 * unconditionally. A predicate that says "the picker is unavailable" earns nothing that the return
 * value of this function does not already say.
 */
export async function pickFile(options: {
  readonly title: string;
  readonly filters?: readonly { readonly name: string; readonly extensions: readonly string[] }[];
}): Promise<string | null> {
  const bridge = (window as unknown as { readonly __TAURI__?: TauriWithDialog }).__TAURI__;
  if (bridge?.dialog === undefined) {
    return null;
  }
  const chosen = await bridge.dialog.open({
    title: options.title,
    multiple: false,
    directory: false,
    ...(options.filters === undefined ? {} : { filters: options.filters }),
  });
  return typeof chosen === "string" ? chosen : null;
}

/** Ask where to write a file. Returns `null` when the user cancels. */
export async function pickSave(options: {
  readonly title: string;
  readonly defaultPath?: string;
  readonly filters?: readonly { readonly name: string; readonly extensions: readonly string[] }[];
}): Promise<string | null> {
  const bridge = (window as unknown as { readonly __TAURI__?: TauriWithDialog }).__TAURI__;
  if (bridge?.dialog === undefined) {
    return null;
  }
  const chosen = await bridge.dialog.save({
    title: options.title,
    ...(options.defaultPath === undefined ? {} : { defaultPath: options.defaultPath }),
    ...(options.filters === undefined ? {} : { filters: options.filters }),
  });
  return typeof chosen === "string" ? chosen : null;
}
