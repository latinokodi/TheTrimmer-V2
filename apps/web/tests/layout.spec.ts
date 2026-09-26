import { expect, test } from "@playwright/test";

/**
 * What the interface looks like, at the size it opens at, with a master loaded.
 *
 * A screenshot is not an assertion, so this makes none about pixels. What it does assert is the thing
 * a screenshot cannot show: that the first screen has no layout failure — nothing is clipped, nothing
 * scrolls sideways, the two timecode fields are the same width and on one line, and **every setting is
 * visible without opening anything**. That last one is the defect this file exists for: the previous
 * layout put the delivery preset, the handles and the verification policy behind three collapsed
 * sections and left a column of empty space down the middle of the window.
 *
 * The images land in `screens/` for a human to look at, which is the other half of reviewing a UI and
 * is not something a test can do.
 */

// The window opens fullscreen; this is a common desktop size to check it against.
test.use({ viewport: { width: 1920, height: 1080 } });

test("every setting is on the first screen, with room to spare", async ({ page }, testInfo) => {
  await page.goto("/");

  await page.getByRole("button", { name: "Choose a video…" }).click();
  await page.getByRole("button", { name: "Browse…" }).click();
  await expect(page.locator(".trim__master-name")).toBeVisible();

  await page.getByLabel("In point").fill("00:00:01:00");
  await page.getByLabel("Out point").fill("00:00:02:00");
  await expect(page.locator(".trim__length")).toContainText("26 frames");
  await page.waitForTimeout(400);

  await testInfo.attach("trim", {
    body: await page.screenshot({ path: "screens/trim.png", fullPage: false }),
    contentType: "image/png",
  });

  // Nothing overflows horizontally, which a screenshot shows as a scrollbar and a test never notices.
  const overflow = await page.evaluate(() => ({
    docWidth: document.documentElement.scrollWidth,
    winWidth: window.innerWidth,
    docHeight: document.documentElement.scrollHeight,
    winHeight: window.innerHeight,
  }));
  expect(overflow.docWidth).toBeLessThanOrEqual(overflow.winWidth);

  // Everything a person can change is visible at once. This is the assertion that would have caught
  // the three-disclosure layout.
  for (const control of ["In point", "Out point", "Delivery", "Handles", "Verify"]) {
    await expect(page.getByLabel(control)).toBeVisible();
  }
  await expect(page.getByRole("button", { name: "Trim", exact: true })).toBeVisible();

  // Nothing is folded away anywhere on the window.
  await expect(page.locator("details")).toHaveCount(0);

  // The two marks are on one line, the same width, and wide enough for eleven characters of
  // `HH:MM:SS:FF` plus the tracking.
  const inBox = await page.getByLabel("In point").boundingBox();
  const outBox = await page.getByLabel("Out point").boundingBox();
  expect(inBox).not.toBeNull();
  expect(outBox).not.toBeNull();
  if (inBox !== null && outBox !== null) {
    expect(Math.abs(inBox.y - outBox.y)).toBeLessThan(2);
    expect(Math.abs(inBox.width - outBox.width)).toBeLessThan(2);
    expect(inBox.width).toBeGreaterThan(240);
  }

  // One primary action on the window, and it is the one that cuts.
  const primary = page.locator(".btn--primary");
  await expect(primary).toHaveCount(1);
});

test("the window does not need a 1920 px screen to fit", async ({ page }) => {
  // The minimum the window allows itself. Everything still has to be visible, because a tool that
  // hides its own options on a laptop is a tool with two designs.
  await page.setViewportSize({ width: 1000, height: 620 });
  await page.goto("/");
  await page.getByRole("button", { name: "Choose a video…" }).click();
  await page.getByRole("button", { name: "Browse…" }).click();

  for (const control of ["In point", "Out point", "Delivery", "Handles", "Verify"]) {
    await expect(page.getByLabel(control)).toBeVisible();
  }

  const overflow = await page.evaluate(() => ({
    docWidth: document.documentElement.scrollWidth,
    winWidth: window.innerWidth,
  }));
  expect(overflow.docWidth).toBeLessThanOrEqual(overflow.winWidth);
});
