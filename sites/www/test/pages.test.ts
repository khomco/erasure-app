// Renders real pages with the Astro container and runs the build-time site
// check over them, so a bare overclaim in page copy fails `pnpm test` as well
// as `astro build`.
import { experimental_AstroContainer as AstroContainer } from "astro/container";
import { parse, type HTMLElement } from "node-html-parser";
import { beforeAll, describe, expect, it } from "vitest";
import { capabilities } from "../src/data/capabilities";
import { checkPage } from "../src/lib/site-check";
import Index from "../src/pages/index.astro";
import EarlyAccess from "../src/pages/early-access.astro";
import Capabilities from "../src/pages/capabilities.astro";

const ORIGIN = "https://wipestation.example";

let container: AstroContainer;
beforeAll(async () => {
  container = await AstroContainer.create({ astroConfig: { site: ORIGIN } });
});

// Astro component factories have no exported type; the container accepts any of them.
type Page = Parameters<AstroContainer["renderToString"]>[0];
const render = (page: Page, path: string) =>
  container.renderToString(page, { partial: false, request: new Request(ORIGIN + path) });

const pages: [string, Page][] = [
  ["/", Index],
  ["/early-access/", EarlyAccess],
  ["/capabilities/", Capabilities],
];

describe.each(pages)("page %s", (path, page) => {
  it("passes the site check", async () => {
    expect(checkPage(await render(page, path), path, capabilities, ORIGIN)).toEqual([]);
  });

  it("ships no JavaScript", async () => {
    expect(await render(page, path)).not.toMatch(/<script/i);
  });
});

describe("landing page", () => {
  let root: HTMLElement;
  beforeAll(async () => {
    root = parse(await render(Index, "/"));
  });

  it("has exactly one primary CTA, to /early-access/", () => {
    const ctas = root.querySelectorAll('[data-cta="primary"]');
    expect(ctas).toHaveLength(1);
    expect(ctas[0].tagName).toBe("A");
    expect(ctas[0].getAttribute("href")).toBe("/early-access/");
  });

  it("has the sections in order, each labelled by its heading", () => {
    const sections = root.querySelectorAll("main section");
    const ids = sections.map((s) => s.getAttribute("aria-labelledby"));
    expect(ids).toEqual(["hero-title", "problem", "certificate", "different", "not", "join"]);
    for (const [i, s] of sections.entries()) {
      const heading = s.querySelector(`#${ids[i]}`);
      expect(heading?.tagName).toBe(i === 0 ? "H1" : "H2");
    }
    const h2s = root.querySelectorAll("main h2").map((h) => h.text.trim());
    expect(h2s).toEqual([
      "The problem: proof that is hard to defend",
      "A certificate anyone can check",
      "Why it is different",
      "What we are not",
      "Early access",
    ]);
  });

  it("labels the claims in every claim section", () => {
    for (const id of ["certificate", "different", "not"]) {
      const section = root.querySelector(`section[aria-labelledby="${id}"]`);
      expect(section?.querySelectorAll("[data-capability]").length, id).toBeGreaterThan(0);
    }
    const cert = root.querySelector('section[aria-labelledby="certificate"]')!;
    const ids = cert.querySelectorAll("[data-capability]").map((e) => e.getAttribute("data-capability"));
    expect(ids).toEqual(
      expect.arrayContaining(["offline-verification", "command-level-evidence", "open-cert-format"]),
    );
  });

  it("states the pre-launch, simulated-drives status near the top", () => {
    const hero = root.querySelector('section[aria-labelledby="hero-title"]')!;
    expect(hero.text).toMatch(/pre-launch/i);
    expect(hero.text).toMatch(/every pre-release build runs against\s+simulated drives/i);
  });
});
