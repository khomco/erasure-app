# sites/www — marketing site

Static [Astro](https://astro.build) + Tailwind 4 site (ADR-0006). No server runtime, zero
client JS by default, readable with JavaScript disabled; only a build given `PUBLIC_HUB_ORIGIN`
ships the lead-form script on the two form pages. Pre-launch: brand "Wipestation" and origin
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
  with the field names of the hub's `POST /api/leads` (lead-capture story 1). No email, phone or
  postal address is published. The form has two modes, chosen at build time:
  - **Closed** (default, `PUBLIC_HUB_ORIGIN` unset): it posts nowhere. No `action`, a `disabled`
    submit button (an enabled button would submit to the page URL), "not open yet" text and no
    script, so the site still ships zero JavaScript.
  - **Open** (`PUBLIC_HUB_ORIGIN=http://localhost:7900`, the hub's origin): the form carries
    `data-hub-origin`; its button is disabled in the HTML and enabled by a small script
    (`src/lib/lead-submit.ts`) that fetches the hub's CSRF token and posts the form with
    `credentials: "include"`, retrying once on a stale token, and shows the outcome in an
    `aria-live` region (success, which never claims an email was sent; the hub's 400 message; a
    429's own message plus "Try again in N minutes" from its `Retry-After`; or a network error). A `<noscript>` note says nothing is sent without
    JavaScript, and an off-screen, `aria-hidden`, `tabindex=-1` honeypot field `website` catches
    naive bots. The page copy and meta no longer say the form is closed. The hub must list this
    site's origin in `--site-origin` (default `http://localhost:4321`, Astro's dev port), and the
    two must be the same site (`localhost:4321` with `localhost:7900` works; `localhost` with
    `127.0.0.1` does not, because the CSRF cookie is `SameSite=Lax`).
  - `astro build` (not `astro dev`) fails when `PUBLIC_HUB_ORIGIN` is set (in the environment or
    a `.env` file) while `site.privacyNoticePublished` in `src/config.ts` is `false`: the
    placeholder privacy page promises the forms will not open before the full notice is
    published. Setting that flag is a vendor sign-off that ships with the real notice.
  - The consent wording lives once, in `src/data/consent.ts`. The label is
    `"<statement> See the privacy notice."`, and that whole text is what the hub stores as
    consent; the hub accepts only its own copy, pinned to this file by a Rust unit test, so
    change both together.

  Development builds (`import.meta.env.DEV`) add a notice saying where the form posts, or how to
  open it.
- `/privacy/`, `/terms/`: placeholders, to be published before launch. Privacy states only what
  is true of a default build: no cookies, no analytics, no third-party resources, forms send
  nothing.

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
