/**
 * The entry point.
 *
 * It loads the stylesheet in the right order, mounts the application, and gives React
 * somewhere to report a crash. A render error in a desktop application must not be a blank
 * window — the user has no console to look at and no way to reload.
 *
 * There is no bridge to install any more. The window talks to the engine over the local
 * backend the Electron main process starts, so the page is an ordinary web page and the
 * same code runs in a browser during development.
 */

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles/fonts.css";
import "./styles/tokens.css";
import "./styles/global.css";
import "./styles/app.css";

const container = document.getElementById("root");
if (container === null) {
  throw new Error("the page has no #root element to mount into");
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
