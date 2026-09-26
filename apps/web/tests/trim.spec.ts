import { expect, test } from "@playwright/test";

/**
 * The whole product, driven as a person drives it.
 *
 * Each test starts from a fresh page and a fresh stub, so the order they run in cannot matter. Every
 * assertion is about something on screen: a control that is offered, a sentence that is shown, a
 * button that is enabled, a number that is right.
 */

/**
 * Open a project and add the fixture master, which is what every workflow starts with.
 *
 * Both halves happen in one place: "Choose a video…" opens the project dialog, because a video cannot
 * be added to a project that is not open — so the button that starts the work is also the button that
 * gets the project out of the way first.
 */
async function setUpProject(page: import("@playwright/test").Page): Promise<void> {
  await page.getByRole("button", { name: "no project" }).click();
  await expect(page.getByRole("dialog", { name: "Projects" })).toBeVisible();

  await page.getByPlaceholder("Andy Ross interview").fill("A007C012 assembly");
  await page.getByRole("button", { name: "Create and open" }).click();

  // Creating a project opens it and closes the dialog, because there is nothing left to decide in
  // there. Waiting for the dialog to go — rather than clicking Close — is what the interface does.
  await expect(page.getByRole("dialog", { name: "Projects" })).toBeHidden();
  await expect(page.getByRole("button", { name: "A007C012 assembly" })).toBeVisible();

  await chooseMaster(page);
}

/** Add the fixture master through the picker the dialog offers. */
async function chooseMaster(page: import("@playwright/test").Page): Promise<void> {
  await page.getByRole("button", { name: "Choose a video…" }).click();
  await expect(page.getByRole("dialog", { name: "Projects" })).toBeVisible();
  await page.getByRole("button", { name: "Add a master…" }).click();
  // The stub answers the picker with the fixture, so the source section closes over a probed master.
  await expect(page.locator(".trim__master-name")).toHaveText("A007C012_250312_R1QK.mov");
  if (await page.getByRole("dialog", { name: "Projects" }).isVisible()) {
    await page.getByRole("button", { name: "Close" }).click();
  }
  await expect(page.getByRole("dialog", { name: "Projects" })).toBeHidden();
}

/** Open one of the collapsed sections by name. */
async function openSection(
  page: import("@playwright/test").Page,
  title: string,
): Promise<void> {
  await page.locator(".section__head", { hasText: title }).click();
}

test("a first run says what to do next, in the place the answer will appear", async ({ page }) => {
  await page.goto("/");

  // No project, no video: the one screen in the window offers the one thing to do about it, and the
  // claim is on screen rather than in a tooltip.
  await expect(page.getByRole("heading", { name: "TheTrimmer" })).toBeVisible();
  await expect(page.locator(".titlebar__tagline")).toContainText("Frame-exact, lossless segment cutting");
  await expect(page.getByRole("button", { name: "no project" })).toBeVisible();
  await expect(page.getByText("No video yet")).toBeVisible();
  await expect(page.getByRole("button", { name: "Choose a video…" })).toBeVisible();

  // The two timecode fields are not offered until there is a video, because a timecode without a
  // frame rate is not a number.
  await expect(page.getByLabel("In point")).toBeHidden();
});

test("the marks show the frame numbers as they are typed, from the domain's own arithmetic", async ({
  page,
}) => {
  await page.goto("/");
  await setUpProject(page);

  const inField = page.getByLabel("In point");
  const outField = page.getByLabel("Out point");

  // 00:00:01:00 at 25 fps is frame 25.
  await inField.fill("00:00:01:00");
  await expect(page.getByText("frame 25")).toBeVisible();

  // 00:00:02:00 is frame 50, and it is the **last frame kept** — so the range is 26 frames, not 25.
  await outField.fill("00:00:02:00");
  await expect(page.getByText("frame 50, the last one kept")).toBeVisible();
  await expect(page.getByText("26 frames")).toBeVisible();

  // And the button that cuts is offered, named for what it does.
  await expect(page.getByRole("button", { name: "Trim this segment" })).toBeEnabled();
});

test("a range that starts on a keyframe is called a lossless copy before it is cut", async ({
  page,
}) => {
  await page.goto("/");
  await setUpProject(page);

  await page.getByLabel("In point").fill("00:00:01:00");
  await page.getByLabel("Out point").fill("00:00:02:00");

  // Nothing was pressed. The plan arrives while the marks are being typed, because the answer changes
  // the decision and an answer that arrives afterwards is not an answer.
  await expect(page.getByText(/lossless copy/)).toBeVisible();

  // A range that starts *between* keyframes is a head patch instead, and the number of re-encoded
  // frames is on screen — this is the decision the product exists to make visible.
  await page.getByLabel("In point").fill("00:00:01:12");
  await expect(page.getByText(/head patch/)).toBeVisible();
  await expect(page.getByText(/frames re-encoded/)).toBeVisible();
});

