import { describe, expect, it } from "vitest";
import { capabilities, getCapability, STATUS_LABEL, type CapabilityDef, type CapabilityStatus } from "../src/data/capabilities";
import { checkCss, checkPage } from "../src/lib/site-check";

const ORIGIN = "https://wipestation.example";

const fixtureCaps: CapabilityDef[] = [
  {
    id: "nvme-sanitize",
    label: "Honest NVMe Sanitize",
    status: "in-development",
    summary: "Designed to sanitize NVMe drives.",
    guardedPhrases: [/\bNVMe Sanitize\b/i, /\bsaniti[sz]e[sd]?\b[^.]{0,60}\b(drives?|NVMe|SSDs?|HDDs?|media)\b/i, /\bNIST Purge\b/i],
  },
  {
    id: "adisa-al5",
    label: "ADISA Assurance Level 5",
    status: "planned",
    summary: "Not ADISA certified today.",
    guardedPhrases: [/\bADISA\b/i, /\bcertified\b/i],
  },
  {
    id: "cc-eal2",
    label: "Common Criteria EAL 2",
    status: "planned",
    summary: "Not certified today.",
    guardedPhrases: [/\bCommon Criteria\b/i, /\bcertified\b/i],
  },
  {
    id: "rest-api",
    label: "REST API",
    status: "available",
    summary: "An HTTP API.",
    guardedPhrases: [/\bREST API\b/i],
  },
  {
    id: "partner-programme",
    label: "Certified partner programme",
    status: "available",
    summary: "A certified partner programme.",
    guardedPhrases: [/\bcertified\b/i],
  },
];

const HEAD = `
  <meta charset="utf-8">
  <title>Fixture | Wipestation</title>
  <meta name="description" content="A fixture page.">
  <link rel="canonical" href="${ORIGIN}/fixture/">
  <meta property="og:title" content="Fixture | Wipestation">
  <meta property="og:description" content="A fixture page.">
  <meta property="og:url" content="${ORIGIN}/fixture/">`;

const page = (body: string, head = HEAD) => `<!doctype html><html lang="en"><head>${head}</head><body>${body}</body></html>`;
const check = (html: string) => checkPage(html, "/fixture/index.html", fixtureCaps, ORIGIN);

const label = (status: string) => STATUS_LABEL[status as CapabilityStatus] ?? status;
const labelled = (id: string, status: string, inner: string, badge = `<span data-status-badge="${status}">${label(status)}</span>`) =>
  `<span data-capability="${id}" data-status="${status}">${inner}${badge}</span>`;

