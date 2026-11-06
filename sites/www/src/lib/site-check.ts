/**
 * Build-time honest-claims and page-hygiene check (ADR-0006 §2, CAP-3, CAP-4).
 *
 * Runs over *rendered* HTML — the surface a visitor sees — so it catches a
 * claim however it got onto the page (component, markdown, layout, metadata).
 * Pure: no filesystem, no network; `src/integrations/site-check.ts` feeds it
 * every emitted page and fails `astro build` on any violation.
 *
 * There is deliberately no attribute or comment that silences it.
 */
import { parse, NodeType, type HTMLElement, type Node } from "node-html-parser";
import { STATUS_LABEL, type CapabilityDef } from "../data/capabilities";

export type ViolationRule =
  | "overclaim"
  | "unknown-capability"
  | "stale-status"
  | "hidden-badge"
  | "missing-meta"
  | "missing-artifact"
  | "third-party";

export interface Violation {
  page: string;
  rule: ViolationRule;
  capability?: string;
  match?: string;
  message: string;
}

/** Elements whose text never reaches the reader. */
const SKIPPED_TAGS = new Set(["script", "style", "template"]);
/** Inline elements: no word break is inserted at their boundaries. */
const INLINE_TAGS = new Set([
  "a", "abbr", "b", "bdi", "bdo", "cite", "code", "data", "dfn", "em", "i", "kbd", "mark",
  "q", "s", "samp", "small", "span", "strong", "sub", "sup", "time", "u", "var",
]);
/** Attributes whose values are read to a visitor (alt text, labels, tooltips). */
const READ_ATTRIBUTES = ["alt", "aria-label", "aria-description", "title", "placeholder", "value"];
/** `<link rel>` values that make the browser contact a host. */
const FETCHING_LINK_RELS = new Set([
  "preconnect", "dns-prefetch", "preload", "modulepreload", "prefetch", "stylesheet",
  "icon", "manifest", "apple-touch-icon", "mask-icon",
]);
/** Element attributes that make the browser fetch a resource. */
const FETCHING_ATTRIBUTES: [selector: string, attrs: string[]][] = [
  ["script", ["src"]],
  ["img", ["src", "srcset"]],
  ["input", ["src"]],
  ["iframe", ["src"]],
  ["frame", ["src"]],
  ["video", ["src", "poster"]],
  ["audio", ["src"]],
  ["source", ["src", "srcset"]],
  ["track", ["src"]],
  ["embed", ["src"]],
  ["object", ["data"]],
];
/** Characters that render as nothing but split a word for a regex. */
const INVISIBLE_CHARS = /[\u00AD\u200B-\u200D\u2060\uFEFF]/g;
/** Tailwind/utility classes that hide an element, after any `md:`-style variant prefix. */
const HIDING_CLASSES = new Set(["hidden", "sr-only", "invisible", "opacity-0"]);

function globalCopy(re: RegExp): RegExp {
  return new RegExp(re.source, re.flags.includes("g") ? re.flags : re.flags + "g");
}

function anyMatch(re: RegExp, text: string): boolean {
  return new RegExp(re.source, re.flags.replace("g", "")).test(text);
}

/**
 * Text grouped by the set of `[data-capability]` elements enclosing it. Text
 * in the same context is concatenated in document order, so a claim split by
 * a nested capability element ("Sanitizes <Capability/> drives") is still
 * read as one sentence by its outer context.
 */
interface TextContext {
  enclosing: string[];
  text: string;
}

function collectText(root: HTMLElement, capabilities: readonly CapabilityDef[]): TextContext[] {
  const contexts = new Map<string, TextContext>();
  const append = (enclosing: string[], text: string) => {
    const key = enclosing.join("\u0000");
    let ctx = contexts.get(key);
    if (!ctx) {
      ctx = { enclosing, text: "" };
      contexts.set(key, ctx);
    }
    ctx.text += text;
  };
  const walk = (node: Node, enclosing: string[]) => {
    for (const child of node.childNodes) {
      if (child.nodeType === NodeType.TEXT_NODE) {
        append(enclosing, child.text);
        continue;
      }
      if (child.nodeType !== NodeType.ELEMENT_NODE) continue;
      const el = child as HTMLElement;
      const tag = el.rawTagName?.toLowerCase() ?? "";
      if (SKIPPED_TAGS.has(tag)) continue;
      // The enclosing capability's own status badge is its label, not a claim;
      // it still separates words. Any other "badge" is scanned like any text.
      const badge = el.getAttribute("data-status-badge");
      const nearest = enclosing.length > 0 ? capabilities.find((c) => c.id === enclosing[enclosing.length - 1]) : undefined;
      if (badge !== undefined && nearest && badge === nearest.status) {
        append(enclosing, " ");
        continue;
      }
      const id = el.getAttribute("data-capability");
      const inner = id !== undefined ? [...enclosing, id] : enclosing;
      const brk = INLINE_TAGS.has(tag) ? "" : " ";
      append(enclosing, brk);
      for (const attr of READ_ATTRIBUTES) {
        const v = el.getAttribute(attr);
        if (v) append(inner, ` ${v} `);
      }
      walk(el, inner);
      append(enclosing, brk);
    }
  };
  walk(root, []);
  return [...contexts.values()].map((c) => ({ ...c, text: c.text.replace(/\s+/g, " ") }));
}

