// `defineConfig` from vite, and the unit tests live in `vitest.config.ts` — see that file for why the
// `test` block is not here. Importing vitest's config type into this file pulls a second copy of
// vite's types into the project, and then every plugin fails to type-check against itself.
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/*
 * The interface is loaded by Tauri from a local `dist` in production, so there is no proxy to set up.
 *
 * ## The `host` line, and why it is explicit
 *
 * Vite's dev server binds `localhost`, which on Windows resolves to `::1` first. Tauri's `devUrl` and
 * Playwright's health check both address `127.0.0.1`, and a server listening only on the IPv6
 * loopback is a connection refused from the IPv4 one — which is a `webServer` timeout with no error
 * message that says so. Binding the IPv4 loopback explicitly makes the address everybody already uses
 * the address that works.
 */
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
    // Tauri's dev server watches the Rust side; watching it here as well would rebuild the page on
    // every `cargo` write.
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "chrome110",
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
    // One bundle, no code splitting: the whole interface is smaller than a single React chunk, and a
    // dynamic import would be a request against the local asset protocol for no benefit.
    rollupOptions: { output: { manualChunks: undefined } },
  },
});
