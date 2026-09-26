import { defineConfig, devices } from "@playwright/test";

/**
 * The interface's browser tests.
 *
 * ## What these are, and what they are not
 *
 * These drive the real interface — the same `App`, the same `commands.ts`, the same stylesheet the
 * window loads — in a browser, against the stub bridge in `src/ipc/stub.ts`. There is no exe, no
 * WebView2, no Rust, and no build: `webServer` starts Vite, which serves the source with HMR, so a
 * run is a couple of seconds.
 *
 * They assert on what a person sees: which controls are offered, what the fields say, what the
 * frame numbers work out to, whether the button that cuts is enabled and what it is called. Those are
 * exactly the failures the Rust suite cannot see — a window that renders and does nothing still
 * passes all 404 of its tests.
 *
 * ## What is *not* covered here, and where it is covered
 *
 * The *shape* of what Rust sends. The stub agrees with the interface by construction, so a renamed
 * field would keep these tests green. That half lives in
 * `apps/desktop/src-tauri/tests/ipc_contract.rs`, which drives the real commands through Tauri's real
 * invoke handler and asserts on the real JSON — including `add_source` returning the same object the
 * `sources` command does, which is a bug this division found.
 *
 * ## Why one browser
 *
 * The window is WebView2, which is Chromium. Testing three engines would be testing code that never
 * runs on any of them.
 */
const ci = process.env["CI"] !== undefined;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  forbidOnly: ci,
  retries: 0,
  // One worker on CI, where the machine is shared and the point is a clean signal rather than a fast
  // one; as many as it likes locally. Built as two configs rather than a conditional property because
  // `exactOptionalPropertyTypes` is on and `workers: undefined` is not the same as omitting it.
  ...(ci ? { workers: 1 } : {}),
  reporter: ci ? [["list"], ["github"]] : "list",
  // Generous, because the slowest test here drives a whole workflow and 15 s is the sort of budget
  // that fails once on a loaded machine and teaches everybody to re-run rather than to look.
  timeout: 30_000,
  expect: { timeout: 5_000 },

  use: {
    baseURL: "http://127.0.0.1:5173",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },

  projects: [{ name: "webview2", use: { ...devices["Desktop Chrome"] } }],

  webServer: {
    command: "npm run dev",
    url: "http://127.0.0.1:5173",
    reuseExistingServer: !ci,
    stdout: "ignore",
    stderr: "pipe",
    timeout: 60_000,
  },
});
