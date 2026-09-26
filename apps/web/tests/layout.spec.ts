import { expect, test } from "@playwright/test";

/**
 * The panel, at the size it opens at, and at the size it is allowed to be smallest.
 *
 * Four things this file exists to hold, and only one of them is about pixels.
 *
 * **The frame does not scroll.** This is a desktop application with a 1440x960 minimum that opens
 * fullscreen, and a scrollbar on the window means a control is off screen. Only three zones get their
 * own scroll — the queue, the transcript hits and the log — because only those can hold an unbounded
 * number of rows, and an unbounded list gets its own well rather than moving the application around it.
 *
 * **Nothing is folded away.** Every setting is visible with its value. An earlier version hid the
 * delivery preset, the handles and the verification policy behind three collapsed sections.
 *
 * **Nothing is clipped.** This is the one that was wrong. A strip shorter than its own line box, a
 * footer whose last two items ran off the right edge, and a transcript list whose viewport was a
 * hard-coded 320 px inside a 60 px zone all produced text that stopped mid-word. The measurement below
 * is deliberately narrow so that it fails on that and not on legitimate truncation: an element is only
 * reported when it hides content **and is not ellipsised**, or when its box leaves the window entirely.
 * A deliberate ellipsis is a decision the operator can see; a clip is not.
 *
 * **There is no second layout.** No viewport media query, asserted against the source itself.
 */

test.use({ viewport: { width: 1920, height: 1080 }, colorScheme: "dark" });

/** The size the window is allowed to be smallest, mirrored from `tauri.conf.json`. */
const MINIMUM = { width: 1440, height: 960 };

/** Choose the fixture master. The stub answers the picker with a real path. */
async function chooseMaster(page: import("@playwright/test").Page): Promise<void> {
  await page.getByRole("button", { name: "Browse" }).click();
  await expect(page.getByRole("dialog", { name: "Choose a video" })).toBeVisible();
  await page.getByRole("button", { name: "Choose a file…" }).click();
  await expect(page.getByLabel("Video file")).toHaveValue(/\.mov$/);
}

/** Type a range, and wait for the plan line to agree that both marks parse. */
async function mark(
  page: import("@playwright/test").Page,
  inPoint: string,
  outPoint: string,
): Promise<void> {
  await page.getByLabel("In point").fill(inPoint);
  await page.getByLabel("Out point").fill(outPoint);
  await expect(page.locator(".range__plan")).not.toContainText("Type both timecodes");
}

/** The shape the clipping probe returns, one entry per problem found. */
interface Clip {
  readonly selector: string;
  readonly kind: string;
  readonly detail: string;
}

/**
 * Find text that is hidden rather than truncated.
 *
 * Runs in the page, because the answer only exists after layout. It skips anything that is not rendered,
 * anything with zero area, and anything whose overflow is `auto`/`scroll` — a scroll well is allowed to
 * hold more than it shows, that is what a well is for.
 */
