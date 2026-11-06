import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  server: {
    // 5173 is apps/desktop's dev server.
    port: 5174,
    strictPort: true,
    proxy: {
      // During Vite dev, proxy the API to a local `wipe-hub serve`.
      // Set e.g. `VITE_API_TARGET=http://127.0.0.1:7901` to override.
      "/api": {
        target: process.env.VITE_API_TARGET || "http://127.0.0.1:7900",
        changeOrigin: true,
      },
    },
  },
  build: {
    // Served by `wipe-hub serve` from apps/portal/dist (or --portal-dir).
    outDir: "dist",
    // crates/wipe-hub/src/lib.rs serves `assets/` with no SPA fallback, so a
    // missing hashed bundle 404s instead of returning index.html. Keep in step.
    assetsDir: "assets",
    // The hub serves dist publicly; do not ship source maps.
    sourcemap: false,
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