describe("checkPage", () => {
  it("passes a labelled claim", () => {
    const html = page(`<p>${labelled("nvme-sanitize", "in-development", "Sanitizes NVMe drives with NIST Purge")}</p>`);
    expect(check(html)).toEqual([]);
  });

  it("flags a bare overclaim, naming page, capability and matched text", () => {
    const v = check(page(`<p>Sanitizes NVMe drives with NIST Purge</p>`));
    expect(v.length).toBeGreaterThan(0);
    expect(v.every((x) => x.rule === "overclaim" && x.page === "/fixture/index.html")).toBe(true);
    expect(v.map((x) => x.capability)).toContain("nvme-sanitize");
    expect(v.map((x) => x.match)).toContain("Sanitizes NVMe drives");
  });

  it("reads a claim split by inline markup as one sentence", () => {
    const v = check(page(`<p>Sani<b></b>tizes <strong>NVMe</strong> drives</p>`));
    expect(v.some((x) => x.capability === "nvme-sanitize")).toBe(true);
  });

  it("flags a claim whose verb sits outside a nested capability element", () => {
    const v = check(page(`<p>Sanitizes ${labelled("adisa-al5", "planned", "ADISA")} drives</p>`));
    expect(v.some((x) => x.rule === "overclaim" && x.capability === "nvme-sanitize")).toBe(true);
  });

  it("flags another capability's phrase inside a capability element", () => {
    const v = check(page(`<p>${labelled("adisa-al5", "planned", "ADISA, and we sanitize SSDs")}</p>`));
    expect(v.some((x) => x.rule === "overclaim" && x.capability === "nvme-sanitize")).toBe(true);
  });

  it("lets an enclosing capability own a phrase it shares with others", () => {
    expect(check(page(`<p>${labelled("adisa-al5", "planned", "ADISA certified: not yet")}</p>`))).toEqual([]);
  });

  it.each([
    ["<title>", HEAD.replace("<title>Fixture | Wipestation</title>", "<title>ADISA certified erasure</title>")],
    ["meta description", HEAD.replace('name="description" content="A fixture page."', 'name="description" content="Sanitizes NVMe drives."')],
    ["og:description", HEAD.replace('property="og:description" content="A fixture page."', 'property="og:description" content="NIST Purge for every drive"')],
    ["other og:*", HEAD + '<meta property="og:image:alt" content="Common Criteria badge">'],
  ])("flags an overclaim in metadata (%s)", (_name, head) => {
    const v = check(page("<p>Hello</p>", head));
    expect(v.some((x) => x.rule === "overclaim")).toBe(true);
  });

  it("flags an overclaim in alt text", () => {
    expect(check(page(`<img src="/a.png" alt="ADISA seal">`)).some((x) => x.rule === "overclaim")).toBe(true);
  });

  it("ignores script and style text", () => {
    expect(check(page(`<style>.adisa{}</style><script>const s = "ADISA certified";</script>`))).toEqual([]);
  });

  it("flags an unknown capability id in rendered HTML", () => {
    const v = check(page(labelled("nope", "planned", "x")));
    expect(v.some((x) => x.rule === "unknown-capability" && x.capability === "nope")).toBe(true);
  });

  it("flags a stale status", () => {
    const v = check(page(labelled("adisa-al5", "available", "ADISA", "")));
    expect(v.some((x) => x.rule === "stale-status" && x.capability === "adisa-al5")).toBe(true);
  });

  it.each([
    ["missing", ""],
    ["hidden attribute", `<span data-status-badge="planned" hidden>Planned</span>`],
    ["sr-only", `<span data-status-badge="planned" class="x sr-only">Planned</span>`],
    ["aria-hidden", `<span data-status-badge="planned" aria-hidden="true">Planned</span>`],
    ["display:none", `<span data-status-badge="planned" style="display: none">Planned</span>`],
    ["empty", `<span data-status-badge="planned"> </span>`],
    ["wrong text", `<span data-status-badge="planned">Available now</span>`],
    ["invisible", `<span data-status-badge="planned" class="invisible">Planned</span>`],
    ["md:hidden", `<span data-status-badge="planned" class="md:hidden">Planned</span>`],
    ["max-sm:sr-only", `<span data-status-badge="planned" class="max-sm:sr-only">Planned</span>`],
    ["opacity-0", `<span data-status-badge="planned" class="opacity-0">Planned</span>`],
    ["visibility:hidden", `<span data-status-badge="planned" style="visibility:hidden">Planned</span>`],
    ["opacity:0", `<span data-status-badge="planned" style="opacity:0">Planned</span>`],
    ["opacity:0.0", `<span data-status-badge="planned" style="opacity: 0.0">Planned</span>`],
    ["opacity:.0", `<span data-status-badge="planned" style="opacity:.0">Planned</span>`],
    ["wrong status", `<span data-status-badge="in-development">In development</span>`],
  ])("flags a hidden badge (%s)", (_name, badge) => {
    const v = check(page(labelled("adisa-al5", "planned", "ADISA", badge)));
    expect(v.some((x) => x.rule === "hidden-badge" && x.capability === "adisa-al5")).toBe(true);
  });

  it("accepts a badge with partial opacity", () => {
    const badge = `<span data-status-badge="planned" style="opacity:0.5">Planned</span>`;
    expect(check(page(labelled("adisa-al5", "planned", "ADISA", badge)))).toEqual([]);
  });

  it("scans a fake badge outside any capability as ordinary text", () => {
    const v = check(page(`<p><span data-status-badge="planned">ADISA certified</span></p>`));
    expect(v.some((x) => x.rule === "overclaim" && x.capability === "adisa-al5")).toBe(true);
  });

  it("scans a badge whose value is not the enclosing capability's status", () => {
    const fake = `<span data-status-badge="in-development">we sanitize SSDs</span>`;
    const v = check(page(labelled("adisa-al5", "planned", `ADISA${fake}`)));
    expect(v.some((x) => x.rule === "overclaim" && x.capability === "nvme-sanitize")).toBe(true);
  });

  it("does not let an available capability own a shared phrase", () => {
    const v = check(page(`<p><span data-capability="partner-programme" data-status="available">certified partner</span></p>`));
    expect(v.some((x) => x.rule === "overclaim" && x.capability === "adisa-al5")).toBe(true);
  });

  it.each([
    ["soft hyphen", "<p>Sani&shy;tizes NVMe drives</p>"],
    ["zero-width space", "<p>ADI&#8203;SA</p>"],
    ["word joiner", "<p>ADI\u2060SA</p>"],
  ])("strips invisible characters before scanning (%s)", (_name, body) => {
    expect(check(page(body)).some((x) => x.rule === "overclaim")).toBe(true);
  });

  it("strips invisible characters in metadata", () => {
    const v = check(page("<p>Hi</p>", HEAD.replace("<title>Fixture | Wipestation</title>", "<title>ADI&#8203;SA</title>")));
    expect(v.some((x) => x.rule === "overclaim" && x.capability === "adisa-al5")).toBe(true);
  });

  it.each([
    ["aria-label", `<a href="/" aria-label="ADISA seal">x</a>`],
    ["aria-description", `<a href="/" aria-description="ADISA seal">x</a>`],
    ["title", `<abbr title="ADISA seal">x</abbr>`],
    ["placeholder", `<input placeholder="ADISA seal">`],
    ["value", `<input type="submit" value="ADISA certified">`],
  ])("scans read-aloud attributes (%s)", (_name, body) => {
    expect(check(page(body)).some((x) => x.rule === "overclaim")).toBe(true);
  });

  it("does not take an inline svg title for the page title", () => {
    const head = HEAD.replace("<title>Fixture | Wipestation</title>", "");
    const v = check(page(`<svg><title>Logo</title></svg>`, head));
    expect(v.filter((x) => x.rule === "missing-meta").map((x) => x.message).join()).toMatch(/<title>/);
  });

  it("flags a badge hidden by an ancestor of the capability element", () => {
    const v = check(page(`<div class="hidden">${labelled("adisa-al5", "planned", "ADISA")}</div>`));
    expect(v.some((x) => x.rule === "hidden-badge")).toBe(true);
  });

  it("does not accept a nested capability's badge as the outer one's", () => {
    const inner = labelled("cc-eal2", "planned", "Common Criteria");
    const v = check(page(`<div data-capability="adisa-al5" data-status="planned">ADISA ${inner}</div>`));
    expect(v.some((x) => x.rule === "hidden-badge" && x.capability === "adisa-al5")).toBe(true);
  });

  it.each([
    ["<title>", /<title>.*<\/title>/],
    ["description", /<meta name="description"[^>]*>/],
    ["canonical", /<link rel="canonical"[^>]*>/],
    ["og:title", /<meta property="og:title"[^>]*>/],
    ["og:description", /<meta property="og:description"[^>]*>/],
    ["og:url", /<meta property="og:url"[^>]*>/],
  ])("flags missing meta (%s)", (_name, re) => {
    const v = check(page("<p>Hello</p>", HEAD.replace(re, "")));
    expect(v.filter((x) => x.rule === "missing-meta")).toHaveLength(1);
  });

  it.each([
    ['<script src="https://www.googletagmanager.com/gtag/js"></script>'],
    ['<script src="//cdn.example.net/a.js"></script>'],
    ['<link rel="preconnect" href="https://fonts.gstatic.com">'],
    ['<link rel="dns-prefetch" href="//tracker.example.org">'],
    ['<link rel="stylesheet" href="https://fonts.googleapis.com/css2">'],
    ['<img src="https://t.example/p.gif" alt="">'],
    ['<img src="/a.png" srcset="/a.png 1x, https://cdn.example.net/a2.png 2x" alt="">'],
    ['<iframe src="https://www.youtube.com/embed/x" title="v"></iframe>'],
    ['<video src="/v.mp4" poster="https://cdn.example.net/p.jpg"></video>'],
    ['<audio src="https://cdn.example.net/a.mp3"></audio>'],
    ['<picture><source srcset="https://cdn.example.net/a.webp"><img src="/a.png" alt=""></picture>'],
    ['<embed src="https://cdn.example.net/a.swf">'],
    ['<object data="https://cdn.example.net/a.pdf"></object>'],
    ['<link rel="icon" href="https://cdn.example.net/favicon.ico">'],
    ['<link rel="manifest" href="https://cdn.example.net/site.webmanifest">'],
    ['<link rel="apple-touch-icon" href="https://cdn.example.net/icon.png">'],
    [`<div style="background:url('https://t.example/p.gif')">x</div>`],
    ['<style>@import "https://fonts.googleapis.com/css2";</style>'],
  ])("flags a third-party fetch: %s", (tag) => {
    expect(check(page(tag)).some((x) => x.rule === "third-party")).toBe(true);
  });

  it("allows same-origin and relative scripts and links", () => {
    const html = page(
      `<script src="/_astro/a.js"></script><script src="${ORIGIN}/b.js"></script><link rel="stylesheet" href="/_astro/c.css"><a href="https://elsewhere.example.com/">a link is not a fetch</a>`,
    );
    expect(check(html)).toEqual([]);
  });

  it("allows an available capability's guarded phrase bare", () => {
    expect(check(page(`<p>Use the REST API.</p>`, HEAD.replace("A fixture page.", "Use the REST API.")))).toEqual([]);
  });
});