function scanForOverclaims(
  text: string,
  enclosing: string[],
  where: string,
  page: string,
  capabilities: readonly CapabilityDef[],
  out: Violation[],
) {
  text = text.replace(INVISIBLE_CHARS, "");
  // Only a non-`available` enclosing capability can own shared words: an
  // `available` one would otherwise launder a planned claim with no badge.
  const owners = capabilities.filter((c) => enclosing.includes(c.id) && c.status !== "available");
  for (const cap of capabilities) {
    if (cap.status === "available" || enclosing.includes(cap.id)) continue;
    for (const phrase of cap.guardedPhrases) {
      for (const m of text.matchAll(globalCopy(phrase))) {
        const matched = m[0];
        // An enclosing capability that guards the same words owns them
        // (e.g. "certified" inside the ADISA element is ADISA's claim).
        if (owners.some((o) => o.guardedPhrases.some((p) => anyMatch(p, matched)))) continue;
        out.push({
          page,
          rule: "overclaim",
          capability: cap.id,
          match: matched,
          message: `${page}: "${matched}" ${where} claims "${cap.label}" (${cap.status}) without its status label — wrap it in <Capability id="${cap.id}">`,
        });
      }
    }
  }
}

function isHiddenElement(el: HTMLElement): boolean {
  if (el.hasAttribute("hidden")) return true;
  if (el.getAttribute("aria-hidden") === "true") return true;
  const classes = (el.getAttribute("class") ?? "").split(/\s+/);
  // `md:hidden`, `max-sm:sr-only`, `!hidden`: compare the utility after the last variant.
  if (classes.some((c) => HIDING_CLASSES.has(c.split(":").pop()!.replace(/^!|!$/g, "")))) return true;
  const style = (el.getAttribute("style") ?? "").replace(/\s+/g, "").toLowerCase();
  return /display:none|visibility:hidden|opacity:(?:0+(?:\.0*)?|\.0+)(?![\d.])/.test(style);
}

function nearestCapabilityAncestor(el: HTMLElement): HTMLElement | null {
  let p = el.parentNode as HTMLElement | null;
  while (p) {
    if (p.getAttribute?.("data-capability") !== undefined) return p;
    p = p.parentNode as HTMLElement | null;
  }
  return null;
}

function checkCapabilityElements(
  root: HTMLElement,
  page: string,
  capabilities: readonly CapabilityDef[],
  out: Violation[],
) {
  for (const el of root.querySelectorAll("[data-capability]")) {
    const id = el.getAttribute("data-capability") ?? "";
    const cap = capabilities.find((c) => c.id === id);
    if (!cap) {
      out.push({ page, rule: "unknown-capability", capability: id, message: `${page}: unknown capability id "${id}"` });
      continue;
    }
    const declared = el.getAttribute("data-status");
    if (declared !== cap.status) {
      out.push({
        page,
        rule: "stale-status",
        capability: id,
        message: `${page}: "${id}" is rendered as "${declared ?? "(none)"}" but the capability list says "${cap.status}"`,
      });
    }
    if (cap.status === "available") continue;
    const badges = el
      .querySelectorAll("[data-status-badge]")
      .filter((b) => nearestCapabilityAncestor(b) === el);
    const visible = badges.filter((b) => {
      if (b.getAttribute("data-status-badge") !== cap.status) return false;
      if (b.text.trim() !== STATUS_LABEL[cap.status]) return false;
      for (let n: HTMLElement | null = b; n; n = n.parentNode as HTMLElement | null) {
        if (isHiddenElement(n)) return false;
        if (n === el) break;
      }
      return true;
    });
    // The element itself, or anything above it, being hidden hides the badge too.
    let hiddenAbove = false;
    for (let n = el.parentNode as HTMLElement | null; n && n.getAttribute; n = n.parentNode as HTMLElement | null) {
      if (isHiddenElement(n)) hiddenAbove = true;
    }
    if (visible.length === 0 || hiddenAbove) {
      out.push({
        page,
        rule: "hidden-badge",
        capability: id,
        message: `${page}: "${id}" is ${cap.status} but has no visible "${cap.status}" status badge`,
      });
    }
  }
}

