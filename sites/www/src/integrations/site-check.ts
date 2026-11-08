/**
 * Astro integration: after the static build, run the honest-claims and
 * page-hygiene check (`src/lib/site-check.ts`) over every emitted `.html`
 * file, scan every emitted `.css` file for third-party `url()`/`@import`, and fail the build on any violation (ADR-0006 §2, CAP-3/CAP-4).
 *
 * At build start (`astro build` only, never `astro dev`), it also refuses to
 * open the lead forms (`PUBLIC_HUB_ORIGIN`, lead-capture story 1) while the
 * privacy notice is a placeholder: the published placeholder promises the
 * forms will not open before the notice exists. The check reads the variable
 * from Vite's resolved env, so a value in a `.env` file counts too.
 *
 * Register it after `@astrojs/sitemap` so the sitemap already exists.
 */
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import type { AstroIntegration } from "astro";
import { site } from "../config";
import { capabilities } from "../data/capabilities";
import { checkCss, checkPage, type Violation } from "../lib/site-check";

function filesWithExtension(dir: string, ext: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...filesWithExtension(full, ext));
    else if (entry.name.endsWith(ext)) out.push(full);
  }
  return out.sort();
}

/**
 * Why a build may not open the lead forms, or `null` when it may: they open
 * only when `hubOrigin` is set, and only once the privacy notice is published.
 */
export function leadFormsBuildError(
  hubOrigin: string | undefined,
  privacyNoticePublished: boolean,
): string | null {
  if (!hubOrigin?.trim() || privacyNoticePublished) return null;
  return (
    "lead forms: PUBLIC_HUB_ORIGIN is set, but the privacy notice is unpublished " +
    "(site.privacyNoticePublished is false in src/config.ts). The privacy page promises the " +
    "forms will not open before the full notice is published: publish it first, or build " +
    "without PUBLIC_HUB_ORIGIN."
  );
}

export default function siteCheck(
  options: { privacyNoticePublished?: boolean } = {},
): AstroIntegration {
  const privacyNoticePublished = options.privacyNoticePublished ?? site.privacyNoticePublished;
  let siteOrigin = "";
  return {
    name: "wipestation:site-check",
    hooks: {
      "astro:config:setup": ({ command, updateConfig }) => {
        if (command !== "build") return;
        updateConfig({
          vite: {
            plugins: [
              {
                name: "wipestation:lead-forms-guard",
                configResolved(resolved: { env: Record<string, unknown> }) {
                  const fromEnv = resolved.env.PUBLIC_HUB_ORIGIN;
                  const value = typeof fromEnv === "string" ? fromEnv : process.env.PUBLIC_HUB_ORIGIN;
                  const error = leadFormsBuildError(value, privacyNoticePublished);
                  if (error) throw new Error(error);
                },
              },
            ],
          },
        });
      },
      "astro:config:done": ({ config }) => {
        if (!config.site) throw new Error("site-check: `site` must be set in astro.config.mjs");
        siteOrigin = config.site;
      },
      "astro:build:done": ({ dir, logger }) => {
        const root = fileURLToPath(dir);
        const files = filesWithExtension(root, ".html");
        const pathOf = (file: string) => "/" + relative(root, file).split(sep).join("/");
        const violations: Violation[] = [];
        if (!existsSync(join(root, "sitemap-index.xml"))) {
          violations.push({ page: "sitemap-index.xml", rule: "missing-artifact", message: "sitemap-index.xml was not generated" });
        }
        for (const file of files) {
          violations.push(...checkPage(readFileSync(file, "utf8"), pathOf(file), capabilities, siteOrigin));
        }
        for (const file of filesWithExtension(root, ".css")) {
          violations.push(...checkCss(readFileSync(file, "utf8"), pathOf(file), siteOrigin));
        }
        if (violations.length > 0) {
          for (const v of violations) {
            logger.error(`[${v.rule}]${v.capability ? ` [${v.capability}]` : ""} ${v.message}`);
          }
          throw new Error(`site-check: ${violations.length} violation(s) in ${files.length} page(s); see above`);
        }
        logger.info(`${files.length} page(s) checked, 0 violations`);
      },
    },
  };
}