async function findClipping(page: import("@playwright/test").Page): Promise<readonly Clip[]> {
  return page.evaluate(() => {
    const found: { selector: string; kind: string; detail: string }[] = [];

    const describe = (element: Element): string => {
      const id = element.id === "" ? "" : `#${element.id}`;
      const classes = [...element.classList].slice(0, 3).join(".");
      const name = `${element.tagName.toLowerCase()}${id}${classes === "" ? "" : `.${classes}`}`;
      const text = (element.textContent ?? "").trim().replaceAll(/\s+/g, " ").slice(0, 40);
      return text === "" ? name : `${name} “${text}”`;
    };

    /*
     * An element inside a scroll well is allowed to sit outside the visible box — that is what a well
     * is. The queue, the transcript hits, the log and the proof panel all hold more than they show, on
     * purpose, and every one of them was measured before this exemption was added.
     */
    const insideScrollWell = (element: Element): boolean => {
      for (let parent = element.parentElement; parent !== null; parent = parent.parentElement) {
        const style = getComputedStyle(parent);
        const scrolls = (axis: string): boolean => axis === "auto" || axis === "scroll";
        if (scrolls(style.overflowX) || scrolls(style.overflowY)) {
          return true;
        }
      }
      return false;
    };

    for (const element of document.querySelectorAll("body *")) {
      // `.sr-only` is a 1x1 box on purpose: it exists to be read by a screen reader and never by an
      // eye, so every clip rule below is wrong about it. It is the one deliberate exception, and it is
      // named here rather than excluded by a heuristic.
      if (element.classList.contains("sr-only")) {
        continue;
      }

      const style = getComputedStyle(element);
      if (style.display === "none" || style.visibility === "hidden" || style.opacity === "0") {
        continue;
      }

      const rect = element.getBoundingClientRect();
      if (rect.width < 1 || rect.height < 1) {
        continue;
      }

      // Leaves the window, and is not merely scrolled within it.
      if (!insideScrollWell(element) && rect.bottom > window.innerHeight + 1) {
        found.push({
          selector: describe(element),
          kind: "outside the window",
          detail: `right ${Math.round(rect.right)} of ${window.innerWidth}, bottom ${Math.round(rect.bottom)} of ${window.innerHeight}`,
        });
      }

      const scrollable = (axis: string): boolean => axis === "auto" || axis === "scroll";
      const hides = (axis: string): boolean =>
        (axis === "hidden" || axis === "clip") && !scrollable(axis);

      // A type floor of 10 px. Below it, letter-spaced capitals stop resolving on a 1x display — which
      // is what made the old zone headers look cut off even where they technically fitted.
      const size = Number.parseFloat(style.fontSize);
      if (size > 0 && size < 10 && (element.textContent ?? "").trim() !== "") {
        found.push({
          selector: describe(element),
          kind: "type below the floor",
          detail: `${size}px < 10px`,
        });
      }

      // Hidden horizontally without an ellipsis: the text simply stops.
      if (hides(style.overflowX) && element.scrollWidth > element.clientWidth + 1) {
        if (style.textOverflow !== "ellipsis") {
          found.push({
            selector: describe(element),
            kind: "clipped horizontally, no ellipsis",
            detail: `${element.scrollWidth}px of content in ${element.clientWidth}px`,
          });
        }
      }

      // Hidden vertically. An ellipsis cannot say "there is more below", so this is always a fault —
      // unless the element is a single line, where the horizontal rule above already covers it.
      if (
        hides(style.overflowY) &&
        element.scrollHeight > element.clientHeight + 1 &&
        style.whiteSpace !== "nowrap"
      ) {
        found.push({
          selector: describe(element),
          kind: "clipped vertically",
          detail: `${element.scrollHeight}px of content in ${element.clientHeight}px`,
        });
      }
    }

    return found;
  });
}