function checkMetadata(
  root: HTMLElement,
  page: string,
  capabilities: readonly CapabilityDef[],
  out: Violation[],
) {
  // The head's title only: an inline `<svg><title>` is not the page title.
  const title = root.querySelector("head")?.querySelector("title")?.text.trim() ?? "";
  const meta = (selector: string) => root.querySelector(selector)?.getAttribute("content")?.trim() ?? "";
  const required: [string, string][] = [
    ["<title>", title],
    ['<meta name="description">', meta('meta[name="description"]')],
    ['<link rel="canonical">', root.querySelector('link[rel="canonical"]')?.getAttribute("href")?.trim() ?? ""],
    ['<meta property="og:title">', meta('meta[property="og:title"]')],
    ['<meta property="og:description">', meta('meta[property="og:description"]')],
    ['<meta property="og:url">', meta('meta[property="og:url"]')],
  ];
  for (const [name, value] of required) {
    if (!value) out.push({ page, rule: "missing-meta", message: `${page}: missing or empty ${name}` });
  }
  // Metadata can carry no badge, so every guarded phrase is scanned with no
  // enclosing capability.
  const metaText = [
    title,
    ...root
      .querySelectorAll("meta")
      .filter((m) => {
        const key = (m.getAttribute("name") ?? m.getAttribute("property") ?? "").toLowerCase();
        return key === "description" || key.startsWith("og:") || key.startsWith("twitter:");
      })
      .map((m) => m.getAttribute("content") ?? ""),
  ];
  for (const text of metaText) scanForOverclaims(text, [], "in page metadata", page, capabilities, out);
}

function foreignOrigin(ref: string | undefined, siteOrigin: string): string | null {
  if (!ref) return null;
  const origin = new URL(siteOrigin).origin;
  let url: URL;
  try {
    url = new URL(ref.trim(), origin);
  } catch {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  return url.origin === origin ? null : url.origin;
}

/** Every `url(...)` and `@import` target in a stylesheet or style attribute. */
function cssReferences(css: string): string[] {
  const refs: string[] = [];
  for (const m of css.matchAll(/url\(\s*(['"]?)([^'")]*)\1\s*\)/gi)) refs.push(m[2]);
  for (const m of css.matchAll(/@import\s+(['"])([^'"]+)\1/gi)) refs.push(m[2]);
  return refs;
}

/**
 * Check a stylesheet (an emitted `.css` file, a `<style>` block or a `style`
 * attribute) for `url()`/`@import` of another origin.
 */
export function checkCss(css: string, file: string, siteOrigin: string): Violation[] {
  const out: Violation[] = [];
  for (const ref of cssReferences(css)) {
    const host = foreignOrigin(ref, siteOrigin);
    if (host) out.push({ page: file, rule: "third-party", match: ref, message: `${file}: stylesheet loads from third-party origin ${host}` });
  }
  return out;
}

function checkThirdParty(root: HTMLElement, page: string, siteOrigin: string, out: Violation[]) {
  const flag = (ref: string, what: string) => {
    const host = foreignOrigin(ref, siteOrigin);
    if (host) out.push({ page, rule: "third-party", match: ref, message: `${page}: ${what} contacts third-party origin ${host}` });
  };
  for (const [tag, attrs] of FETCHING_ATTRIBUTES) {
    for (const el of root.querySelectorAll(tag)) {
      for (const attr of attrs) {
        const v = el.getAttribute(attr);
        if (!v) continue;
        // srcset: comma-separated "url descriptor" candidates.
        const refs = attr === "srcset" ? v.split(",").map((c) => c.trim().split(/\s+/)[0]) : [v];
        for (const ref of refs) flag(ref, `<${tag} ${attr}>`);
      }
    }
  }
  for (const l of root.querySelectorAll("link[rel]")) {
    const rels = (l.getAttribute("rel") ?? "").toLowerCase().split(/\s+/);
    if (!rels.some((r) => FETCHING_LINK_RELS.has(r))) continue;
    flag(l.getAttribute("href") ?? "", `<link rel="${rels.join(" ")}">`);
  }
  for (const el of root.querySelectorAll("[style]")) {
    for (const v of checkCss(el.getAttribute("style") ?? "", page, siteOrigin)) out.push(v);
  }
  for (const el of root.querySelectorAll("style")) {
    for (const v of checkCss(el.text, page, siteOrigin)) out.push(v);
  }
}

/**
 * Check one rendered page. `path` is only used to name the page in messages;
 * `siteOrigin` is the site's own origin — anything else is third-party.
 */
export function checkPage(
  html: string,
  path: string,
  capabilities: readonly CapabilityDef[],
  siteOrigin: string,
): Violation[] {
  const out: Violation[] = [];
  const root = parse(html, { comment: false, blockTextElements: { script: true, style: true } });
  checkMetadata(root, path, capabilities, out);
  checkCapabilityElements(root, path, capabilities, out);
  const body = root.querySelector("body") ?? root;
  for (const ctx of collectText(body, capabilities)) {
    scanForOverclaims(ctx.text, ctx.enclosing, "in page text", path, capabilities, out);
  }
  checkThirdParty(root, path, siteOrigin, out);
  return out;
}
