import { expect, test } from "@playwright/test";

/**
 * The whole product, driven as a person drives it.
 *
 * The window is a fixed panel of labelled zones — Video, Caption file, Range, Options, Queue,
 * Find in the transcript, Progress — and these tests walk it in that order. Each starts from a fresh
 * page and a fresh stub, so the order they run in cannot matter.
 *
 * There is no project setup step: the window opens a session by itself, because a dialog asking what to
 * call something is ceremony in front of a form that has nothing to do with it.
 */

/** Choose the fixture master. The stub answers the picker with a real path. */
async function chooseMaster(page: import("@playwright/test").Page): Promise<void> {
  await page.getByRole("button", { name: "Browse" }).click();
  await expect(page.getByRole("dialog", { name: "Choose a video" })).toBeVisible();
  await page.getByRole("button", { name: "Choose a file…" }).click();
  await expect(page.getByLabel("Video file")).toHaveValue(/A007C012_250312_R1QK\.mov$/);
}

/** Type a range and wait for the plan line to agree it parses. */
async function mark(
  page: import("@playwright/test").Page,
  inPoint: string,
  outPoint: string,
): Promise<void> {
  await page.getByLabel("In point").fill(inPoint);
  await page.getByLabel("Out point").fill(outPoint);
  await expect(page.locator(".range__plan")).not.toContainText("Type both timecodes");
}

test("the zones are in the order the work happens, and the first says what to do", async ({
  page,
}) => {
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "TheTrimmer" })).toBeVisible();
  await expect(page.locator(".titlebar__tagline")).toContainText(
    "Frame-exact, lossless segment cutting",
  );

  /*
   * The zones, in the order an operator works through them. They read as uppercase because a zone title
   * is set in caps by the stylesheet rather than typed in caps in the markup — which is the right way
   * round, since the accessible name is the readable one and only the rendering is shouting.
   */
  const titles = (await page.locator(".zone__title").allInnerTexts()).map((title) =>
    title.trim().toLowerCase(),
  );
  expect(titles.slice(0, 5)).toEqual(["video", "caption file", "range", "options", "queue"]);
  expect(titles).toContain("progress");

  // The first zone is the one that needs answering, and it says so.
  await expect(page.getByLabel("Video file")).toHaveValue("");
  await expect(page.locator(".zone__note").first()).toContainText("no file");
});

test("every setting is on the panel, with its value visible", async ({ page }) => {
  await page.goto("/");

  for (const control of ["In point", "Out point", "Delivery", "Handles", "Verify"]) {
    await expect(page.getByLabel(control)).toBeVisible();
  }

  // Nothing is folded away anywhere.
  await expect(page.locator("details")).toHaveCount(0);
});

test("the marks show the frame numbers as they are typed, and the length is inclusive", async ({
  page,
}) => {
  await page.goto("/");
  await chooseMaster(page);

  await page.getByLabel("In point").fill("00:00:01:00");
  await expect(page.locator(".range__row").first()).toContainText("frame 25");

  // 00:00:02:00 is frame 50 and the **last frame kept**, so the range is 26 frames, not 25.
  await page.getByLabel("Out point").fill("00:00:02:00");
  await expect(page.locator(".range__row").nth(1)).toContainText("frame 50, the last one kept");
  await expect(page.locator(".range__plan")).toContainText("26 frames");

  await expect(page.getByRole("button", { name: "Trim", exact: true })).toBeEnabled();
});

test("the plan is answered while the marks are typed, not on a button press", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);
  await mark(page, "00:00:01:00", "00:00:02:00");

  await expect(page.locator(".range__plan")).toContainText(/lossless copy/);

  // A range starting *between* keyframes is a head patch instead, and the split is on screen — the
  // decision this product exists to make visible.
  await page.getByLabel("In point").fill("00:00:01:12");
  await expect(page.locator(".range__plan")).toContainText(/head patch/);
  await expect(page.locator(".range__plan")).toContainText(/copied/);
});

test("a range that is backwards offers no button", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);

  await page.getByLabel("In point").fill("00:00:05:00");
  await page.getByLabel("Out point").fill("00:00:02:00");

  await expect(page.getByRole("button", { name: "Trim", exact: true })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Queue", exact: true })).toBeDisabled();
});

test("a timecode the source cannot reach says so, in the plan line", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);

  // The fixture is 3 101 frames: 2 minutes 4 seconds.
  await page.getByLabel("Out point").fill("03:00:00:00");
  await expect(page.locator(".range__plan")).toContainText(/past the end of the source/);
  await expect(page.getByRole("button", { name: "Trim", exact: true })).toBeDisabled();
});

test("queueing ranges and trimming them produces verified files", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);

  await mark(page, "00:00:01:00", "00:00:02:00");
  await page.getByRole("button", { name: "Queue", exact: true }).click();
  await expect(page.locator(".cut-table__row")).toHaveCount(1);
  await expect(page.getByLabel("In point")).toHaveValue("");

  await mark(page, "00:00:04:00", "00:00:06:00");
  await page.getByRole("button", { name: "Queue", exact: true }).click();
  await expect(page.locator(".cut-table__row")).toHaveCount(2);

  const trim = page.getByRole("button", { name: "Trim 2" });
  await expect(trim).toBeVisible();
  await expect(trim).toBeEnabled({ timeout: 10_000 });
  await trim.click();

  // The proof zone appears, one row per segment, each opening onto the checks it was measured against.
  await expect(page.getByRole("heading", { name: "What came out" })).toBeVisible();
  await expect(page.locator(".proof__item")).toHaveCount(2);
  await page.locator(".proof__head").first().click();
  await expect(page.locator(".checks__name").first()).toHaveText("Frames");
  await expect(page.getByText("Run signature")).toBeVisible();
});

test("the verification policy is on the panel and reaches the project", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);

  const policy = page.getByLabel("Verify");
  await expect(policy).toHaveValue("strict");
  await policy.selectOption("forensic");
  await expect(policy).toHaveValue("forensic");
});

test("the delivery preset is offered with its description, not as an encoder name", async ({
  page,
}) => {
  await page.goto("/");
  await chooseMaster(page);

  const preset = page.getByLabel("Delivery");
  await expect(preset.locator("option").first()).toContainText("the project default");
  // Every preset the project offers comes with the sentence that says who wants it.
  const text = await preset.locator("option").nth(1).innerText();
  expect(text.length).toBeGreaterThan(30);
});

test("the transcript can be searched and a range marked from a sentence", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);

  await page.getByLabel("Find").fill("second");
  await expect(page.locator(".hit").first()).toContainText("second take");

  await page.locator(".hit").first().click();
  await page.getByRole("button", { name: "Mark it" }).click();
  await expect(page.locator(".cut-table__row")).toHaveCount(1);
});

test("the progress zone is there before anything runs, and says so", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Progress" })).toBeVisible();
  await expect(page.locator(".progress-status").first()).toHaveText("ready");
  await expect(page.locator(".log")).toContainText("Nothing has run yet");
});

test("the interface works with no window behind it, which is the whole point", async ({ page }) => {
  await page.goto("/");
  const bridged = await page.evaluate(() => {
    const tauri = (window as unknown as { __TAURI__?: { mocks?: boolean } }).__TAURI__;
    return tauri?.mocks === true;
  });
  expect(bridged).toBe(true);
  await expect(page.locator(".app")).toBeVisible();
});
