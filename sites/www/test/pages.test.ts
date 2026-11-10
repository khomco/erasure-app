// Renders real pages with the Astro container and runs the build-time site
// check over them, so a bare overclaim in page copy fails `pnpm test` as well
// as `astro build`.
import { experimental_AstroContainer as AstroContainer } from "astro/container";
import { parse, type HTMLElement } from "node-html-parser";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { capabilities } from "../src/data/capabilities";
import { checkPage } from "../src/lib/site-check";
import Index from "../src/pages/index.astro";
import EarlyAccess from "../src/pages/early-access.astro";
import Capabilities from "../src/pages/capabilities.astro";
import Product from "../src/pages/product.astro";
import Standards from "../src/pages/standards.astro";
import Segments from "../src/pages/segments/index.astro";
import SegmentItad from "../src/pages/segments/itad.astro";
import SegmentEnterpriseIt from "../src/pages/segments/enterprise-it.astro";
import SegmentGovernment from "../src/pages/segments/government.astro";
import Pricing from "../src/pages/pricing.astro";
import Contact from "../src/pages/contact.astro";
import Privacy from "../src/pages/privacy.astro";
import Terms from "../src/pages/terms.astro";
import LeadForm from "../src/components/LeadForm.astro";
import { consentText } from "../src/data/consent";

const ORIGIN = "https://wipestation.example";
const HUB = "http://localhost:7900";

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
  ["/product/", Product],
  ["/standards/", Standards],
  ["/segments/", Segments],
  ["/segments/itad/", SegmentItad],
  ["/segments/enterprise-it/", SegmentEnterpriseIt],
  ["/segments/government/", SegmentGovernment],
  ["/pricing/", Pricing],
  ["/contact/", Contact],
  ["/privacy/", Privacy],
  ["/terms/", Terms],
];

describe.each(pages)("page %s", (path, page) => {
  it("passes the site check", async () => {
    expect(checkPage(await render(page, path), path, capabilities, ORIGIN)).toEqual([]);
  });

  it("ships no JavaScript", async () => {
    expect(await render(page, path)).not.toMatch(/<script/i);
  });

  it("has one h1, a title and a description", async () => {
    const root = parse(await render(page, path));
    expect(root.querySelectorAll("h1")).toHaveLength(1);
    expect(root.querySelector("head title")?.text.trim()).toBeTruthy();
    expect(root.querySelector('meta[name="description"]')?.getAttribute("content")?.trim()).toBeTruthy();
  });
});

const LEAD_FIELDS = {
  "early-access": ["name", "email", "company", "role", "segment", "message", "consent"],
  contact: ["name", "email", "company", "message", "consent"],
} as const;

/** The form's submit controls (an implicit-type button submits). */
const submitsOf = (form: HTMLElement) =>
  form
    .querySelectorAll("button, input")
    .filter((el) =>
      el.tagName === "BUTTON"
        ? (el.getAttribute("type") ?? "submit") === "submit"
        : ["submit", "image"].includes(el.getAttribute("type") ?? ""),
    );

const formPages = [
  ["/early-access/", EarlyAccess, "early-access"],
  ["/contact/", Contact, "contact"],
] as const;