describe("checkCss", () => {
  it("flags foreign url() and @import", () => {
    const css = `@import "https://fonts.googleapis.com/a.css"; @import url(//cdn.example.net/b.css); .a{background:url("https://t.example/p.gif")}`;
    expect(checkCss(css, "/_astro/a.css", ORIGIN).map((v) => v.match).sort()).toEqual(
      ["https://fonts.googleapis.com/a.css", "//cdn.example.net/b.css", "https://t.example/p.gif"].sort(),
    );
    expect(checkCss(css, "/_astro/a.css", ORIGIN).every((v) => v.rule === "third-party" && v.page === "/_astro/a.css")).toBe(true);
  });

  it("allows relative, same-origin and data: references", () => {
    const css = `@import "./b.css"; .a{background:url(/img/a.png)} .b{background:url('${ORIGIN}/c.png')} .c{background:url(data:image/png;base64,AAAA)}`;
    expect(checkCss(css, "/_astro/a.css", ORIGIN)).toEqual([]);
  });
});

describe("real capability list against copy", () => {
  const real = (body: string) => checkPage(page(body), "/real", capabilities, ORIGIN);
  const flagged = (body: string) => real(body).filter((v) => v.rule === "overclaim").map((v) => v.capability);

  it.each([
    ["Erases 2.5in drives", "drive-sanitization"],
    ["e.g. wipes SSDs", "drive-sanitization"],
    ["Certified destruction with chain of custody", "destruction-attestation"],
    ["Shredding on site", "destruction-attestation"],
    ["R2v3 aligned", "r2v3-recognition"],
    ["Independent certifications held", "independent-certification"],
    ["NIST SP 800-88 Rev. 2 compliant", "nist-ieee-conformance"],
  ])("flags bare %j", (text, id) => {
    expect(flagged(`<p>${text}</p>`)).toContain(id);
  });

  it("lets generic drive-sanitization copy wear the generic badge", () => {
    const inner = `Erases HDDs to NIST Purge<span data-status-badge="in-development">In development</span>`;
    expect(real(`<p><span data-capability="drive-sanitization" data-status="in-development">${inner}</span></p>`)).toEqual([]);
  });

  it("does not treat 'certificate' as a certification claim", () => {
    expect(flagged("<p>Read the certificate.</p>")).toEqual([]);
  });
});

