/**
 * The runner for `specs/features/window.feature`.
 *
 * ## Why there is a runner here and pytest-bdd on the engine side
 *
 * The engine's scenarios run under pytest-bdd, which is the standard tool for the job. Vitest has
 * no Gherkin plugin in this project, and adding one for six scenarios is a dependency that would
 * have to be maintained to run fifteen lines of logic. So the feature file is parsed here — it
 * stays the source of truth, and a scenario that is not implemented fails rather than being
 * quietly skipped — and the steps are a table of patterns.
 *
 * ## What a step may do
 *
 * The same rule as the engine's steps: build the smallest state the sentence needs, call the real
 * function, and let the `Then` read as the requirement. Nothing here asserts.
 */

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { type LogLine, type RunProgress, newestFirst, remainingSeconds, stepFraction } from "./useRunLog";

const HERE = dirname(fileURLToPath(import.meta.url));
const FEATURE = resolve(HERE, "../../../specs/features/window.feature");

/** One step of a scenario, with the keyword it inherited from the last one that named one. */
interface Step {
  readonly keyword: "Given" | "When" | "Then";
  readonly text: string;
}

interface Scenario {
  readonly name: string;
  readonly steps: readonly Step[];
}

interface World {
  progress: RunProgress;
  bar: number | null;
  estimate: number | null;
  lines: readonly LogLine[];
  ordered: readonly LogLine[];
}

function blank(): World {
  return {
    progress: {
      stage: null,
      doneSeconds: null,
      totalSeconds: null,
      speed: null,
      frame: null,
      size: null,
    },
    bar: null,
    estimate: null,
    lines: [],
    ordered: [],
  };
}

/** Parse the feature file into scenarios. Deliberately small: the file is written, not generated. */
function parse(source: string): Scenario[] {
  const scenarios: Scenario[] = [];
  let current: { name: string; steps: Step[] } | null = null;
  let keyword: Step["keyword"] = "Given";

  for (const raw of source.split(/\r?\n/)) {
    const line = raw.trim();
    if (line === "" || line.startsWith("#") || line.startsWith("Feature:")) {
      continue;
    }
    if (line.startsWith("Scenario:")) {
      current = { name: line.slice("Scenario:".length).trim(), steps: [] };
      scenarios.push(current);
      continue;
    }
    const match = /^(Given|When|Then|And|But)\s+(.*)$/.exec(line);
    if (match === null || current === null) {
      continue;
    }
    const [, named, text] = match;
    if (named !== "And" && named !== "But") {
      keyword = named as Step["keyword"];
    }
    current.steps.push({ keyword, text: text ?? "" });
  }
  return scenarios;
}

type StepFunction = (world: World, ...args: string[]) => void;

/** The steps, as patterns. A step with no entry fails the scenario that uses it. */
const STEPS: readonly (readonly [RegExp, StepFunction])[] = [
  [
    /^a pass of (\d+) seconds that is (\d+) seconds in$/,
    (world, total, done) => {
      world.progress = {
        ...world.progress,
        totalSeconds: Number(total),
        doneSeconds: Number(done),
      };
    },
  ],
  [
    /^a pass of (\d+) seconds that is (\d+) seconds in at (\d+) times speed$/,
    (world, total, done, speed) => {
      world.progress = {
        ...world.progress,
        totalSeconds: Number(total),
        doneSeconds: Number(done),
        speed: Number(speed),
      };
    },
  ],
  [
    /^a pass of (\d+) seconds that is (\d+) seconds in without a rate$/,
    (world, total, done) => {
      world.progress = {
        ...world.progress,
        totalSeconds: Number(total),
        doneSeconds: Number(done),
        speed: null,
      };
    },
  ],
  [
    /^a pass 25 seconds in whose length is not known$/,
    (world) => {
      world.progress = { ...world.progress, doneSeconds: 25, totalSeconds: null };
    },
  ],
  [
    /^a pass whose length is 100 seconds and which has reported nothing$/,
    (world) => {
      world.progress = { ...world.progress, doneSeconds: null, totalSeconds: 100 };
    },
  ],
  [/^the bar is drawn$/, (world) => { world.bar = stepFraction(world.progress); }],
  [/^the estimate is drawn$/, (world) => { world.estimate = remainingSeconds(world.progress); }],
  [
    /^the bar is ([\d.]+) full$/,
    (world, wanted) => { expect(world.bar).toBeCloseTo(Number(wanted), 6); },
  ],
  [
    /^there is no fraction to draw$/,
    (world) => { expect(world.bar).toBeNull(); },
  ],
  [
    /^(\d+) seconds are left$/,
    (world, wanted) => { expect(world.estimate).toBeCloseTo(Number(wanted), 6); },
  ],
  [
    /^there is no estimate to draw$/,
    (world) => { expect(world.estimate).toBeNull(); },
  ],
  [
    /^a run that has said "(.+)"$/,
    (world, quoted) => {
      world.lines = quoted.split('", "').map((text, index) => ({
        at: index + 1,
        offset: index,
        text: text.replace(/^"|"$/g, ""),
        tone: "stage" as const,
      }));
    },
  ],
  [/^the log is drawn$/, (world) => { world.ordered = newestFirst(world.lines); }],
  [
    /^the first line is "(.+)"$/,
    (world, wanted) => { expect(world.ordered[0]?.text).toBe(wanted); },
  ],
  [
    /^the last line is "(.+)"$/,
    (world, wanted) => { expect(world.ordered[world.ordered.length - 1]?.text).toBe(wanted); },
  ],
];

function run(step: Step, world: World): void {
  for (const [pattern, handler] of STEPS) {
    const match = pattern.exec(step.text);
    if (match !== null) {
      handler(world, ...match.slice(1));
      return;
    }
  }
  throw new Error(
    `no step definition for: ${step.keyword} "${step.text}" — the feature and the steps have parted company`,
  );
}

const scenarios = parse(readFileSync(FEATURE, "utf8"));

describe("specs/features/window.feature", () => {
  it("has scenarios to run", () => {
    expect(scenarios.length).toBeGreaterThan(0);
  });

  for (const scenario of scenarios) {
    it(scenario.name, () => {
      const world = blank();
      for (const step of scenario.steps) {
        run(step, world);
      }
    });
  }
});