describe.each(formPages)("closed form on %s (no PUBLIC_HUB_ORIGIN)", (path, page, kind) => {
  let html: string;
  let root: HTMLElement;
  beforeAll(async () => {
    html = await render(page, path);
    root = parse(html);
  });

  it("posts nowhere: one form, no action, its only submit button disabled", () => {
    const forms = root.querySelectorAll("form");
    expect(forms).toHaveLength(1);
    const form = forms[0];
    expect(form.hasAttribute("action")).toBe(false);
    expect(form.hasAttribute("method")).toBe(false);
    expect(form.hasAttribute("data-hub-origin")).toBe(false);
    const submits = submitsOf(form);
    expect(submits).toHaveLength(1);
    expect(submits[0].hasAttribute("disabled")).toBe(true);
    expect(form.text).toMatch(/not open yet/i);
    expect(form.text).toMatch(/nothing you type here is sent/i);
  });

  it("ships no script and no honeypot, and the page says the form is not open", () => {
    expect(html).not.toMatch(/<script|<noscript/i);
    expect(root.querySelector('[name="website"]')).toBeNull();
    expect(root.querySelector('meta[name="description"]')!.getAttribute("content")).toMatch(/not open yet/);
  });

  it("has exactly the lead-capture field names, each with a visible label", () => {
    const form = root.querySelector("form")!;
    const controls = form.querySelectorAll("input, select, textarea");
    expect(controls.map((c) => c.getAttribute("name"))).toEqual([...LEAD_FIELDS[kind]]);
    for (const c of controls) {
      const id = c.getAttribute("id");
      expect(id, c.getAttribute("name")).toBeTruthy();
      const label = form.querySelector(`label[for="${id}"]`);
      expect(label, c.getAttribute("name")).not.toBeNull();
      expect(label!.text.trim().length).toBeGreaterThan(0);
    }
    const consent = form.querySelector('input[name="consent"]')!;
    expect(consent.getAttribute("type")).toBe("checkbox");
    const consentLabel = form.querySelector(`label[for="${consent.getAttribute("id")}"]`)!;
    expect(consentLabel.querySelector('a[href="/privacy/"]')).not.toBeNull();
  });

  it("labels the consent box with exactly the text the hub stores", () => {
    const consent = root.querySelector('input[name="consent"]')!;
    const label = root.querySelector(`label[for="${consent.getAttribute("id")}"]`)!;
    expect(label.text.replace(/\s+/g, " ").trim()).toBe(consentText(kind));
  });

  it("collects nothing by other means", () => {
    expect(html).not.toMatch(/mailto:|tel:/i);
    expect(html).not.toMatch(/\bfetch\(/);
  });
});

describe.each(formPages)("open form on %s (PUBLIC_HUB_ORIGIN set)", (path, page, kind) => {
  let html: string;
  let root: HTMLElement;
  beforeAll(async () => {
    vi.stubEnv("PUBLIC_HUB_ORIGIN", `${HUB}/`);
    try {
      html = await render(page, path);
    } finally {
      vi.unstubAllEnvs();
    }
    root = parse(html);
  });

  it("names the hub, has no action, and leaves its button for the script to enable", () => {
    const forms = root.querySelectorAll("form");
    expect(forms).toHaveLength(1);
    const form = forms[0];
    expect(form.getAttribute("data-hub-origin")).toBe(HUB);
    expect(form.getAttribute("data-lead-form")).toBe(kind);
    expect(form.hasAttribute("action")).toBe(false);
    const submits = submitsOf(form);
    expect(submits).toHaveLength(1);
    expect(submits[0].hasAttribute("disabled")).toBe(true);
    expect(form.text).not.toMatch(/not open yet/i);
    expect(form.querySelector('[data-lead-status][aria-live="polite"]')).not.toBeNull();
  });

  it("ships the script and a noscript note", () => {
    expect(html).toMatch(/<script/i);
    const noscript = root.querySelector("form noscript");
    expect(noscript?.text).toMatch(/needs JavaScript/);
  });

  it("adds a labelled, off-screen, aria-hidden, untabbable honeypot", () => {
    const form = root.querySelector("form")!;
    const controls = form.querySelectorAll("input, select, textarea");
    const names = controls.map((c) => c.getAttribute("name"));
    expect(names.filter((n) => n !== "website")).toEqual([...LEAD_FIELDS[kind]]);
    const trap = form.querySelector('input[name="website"]')!;
    expect(trap.getAttribute("tabindex")).toBe("-1");
    expect(trap.getAttribute("autocomplete")).toBe("off");
    expect(trap.hasAttribute("required")).toBe(false);
    expect(form.querySelector(`label[for="${trap.getAttribute("id")}"]`)).not.toBeNull();
    const wrapper = trap.parentNode as HTMLElement;
    expect(wrapper.getAttribute("aria-hidden")).toBe("true");
    expect(wrapper.getAttribute("class")).toMatch(/absolute -left-\[10000px\]/);
  });

  it("passes the site check and no longer says it sends nothing", () => {
    expect(checkPage(html, path, capabilities, ORIGIN)).toEqual([]);
    expect(root.querySelector('meta[name="description"]')!.getAttribute("content")).not.toMatch(
      /not open|sends nothing/,
    );
    expect(root.querySelector("main")!.text).not.toMatch(/sends nothing|until sign-up opens|when the contact form opens/i);
  });
});

describe("early-access segments", () => {
  it("offers exactly the four segment values behind an empty, disabled placeholder", async () => {
    const root = parse(await render(EarlyAccess, "/early-access/"));
    const [placeholder, ...options] = root.querySelectorAll('select[name="segment"] option');
    expect(placeholder.getAttribute("value")).toBe("");
    expect(placeholder.hasAttribute("disabled")).toBe(true);
    expect(placeholder.hasAttribute("selected")).toBe(true);
    expect(options.map((o) => o.getAttribute("value"))).toEqual(["itad", "enterprise-it", "government", "other"]);
    expect(options.some((o) => o.hasAttribute("selected"))).toBe(false);
  });
});

describe("LeadForm dev notice", () => {
  const renderForm = (devNotice: boolean) =>
    container.renderToString(LeadForm, { props: { kind: "early-access", devNotice } });

  it("says how to open the form when dev is on", async () => {
    const root = parse(await renderForm(true));
    const notice = root.querySelector("[data-dev-notice]");
    expect(notice?.text).toMatch(/PUBLIC_HUB_ORIGIN/);
    expect(notice?.text).toMatch(/POST \/api\/leads/);
  });

  it("names the hub it posts to when dev is on and the form is open", async () => {
    const html = await container.renderToString(LeadForm, {
      props: { kind: "contact", devNotice: true, hubOrigin: HUB },
    });
    expect(parse(html).querySelector("[data-dev-notice]")?.text).toContain(`${HUB}/api/leads`);
  });

  describe.each([
    ["/early-access/", EarlyAccess],
    ["/contact/", Contact],
  ] as const)("default on %s (import.meta.env.DEV)", (path, page) => {
    afterEach(() => {
      vi.unstubAllEnvs();
    });

    it("shows the notice in a dev build", async () => {
      vi.stubEnv("DEV", true);
      expect(parse(await render(page, path)).querySelector("[data-dev-notice]")).not.toBeNull();
    });

    it("hides the notice in a production build, still saying it is not open", async () => {
      vi.stubEnv("DEV", false);
      const root = parse(await render(page, path));
      expect(root.querySelector("[data-dev-notice]")).toBeNull();
      expect(root.querySelector("form")!.text).toMatch(/not open yet/i);
    });
  });

  it("is absent when dev is off, and the form still says it is not open", async () => {
    const html = await renderForm(false);
    expect(html).not.toMatch(/data-dev-notice|PUBLIC_HUB_ORIGIN/);
    expect(parse(html).text).toMatch(/Sign-up is not open yet/);
  });
});

describe("pricing page", () => {
  it("is honest: pricing on request, no amount or currency, links to early access", async () => {
    const html = await render(Pricing, "/pricing/");
    const root = parse(html);
    expect(root.querySelector("main")!.text).toContain("Early-access pricing on request");
    // The whole document: body text, <title>, meta description and OG tags.
    const meta = root.querySelectorAll("meta").map((m) => m.getAttribute("content") ?? "");
    const text = [root.text, ...meta].join(" ");
    expect(text).not.toMatch(/[$€£¥]\s?\d/);
    expect(text).not.toMatch(/\b(USD|EUR|GBP|CAD|AUD|CHF|JPY)\s?\d|\d[\d,.]*\s?(USD|EUR|GBP|CAD|AUD|CHF|JPY)\b/);
    expect(text).not.toMatch(/\b(dollars?|euros?|pounds?( sterling)?|sterling)\b/i);
    expect(text).not.toMatch(/\d[\d,.]*\s*(per|\/|a|an)\s+(station|seat|drive|asset|device|erasure|wipe|user|month|year|licen[cs]e|credit)s?\b/i);
    expect(root.querySelector('main a[href="/early-access/"]')).not.toBeNull();
  });
});

describe("pricing page with published offerings (PRICING_HUB_ORIGIN set)", () => {
  const PRICING_HUB = "http://127.0.0.1:7900";
  const headline = "Renews automatically every year at 1,200.00 USD until you cancel.";
  const annual = {
    id: "11111111-1111-4111-8111-111111111111",
    name: "Station annual",
    description: "One station.\nRenews yearly.",
    price_minor: 120000,
    currency: "USD",
    interval: "annual",
    trial: false,
    grants: { scope: "machine", seats: 1, quota_erasures: null, features: ["hub_sync"], term_months: 12 },
    renewal_notice: { headline, detail: "Your first year ends on 8 November 2027." },
  };
  const oneOff = {
    id: "22222222-2222-4222-8222-222222222222",
    name: "Site pack",
    description: "Five stations at one site.",
    price_minor: 500000,
    currency: "JPY",
    interval: "one_off",
    trial: false,
    grants: { scope: "site", seats: 5, quota_erasures: 10000, features: [], term_months: 24 },
    renewal_notice: null,
  };

  /** Renders /pricing/ against a hub answering `offerings`; returns the HTML and the URLs fetched. */
  async function renderPriced(offerings: unknown[]) {
    const seen: string[] = [];
    vi.stubEnv("PRICING_HUB_ORIGIN", PRICING_HUB);
    vi.stubGlobal("fetch", async (input: RequestInfo | URL) => {
      seen.push(String(input));
      return new Response(JSON.stringify({ offerings }), { status: 200 });
    });
    try {
      return { html: await render(Pricing, "/pricing/"), seen };
    } finally {
      vi.unstubAllEnvs();
      vi.unstubAllGlobals();
    }
  }

  it("lists each offering, passes the site check and ships no script", async () => {
    const { html, seen } = await renderPriced([annual, oneOff]);
    expect(seen).toEqual([`${PRICING_HUB}/api/offerings`]);
    expect(checkPage(html, "/pricing/", capabilities, ORIGIN)).toEqual([]);
    expect(html).not.toMatch(/<script/i);
    expect(html).not.toContain(PRICING_HUB);
    const root = parse(html);
    expect(root.querySelectorAll("h1")).toHaveLength(1);
    expect(root.querySelector('meta[name="description"]')!.getAttribute("content")).toBe(
      "Published prices for Wipestation offerings.",
    );
    expect(root.querySelector("main")!.text).not.toContain("Early-access pricing on request");
    // The licensing principles stay.
    expect(root.querySelector('section[aria-labelledby="principles"]')).not.toBeNull();

    const items = root.querySelectorAll("ul[data-price-list] > li[data-offering]");
    expect(items.map((li) => li.getAttribute("data-offering"))).toEqual([annual.id, oneOff.id]);
    const [a, o] = items;
    const flat = (el: HTMLElement) => el.text.replace(/\s+/g, " ").trim();
    expect(a.querySelector("h2")!.text).toBe("Station annual");
    expect(flat(a.querySelector("[data-price]")!)).toBe("1,200.00 USD per year");
    expect(a.querySelector("[data-description]")!.innerHTML).toMatch(/One station\.\s*<br>\s*Renews yearly\./);
    expect(flat(a)).toContain("1 seat");
    expect(flat(a)).toContain("Machine licence");
    expect(flat(a)).toContain("Unlimited erasures");
    expect(flat(a)).toContain("12-month term");
    expect(a.querySelector("[data-renewal-notice]")!.text.trim()).toBe(headline);
    expect(flat(a)).not.toContain("first year ends");
    expect(flat(a)).not.toContain("hub_sync");

    expect(o.querySelector("h2")!.text).toBe("Site pack");
    expect(flat(o.querySelector("[data-price]")!)).toBe("500,000 JPY one-off payment");
    expect(flat(o)).toContain("5 seats");
    expect(flat(o)).toContain("Site licence");
    expect(flat(o)).toContain("10,000 erasures");
    expect(flat(o)).toContain("24-month term");
    expect(o.querySelector("[data-renewal-notice]")).toBeNull();
    expect(flat(o)).not.toContain("Renews automatically");
  });

  it("shows a free trial as free with a Start free trial link to the portal's /buy", async () => {
    const trial = {
      ...oneOff,
      id: "33333333-3333-4333-8333-333333333333",
      name: "Free trial",
      description: "Try it on one bench.",
      price_minor: 0,
      currency: "USD",
      trial: true,
      grants: { scope: "site", seats: 1, quota_erasures: 25, features: [], term_months: 1 },
    };
    const { html } = await renderPriced([annual, trial]);
    expect(checkPage(html, "/pricing/", capabilities, ORIGIN)).toEqual([]);
    expect(html).not.toMatch(/<script/i);
    const root = parse(html);
    const t = root.querySelector(`li[data-offering="${trial.id}"]`)!;
    const flat = (el: HTMLElement) => el.text.replace(/\s+/g, " ").trim();
    expect(flat(t.querySelector("[data-price]")!)).toBe("Free trial");
    const a = t.querySelector("a[data-start-trial]")!;
    expect(a.text.trim()).toBe("Start free trial");
    expect(a.getAttribute("href")).toBe(`${PRICING_HUB}/buy`);
    expect(flat(t)).toContain("25 erasures");
    expect(flat(t)).not.toContain("USD");
    expect(flat(t)).not.toContain("one-off payment");
    expect(t.querySelector("[data-renewal-notice]")).toBeNull();
    // A priced offering has no trial link.
    expect(root.querySelector(`li[data-offering="${annual.id}"] a[data-start-trial]`)).toBeNull();
  });

  it("renders markup in vendor text as text", async () => {
    const { html } = await renderPriced([{ ...oneOff, description: "<script>alert(1)</script>" }]);
    expect(html).not.toMatch(/<script/i);
    expect(html).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
    expect(parse(html).querySelector("[data-description]")!.text).toBe("<script>alert(1)</script>");
  });

  it("renders a CRLF line break in vendor text as <br>", async () => {
    const { html } = await renderPriced([{ ...oneOff, description: "a\r\nb" }]);
    expect(parse(html).querySelector("[data-description]")!.innerHTML).toBe("a<br>b");
    expect(html).not.toContain("\r");
  });

  it("fails the site check on an overclaim in vendor text", async () => {
    const { html } = await renderPriced([{ ...oneOff, description: "Wipes NVMe drives." }]);
    const violations = checkPage(html, "/pricing/", capabilities, ORIGIN);
    expect(violations.map((v) => v.rule)).toContain("overclaim");
  });

  it("renders the default page for an empty catalog", async () => {
    const { html, seen } = await renderPriced([]);
    expect(seen).toHaveLength(1);
    expect(html).toBe(await render(Pricing, "/pricing/"));
    expect(parse(html).querySelector("main")!.text).toContain("Early-access pricing on request");
  });

  it("fails the render when the hub answers an error", async () => {
    vi.stubEnv("PRICING_HUB_ORIGIN", PRICING_HUB);
    vi.stubGlobal("fetch", async () => new Response("{}", { status: 401 }));
    try {
      await expect(render(Pricing, "/pricing/")).rejects.toThrow(
        `pricing: could not read ${PRICING_HUB}/api/offerings (PRICING_HUB_ORIGIN): HTTP status 401`,
      );
    } finally {
      vi.unstubAllEnvs();
      vi.unstubAllGlobals();
    }
  });

  it("makes no fetch without PRICING_HUB_ORIGIN", async () => {
    const seen: unknown[] = [];
    vi.stubGlobal("fetch", async (input: unknown) => {
      seen.push(input);
      throw new Error("no fetch expected");
    });
    try {
      await render(Pricing, "/pricing/");
    } finally {
      vi.unstubAllGlobals();
    }
    expect(seen).toEqual([]);
  });
});

describe("standards page", () => {
  it("labels the certification roadmap as future, one planned capability per target", async () => {
    const root = parse(await render(Standards, "/standards/"));
    const section = root.querySelector('section[aria-labelledby="roadmap"]')!;
    expect(section.querySelector("#roadmap")!.text).toMatch(/future/i);
    for (const id of ["adisa-al5", "common-criteria-eal2", "r2v3-recognition", "nato-niapc"]) {
      const el = section.querySelector(`[data-capability="${id}"]`);
      expect(el, id).not.toBeNull();
      expect(el!.getAttribute("data-status"), id).toBe("planned");
      expect(el!.text, id).toMatch(/Year/);
    }
  });

  it("wraps the conformance posture in its capability", async () => {
    const root = parse(await render(Standards, "/standards/"));
    expect(root.querySelector('[data-capability="nist-ieee-conformance"]')).not.toBeNull();
  });
});

describe("findable", () => {
  let root: HTMLElement;
  beforeAll(async () => {
    root = parse(await render(Index, "/"));
  });

  it("links the main pages from the header nav", () => {
    const hrefs = root.querySelectorAll('header nav a').map((a) => a.getAttribute("href"));
    expect(hrefs).toEqual(expect.arrayContaining(["/product/", "/standards/", "/segments/", "/pricing/", "/capabilities/"]));
  });

  it("links contact, privacy and terms from the footer", () => {
    const hrefs = root.querySelectorAll("footer a").map((a) => a.getAttribute("href"));
    expect(hrefs).toEqual(expect.arrayContaining(["/contact/", "/privacy/", "/terms/"]));
  });

  it("links the three segment pages from /segments/, each ending with an early-access link", async () => {
    const segRoot = parse(await render(Segments, "/segments/"));
    const hrefs = segRoot.querySelectorAll("main a").map((a) => a.getAttribute("href"));
    expect(hrefs).toEqual(
      expect.arrayContaining(["/segments/itad/", "/segments/enterprise-it/", "/segments/government/"]),
    );
    for (const [path, page] of [
      ["/segments/itad/", SegmentItad],
      ["/segments/enterprise-it/", SegmentEnterpriseIt],
      ["/segments/government/", SegmentGovernment],
    ] as const) {
      const links = parse(await render(page, path)).querySelectorAll("main a");
      expect(links[links.length - 1].getAttribute("href"), path).toBe("/early-access/");
    }
  });

  it("marks the current page in the nav", async () => {
    const pr = parse(await render(Pricing, "/pricing/"));
    expect(pr.querySelector('header nav a[aria-current="page"]')?.getAttribute("href")).toBe("/pricing/");
  });

  it("marks the section current on a segment subpage", async () => {
    const sub = parse(await render(SegmentItad, "/segments/itad/"));
    const current = sub.querySelectorAll('header nav a[aria-current="page"]').map((a) => a.getAttribute("href"));
    expect(current).toEqual(["/segments/"]);
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
