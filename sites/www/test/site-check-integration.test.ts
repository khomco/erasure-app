import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import siteCheck, { leadFormsBuildError } from "../src/integrations/site-check";

const SITE = "https://wipestation.example";

const page = (body: string) => `<!doctype html><html lang="en"><head>
  <title>Fixture | Wipestation</title>
  <meta name="description" content="A fixture page.">
  <link rel="canonical" href="${SITE}/">
  <meta property="og:title" content="Fixture | Wipestation">
  <meta property="og:description" content="A fixture page.">
  <meta property="og:url" content="${SITE}/">
</head><body>${body}</body></html>`;

const dirs: string[] = [];
afterEach(() => {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

function fixtureDir(files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), "www-site-check-"));
  dirs.push(dir);
  for (const [rel, content] of Object.entries(files)) {
    mkdirSync(join(dir, rel, ".."), { recursive: true });
    writeFileSync(join(dir, rel), content);
  }
  return dir;
}

/** Run the integration's hooks the way `astro build` does; returns the logged errors. */
async function runHooks(dir: string): Promise<{ errors: string[]; run: Promise<void> }> {
  const integration = siteCheck();
  const hooks = integration.hooks as Record<string, (opts: unknown) => unknown>;
  const errors: string[] = [];
  const logger = {
    error: (m: string) => errors.push(m),
    info: () => {},
    warn: () => {},
    debug: () => {},
  };
  await hooks["astro:config:done"]({ config: { site: SITE }, logger });
  const run = (async () => {
    await hooks["astro:build:done"]({ dir: pathToFileURL(dir + "/"), pages: [], assets: new Map(), logger });
  })();
  return { errors, run };
}

describe("site-check integration (astro:build:done)", () => {
  it("passes a clean build directory", async () => {
    const dir = fixtureDir({
      "index.html": page("<p>Hello</p>"),
      "sitemap-index.xml": "<sitemapindex/>",
      "_astro/a.css": ".a{background:url(/a.png)}",
    });
    const { errors, run } = await runHooks(dir);
    await expect(run).resolves.toBeUndefined();
    expect(errors).toEqual([]);
  });

  it("recurses into subdirectories and fails on a bare guarded phrase, logging page and capability", async () => {
    const dir = fixtureDir({
      "index.html": page("<p>Hello</p>"),
      "sitemap-index.xml": "<sitemapindex/>",
      "nested/deep/index.html": page("<p>Sanitizes NVMe drives</p>"),
    });
    const { errors, run } = await runHooks(dir);
    await expect(run).rejects.toThrow(/site-check: \d+ violation/);
    const log = errors.join("\n");
    expect(log).toContain("/nested/deep/index.html");
    expect(log).toContain("nvme-sanitize");
  });

  it("fails when an emitted stylesheet loads from another origin", async () => {
    const dir = fixtureDir({
      "index.html": page("<p>Hello</p>"),
      "sitemap-index.xml": "<sitemapindex/>",
      "_astro/a.css": '@import "https://fonts.googleapis.com/css2";',
    });
    const { errors, run } = await runHooks(dir);
    await expect(run).rejects.toThrow();
    expect(errors.join("\n")).toMatch(/\[third-party\].*\/_astro\/a\.css/);
  });

  it("fails when the sitemap is missing", async () => {
    const dir = fixtureDir({ "index.html": page("<p>Hello</p>") });
    const { errors, run } = await runHooks(dir);
    await expect(run).rejects.toThrow();
    expect(errors.join("\n")).toMatch(/\[missing-artifact\].*sitemap-index\.xml/);
  });
});

describe("lead-forms guard (astro build only)", () => {
  type Plugin = { name: string; configResolved: (c: { env: Record<string, unknown> }) => void };

  /** The Vite plugins the integration adds for `command`. */
  async function pluginsFor(command: string, privacyNoticePublished?: boolean): Promise<Plugin[]> {
    const integration = siteCheck(privacyNoticePublished === undefined ? {} : { privacyNoticePublished });
    const hooks = integration.hooks as Record<string, (opts: unknown) => unknown>;
    const plugins: Plugin[] = [];
    await hooks["astro:config:setup"]({
      command,
      updateConfig: (c: { vite?: { plugins?: Plugin[] } }) => plugins.push(...(c.vite?.plugins ?? [])),
    });
    return plugins;
  }

  it("names the unpublished privacy notice when PUBLIC_HUB_ORIGIN is set", () => {
    const error = leadFormsBuildError("http://localhost:7900", false);
    expect(error).toMatch(/PUBLIC_HUB_ORIGIN/);
    expect(error).toMatch(/privacy notice is unpublished/);
    expect(error).toMatch(/privacyNoticePublished/);
  });

  it("allows closed forms, and open ones once the notice is published", () => {
    for (const unset of [undefined, "", "  "]) expect(leadFormsBuildError(unset, false)).toBeNull();
    expect(leadFormsBuildError("http://localhost:7900", true)).toBeNull();
  });

  it("fails a build given a hub origin while the notice is a placeholder", async () => {
    const [plugin] = await pluginsFor("build");
    expect(plugin.name).toBe("wipestation:lead-forms-guard");
    expect(() => plugin.configResolved({ env: { PUBLIC_HUB_ORIGIN: "http://localhost:7900" } })).toThrow(
      /privacy notice is unpublished/,
    );
    expect(() => plugin.configResolved({ env: {} })).not.toThrow();
    const [published] = await pluginsFor("build", true);
    expect(() => published.configResolved({ env: { PUBLIC_HUB_ORIGIN: "http://localhost:7900" } })).not.toThrow();
  });

  it("leaves the dev server alone", async () => {
    expect(await pluginsFor("dev")).toEqual([]);
  });
});
