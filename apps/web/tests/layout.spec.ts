import { expect, test } from "@playwright/test";

/**
 * What the interface looks like, at a size a person uses.
 *
 * A screenshot is not an assertion, so this test makes none about pixels. What it does assert is the
 * thing a screenshot cannot show: that the first screen has no layout failure — nothing is clipped,
 * nothing scrolls sideways, and the two timecode fields are the same width and on one line. Those are
 * the failures that a passing behavioural test happily ignores and a person sees immediately.
 *
 * The images land in `screens/` for a human to look at, which is the other half of reviewing a UI and
 * is not something a test can do.
 */

test.use({ viewport: { width: 1440, height: 900 } });

test("the trim screen holds its layout with a master loaded", async ({ page }, testInfo) => {
  await page.goto("/");

  await page.getByRole("button", { name: "no project" }).click();
  await page.getByPlaceholder("Andy Ross interview").fill("A007C012 assembly");
  await page.getByRole("button", { name: "Create and open" }).click();
  await expect(page.getByRole("dialog", { name: "Projects" })).toBeHidden();

  await page.getByRole("button", { name: "Choose a video…" }).click();
  await page.getByRole("button", { name: "Add a master…" }).click();
  await expect(page.locator(".trim__master-name")).toBeVisible();
  const close = page.getByRole("button", { name: "Close" });
  if (await close.isVisible()) {
    await close.click();
  }
  await expect(page.getByRole("dialog", { name: "Projects" })).toBeHidden();

  await page.getByLabel("In point").fill("00:00:01:00");
  await page.getByLabel("Out point").fill("00:00:02:00");
  await expect(page.getByText("26 frames")).toBeVisible();
  await page.waitForTimeout(500);

  await testInfo.attach("trim", {
    body: await page.screenshot({ path: "screens/trim.png", fullPage: false }),
    contentType: "image/png",
  });

  // Nothing overflows the window horizontally, which is the failure that a screenshot shows as a
  // scrollbar and a test never notices.
  const overflow = await page.evaluate(() => ({
    docWidth: document.documentElement.scrollWidth,
    winWidth: window.innerWidth,
  }));
  expect(overflow.docWidth).toBeLessThanOrEqual(overflow.winWidth);

  // The two marks are on one line and the same width, because comparing them is the whole job.
  const inBox = await page.getByLabel("In point").boundingBox();
  const outBox = await page.getByLabel("Out point").boundingBox();
  expect(inBox).not.toBeNull();
  expect(outBox).not.toBeNull();
  if (inBox !== null && outBox !== null) {
    expect(Math.abs(inBox.y - outBox.y)).toBeLessThan(2);
    expect(Math.abs(inBox.width - outBox.width)).toBeLessThan(2);
    // A timecode field has to hold eleven characters of `HH:MM:SS:FF` without clipping.
    expect(inBox.width).toBeGreaterThan(220);
  }

  // The one primary action is the one that cuts, and there is exactly one of it on the screen.
  const primary = page.locator(".btn--primary");
  await expect(primary).toHaveCount(1);
  await expect(primary).toHaveText("Trim this segment");
});
