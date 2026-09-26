import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The interface is loaded by Tauri from a local `dist`, so there is no server to configure and no
// proxy to set up. `build.target` is pinned to the WebView2 baseline every supported Windows
// machine has, and the source maps are off in production because the bundle ships inside an
// installer rather than to a browser.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
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
    // One bundle, no code splitting: the whole interface is smaller than a single React chunk, and
    // a dynamic import would be a request against the local asset protocol for no benefit.
    rollupOptions: { output: { manualChunks: undefined } },
  },
});
