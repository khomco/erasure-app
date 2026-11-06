/// <reference types="vitest/config" />
// Astro's Vite config, so tests can import and render `.astro` pages.
import { getViteConfig } from "astro/config";

export default getViteConfig({ test: {} });
