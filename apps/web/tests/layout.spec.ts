import { expect, test } from "@playwright/test";

/**
 * The panel, at the size it opens at.
 *
 * Two things this file exists to hold, and neither is about pixels.
 *
 * **The frame does not scroll.** This is a desktop application with a 1280x800 minimum that opens
 * fullscreen, and a scrollbar on the window means a control is off screen. Only three zones get their
 * own scroll — the queue, the transcript hits and the log — because only those can hold an unbounded
 * number of rows, and an unbounded list gets its own well rather than moving the application around it.
 *
 * **Nothing is folded away.** Every setting is visible with its value. An earlier version hid the
 * delivery preset, the handles and the verification policy behind three collapsed sections.
 */

test.use({ viewport: { width: 1920, height: 1080 }, colorScheme: "dark" });

/** Choose the fixture master. The stub answers the picker with a real path. */
async function chooseMaster(page: import("@playwright/test").Page): Promise<void> {
  await page.getByRole("button", { name: "Browse" }).click();
  await expect(page.getByRole("dialog", { name: "Choose a video" })).toBeVisible();
  await page.getByRole("button", { name: "Choose a file…" }).click();
  await expect(page.getByLabel("Video file")).toHaveValue(/\.mov$/);
}

test("the panel fits the window, and the frame does not scroll", async ({ page }, testInfo) => {
  await page.goto("/");
  await chooseMaster(page);

  await page.getByLabel("In point").fill("00:00:01:00");
  await page.getByLabel("Out point").fill("00:00:02:00");
  await expect(page.locator(".range__plan")).toContainText("26 frames");
  await page.waitForTimeout(400);

  await testInfo.attach("trim", {
    body: await page.screenshot({ path: "screens/trim.png", fullPage: false }),
    contentType: "image/png",
  });

  const frame = await page.evaluate(() => {
    const root = document.documentElement;
    const app = document.querySelector(".app");
    return {
      docScrollW: root.scrollWidth,
      docClientW: root.clientWidth,
      docScrollH: root.scrollHeight,
      docClientH: root.clientHeight,
      appH: app?.getBoundingClientRect().height ?? 0,
      winH: window.innerHeight,
    };
  });

  // The frame is exactly the viewport in both directions, and nothing overflows it.
  expect(frame.docScrollW).toBeLessThanOrEqual(frame.docClientW);
  expect(frame.docScrollH).toBeLessThanOrEqual(frame.docClientH);
  expect(Math.abs(frame.appH - frame.winH)).toBeLessThanOrEqual(1);

  // Every zone is on screen: nothing below the fold, nothing clipped.
  for (const zone of [
    "Video",
    "Caption file",
    "Range",
    "Options",
    "Queue",
    "Find in the transcript",
    "Progress",
  ]) {
    const heading = page.getByRole("heading", { name: zone, exact: true });
    await expect(heading).toBeVisible();
    const box = await heading.boundingBox();
    expect(box).not.toBeNull();
    if (box !== null) {
      expect(box.y).toBeGreaterThanOrEqual(0);
      expect(box.y + box.height).toBeLessThanOrEqual(frame.winH);
    }
  }
});

test("every setting is visible without opening anything", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);

  for (const control of ["In point", "Out point", "Delivery", "Handles", "Verify"]) {
    await expect(page.getByLabel(control)).toBeVisible();
  }

  // No disclosure anywhere: the whole point is that the panel shows its own state.
  await expect(page.locator("details")).toHaveCount(0);

  // One primary action in the window, and it is the one that cuts.
  await expect(page.locator(".btn--primary")).toHaveCount(1);
});

test("there is no breakpoint, because there is no second size", async ({ page }) => {
  // The whole point, asserted directly. A professional panel does not rearrange itself: an operator's
  // hand knows where the In field is, and the stylesheet must contain no viewport media query that
  // could move it. Checking the source is what makes that a fact rather than a hope.
  const layout = await (await page.request.get("/src/styles/app.css")).text();
  const tokens = await (await page.request.get("/src/styles/tokens.css")).text();

  for (const css of [layout, tokens]) {
    expect(css).not.toMatch(/@media\s*\(\s*max-width/);
    expect(css).not.toMatch(/@media\s*\(\s*min-width/);
  }

  // The media queries that *are* present are about the user's preferences, not about the viewport.
  expect(tokens).toContain("prefers-reduced-motion");
  expect(tokens).toContain("forced-colors");
});

test("the panel still fits at the window's own minimum", async ({ page }) => {
  // 1280x800 is the minimum the window declares. The layout does not reflow — it is a grid that fills
  // whatever it is given — so the test is that nothing overflows and every control is still reachable.
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await chooseMaster(page);

  for (const control of ["In point", "Out point", "Delivery", "Handles", "Verify"]) {
    await expect(page.getByLabel(control)).toBeVisible();
  }

  const frame = await page.evaluate(() => ({
    docScrollW: document.documentElement.scrollWidth,
    docClientW: document.documentElement.clientWidth,
    docScrollH: document.documentElement.scrollHeight,
    docClientH: document.documentElement.clientHeight,
  }));
  expect(frame.docScrollW).toBeLessThanOrEqual(frame.docClientW);
  expect(frame.docScrollH).toBeLessThanOrEqual(frame.docClientH);
});
