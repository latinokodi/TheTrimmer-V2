import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

/**
 * The window's build.
 *
 * `base: "./"` because the page is served by the engine at the root of its origin and must
 * also work from any other mount point, including the Vite dev server. An absolute
 * `/assets/...` would tie the build to being mounted at exactly `/`.
 */
export default defineConfig({
  base: "./",
  plugins: [react()],
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // The window is a single page with no code splitting worth doing, and a name that does
    // not change between builds means the Electron shell cannot cache a stale one.
    rollupOptions: { output: { entryFileNames: "app.js", assetFileNames: "app.[ext]" } },
  },
});
