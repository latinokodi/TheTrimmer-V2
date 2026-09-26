import { expect, test } from "@playwright/test";

import { chooseMaster, mark, startRun } from "./support";

/**
 * What the window shows while a cut is running.
 *
 * ## Why this is its own file
 *
 * A cut is minutes of ffmpeg with nothing to look at, and the difference between "working" and "stopped"
 * is the entire value of the progress zone. It is also the surface with the most ways to lie: a bar that
 * fills to a number nobody measured, a percentage that goes backwards, an estimate that counts up. So it
 * gets its own assertions rather than a line in the panel's.
 *
 * ## Why nothing checked it before
 *
 * The stub answered `run_batch` with a value and emitted no events at all — so in a browser there was no
 * progress to display, and the whole zone was exercised by nothing. That is the fixture lesson from
 * ADR-021: a double that cannot produce the state leaves the code that handles it untested. The stub now
 * narrates a compressed run over the same event the window uses, and
 * `apps/desktop/src-tauri/tests/ipc_contract.rs` asserts that event's shape against the real Rust
 * serialisation, so these tests cannot pass on a payload the window would never send.
 */

test.use({ viewport: { width: 1920, height: 1080 }, colorScheme: "dark" });

test("the bar is determinate while a pass of known length runs, and says the number", async ({
  page,
}, testInfo) => {
  await page.goto("/");
  await chooseMaster(page);

  // Before anything runs there is no fraction to show, and the window does not pretend otherwise.
  await expect(page.locator("progress.progress-bar")).toHaveCount(0);
  await expect(page.locator(".progress-status").first()).toHaveText("ready");

  await startRun(page);

  /*
   * A real `<progress>` with a value, rather than a decorated div: a screen reader announces it, and the
   * percentage printed beside it is the same figure. Both are asserted — and read in a single evaluate,
   * because two round trips take long enough for the run to move between them and the mismatch would be
   * the test's own doing rather than the interface's.
   */
  const bar = page.locator("progress.progress-bar").first();
  await expect(bar).toBeVisible({ timeout: 10_000 });

  // The panel mid-cut, committed as evidence: a run in progress is the one state a screenshot of the
  // idle window cannot show.
  await testInfo.attach("running", {
    body: await page.screenshot({ path: "screens/running.png", fullPage: false }),
    contentType: "image/png",
  });

  await expect
    .poll(
      async () =>
        await page.evaluate(() => {
          const element = document.querySelector("progress.progress-bar");
          const shown = document.querySelector(".progress-percent")?.textContent ?? "";
          if (element === null) {
            return null;
          }
          const value = Number((element as HTMLProgressElement).value);
          const printed = Number.parseInt(shown, 10);
          return Number.isFinite(printed) ? Math.abs(printed - Math.round(value * 100)) : null;
        }),
      { timeout: 10_000 },
    )
    .toBeLessThanOrEqual(1);
});

test("the readout names the step, the segment, the rate and what is left", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);
  await startRun(page);

  const strip = page.locator(".progress-strip");
  // Every one of these is something a person waiting is asking, and each is left out when it is unknown
  // rather than shown as a zero. Polled rather than read once, because this is a window into a run that
  // is still moving.
  await expect.poll(async () => await strip.innerText(), { timeout: 10_000 }).toContain("segment 1 of 1");
  await expect
    .poll(async () => await strip.innerText(), { timeout: 10_000 })
    .toMatch(/head encode|body|join/);
  await expect.poll(async () => await strip.innerText(), { timeout: 10_000 }).toMatch(/\d+\.\d+×/);
  await expect.poll(async () => await strip.innerText(), { timeout: 10_000 }).toContain("left");
  // A position, as a percentage of the pass, and a clock rather than a magnitude.
  await expect.poll(async () => await strip.innerText(), { timeout: 10_000 }).toMatch(/\d+%/);
  await expect.poll(async () => await strip.innerText(), { timeout: 10_000 }).toMatch(/\d+:\d\d/);
});

test("the log separates a pass ending from a segment ending", async ({ page }) => {
  /*
   * Both vocabularies have a `finished`. `trimmer-media` means "this ffmpeg pass ended" and carries the
   * pass's label, its seconds and whether it exited zero; `trimmer-app` means "this segment ended" and
   * carries the job it belongs to.
   *
   * The listener sent every `finished` to the queue handler, so the log never showed a single pass's
   * timing — every pass was announced as `segment finished`, and the count of segments that had finished
   * was in fact the count of ffmpeg passes that had. Nothing caught it because nothing in a browser could
   * produce either event.
   */
  await page.goto("/");
  await chooseMaster(page);
  await startRun(page);

  await expect(page.locator(".progress-status").first()).toHaveText("ready", { timeout: 15_000 });

  const lines = await page.locator(".log__line").allInnerTexts();
  const text = lines.join("\n");

  // Five passes, each with its own verdict and its own timing.
  const passVerdicts = lines.filter((line) => /— ok \(\d+\.\d+s\)/.test(line));
  expect(passVerdicts).toHaveLength(5);
  expect(text).toContain("head encode, frames 25..50 — ok");
  expect(text).toContain("body mux, picture and sound — ok");
  expect(text).toContain("join head and body — ok");

  // And exactly one line saying the segment itself finished — not one per pass.
  const segmentVerdicts = lines.filter((line) => line.includes("finished") && !line.includes("— ok"));
  expect(segmentVerdicts).toHaveLength(1);

  // The run says how it went at the end, in the same words the proof panel uses.
  expect(text).toContain("done: 1 written, 0 uncertified, 0 failed, 0 skipped");
});

