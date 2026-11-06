// @ts-check
import { defineConfig } from "astro/config";
import sitemap from "@astrojs/sitemap";
import tailwindcss from "@tailwindcss/vite";
import siteCheck from "./src/integrations/site-check.ts";

// Placeholder origin until the vendor picks a domain; override with SITE_URL.
const site = process.env.SITE_URL || "https://wipestation.example";

export default defineConfig({
  site,
  output: "static",
  trailingSlash: "ignore",
  // Order matters: the site check runs after the sitemap is written.
  integrations: [sitemap(), siteCheck()],
  vite: {
    plugins: [tailwindcss()],
  },
});
