/**
 * Helpers the browser tests share.
 *
 * Not a spec — Playwright only collects `*.spec.ts` — so nothing here runs on its own. It exists because
 * three files now need the same four moves: choose a master, mark a range, pick a *different* file, and
 * take the picker away. A copy of each in every file is how they drift.
 */

import { expect } from "@playwright/test";

type Page = import("@playwright/test").Page;

/**
 * Choose the fixture master.
 *
 * `Browse` opens the file picker, and in a browser the picker is the stub — it answers with a real path
 * immediately, so this is one click. That is also the behaviour in the window: one click, the operating
 * system's own dialog, done.
 */
export async function chooseMaster(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Browse" }).click();
  await expect(page.getByLabel("Video file")).toHaveValue(/\.mov$/);
}

/** Type a range, and wait for the plan line to agree that both marks parse. */
export async function mark(page: Page, inPoint: string, outPoint: string): Promise<void> {
  await page.getByLabel("In point").fill(inPoint);
  await page.getByLabel("Out point").fill(outPoint);
  await expect(page.locator(".range__plan")).not.toContainText("Type both timecodes");
}

/**
 * Answer the next picker call with a chosen path, so a test can pick a *different* file.
 *
 * The stub's picker returns the fixture master every time, which is the right default and the wrong
 * thing for testing what happens when the operator changes their mind.
 */
export async function pickInstead(page: Page, path: string): Promise<void> {
  await page.evaluate((chosen) => {
    (window as unknown as { __TAURI__: { dialog: { open: unknown } } }).__TAURI__.dialog.open =
      async () => chosen;
  }, path);
}

/**
 * Take the picker away, so `Browse` has nothing to open.
 *
 * The stub installs `window.__TAURI__.dialog` itself, which means the fallback path — the case that
 * matters when a build has no dialog plugin — is unreachable in a browser test unless it is removed on
 * purpose. `__TAURI_INTERNALS__` is absent in a browser too, so `pickFile` reports `unavailable` and the
 * in-app dialog is what should appear.
 */
export async function removeThePicker(page: Page): Promise<void> {
  await page.evaluate(() => {
    delete (window as unknown as { __TAURI__?: { dialog?: unknown } }).__TAURI__?.dialog;
  });
}

/**
 * Queue one range and start the run, returning once the window has begun working.
 *
 * The stub narrates a run — step boundaries, command lines, positions, verdicts — over the same
 * `cut-progress` event the window uses, compressed to about a third of a second per segment. Nothing
 * else in a browser produces those events, so this is the only way to see the progress display at all.
 */
export async function startRun(page: Page, inPoint = "00:00:01:00", outPoint = "00:00:02:00"): Promise<void> {
  await mark(page, inPoint, outPoint);
  await page.getByRole("button", { name: "Queue" }).click();
  await expect(page.locator(".cut-table__row")).toHaveCount(1);
  await page.getByRole("button", { name: "Trim 1" }).click();
}
