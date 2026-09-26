/**
 * TheTrimmer's window.
 *
 * ## What this process is for
 *
 * It does three things and nothing else: it starts the Python engine, it opens a window on
 * whatever the engine is serving, and it answers the four questions a page cannot answer for
 * itself — where is a file, show me that in Explorer, and make the window fill the screen.
 *
 * Everything else is the engine's. The page is served by the engine and talks to it on that
 * same origin, so this process never has to grow a second API and the page needs no CORS.
 *
 * ## Why the page comes from the engine rather than from disk
 *
 * `loadFile` was the first attempt and it cannot work. A Vite build is an ES module, and
 * Chromium refuses a module script loaded from `file://` — the origin is opaque, so it fails
 * CORS before a line of the application runs. Even had it loaded, every call to the engine
 * would have been `file://` -> `http://127.0.0.1:8765`: cross-origin as well, and so needing
 * CORS headers on every response to talk to its own backend. One origin deletes both.
 *
 * ## Why the window opens maximized rather than fullscreen
 *
 * A borderless fullscreen window has no titlebar, and a window with no titlebar cannot be
 * restored, minimized or closed. It was shipped that way once: `GetWindowLong` on it
 * returned `WS_VISIBLE | WS_CLIPCHILDREN` and nothing else — no caption, no system menu, no
 * minimize box, no maximize box. Maximized fills the screen and keeps all of them, and F11
 * is offered as an explicit, reversible alternative.
 */

const { app, BrowserWindow, dialog, ipcMain, shell } = require("electron");
const path = require("path");
const fs = require("fs");
const { spawn } = require("child_process");

const BACKEND_PORT = 8765;
const BACKEND = `http://127.0.0.1:${BACKEND_PORT}`;
const ROOT = path.join(__dirname, "..");

let mainWindow = null;
let backend = null;
/** True once the application has decided to quit, so a shutdown is not reported as a fault. */
let stopping = false;

/** Only one copy of the application at a time: two would fight over the port and the store. */
if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on("second-instance", () => {
    if (mainWindow) {
      if (mainWindow.isMinimized()) mainWindow.restore();
      mainWindow.focus();
    }
  });
}

/** The project's own virtual environment, or the system interpreter if there is not one. */
function interpreter() {
  const inVenv = path.join(ROOT, "venv", "Scripts", "python.exe");
  return fs.existsSync(inVenv) ? inVenv : "python";
}

