/**
 * The entry point.
 *
 * It loads the stylesheet in the right order, installs the browser bridge when there is no window to
 * talk to, mounts the application, and gives React somewhere to report a crash. A render error in a
 * desktop application must not be a blank window — the user has no console to look at and no way to
 * reload.
 *
 * ## The browser bridge, and why it is first
 *
 * `installStub()` is a no-op inside the real window, because the real bridge is already there and the
 * stub refuses to replace it. In a plain browser — `npm run dev`, or a Playwright test — it is what
 * makes the interface work at all. Importing it eagerly costs one module and removes the need for any
 * other way to develop or test the interface: no exe, no WebView2, no Tauri, no build.
 */

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { installStub } from "./ipc/stub";
import "./styles/tokens.css";
import "./styles/global.css";
import "./styles/app.css";

installStub();

const container = document.getElementById("root");
if (container === null) {
  throw new Error("the page has no #root element to mount into");
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