/** Fail with the whole list, so one run tells you every string that is cut. */
function expectNothingClipped(clips: readonly Clip[]): void {
  expect(
    clips.map((clip) => `${clip.kind}: ${clip.selector} (${clip.detail})`),
    "text is being hidden rather than truncated",
  ).toEqual([]);
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

  // Every zone is on screen: nothing below the fold.
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

test("no text is clipped, at the opening size", async ({ page }) => {
  await page.goto("/");
  await chooseMaster(page);
  await page.getByLabel("In point").fill("00:00:01:00");
  await page.getByLabel("Out point").fill("00:00:02:00");
  await page.waitForTimeout(400);

  expectNothingClipped(await findClipping(page));
});

test("no text is clipped, at the window's own minimum", async ({ page }) => {
  await page.setViewportSize(MINIMUM);
  await page.goto("/");
  await chooseMaster(page);
  await page.getByLabel("In point").fill("00:00:01:00");
  await page.getByLabel("Out point").fill("00:00:02:00");
  await page.waitForTimeout(400);

  expectNothingClipped(await findClipping(page));
});

test("no text is clipped with a full queue and an outcome to report", async ({ page }) => {
  // The empty panel is the easy case. A queue with rows in it, a selected row, and a proof panel with
  // a checks table are where a fixed grid column meets a filename that is four words long.
  await page.goto("/");
  await chooseMaster(page);

  let queuedSoFar = 0;

  for (const [start, end] of [
    ["00:00:01:00", "00:00:02:00"],
    ["00:00:03:00", "00:00:04:00"],
    ["00:00:05:00", "00:00:06:00"],
  ] as const) {
    await mark(page, start, end);
    const queue = page.getByRole("button", { name: "Queue" });
    // The button is gated on both marks parsing, and the parse is a command. Waiting for it here rather
    // than letting `click` retry keeps the failure naming the control instead of the timeout.
    await expect(queue).toBeEnabled();
    await queue.click();
    // Wait for the queue to actually grow. `queue()` awaits the command and *then* clears the marks,
    // so moving straight to the next range races that reset: it lands after the next In point has been
    // typed and wipes it. Which is a fault in the test, not in the application — but it is only
    // visible because the assertion is here.
    await expect(page.locator(".cut-table__row")).toHaveCount(queuedSoFar + 1);
    queuedSoFar += 1;
  }

  await expect(page.locator(".cut-table__row")).toHaveCount(3);
  await page.locator(".cut-table__row").first().click();
  await expect(page.locator(".cut-table__row--selected")).toHaveCount(1);

  // A populated queue and a selected row, which is the densest the left column gets.
  expectNothingClipped(await findClipping(page));

  // Now run it, so the proof zone exists too — the checks table is the widest fixed-column layout in
  // the window and the place a long `expected`/`measured` pair would run off the edge.
  const trim = page.getByRole("button", { name: "Trim 3" });
  await expect(trim).toBeEnabled({ timeout: 10_000 });
  await trim.click();
  await expect(page.getByRole("heading", { name: "What came out" })).toBeVisible();
  await expect(page.locator(".proof__item")).toHaveCount(3);
  await page.locator(".proof__head").first().click();
  await expect(page.locator(".checks__name").first()).toHaveText("Frames");

  expectNothingClipped(await findClipping(page));
});

test("no text is clipped inside the dialogs", async ({ page }) => {
  // A dialog is a fixed-width panel in a scrim, so it is the one place where a long note and a long
  // path have nowhere to go. Both are checked.
  await page.goto("/");

  await page.getByRole("button", { name: "Browse" }).click();
  const source = page.getByRole("dialog", { name: "Choose a video" });
  await expect(source).toBeVisible();
  await page.waitForTimeout(200);
  expectNothingClipped(await findClipping(page));
  // Scoped to the dialog: the panel behind it also has a Cancel, and an unscoped role query is
  // ambiguous the moment it does.
  await source.getByRole("button", { name: "Cancel" }).click();

  await chooseMaster(page);
  await page.getByLabel("In point").fill("00:00:01:00");
  await page.getByLabel("Out point").fill("00:00:02:00");
  await page.getByRole("button", { name: "Queue" }).click();
  await expect(page.locator(".cut-table__row")).toHaveCount(1);

  await page.getByRole("button", { name: "Export" }).click();
  await expect(page.getByRole("dialog", { name: "Export the timeline" })).toBeVisible();
  await page.waitForTimeout(200);
  expectNothingClipped(await findClipping(page));
});

test("the light theme is the same panel, and nothing is clipped in it either", async ({
  page,
}, testInfo) => {
  // The light value set exists for a machine with a light desktop theme and a bright room. It is not
  // the design, but it is shipped, so it is measured like the design: same zones, same geometry, and no
  // value that only clears contrast on the dark substrate.
  await page.emulateMedia({ colorScheme: "light" });
  await page.goto("/");
  await chooseMaster(page);
  await mark(page, "00:00:01:00", "00:00:02:00");

  // The interface follows the operating system until somebody says otherwise.
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

  await page.waitForTimeout(400);
  await testInfo.attach("trim-light", {
    body: await page.screenshot({ path: "screens/trim-light.png", fullPage: false }),
    contentType: "image/png",
  });

  expectNothingClipped(await findClipping(page));
});

test("no zone is squeezed below its own content, at either size", async ({ page }) => {
  /*
   * The check that was missing. A flex item squeezed under its content does not clip — its content
   * overflows it *visibly* and lands on top of whatever is below, so nothing in a browser calls it a
   * fault and the clipping probe above cannot see it either. The first version of this panel declared a
   * 900 px minimum while its form column needed 572 px in a 538 px space, and two zones were silently
   * overlapping by 34 px at the very size the window promised to support.
   *
   * A zone whose body reports more content than it has box is that fault, exactly.
   */
  for (const viewport of [{ width: 1920, height: 1080 }, MINIMUM]) {
    await page.setViewportSize(viewport);
    await page.goto("/");
    await chooseMaster(page);
    await mark(page, "00:00:01:00", "00:00:02:00");

    const squeezed = await page.evaluate(() => {
      const found: string[] = [];
      for (const zone of document.querySelectorAll(".zone")) {
        const body = zone.querySelector(".zone__body");
        if (body === null) {
          continue;
        }
        const over = body.scrollHeight - body.clientHeight;
        if (over > 1) {
          const title = zone.querySelector(".zone__title")?.textContent ?? "?";
          found.push(`${title}: ${over}px of content in ${body.clientHeight}px`);
        }
      }
      return found;
    });

    expect(squeezed, `at ${viewport.width}x${viewport.height}`).toEqual([]);
  }
});

test("every colour token clears its WCAG threshold, in both themes", async ({ page }) => {
  /*
   * Contrast is the one thing in a palette that cannot be eyeballed, and it is not symmetric: a grey
   * that reads as quiet on `#171c21` is nearly invisible on `#fdfdfe`. The light value set was wrong in
   * exactly one place when this test was written — `--phosphor-faint` at 4.17:1 — and no screenshot
   * would have shown it.
   *
   * Two thresholds, because WCAG has two: **4.5:1** for text (1.4.3) and **3:1** for the parts of a
   * control that are not text (1.4.11) — a focus ring, a selection edge, a progress fill. Measuring a
   * focus ring against the text rule would be inventing a requirement; measuring it against no rule at
   * all is how a ring ends up invisible.
   *
   * The pairs are the ones that actually occur in the stylesheet, read from the live custom properties
   * rather than from a second copy of the numbers, so a retuned token is measured where it lands.
   */
  const TEXT: readonly (readonly [string, string])[] = [
    ["--phosphor-bright", "--substrate-200"],
    ["--phosphor", "--substrate-200"],
    ["--phosphor-dim", "--substrate-200"],
    ["--phosphor-faint", "--substrate-200"],
    ["--phosphor-faint", "--substrate-000"],
    ["--verified", "--substrate-200"],
    ["--caution", "--substrate-200"],
    ["--hazard", "--substrate-200"],
    ["--primary-ink", "--primary"],
  ];

  const NON_TEXT: readonly (readonly [string, string])[] = [
    ["--select", "--substrate-200"],
    ["--focus", "--substrate-200"],
    ["--hazard-edge", "--substrate-200"],
    ["--primary", "--substrate-200"],
  ];

  for (const scheme of ["dark", "light"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.goto("/");
    // The choice is remembered in `localStorage`, so without clearing it the second pass inherits the
    // first pass's theme rather than following the system. Clearing it is what makes this a test of the
    // operating system preference and not of the storage key.
    await page.evaluate(() => window.localStorage.clear());
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("data-theme", scheme);

    const measured = await page.evaluate(
      ([text, nonText]) => {
        const parse = (value: string): [number, number, number] => {
          const source = value.trim();
          if (source.startsWith("#")) {
            const hex = source.slice(1);
            const full =
              hex.length === 3
                ? hex
                    .split("")
                    .map((digit) => digit + digit)
                    .join("")
                : hex;
            return [
              Number.parseInt(full.slice(0, 2), 16),
              Number.parseInt(full.slice(2, 4), 16),
              Number.parseInt(full.slice(4, 6), 16),
            ];
          }
          const numbers = source.match(/[\d.]+/g) ?? [];
          return [Number(numbers[0]), Number(numbers[1]), Number(numbers[2])];
        };

        const luminance = (rgb: [number, number, number]): number => {
          const channel = (part: number): number => {
            const value = part / 255;
            return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
          };
          return 0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2]);
        };

        const root = getComputedStyle(document.documentElement);
        const ratioOf = ([ink, surface]: readonly [string, string]): number => {
          const foreground = luminance(parse(root.getPropertyValue(ink)));
          const background = luminance(parse(root.getPropertyValue(surface)));
          const lighter = Math.max(foreground, background);
          const darker = Math.min(foreground, background);
          return Math.round(((lighter + 0.05) / (darker + 0.05)) * 100) / 100;
        };

        return {
          text: text.map((pair) => [pair[0], pair[1], ratioOf(pair)] as const),
          nonText: nonText.map((pair) => [pair[0], pair[1], ratioOf(pair)] as const),
        };
      },
      [TEXT, NON_TEXT] as const,
    );

    for (const [ink, surface, ratio] of measured.text) {
      expect(ratio, `${scheme} text: ${ink} on ${surface} is ${ratio}:1`).toBeGreaterThanOrEqual(4.5);
    }
    for (const [ink, surface, ratio] of measured.nonText) {
      expect(ratio, `${scheme} non-text: ${ink} on ${surface} is ${ratio}:1`).toBeGreaterThanOrEqual(3);
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

test("the two bundled faces, and nothing else, are what the window asks for", async ({ page }) => {
  await page.goto("/");

  // The fonts are bundled, so the interface does not change shape depending on what the operator
  // happens to have installed. If a machine's own face were still first in the stack, this would pass
  // on the developer's machine and fail on a customer's.
  const stacks = await page.evaluate(() => {
    const first = (value: string): string =>
      (value.split(",")[0] ?? "").trim().replaceAll(/^"|"$/g, "");
    return {
      ui: first(getComputedStyle(document.body).fontFamily),
      data: first(getComputedStyle(document.querySelector(".log") as Element).fontFamily),
    };
  });
  expect(stacks.ui).toBe("Inter");
  expect(stacks.data).toBe("JetBrains Mono");

  // Both faces are actually loaded from the bundle rather than merely named: a woff2 that 404s falls
  // back silently and the stack assertion above would still pass.
  const loaded = await page.evaluate(async () => {
    await document.fonts.ready;
    return [...document.fonts].map((face) => `${face.family} ${face.weight}`);
  });
  for (const face of ["Inter 400", "Inter 600", "Inter 700", "JetBrains Mono 400", "JetBrains Mono 500"]) {
    expect(loaded).toContain(face);
  }
});

test("there is no breakpoint, because there is no second size", async ({ page }) => {
  // The whole point, asserted directly. A professional panel does not rearrange itself: an operator's
  // hand knows where the In field is, and the stylesheet must contain no viewport media query that
  // could move it. Checking the source is what makes that a fact rather than a hope.
  const sheets = await Promise.all(
    ["app.css", "tokens.css", "global.css"].map(
      async (name) => await (await page.request.get(`/src/styles/${name}`)).text(),
    ),
  );
  const [layout, tokens, global] = sheets as [string, string, string];

  for (const css of [layout, tokens, global]) {
    expect(css).not.toMatch(/@media\s*\(\s*max-width/);
    expect(css).not.toMatch(/@media\s*\(\s*min-width/);
  }

  // The media queries that *are* present are about the user's preferences, not about the viewport.
  expect(tokens).toContain("prefers-reduced-motion");
  expect(tokens).toContain("forced-colors");
});

test("the panel still fits at the window's own minimum", async ({ page }) => {
  await page.setViewportSize(MINIMUM);
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

  // The form column is the safety valve below the supported range. At the supported minimum it must
  // not be scrolled at all — a scrollbar inside the frame is what this file exists to prevent.
  const formOverflow = await page.evaluate(() => {
    const column = document.querySelector(".app__col--form");
    return column === null ? 0 : column.scrollHeight - column.clientHeight;
  });
  expect(formOverflow).toBeLessThanOrEqual(1);
});
