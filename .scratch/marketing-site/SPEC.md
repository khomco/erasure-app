---
id: SPEC-marketing-site
companions: [../../docs/adr/0006-web-presence-and-local-first-verification.md, ../../CONTEXT.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# A pre-launch site that builds an early-access list, honestly

## Why

An opportunity to capture: the vendor wants to market Wipestation before launch, drive traffic, and collect interested parties. The positioning in CONTEXT §1–§3 is strong (offline-verifiable certificates, command-level evidence, per-success pricing, "what we are not"). It must be told without claiming capabilities the simulated product does not yet have (ROADMAP §0.1).

## Capabilities

- **CAP-1**
  - **intent:** A visitor understands the problem, the offer and why it is different within one scroll.
  - **success:** Landing page with problem, offline-verifiable certificate, differentiators, "what we are not", and one primary call to action (join early access); renders with no JavaScript required.
- **CAP-2**
  - **intent:** Buyers by segment and auditors find the pages that speak to them.
  - **success:** Pages for product, standards posture (NIST 800-88 Rev. 2 / IEEE 2883 / R2v3 alignment with the certification roadmap labelled as future), segments (ITAD, enterprise IT, government), pricing ("early-access pricing on request" until `offerings-and-payments` wires published prices), contact, privacy and terms placeholders. The early-access call to action links to a sign-up form whose backend arrives in `lead-capture`; until then it is a static form that posts nowhere and says so in development builds.
- **CAP-3**
  - **intent:** No page overclaims.
  - **success:** A shared capability list gives every capability a status (`available`, `in-development`, `planned`); a site test fails if copy presents a non-`available` capability as available (frontmatter/component check), and every pre-launch claim is visibly labelled.
- **CAP-4**
  - **intent:** The site is fast, accessible and findable.
  - **success:** Static build; Lighthouse-style checks run locally (performance, accessibility ≥ 90); sitemap, meta/OG tags, no third-party trackers by default (analytics hook off).

## Constraints

- Astro at `sites/www`, Tailwind; no server runtime of its own.
- Create the root `pnpm-workspace.yaml` (listing `apps/*` and `sites/*`) if it does not exist yet; `apps/desktop` must keep building and testing.
- Copy draws on CONTEXT §1–§3 but never states hardware sanitization or certification as current fact.
- Vendor signs off copy before anything is published (done checkpoint).

## Non-goals

- Hosting, domain, analytics vendor, blog, documentation pages (later).
- Storing leads (`lead-capture`) and live prices (`offerings-and-payments`).

## Success signal

- `pnpm --filter www dev` shows the full site locally, every claim is labelled, and the overclaim test passes.

## Open Questions

- Product name, domain and brand assets for the published site (placeholder "Wipestation" until decided).
