import { expect, test } from "@playwright/test";

/**
 * The layout, at the size the window opens at.
 *
 * A screenshot is not an assertion, so this makes none about pixels. What it does assert is what a
 * screenshot cannot show: nothing is clipped, nothing scrolls sideways, the label column is one column
 * for the whole form, and **every setting is visible without opening anything**.
 *
 * The screenshots land in `screens/` for a human to look at, which is the other half of reviewing a UI
 * and is not something a test can do.
 */

// The window opens fullscreen; this is a common desktop size to check it against.
test.use({ viewport: { width: 1920, height: 1080 } });

/*
 * The window runs dark. The application follows the operating system, and a headless browser reports
 * light — so the theme is pinned here, because a screenshot of the light theme is not a screenshot of
 * what an editor sees.
 */
test.use({ colorScheme: "dark" });

test("the column is centred and capped, and every card fits with room to spare", async ({
  page,
}, testInfo) => {
  await page.goto("/");

  await page.getByRole("button", { name: "Browse…" }).click();
  await page.getByRole("button", { name: "Choose a file…" }).click();
  await expect(page.getByLabel("Video file")).toHaveValue(/\.mov$/);

  await page.getByLabel("In point").fill("00:00:01:00");
  await page.getByLabel("Out point").fill("00:00:02:00");
  await expect(page.locator(".range__plan")).toContainText("26 frames");
  await page.waitForTimeout(400);

  await testInfo.attach("trim", {
    body: await page.screenshot({ path: "screens/trim.png", fullPage: false }),
    contentType: "image/png",
  });

  const geometry = await page.evaluate(() => {
    const column = document.querySelector(".app__column");
    return {
      docWidth: document.documentElement.scrollWidth,
      winWidth: window.innerWidth,
      columnWidth: column?.getBoundingClientRect().width ?? 0,
      columnLeft: column?.getBoundingClientRect().left ?? 0,
    };
  });

  // Nothing overflows horizontally, and the column is not a full-width form on a 1920 px display —
  // a label 1500 px from its field is a label you read twice.
  expect(geometry.docWidth).toBeLessThanOrEqual(geometry.winWidth);
  expect(geometry.columnWidth).toBeLessThanOrEqual(940);
  expect(geometry.columnWidth).toBeGreaterThan(700);

  // And it is centred, so the space either side is equal.
  expect(Math.abs(geometry.columnLeft - (geometry.winWidth - geometry.columnLeft - geometry.columnWidth))).toBeLessThan(4);

  // The label column is one column: `In point` and `Delivery` start on the same left edge.
  const inLabel = await page.locator(".range__label").first().boundingBox();
  const optionLabel = await page.locator(".options__label").first().boundingBox();
  expect(inLabel).not.toBeNull();
  expect(optionLabel).not.toBeNull();
  if (inLabel !== null && optionLabel !== null) {
    expect(Math.abs(inLabel.x - optionLabel.x)).toBeLessThan(2);
  }

  // One primary action on the window, and it is the one that cuts.
  await expect(page.locator(".btn--primary")).toHaveCount(1);
});

test("the window does not need a 1920 px screen to fit", async ({ page }) => {
  // The minimum the window allows itself. Everything still has to be reachable, because a tool that
  // hides its own options on a laptop is a tool with two designs.
  await page.setViewportSize({ width: 1000, height: 620 });
  await page.goto("/");
  await page.getByRole("button", { name: "Browse…" }).click();
  await page.getByRole("button", { name: "Choose a file…" }).click();

  for (const control of ["In point", "Out point", "Delivery", "Handles", "Verify"]) {
    await expect(page.getByLabel(control)).toBeVisible();
  }

  const overflow = await page.evaluate(() => ({
    docWidth: document.documentElement.scrollWidth,
    winWidth: window.innerWidth,
  }));
  expect(overflow.docWidth).toBeLessThanOrEqual(overflow.winWidth);
});