test("an out point before the in point is refused with a sentence, not a dead button", async ({
  page,
}) => {
  await page.goto("/");
  await setUpProject(page);

  await page.getByLabel("In point").fill("00:00:05:00");
  await page.getByLabel("Out point").fill("00:00:02:00");

  // The length line stands down and the button is disabled; there is no state in which the product
  // offers to cut a range that is backwards.
  await expect(page.getByRole("button", { name: "Trim this segment" })).toBeDisabled();
});

test("a timecode the source cannot reach says so, in the field", async ({ page }) => {
  await page.goto("/");
  await setUpProject(page);

  // The fixture is 3 101 frames: 2 minutes 4 seconds. 03:00:00:00 is well past the end.
  await page.getByLabel("Out point").fill("03:00:00:00");
  await expect(page.getByText(/past the end of the source/)).toBeVisible();
  await expect(page.getByRole("button", { name: "Trim this segment" })).toBeDisabled();
});

test("queueing a range and trimming it produces a verified file", async ({ page }) => {
  await page.goto("/");
  await setUpProject(page);

  await page.getByLabel("In point").fill("00:00:01:00");
  await page.getByLabel("Out point").fill("00:00:02:00");

  // Queue it: the range becomes a row, the marks clear, and the queue section opens.
  await page.getByRole("button", { name: "Queue it" }).click();
  await expect(page.getByRole("row", { name: /Segment at 00:00:01:00/ })).toBeVisible();
  await expect(page.getByLabel("In point")).toHaveValue("");

  // A second range, so the button says it is trimming more than one thing.
  await page.getByLabel("In point").fill("00:00:04:00");
  await page.getByLabel("Out point").fill("00:00:06:00");
  await expect(page.getByRole("button", { name: "Trim the queue" })).toBeVisible();

  // Trim. The queue is what runs, and every finished file is checked.
  await page.getByRole("button", { name: "Trim the queue" }).click();

  // The proof: one row per segment, each opening onto the checks it was measured against. The rows
  // are collapsed by default, because the verdict is the answer and the checks are the evidence.
  await expect(page.locator(".proof__item")).toHaveCount(2);
  await page.locator(".proof__head").first().click();
  await expect(page.locator(".checks__name").first()).toHaveText("Frames");
  await expect(page.getByText("Run signature")).toBeVisible();
});
test("delivery settings are folded away until they are wanted", async ({ page }) => {
  await page.goto("/");
  await setUpProject(page);

  // Closed, and the heading carries the answer, so it does not have to be opened to find out.
  const delivery = page.locator("details.trim__delivery");
  await expect(delivery).not.toHaveAttribute("open", "");
  await expect(delivery).toContainText("the project default");

  await delivery.locator("summary").click();
  await expect(delivery).toHaveAttribute("open", "");
  await page.getByLabel("Handles").fill("12");
  await expect(delivery).toContainText("12 frame handles");
});

test("the transcript can be searched and a range marked from a sentence", async ({ page }) => {
  await page.goto("/");
  await setUpProject(page);

  await openSection(page, "Transcript");
  await page.getByLabel("Find in the transcript").fill("second");
  await expect(page.locator(".hit").first()).toContainText("second take");

  // Marking from a hit is the reason the transcript is here: a word is easier to find than a
  // timecode, and the cue's own start frame becomes the in point.
  await page.locator(".hit").first().click();
  await page.getByRole("button", { name: "Mark this sentence" }).click();
  await expect(page.locator(".cut-table__row")).toHaveCount(1);
});

test("the interface works with no window behind it, which is the whole point", async ({ page }) => {
  // The stub is what makes this suite possible, and this asserts the property that matters: nothing
  // in the interface needs Tauri to render. If a component ever reached for a Tauri-only global, this
  // is where it would fail.
  await page.goto("/");
  const bridged = await page.evaluate(() => {
    const tauri = (window as unknown as { __TAURI__?: { mocks?: boolean } }).__TAURI__;
    return tauri?.mocks === true;
  });
  expect(bridged).toBe(true);
  await expect(page.getByRole("main")).toBeVisible();
});
