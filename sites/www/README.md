# sites/www — marketing site

Static [Astro](https://astro.build) + Tailwind 4 site (ADR-0006). No server runtime, zero
client JS, readable with JavaScript disabled. Pre-launch: brand "Wipestation" and origin
`https://wipestation.example` are placeholders; set `SITE_URL` to override the origin.

```bash
pnpm install                    # at the repo root (pnpm workspace, per-package lockfiles)
pnpm --filter www dev           # local dev server
pnpm --filter www build         # static build to dist/, runs the site check
pnpm --filter www typecheck     # astro check
pnpm --filter www test          # vitest, no network, no browser
pnpm --filter www lighthouse    # build + preview + Lighthouse in local headless Chrome; fails < 90
```

## Pages

- `/` (`src/pages/index.astro`): the landing page. Hero with the pre-launch notice and the one
  primary call to action (`data-cta="primary"`, to `/early-access/`), then the problem, the
  offline-verifiable certificate, differentiators, "what we are not", and a closing early-access
  block that uses a secondary link. Competitors are not named in copy, and the copy awaits
  vendor sign-off.
- `/capabilities/`: every capability with its status.
- `/product/`: how the product is designed to work, from bench to job to certificate to
  verification, the destroy path, fleet and integration, and licensing.
- `/standards/`: NIST SP 800-88 Rev. 2 / IEEE 2883-2022 posture (self-attestation target), R2v3
  alignment, and a "Certification roadmap: future targets" section with one planned capability
  per target and its target year (CONTEXT §13).
- `/segments/` and `/segments/itad/`, `/segments/enterprise-it/`, `/segments/government/`: the
  three buyers in CONTEXT §2; each segment page ends with a secondary link to `/early-access/`.
- `/pricing/`: "Early-access pricing on request", no amounts or currency; licensing principles
  only as wrapped capabilities. Prices arrive with `offerings-and-payments`.
- `/early-access/` and `/contact/`: copy plus `src/components/LeadForm.astro`, one form for both
  with the `lead-capture` CAP-1 field names. It posts nowhere: no `action` and a `disabled`
  submit button (an enabled button would submit to the page URL), and it says sign-up is not
  open yet. Development builds (`import.meta.env.DEV`) add a notice that it posts nowhere until
  `lead-capture` adds `POST /api/leads`; that story adds `action`/`method` and enables the
  button. No email, phone or postal address is published.
- `/privacy/`, `/terms/`: placeholders, to be published before launch. Privacy states only what
  is true today: no cookies, no analytics, no third-party resources, forms send nothing.

The header nav links product, standards, segments, pricing and capabilities; the footer links
early access, contact, privacy and terms. Shared class strings live in `src/lib/ui.ts`.

`test/pages.test.ts` renders real pages with the Astro container (`vitest.config.ts` uses
`getViteConfig`, so `.astro` files compile in tests) and runs `checkPage` over them. A bare
overclaim therefore fails `pnpm test` as well as `astro build`. To test a new page, add it there.

## Honest claims are enforced, not remembered

- `src/data/capabilities.ts` is the only list of capabilities the site may mention, each with
  a status: `available`, `in-development` or `planned`. **Statuses are a vendor sign-off
  decision.** Nothing is `available` while the only device backend is the simulator.
- `<Capability id="…">` (`src/components/Capability.astro`) renders a capability with
  `data-capability`/`data-status` and a visible status badge. An unknown id fails the build.
- `src/lib/site-check.ts` runs on every built page (via `src/integrations/site-check.ts` in
  `astro:build:done`) and fails the build when:
  - a guarded phrase of a non-`available` capability appears outside that capability's
    element. This covers body text, `alt`, `aria-label`, `aria-description`, `title`,
    `placeholder` and `value` attributes, the head's `<title>`, the meta description and
    `og:*`/`twitter:*` tags. Soft hyphens and zero-width characters are stripped first. Only
    the enclosing capability's own badge is exempt from scanning, and an `available`
    capability cannot "own" a phrase it shares with a non-`available` one;
  - a capability element has an unknown id, a stale `data-status`, or no visible badge. A
    visible badge carries the capability's status and its exact label text ("In development",
    "Planned"), and is not hidden by `hidden`, `aria-hidden`, `sr-only`, `invisible`,
    `opacity-0` (with any `md:`-style variant), `display:none`, `visibility:hidden` or
    `opacity:0`;
  - `<title>`, description, canonical, `og:title`, `og:description` or `og:url` is missing
    (rule `missing-meta`), or `sitemap-index.xml` was not emitted (`missing-artifact`);
  - anything is fetched from another origin: `script`, `img`/`source` `src`/`srcset`, `iframe`,
    `video`/`audio`/`track`/`embed` `src` and `poster`, `object[data]`, `link` preconnect,
    dns-prefetch, preload, prefetch, stylesheet, icon, manifest or apple-touch-icon, `url()` in
    `style` attributes and `<style>` blocks, and `url()`/`@import` in every emitted `.css` file.

Generic copy has generic capabilities to wrap in: `drive-sanitization` for "erases/wipes/sanitizes
drives" and NIST Clear/Purge, and `independent-certification` for "certified"/"certification"
("certificate" is not guarded).

When the check flags copy, wrap it in `<Capability>` so it gets a badge. Do not reword the
copy to dodge a pattern, and do not weaken the pattern. There is no attribute that silences
the check.

## Analytics

`src/config.ts` → `analytics.enabled` is `false`, and `<Analytics />` renders nothing. No
vendor or consent banner has been chosen (ADR-0006, "Not decided here").
