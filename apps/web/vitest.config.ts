import { defineConfig } from "vitest/config";

/**
 * The unit tests, kept out of `vite.config.ts` on purpose.
 *
 * There are two test runners here and they want different things. Vitest runs `src/**\/*.test.ts` in
 * Node with no DOM, because what is worth unit-testing in this interface is the arithmetic and the
 * formatting — `lib/format.ts` and nothing else — and a rendered component test would assert that
 * this build of React puts a `<span>` where the last one did.
 *
 * Playwright runs everything under `tests/`, in a real browser, against the real interface and the
 * stub bridge. That is where the behaviour is checked: which controls are offered, what the fields
 * say, whether the button that cuts is enabled.
 *
 * Two configs rather than one `vite.config.ts` with a `test` block, because adding vitest's config
 * type to vite's config file pulls a second copy of vite's types into the project and every plugin
 * then fails to type-check against itself. Two files is the smaller price.
 */
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    environment: "node",
    reporters: "default",
  },
});
