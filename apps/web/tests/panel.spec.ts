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

/** Choose the fixture master. `Browse` opens the picker, and the stub is the picker in a browser. */
async function chooseMaster(page: import("@playwright/test").Page): Promise<void> {
  await page.getByRole("button", { name: "Browse" }).click();
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

/**
 * Answer the next picker call with a chosen path, so a test can pick a *different* file.
 *
 * The stub's picker returns the fixture master every time, which is the right default and the wrong
 * thing for testing what happens when the operator changes their mind.
 */
async function pickInstead(page: import("@playwright/test").Page, path: string): Promise<void> {
  await page.evaluate((chosen) => {
    (window as unknown as { __TAURI__: { dialog: { open: unknown } } }).__TAURI__.dialog.open =
      async () => chosen;
  }, path);
}

test("picking a different video switches to it instead of sticking on the first", async ({ page }) => {
  /*
   * Reported: "When I select a file with browse, then try to select a different one, the first file gets
   * stuck and cannot change."
   *
   * It was not the picker. `browse()` added the master to the project and never selected it, and nothing
   * in the interface ever called the draft's `setSource` — so the panel showed `sources[0]` for ever. The
   * project ended up holding both files while the window showed the first one, twice.
   *
   * That this shipped is also a fixture problem: the stub's `sources` command returned a single fixture
   * behind a `hasSource` boolean, so a browser could not represent "two masters in this session" and the
   * panel displaying *the* source looked correct. The stub holds a list now, which is what makes this
   * test possible at all.
   */
  await page.goto("/");
  await chooseMaster(page);
  await expect(page.getByLabel("Video file")).toHaveValue(/A007C012_250312_R1QK\.mov$/);

  await pickInstead(page, "H:\\masters\\reel 3\\B014C003_250401_R2QK.mov");
  await page.getByRole("button", { name: "Browse" }).click();

  // The panel follows the file that was just chosen.
  await expect(page.getByLabel("Video file")).toHaveValue(/B014C003_250401_R2QK\.mov$/);

  // And the first one is not stranded: the session appears as a list, and going back works.
  const session = page.getByLabel("Source in this session");
  await expect(session).toBeVisible();
  await expect(session.locator("option")).toHaveCount(2);
  await session.selectOption("H:\\masters\\reel 2\\A007C012_250312_R1QK.mov");
  await expect(page.getByLabel("Video file")).toHaveValue(/A007C012_250312_R1QK\.mov$/);

  // Removing a master takes it out of the session, and the panel falls back to the one that is left.
  await session.selectOption("H:\\masters\\reel 3\\B014C003_250401_R2QK.mov");
  await page.getByRole("button", { name: "Remove this source" }).click();
  await expect(page.getByLabel("Video file")).toHaveValue(/A007C012_250312_R1QK\.mov$/);
  await expect(page.getByLabel("Source in this session")).toBeHidden();
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

test("Browse opens the picker itself, with no dialog in between", async ({ page }) => {
  /*
   * The ask, asserted directly. `Browse` used to open an in-app dialog and leave the operator to find a
   * second button inside it before the operating system's picker appeared — a click that existed only
   * because the picker might be missing, and which hid the one dialog that has the operator's
   * favourites, recent folders, mapped drives and search in it.
   *
   * So: one click, a picker call, a source. No dialog.
   */
  await page.goto("/");

  const asked = await page.evaluate(() => {
    const calls: string[] = [];
    const bridge = (
      window as unknown as {
        __TAURI__: { dialog: { open: (options: unknown) => Promise<unknown> } };
      }
    ).__TAURI__;
    const original = bridge.dialog.open.bind(bridge.dialog);
    bridge.dialog.open = async (options: unknown) => {
      calls.push(JSON.stringify(options));
      return await original(options);
    };
    (window as unknown as { __PICKER_CALLS__: string[] }).__PICKER_CALLS__ = calls;
    return true;
  });
  expect(asked).toBe(true);

  await page.getByRole("button", { name: "Browse" }).click();

  await expect(page.getByLabel("Video file")).toHaveValue(/\.mov$/);
  await expect(page.getByRole("dialog")).toHaveCount(0);

  // And it asked for a video, not for anything at all.
  const calls = await page.evaluate(
    () => (window as unknown as { __PICKER_CALLS__: string[] }).__PICKER_CALLS__,
  );
  expect(calls).toHaveLength(1);
  expect(calls[0]).toContain("mp4");
  expect(calls[0]).toContain("mxf");
});

test("with no picker at all, Browse says why and offers the path field instead of doing nothing", async ({
  page,
}) => {
  /*
   * The other half, and the half that used to be a lie. `pickFile` answered `null` both when the
   * operator cancelled and when there was no picker, so a build with no dialog plugin had a Browse
   * button that did nothing at all and said nothing about it.
   *
   * Now the two are different: cancelling is silent and respected, and unavailable opens the fallback
   * with the reason in it.
   */
  await page.goto("/");
  await page.evaluate(() => {
    delete (window as unknown as { __TAURI__?: { dialog?: unknown } }).__TAURI__?.dialog;
  });

  await page.getByRole("button", { name: "Browse" }).click();

  const dialog = page.getByRole("dialog", { name: "Choose a video" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("status")).toContainText("no file picker here");
  await expect(dialog.getByLabel("Path to the video")).toBeFocused();

  // The fallback is not a dead end: typing the path adds the master.
  await page.getByLabel("Path to the video").fill("H:\\masters\\reel 2\\A007C012_250312_R1QK.mov");
  await dialog.getByRole("button", { name: "Add" }).click();
  await expect(page.getByLabel("Video file")).toHaveValue(/A007C012_250312_R1QK\.mov$/);
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

test("a dialog takes the keyboard, keeps it, and gives it back", async ({ page }) => {
  /*
   * `aria-modal="true"` is a claim, and both dialogs made it while enforcing nothing. Tab from the last
   * control went to the Cancel button in the panel *behind* the scrim, so a keyboard user could operate
   * an application they could not see past — and Escape, the first thing every Windows user tries, did
   * nothing. A claim nothing enforces is worse than no claim: a screen reader then refuses to read
   * content that is in fact reachable.
   *
   * The source dialog is reached by taking the picker away, because that is the only thing that opens
   * it now — which is itself the fix the operator asked for.
   */
  await page.goto("/");
  await page.evaluate(() => {
    delete (window as unknown as { __TAURI__?: { dialog?: unknown } }).__TAURI__?.dialog;
  });
  await page.getByRole("button", { name: "Browse" }).click();

  const dialog = page.getByRole("dialog", { name: "Choose a video" });
  await expect(dialog).toBeVisible();

  // Focus moved in, to the dialog's first control rather than to whatever was behind it.
  await expect(dialog.getByLabel("Path to the video")).toBeFocused();

  // Tab cycles within. Six presses from the first control is more than the number of controls it has,
  // so a leak would have shown up.
  for (let step = 0; step < 6; step += 1) {
    await page.keyboard.press("Tab");
    const inside = await page.evaluate(
      () => document.querySelector(".dialog")?.contains(document.activeElement) ?? false,
    );
    expect(inside, `Tab ${step + 1} left the dialog`).toBe(true);
  }

  // Shift+Tab from the first control wraps to the last rather than escaping.
  await dialog.getByLabel("Path to the video").focus();
  await page.keyboard.press("Shift+Tab");
  const stillInside = await page.evaluate(
    () => document.querySelector(".dialog")?.contains(document.activeElement) ?? false,
  );
  expect(stillInside).toBe(true);

  // Escape closes it, and the keyboard goes back to the control that opened it.
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(page.getByRole("button", { name: "Browse" })).toBeFocused();
});

test("the window opens with a titlebar, and fullscreen is a choice rather than the starting state", async ({
  page,
}) => {
  /*
   * A window with no titlebar cannot be restored, minimized or closed, and that is what the shipped
   * window was: borderless fullscreen. `GetWindowLong` on it returned `0x14000000` — `WS_VISIBLE |
   * WS_CLIPCHILDREN` and nothing else, so there was no caption, no system menu, no minimize box and no
   * maximize box, and the page could not do any of it either because the capability file refuses
   * `minimize` and `set_fullscreen`.
   *
   * The window now opens maximized with the operating system's titlebar, so those are Windows' job —
   * and fullscreen is still available as something the operator asks for and can undo. This test holds
   * the offer, not the operating system's chrome, which `tools/smoke-window.ps1` checks against the
   * real style bits.
   */
  await page.goto("/");

  const fullscreen = page.getByRole("button", { name: "Fullscreen" });
  await expect(fullscreen).toBeVisible();
  await expect(fullscreen).toHaveAttribute("title", /Hide the titlebar/);

  // Pressing it asks the window, and the label follows what the window reports rather than what was
  // requested — the difference matters the moment somebody uses the titlebar's maximize button.
  await fullscreen.click();
  const restore = page.getByRole("button", { name: "Restore" });
  await expect(restore).toBeVisible();
  await expect(restore).toHaveAttribute("title", /get the titlebar back/);

  // F11 goes both ways, which is what makes offering it safe: a user who hides the titlebar with a key
  // has the same key to bring it back.
  await page.evaluate(() => {
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "F11", bubbles: true }));
  });
  await expect(page.getByRole("button", { name: "Fullscreen" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Fullscreen" })).toHaveAttribute(
    "title",
    /Hide the titlebar/,
  );
});

test("motion is feedback, and the system can switch it off", async ({ page }) => {
  const sweep = async (): Promise<string> => {
    return await page.evaluate(() => {
      const bar = document.createElement("div");
      bar.className = "progress-bar progress-bar--running";
      document.body.append(bar);
      const name = getComputedStyle(bar, "::after").animationName;
      bar.remove();
      return name;
    });
  };

  await page.goto("/");
  // The bar is indeterminate while a run is in flight: the engine reports per-step timing, not a
  // fraction of the whole, and a bar that filled to 40 % and stopped would be inventing a number.
  expect(await sweep()).toBe("sweep");

  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");
  // A 1.3 s infinite sweep is exactly the motion the preference exists to stop. It is a token so the
  // preference can reach it, and the animation is switched off outright rather than set to zero — an
  // `infinite` animation with a zero duration still schedules work.
  expect(await sweep()).toBe("none");
});
