/**
 * The bridge between the page and the window.
 *
 * Five functions, and that is the whole surface. Everything else the page needs is HTTP to
 * the engine on loopback, which needs no bridge — so the page holds no filesystem, no
 * process and no node handle it could be tricked into using, and the preload stays small
 * enough to read in one go.
 */

const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("electronAPI", {
  /** Ask for a video with the operating system's own dialog. `null` when cancelled. */
  openVideo: () => ipcRenderer.invoke("open-video"),

  /** Show a file in Explorer. */
  reveal: (target) => ipcRenderer.invoke("reveal", target),

  /** Fullscreen, which the window owns because it is the window's state and not the page's. */
  toggleFullscreen: () => ipcRenderer.invoke("toggle-fullscreen"),
  isFullscreen: () => ipcRenderer.invoke("is-fullscreen"),

  /** Where the engine is listening. */
  backendUrl: () => ipcRenderer.invoke("backend-url"),
});
