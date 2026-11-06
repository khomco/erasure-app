#!/usr/bin/env node
// Local Lighthouse check (CAP-4; no CI exists, ROADMAP §0.5).
// Builds the site, serves it with `astro preview`, runs Lighthouse in headless
// local Chrome on every URL in the sitemap, and exits non-zero if performance
// or accessibility is below the threshold on any page.
import { spawn, spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as chromeLauncher from "chrome-launcher";
import lighthouse from "lighthouse";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const PORT = Number(process.env.LH_PORT || 4329);
const BASE = `http://localhost:${PORT}`;
const THRESHOLD = 0.9;
const CATEGORIES = ["performance", "accessibility"];

function sh(cmd, args) {
  const r = spawnSync(cmd, args, { cwd: root, stdio: "inherit" });
  if (r.status !== 0) process.exit(r.status ?? 1);
}

function sitemapPaths() {
  const dist = join(root, "dist");
  const index = readFileSync(join(dist, "sitemap-index.xml"), "utf8");
  const paths = [];
  for (const [, loc] of index.matchAll(/<loc>([^<]+)<\/loc>/g)) {
    const file = new URL(loc).pathname.replace(/^\//, "");
    const xml = readFileSync(join(dist, file), "utf8");
    for (const [, page] of xml.matchAll(/<loc>([^<]+)<\/loc>/g)) paths.push(new URL(page).pathname);
  }
  return paths;
}

async function waitForServer(url, timeoutMs = 20000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(url);
      if (res.ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`astro preview did not answer on ${url}`);
}

sh("pnpm", ["run", "build"]);
const paths = sitemapPaths();
if (paths.length === 0) {
  console.error("lighthouse: sitemap lists no pages");
  process.exit(1);
}

const preview = spawn("pnpm", ["exec", "astro", "preview", "--port", String(PORT), "--host", "localhost"], {
  cwd: root,
  stdio: "ignore",
  detached: true,
});
let chrome;
let failed = false;
try {
  await waitForServer(BASE + "/");
  chrome = await chromeLauncher.launch({ chromeFlags: ["--headless=new", "--no-first-run"] });
  for (const path of paths) {
    const result = await lighthouse(BASE + path, {
      port: chrome.port,
      output: "json",
      logLevel: "error",
      onlyCategories: CATEGORIES,
    });
    const scores = CATEGORIES.map((c) => [c, result?.lhr.categories[c]?.score ?? 0]);
    const ok = scores.every(([, s]) => s >= THRESHOLD);
    if (!ok) failed = true;
    console.log(`${ok ? "PASS" : "FAIL"} ${path}  ${scores.map(([c, s]) => `${c}=${Math.round(s * 100)}`).join("  ")}`);
  }
} catch (err) {
  console.error(err);
  failed = true;
} finally {
  if (chrome) await chrome.kill();
  try {
    process.kill(-preview.pid);
  } catch {
    preview.kill();
  }
}
if (failed) {
  console.error(`lighthouse: a score is below ${THRESHOLD * 100}`);
  process.exit(1);
}
console.log(`lighthouse: ${paths.length} page(s), all scores >= ${THRESHOLD * 100}`);