describe("capability list", () => {
  it("has unique ids and nothing available pre-launch", () => {
    expect(new Set(capabilities.map((c) => c.id)).size).toBe(capabilities.length);
    expect(capabilities.filter((c) => c.status === "available")).toEqual([]);
  });

  it.each(capabilities.map((c) => [c.id, c] as const))("every guarded phrase of %s matches its own label or summary", (_id, cap) => {
    expect(cap.guardedPhrases.length).toBeGreaterThan(0);
    for (const phrase of cap.guardedPhrases) {
      expect(phrase.test(cap.label) || phrase.test(cap.summary), `${phrase} on ${cap.id}`).toBe(true);
    }
  });

  it("each capability's own copy passes the check when wrapped", () => {
    const body = capabilities
      .map((c) => `<li data-capability="${c.id}" data-status="${c.status}"><span data-status-badge="${c.status}">${STATUS_LABEL[c.status]}</span><h3>${c.label}</h3><p>${c.summary}</p></li>`)
      .join("");
    expect(checkPage(page(`<ul>${body}</ul>`), "/list", capabilities, ORIGIN)).toEqual([]);
  });

  it("getCapability returns known ids and throws on unknown ones", () => {
    expect(getCapability("nvme-sanitize").status).toBe("in-development");
    expect(() => getCapability("nope")).toThrow(/nope/);
  });
});
