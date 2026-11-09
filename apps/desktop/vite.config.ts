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
    port: 5173,
    strictPort: true,
    proxy: {
      // During Vite dev, proxy API + WebSocket to the local wipestation server.
      // Set `VITE_API_TARGET=http://127.0.0.1:7878` to override.
      "/api": {
        target: process.env.VITE_API_TARGET || "http://127.0.0.1:7878",
        changeOrigin: true,
        ws: true,
      },
    },
  },
  build: {
    outDir: "dist",
    sourcemap: true,
  },
  test: {
    // Node environment by default: most units under test are pure domain
    // logic. A component test that needs a DOM opts in per file with a
    // `// @vitest-environment happy-dom` header (e.g. ScanInput.test.ts).
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
