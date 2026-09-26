/**
 * The entry point.
 *
 * It does three things and nothing else: load the stylesheet in the right order so the tokens are
 * available to everything below them, mount the application, and give React somewhere to report a
 * crash. A render error in a desktop application must not be a blank window — the user has no
 * console to look at and no way to reload.
 */

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles/tokens.css";
import "./styles/global.css";

const container = document.getElementById("root");
if (container === null) {
  throw new Error("the page has no #root element to mount into");
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
