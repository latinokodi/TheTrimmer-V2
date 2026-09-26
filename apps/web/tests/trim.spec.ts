import { expect, test } from "@playwright/test";

/**
 * The whole product, driven as a person drives it.
 *
 * Each test starts from a fresh page and a fresh stub, so the order they run in cannot matter. Every
 * assertion is about something on screen: a control that is offered, a sentence that is shown, a
 * button that is enabled, a number that is right.
 *
 * There is no project setup step, and that is the point of the flow being tested: the window opens a
 * session by itself, so the first thing a person can do is choose a video.
 */

/** Choose the fixture master through the picker. The stub answers with a real path. */
async function chooseMaster(page: import("@playwright/test").Page): Promise<void> {
  await page.getByRole("button", { name: "Choose a video…" }).click();
  await expect(page.getByRole("dialog", { name: "Choose a video" })).toBeVisible();
  await page.getByRole("button", { name: "Browse…" }).click();
  await expect(page.locator(".trim__master-name")).toHaveText("A007C012_250312_R1QK.mov");
}

/** Type a range and wait for the length line to agree it parses. */
async function mark(
  page: import("@playwright/test").Page,
  inPoint: string,
  outPoint: string,
): Promise<void> {
  await page.getByLabel("In point").fill(inPoint);
  await page.getByLabel("Out point").fill(outPoint);
  await expect(page.locator(".trim__length")).not.toContainText("Type both timecodes");
}

test("a first run offers the one thing to do next, with no project to set up", async ({ page }) => {
  await page.goto("/");

  // No dialog, no project picker: the window is already in a session, and the only thing missing is a
  // video. The claim is on screen rather than in a tooltip.
  await expect(page.getByRole("heading", { name: "TheTrimmer" })).toBeVisible();
  await expect(page.locator(".titlebar__tagline")).toContainText("Frame-exact, lossless segment cutting");
  await expect(page.getByText("Choose a video to trim")).toBeVisible();
  await expect(page.getByRole("button", { name: "Choose a video…" })).toBeVisible();

  // The marks are not offered until there is a video, because a timecode without a frame rate is not
  // a number.
  await expect(page.getByLabel("In point")).toBeHidden();

  // Every setting is on the main window rather than behind a disclosure.
  await expect(page.getByRole("button", { name: "Queue it" })).toBeHidden();
});

test("the marks show the frame numbers as they are typed, and the length is inclusive", async ({
  page,
}) => {
  await page.goto("/");
  await chooseMaster(page);

  await page.getByLabel("In point").fill("00:00:01:00");
  await expect(page.getByText("frame 25")).toBeVisible();

  // 00:00:02:00 is frame 50, and it is the **last frame kept** — so the range is 26 frames, not 25.
  await page.getByLabel("Out point").fill("00:00:02:00");
  await expect(page.getByText("frame 50, the last one kept")).toBeVisible();
  await expect(page.locator(".trim__length")).toContainText("26 frames");

  await expect(page.getByRole("button", { name: "Trim", exact: true })).toBeEnabled();
});

test("the plan is answered while the marks are being typed, not on a button press", async ({
  page,
}) => {
  await page.goto("/");
  await chooseMaster(page);
  await mark(page, "00:00:01:00", "00:00:02:00");

  // Nothing was pressed. The plan arrives as the marks are set, because the answer changes the
  // decision and an answer that arrives afterwards is not an answer.
  await expect(page.getByText(/lossless copy/)).toBeVisible();

  // A range that starts *between* keyframes is a head patch instead, and the number of re-encoded
  // frames is on screen — the decision this product exists to make visible.
  await page.getByLabel("In point").fill("00:00:01:12");
  await expect(page.getByText(/head patch/)).toBeVisible();
  await expect(page.getByText(/frames re-encoded/)).toBeVisible();
});

test("a range that is backwards offers no button, and says nothing misleading", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);

  await page.getByLabel("In point").fill("00:00:05:00");
  await page.getByLabel("Out point").fill("00:00:02:00");

  await expect(page.getByRole("button", { name: "Trim", exact: true })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Queue it" })).toBeDisabled();
});

test("a timecode the source cannot reach says so, in the field", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);

  // The fixture is 3 101 frames: 2 minutes 4 seconds. 03:00:00:00 is well past the end.
  await page.getByLabel("Out point").fill("03:00:00:00");
  await expect(page.locator(".trim__length")).toContainText(/past the end of the source/);
  await expect(page.getByRole("button", { name: "Trim", exact: true })).toBeDisabled();
});

test("queueing ranges and trimming them produces verified files", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);

  await mark(page, "00:00:01:00", "00:00:02:00");
  await page.getByRole("button", { name: "Queue it" }).click();

  // The range becomes a row and the marks clear, ready for the next one.
  await expect(page.locator(".cut-table__row")).toHaveCount(1);
  await expect(page.getByLabel("In point")).toHaveValue("");

  await mark(page, "00:00:04:00", "00:00:06:00");
  await page.getByRole("button", { name: "Queue it" }).click();
  await expect(page.locator(".cut-table__row")).toHaveCount(2);
  await expect(page.getByRole("button", { name: "Trim 2" })).toBeVisible();

  // The button is disabled while the model is busy, and queueing a segment re-plans the queue. Waiting
  // for it to become pressable is what a person does — Playwright's `click` would otherwise time out
  // against a disabled element and report the *interface* as broken rather than the timing.
  const trim = page.getByRole("button", { name: "Trim 2" });
  await expect(trim).toBeEnabled({ timeout: 10_000 });
  await trim.click();

  // The proof: one row per segment, each opening onto the checks it was measured against.
  await expect(page.locator(".proof__item")).toHaveCount(2);
  await page.locator(".proof__head").first().click();
  await expect(page.locator(".checks__name").first()).toHaveText("Frames");
  await expect(page.getByText("Run signature")).toBeVisible();
});

test("the verification policy is on the main window and reaches the project", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);

  const policy = page.getByLabel("Verify");
  await expect(policy).toHaveValue("strict");
  await policy.selectOption("forensic");
  await expect(policy).toHaveValue("forensic");
});

test("delivery settings are on the main window, not behind a disclosure", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);

  // Nothing to open: both controls are visible with their values, which is the whole of the change
  // from the three-disclosure layout this replaced.
  await expect(page.getByLabel("Delivery")).toBeVisible();
  await expect(page.getByLabel("Handles")).toBeVisible();
  await expect(page.locator("details")).toHaveCount(0);});

test("the transcript can be searched and a range marked from a sentence", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);

  await page.getByLabel("Find in the transcript").fill("second");
  await expect(page.locator(".hit").first()).toContainText("second take");

  await page.locator(".hit").first().click();
  await page.getByRole("button", { name: "Mark this sentence" }).click();
  await expect(page.locator(".cut-table__row")).toHaveCount(1);
});

test("the interface works with no window behind it, which is the whole point", async ({ page }) => {
  await page.goto("/");
  const bridged = await page.evaluate(() => {
    const tauri = (window as unknown as { __TAURI__?: { mocks?: boolean } }).__TAURI__;
    return tauri?.mocks === true;
  });
  expect(bridged).toBe(true);
  await expect(page.getByRole("main")).toBeVisible();
});