test("every log line is stamped with when it happened", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);
  await startRun(page);
  await expect(page.locator(".progress-status").first()).toHaveText("ready", { timeout: 15_000 });

  // The whole reason a log is kept: which of these took the four minutes. A clock column of fixed width
  // is what makes it scannable rather than merely present — `formatClock`, not `formatDuration`, which
  // says `8.4s` and then `3m 12s` and so changes width as it counts.
  const stamps = await page.locator(".log__at").allInnerTexts();
  expect(stamps.length).toBeGreaterThan(5);
  for (const stamp of stamps) {
    expect(stamp).toMatch(/^\d+:\d\d$/);
  }
  // The last pass ended later than the first began, which is the ordering a log is read for.
  const seconds = stamps.map((stamp) => {
    const [minutes = "0", secs = "0"] = stamp.split(":");
    return Number(minutes) * 60 + Number(secs);
  });
  expect(seconds[seconds.length - 1]).toBeGreaterThanOrEqual(seconds[0] ?? 0);

  // And the command line that ran is in there, because it is the one thing that makes a slow pass
  // explicable.
  await expect(page.locator(".log__line--command").first()).toContainText("ffmpeg");
});

test("the bar falls back to indeterminate when there is no fraction to draw", async ({ page }) => {
  /*
   * A pass whose length is not known reports counters without an expectation, and there is then no honest
   * fraction to draw. The alternative — keeping the previous pass's denominator — would measure this pass
   * against the last one's length, which is a made-up number on a real bar.
   *
   * The *model* half of that rule is unit-tested (`stepFraction` is `null` without an expectation). This
   * is the rendering half, and it is asserted at the two points where the state is stable and known: the
   * window before a run, and the window after one. Racing a moving narration to catch the middle would be
   * a flaky test of a deterministic rule.
   */
  await page.goto("/");
  await chooseMaster(page);

  // Nothing has run, so there is no fraction and no `<progress>` — an indeterminate track, which is
  // exactly what the HTML element's own semantics say an unknown-length task is.
  await expect(page.locator("progress.progress-bar")).toHaveCount(0);
  await expect(page.locator(".progress-percent")).toHaveCount(0);
  await expect(page.locator(".progress-bar")).toHaveCount(1);

  await startRun(page);
  await expect(page.locator(".progress-status").first()).toHaveText("ready", { timeout: 15_000 });

  // And after it, the same: a finished run has no fraction, so it must not leave a filled bar behind.
  await expect(page.locator("progress.progress-bar")).toHaveCount(0);
  await expect(page.locator(".progress-bar--running")).toHaveCount(0);
});

test("a finished run clears the fraction rather than leaving the last number on the bar", async ({
  page,
}) => {
  await page.goto("/");
  await chooseMaster(page);
  await startRun(page);
  await expect(page.locator(".progress-status").first()).toHaveText("ready", { timeout: 15_000 });

  // A bar left at 100 % after the run reads as a job still finishing.
  await expect(page.locator("progress.progress-bar")).toHaveCount(0);
  await expect(page.locator(".progress-percent")).toHaveCount(0);
  await expect(page.locator(".progress-strip")).not.toContainText("left");
});

test("a run that fails says so in the log and does not leave a bar behind", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);
  await mark(page, "00:00:01:00", "00:00:02:00");
  await page.getByRole("button", { name: "Queue" }).click();
  await expect(page.locator(".cut-table__row")).toHaveCount(1);

  // ffmpeg says why on stderr; the engine forwards those lines as they arrive, so the reason is
  // readable while the step is happening rather than only in a tail once it has decided it is over.
  await page.getByRole("button", { name: "Trim 1" }).click();
  await page.waitForFunction(() => document.querySelector("progress.progress-bar") !== null, null, {
    timeout: 10_000,
  });
  await page.evaluate(() => {
    const bridge = (window as unknown as { __TAURI__: { event: { emit(n: string, p: unknown): void } } })
      .__TAURI__;
    bridge.event.emit("cut-progress", {
      kind: "message",
      text: "Non-monotonous DTS in output stream 0:0; this may result in incorrect timestamps",
    });
  });

  await expect(page.locator(".log__line--error").first()).toContainText("Non-monotonous DTS");
});
