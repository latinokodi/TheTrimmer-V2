import { defineConfig } from "vitest/config";

/**
 * The interface's unit tests.
 *
 * Kept out of `vite.config.ts` on purpose: adding vitest's config type to vite's config file pulls a
 * second copy of vite's types into the project, and every plugin then fails to type-check against
 * itself. Two files is the smaller price.
 *
 * The scope is deliberately narrow. What is worth testing without a browser is the arithmetic and
 * the formatting — `lib/format.ts` and the two figures the progress bar is allowed to show — because
 * those are the places a wrong answer is invisible. Everything else in this interface is a layout
 * question, and the way to check a layout is to look at the running window.
 */
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    environment: "node",
    reporters: "default",
  },
});