function startBackend() {
  const script = path.join(ROOT, "backend", "server.py");
  console.log(`[thetrimmer] backend: ${interpreter()} ${script}`);
  backend = spawn(interpreter(), [script], {
    cwd: ROOT,
    env: { ...process.env, PORT: String(BACKEND_PORT), PYTHONIOENCODING: "utf-8" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  backend.stdout.on("data", (chunk) => process.stdout.write(`[engine] ${chunk}`));
  backend.stderr.on("data", (chunk) => process.stderr.write(`[engine] ${chunk}`));
  backend.on("close", (code) => {
    // A backend killed by `stopBackend` reports a null code, because a signal is not an exit
    // status. Saying "exited with null" during a normal quit reads as a crash, and a log that
    // cries wolf on every clean shutdown is a log nobody reads on the one that matters.
    console.log(
      stopping
        ? "[thetrimmer] backend stopped"
        : `[thetrimmer] backend exited unexpectedly (code ${code}); the window cannot cut ` +
            `anything until it is restarted`,
    );
    backend = null;
  });
}

function stopBackend() {
  stopping = true;
  if (backend) {
    backend.kill();
    backend = null;
  }
}

/**
 * Wait until the engine answers.
 *
 * The window is loaded with `loadURL` against the engine, so navigating before it is
 * listening shows "connection refused" instead of the application. A cut cannot start for a
 * few hundred milliseconds either way; a wrong first paint is what people remember.
 */
async function waitForBackend(timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const answer = await fetch(`${BACKEND}/api/health`);
      if (answer.ok) {
        return true;
      }
    } catch {
      // Not listening yet. That is the expected answer for the first few attempts.
    }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  return false;
}

/** A window with a reason in it, for when the engine never came up. */
function failurePage(detail) {
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8">
<title>TheTrimmer</title><style>
body{margin:0;display:grid;place-items:center;height:100vh;background:#0d1013;color:#e9edf1;
font-family:system-ui,sans-serif}
main{max-width:46rem;padding:2rem}h1{font-size:1.1rem;font-weight:600;margin:0 0 .75rem}
p{margin:0;color:#9aa5b1;line-height:1.6}code{font-family:Consolas,monospace;color:#e9edf1}
</style></head><body><main><h1>The engine did not start</h1>
<p>${detail}</p>
<p style="margin-top:1rem">Run <code>start.bat</code> from the project folder to see the
full error.</p></main></body></html>`;
  return `data:text/html;charset=utf-8,${encodeURIComponent(html)}`;
}

function createWindow(engineUp) {
  mainWindow = new BrowserWindow({
    width: 1600,
    height: 1000,
    minWidth: 1440,
    minHeight: 960,
    // Maximized, not fullscreen: the operating system's titlebar is what makes the window
    // restorable, minimizable and closable, and none of that can be given up.
    show: false,
    autoHideMenuBar: true,
    backgroundColor: "#0d1013",
    title: "TheTrimmer",
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
    },
  });

  mainWindow.setMenu(null);
  mainWindow.maximize();

  // The engine's own page, or the dev server when one was asked for. `THE_TRIMMER_DEV` is how
  // a hot-reloading session is started without a second entry point.
  const dev = process.env.THE_TRIMMER_DEV;
  if (dev) {
    mainWindow.loadURL(dev);
  } else if (engineUp) {
    mainWindow.loadURL(`${BACKEND}/`);
  } else {
    mainWindow.loadURL(
      failurePage(
        `Nothing answered on <code>${BACKEND}/api/health</code> after 30 seconds. ` +
          `The interpreter used was <code>${interpreter()}</code>.`,
      ),
    );
  }

  mainWindow.webContents.on("did-fail-load", (_event, code, description) => {
    console.error(`[thetrimmer] the interface did not load: ${code} ${description}`);
  });

  mainWindow.once("ready-to-show", () => mainWindow.show());
  mainWindow.on("closed", () => {
    mainWindow = null;
  });

  // A renderer that reports its own fullscreen state is a renderer that can be wrong; the
  // window knows, so it is asked.
  mainWindow.on("enter-full-screen", () => mainWindow.webContents.send("fullscreen", true));
  mainWindow.on("leave-full-screen", () => mainWindow.webContents.send("fullscreen", false));
}

app.whenReady().then(async () => {
  startBackend();
  const engineUp = await waitForBackend();
  if (!engineUp) {
    console.error("[thetrimmer] the engine never answered on " + BACKEND);
  }
  createWindow(engineUp);
  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow(engineUp);
  });
});

app.on("window-all-closed", () => {
  stopBackend();
  app.quit();
});
app.on("before-quit", stopBackend);

// ---------------------------------------------------------------------------------------
// The four things a page cannot do for itself
// ---------------------------------------------------------------------------------------

ipcMain.handle("open-video", async () => {
  const result = await dialog.showOpenDialog(mainWindow, {
    title: "Choose a video",
    properties: ["openFile"],
    filters: [
      {
        name: "Video",
        extensions: ["mp4", "mov", "mkv", "m4v", "mxf", "avi", "webm", "mts", "m2ts"],
      },
    ],
  });
  // Cancelling is a decision and not a fault, so it is `null` rather than an error.
  return result.canceled || result.filePaths.length === 0 ? null : result.filePaths[0];
});

ipcMain.handle("reveal", async (_event, target) => {
  if (fs.existsSync(target) && fs.statSync(target).isFile()) {
    shell.showItemInFolder(target);
  } else {
    await shell.openPath(target);
  }
});

ipcMain.handle("toggle-fullscreen", () => {
  if (mainWindow === null) return false;
  mainWindow.setFullScreen(!mainWindow.isFullScreen());
  return mainWindow.isFullScreen();
});

ipcMain.handle("is-fullscreen", () => (mainWindow === null ? false : mainWindow.isFullScreen()));

ipcMain.handle("backend-url", () => `http://127.0.0.1:${BACKEND_PORT}`);
