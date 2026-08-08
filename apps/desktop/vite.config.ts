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
    // Node environment: the units under test are pure domain logic, not
    // components. A DOM harness can be added when component tests arrive.
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
