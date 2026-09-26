/**
 * The entry point.
 *
 * It loads the stylesheet in the right order, installs the browser bridge when there is no window to
 * talk to, mounts the application, and gives React somewhere to report a crash. A render error in a
 * desktop application must not be a blank window — the user has no console to look at and no way to
 * reload.
 *
 * ## Why the stub is behind `import.meta.env.DEV`, and not merely guarded
 *
 * `installStub()` used to be called unconditionally, on the argument that it refuses to replace a real
 * bridge and is therefore safe. It is not safe, and the reason is a race: it tested for
 * `window.__TAURI__`, the convenience global that `withGlobalTauri` injects **late**, while the actual
 * IPC bridge `window.__TAURI_INTERNALS__` is there from the first script. The stub won.
 *
 * The shipped window therefore ran the whole application on fixtures — `doctor` reported a hard-coded
 * ffmpeg build string, the plan and the cut were computed in JavaScript, and the file picker returned a
 * fixed path without opening anything. Every browser test passed, the smoke test passed, and the
 * application did nothing.
 *
 * A dev-only affordance does not belong in a shipped bundle at all, so it is no longer in one: Vite
 * folds `import.meta.env.DEV` to `false` for a production build, the branch goes, and Rollup drops the
 * module with it. `tools/check-bundle.mjs` then fails the build if any trace of the stub survives into
 * `dist` — which is what makes this a fact rather than an intention.
 *
 * In the window the real bridge is used, or the interface fails loudly. There is no third outcome, and
 * that is the point: a silent fallback to fake data is worse than an error.
 */

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { installStub } from "./ipc/stub";
import "./styles/fonts.css";
import "./styles/tokens.css";
import "./styles/global.css";
import "./styles/app.css";

/*
 * A static import, because a top-level `await import()` would raise the bundle's syntax target for one
 * development affordance. Rollup folds the branch away — `import.meta.env.DEV` is literally `false`
 * here — and then `installStub` has no remaining reference, so the module goes with it.
 * `tools/check-bundle.mjs` proves that happened instead of trusting it.
 */
if (import.meta.env.DEV) {
  installStub();
}

const container = document.getElementById("root");
if (container === null) {
  throw new Error("the page has no #root element to mount into");
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
