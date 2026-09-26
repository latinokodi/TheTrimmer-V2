/**
 * Fail the build if the browser stub shipped.
 *
 * ## Why this is a build step and not a test
 *
 * The stub is a development affordance: it answers every IPC command from fixtures so the interface can
 * be run and driven in a browser with no Rust, no WebView2 and no build. It was, for several revisions,
 * also present in the shipped window — because the guard meant to keep it out tested
 * `window.__TAURI__`, the convenience global that `withGlobalTauri` injects late, while the real IPC
 * bridge `window.__TAURI_INTERNALS__` is there from the first script. The stub won the race.
 *
 * The consequence was not a broken button. The shipped application ran *entirely* on fixtures: `doctor`
 * reported a hard-coded ffmpeg build string, the plan and the cut were computed in JavaScript, and the
 * file picker returned a fixed path without opening anything. All 26 browser tests passed, because they
 * run in a browser where the stub is *supposed* to be the bridge, and `tools/smoke-window.ps1` passed
 * because the fixture string it looked for was the string the stub returned.
 *
 * So the check has to be of the **artefact**, not of the behaviour: after `vite build`, look inside
 * `dist` for anything that could only have come from the stub. This runs as part of `npm run build`, so
 * it is impossible to produce a shipped bundle that contains one, and it cannot be skipped by running
 * the tests a different way.
 *
 *   node tools/check-bundle.mjs
 *
 * Exits 0 when the bundle is clean, 1 with the offending file and needle when it is not.
 */

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const webRoot = fileURLToPath(new URL("..", import.meta.url));
const dist = join(webRoot, "dist");

/**
 * Strings that exist only in the stub.
 *
 * The two code markers are literal: the diagnostic marker the stub sets on `window`, and the sentence
 * its `invoke` refuses with. `mocks:!0` is how both esbuild and Rollup minify `mocks: true`, so it
 * survives any renaming of the file.
 *
 * The fixture path is **read out of the stub's own source** rather than copied here. The first version
 * of this list hard-coded it, and it caught a false positive immediately: the same path had been used as
 * a placeholder in the source dialog, which is legitimate user-facing copy. Reading it from the stub
 * means the check follows the stub instead of drifting from it — and one needle that must not collide
 * with anything else is one needle worth deriving.
 */
function needlesFromStub() {
  const source = readFileSync(join(webRoot, "src", "ipc", "stub.ts"), "utf8");
  const needles = [
    ["the browser harness has no", "the stub's refusal sentence"],
    ["__TAURI_STUB__", "the stub's diagnostic marker"],
    ["mocks:!0", "the stub's `mocks` flag, minified"],
  ];
  const fixture = /const FIXTURE_PATH = "((?:[^"\\]|\\.)*)"/.exec(source);
  if (fixture !== null) {
    // The literal is written with doubled backslashes in TypeScript; the bundle carries the real path.
    needles.push([JSON.parse(`"${fixture[1]}"`), "the stub's fixture master path"]);
  }
  return needles;
}

/** Every file under `dist`, so a stub inlined into the HTML is found as well as one in a chunk. */
function walk(directory) {
  return readdirSync(directory).flatMap((entry) => {
    const path = join(directory, entry);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}

if (!statSync(dist, { throwIfNoEntry: false })?.isDirectory()) {
  console.error(`check-bundle: no build at ${dist}. Run \`vite build\` first.`);
  process.exit(1);
}

const NEEDLES = needlesFromStub();
const files = walk(dist);
const found = [];
let bytes = 0;

for (const file of files) {
  const contents = readFileSync(file);
  bytes += contents.length;
  // Fonts and images are binary; a string search across them is both slow and meaningless.
  if (!/\.(js|css|html|json|svg)$/.test(file)) {
    continue;
  }
  const text = contents.toString("utf8");
  for (const [needle, why] of NEEDLES) {
    if (text.includes(needle)) {
      found.push(`${relative(webRoot, file)} contains ${JSON.stringify(needle)} — ${why}`);
    }
  }
}

if (found.length > 0) {
  console.error("check-bundle: FAILED — the browser stub is in the shipped bundle.");
  for (const line of found) {
    console.error(`  ${line}`);
  }
  console.error(
    "\n  The stub must be behind `import.meta.env.DEV` in src/main.tsx so that a production build\n" +
      "  removes it. If this fails, the window will run on fixtures and every command will be fake.",
  );
  process.exit(1);
}

console.log(`check-bundle: ok — ${files.length} files, ${Math.round(bytes / 1024)} kB, no stub`);
