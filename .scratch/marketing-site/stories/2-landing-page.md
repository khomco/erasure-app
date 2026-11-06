---
title: 'Landing page'
type: 'feature'
created: '2026-11-06'
status: 'done'
baseline_revision: '5663b1159b901895e4e348b1660ff2bcc2423079'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '{project-root}/docs/adr/0006-web-presence-and-local-first-verification.md'
  - '{project-root}/.scratch/marketing-site/SPEC.md'
  - '{project-root}/sites/www/README.md'
warnings: [oversized]
deferred: []
---

<intent-contract>

## Intent

**Problem:** `sites/www` has only a placeholder home (story 1). A visitor cannot learn the problem, the offer, or why it is different, and there is no early-access call to action (CAP-1).

**Approach:** Replace `src/pages/index.astro` with a landing page built on the existing `Base` layout. It has a hero, the problem, the offline-verifiable certificate, the differentiators, "what we are not", and one primary early-access call to action. Copy comes from CONTEXT §1–§3. Every guarded claim is wrapped in `<Capability>`, so the build-time check passes and every claim shows its status badge.

## Boundaries & Constraints

**Always:**
- Sections, in order: hero (offer + primary CTA), problem (CONTEXT §1 weaknesses as market problems), the offline-verifiable certificate (`offline-verification`, `command-level-evidence`, `open-cert-format`), differentiators (a short selection of CONTEXT §3 items, each a `<Capability>`), "what we are not" (CONTEXT §3), and a closing early-access block. Keep it short enough to read in about one scroll on desktop. Each section is a `<section aria-labelledby>` with an `h2`.
- Exactly one primary CTA element (`data-cta="primary"`), linking to `/early-access/`. Other links to that page (for example the closing block) are styled as secondary links.
- Readable with JavaScript disabled; no `<script>` and no client islands.
- Copy describes what the product is *designed* to do. A guarded phrase is wrapped in its capability, never reworded to dodge the pattern, and the pattern is never weakened (sites/www/README.md). A visible line near the top says the product is pre-launch and every pre-release build runs against simulated drives.
- Title, description and OG text contain no guarded phrase (metadata can carry no badge).
- Competitors are not named in copy. CONTEXT's "not a Blancco clone" becomes "not per-event billing with a new logo". Naming competitors is a vendor/legal decision, not ours.

**Never:**
- No change to `src/data/capabilities.ts` statuses or patterns, or to `src/lib/site-check.ts` rules.
- No product/standards/segments/pricing/contact/legal pages and no sign-up form (story 3). `/early-access/` is a stub page only, with no form and no address, which story 3 replaces.
- No prices, no hardware-sanitization or certification claim as current fact, no testimonials or customer logos.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Landing renders | `/` built | Four content sections plus hero and closing block, one `[data-cta=primary]` → `/early-access/` | Build passes |
| Guard over landing | Rendered `/` passed to `checkPage` | Zero violations | Test fails on any violation |
| Bare claim slips in | Unwrapped guarded phrase added to landing copy | Build and test fail naming `/` | Wrap in `<Capability>` |
| No JS | Rendered `/` | No `<script` in HTML | Test fails |
| CTA target exists | Built `dist/` | `early-access/index.html` exists, and the guard passes it | Build passes |

</intent-contract>

## Code Map

- `sites/www/src/pages/index.astro` -- placeholder home to replace.
- `sites/www/src/layouts/Base.astro` -- shell with `title`/`description` props, nav (only `/capabilities/`), and the footer pre-launch notice. Reuse it unchanged.
- `sites/www/src/components/Capability.astro` -- `id`, `as`, `badge: start|end`, `class`, and a slot that overrides the label. Wrap prose like `<Capability id="drive-sanitization">…</Capability>`; for a list item use `as="li"`.
- `sites/www/src/data/capabilities.ts` -- read-only. Ids available: `command-level-evidence`, `outcome-bearing-job`, `offline-verification`, `erp-integration`, `per-success-licensing`, `drive-sanitization`, `nvme-sanitize`, `hpa-dco-clear`, `destruction-attestation`, `open-cert-format`, `independent-certification`, `adisa-al5`, `common-criteria-eal2`, …. Pattern traps: "certified/certification", "webhooks", "every attempt", "Ed25519", "NIST Clear/Purge", "wipe/erase … drives", "shred", "chain of custody", "JSON-LD", "HPA"/"DCO", "per-success", "failures … free".
- `sites/www/src/lib/site-check.ts:332` -- `checkPage(html, path, capabilities, siteOrigin): Violation[]`, reused in the new test.
- `sites/www/src/pages/capabilities.astro` -- reference for wrapping style and tone.
- Container rendering is verified to work: `vitest.config.ts` with `getViteConfig({ test: {} })` from `astro/config`, then `experimental_AstroContainer.create({ astroConfig: { site: "https://wipestation.example" } })` and `renderToString(Page, { partial: false, request: new Request(origin + path) })`. Without `astroConfig.site`, `Base` throws on `new URL(…, Astro.site)`. The existing 110 tests still pass under that config.
- `CONTEXT.md` §10.2 marketing-site row, `docs/ROADMAP.md:180` W1.2, `README.md:49` and `:172`, `sites/www/README.md` -- docs to update.

## Tasks & Acceptance

**Execution:**
- `sites/www/src/pages/index.astro` -- write the landing page per Boundaries; metadata like "Wipestation: sanitization proof you can check yourself (pre-launch)" -- CAP-1.
- `sites/www/src/pages/early-access.astro` -- stub: what early access means, that sign-up is not open yet and nothing is collected here, and a link back to `/capabilities/` -- so the CTA has a real target until story 3.
- `sites/www/vitest.config.ts` -- `getViteConfig` so `.astro` imports compile in tests.
- `sites/www/test/pages.test.ts` -- render `/` and `/early-access/` with the container. For each page, assert `checkPage` returns `[]` and there is no `<script`. For `/`, assert exactly one `[data-cta="primary"]` with href `/early-access/`, the section headings in order, and at least one `[data-capability]` per claim section -- ACs below.
- `CONTEXT.md` §10.2, `docs/ROADMAP.md` W1.2 (strike, noting the copy awaits vendor sign-off), `README.md` layout and test-count rows, `sites/www/README.md` -- docs in the same commit.

**Acceptance Criteria:**
- Given `pnpm --filter www build`, when it finishes, then it exits 0 with zero site-check violations, and `dist/index.html` and `dist/early-access/index.html` exist.
- Given the rendered landing page, when read with JavaScript disabled, then the problem, the offline-verifiable certificate, the differentiators, "what we are not" and one primary early-access CTA are all present, and every guarded claim shows a visible status badge.
- Given `pnpm --filter www test` and `pnpm --filter www typecheck`, when run, then both pass.
- Given `pnpm --filter www lighthouse` with local Chrome, when run, then performance and accessibility are ≥ 90 on every page.

## Spec Change Log

## Review Triage Log

### 2026-11-06 — Review pass
- verdicts: 25 findings — high 0, medium 6, low 14, false 5, maybe-false 0
- findings:
  - `[medium]` `[patch]` Blind: offline-verification claim reworded in hero and meta description, escaping the guard unbadged — hero clause wrapped in `<Capability id="offline-verification">`; description now says "pre-launch … designed around certificates…".
  - `[medium]` `[patch]` Blind: "We remove per-event billing" is an unbadged present-tense per-success-licensing claim — rephrased as "designed to replace per-event billing with per-success licensing", wrapped in `per-success-licensing`.
  - `[low]` `[reject]` Blind: "Join early access" leads to a page where you cannot yet join — the intent names this CTA "join early access" (SPEC CAP-1), and story 3 makes the target joinable before anything is published (vendor sign-off checkpoint).
  - `[low]` `[patch]` Blind: problem section generalises CONTEXT §1 claims about particular/many products to the whole market — qualified as "most / usually / some tools / some shops / many tools".
  - `[low]` `[patch]` Blind: `/capabilities/` not in the rendered-page tests — added to `pages` in `test/pages.test.ts`.
  - `[low]` `[reject]` Blind: stub `/early-access/` is in the sitemap and indexable — the site is not published before story 3 replaces the stub; a `noindex`/filter would be a temporary branch.
  - `[false]` `[reject]` Blind: ACs not run or recorded — build, test, typecheck and Lighthouse were run at verification and are recorded under Auto Run Result; the triage log is written by this step.
  - `[false]` `[reject]` Blind: claim-section test weaker than the story requires — the spec asks exactly for at least one `[data-capability]` per claim section, which the test asserts; deleted claims are not an overclaim.
  - `[low]` `[reject]` Blind: each page rendered several times per test run — whole file runs in ~50 ms; no harm to users or developers.
  - `[low]` `[reject]` Blind: Code Map says the existing suite was 110 tests (actual 109) — the fix edits this spec.
  - `[low]` `[patch]` Blind: "ADISA AL5 and Common Criteria EAL 2 as the certification floor" reads as if held — now "target certification floor".
  - `[low]` `[patch]` Edge: `/capabilities/` missing from page tests — same group as the Blind row; fixed there.
  - `[low]` `[reject]` Edge: stub indexable via sitemap — same as the Blind sitemap row.
  - `[medium]` `[patch]` Edge (claim): page runs several screens, not "about one scroll" — certificate and differentiator cards laid out as responsive 1/2/3-column grids, spacing tightened, one sentence per card body.
  - `[medium]` `[patch]` Edge (claim): present-tense facts ("Every attempt … composes", "A licence credit is spent", "REST in and out, webhooks") contradict ADR-0006 §2 "may not say it does it" — each rephrased as designed-to/built-to; same group as the per-event-billing row.
  - `[medium]` `[patch]` Edge (claim): metadata/hero reword offline-verification — same group as the first Blind row.
  - `[low]` `[reject]` VerificationGap: "ships no JavaScript" checks container output, so a same-origin script injected by config (e.g. `prefetchAll`) passes test and build — reaching it needs a deliberate config change, and a build-wide no-script rule would contradict story 1's analytics hook (self-hosted script allowed when enabled), so the fix is a new rule with an exception branch.
  - `[medium]` `[patch]` Intent: "within one scroll" untested and unmet — same group as the Edge one-scroll row.
  - `[false]` `[reject]` Intent: a second primary-styled link without `data-cta` would pass — no such link exists; the closing link is styled as a text link.
  - `[low]` `[reject]` Intent: no-JS is a string check on container output — same as the VerificationGap row.
  - `[false]` `[reject]` Intent: no evidence build/Lighthouse were run — they were run at verification (recorded below).
  - `[false]` `[reject]` Intent: differentiator selection not checked — the intent says "differentiators", not a fixed set; presence is checked.
  - `[low]` `[reject]` Intent: no working sign-up (conversion) — the intent's CTA target form belongs to CAP-2 / story 3 per SPEC.
  - `[low]` `[reject]` Intent: vendor copy approval not settled — the `done_checkpoint` on this story is the caller's sign-off gate, not this build's.
  - `[low]` `[patch]` Intent: problem-section market claims unguarded — same group as the Blind problem-section row.

## Design Notes

The CTA target is the open question between stories 2 and 3. A link to a missing page would ship a 404, and building the form here would take over story 3. A stub `/early-access/` keeps each story shippable on its own. Story 3 replaces the stub's body with the form that posts nowhere.

Wrap example: `<p>Pricing built so <Capability id="per-success-licensing">a licence credit is spent only on a verified success; failures and re-runs are free</Capability>.</p>`

## Verification

**Commands:**
- `pnpm --filter www build` -- expected: exit 0, 0 violations, 3 pages.
- `pnpm --filter www test` and `pnpm --filter www typecheck` -- expected: pass.
- `pnpm --filter www lighthouse` -- expected: all scores ≥ 90.

## Auto Run Result

Status: done

**Summary:** `sites/www/src/pages/index.astro` is now the landing page. It has a pre-launch notice, a hero with one primary CTA (`data-cta="primary"`, to `/early-access/`), the problem, "A certificate anyone can check", "Why it is different", "What we are not" (competitors unnamed) and a closing early-access block. Every claim sits in a `<Capability>` with a visible badge and is worded as designed-to copy. `/early-access/` is a stub with no form, so the CTA has a real target until story 3. A container-rendered page test runs the site check over `/`, `/capabilities/` and `/early-access/`.

**Files changed:**
- `sites/www/src/pages/index.astro` — landing page.
- `sites/www/src/pages/early-access.astro` — CTA target stub; collects nothing.
- `sites/www/vitest.config.ts` — `getViteConfig` so tests can render `.astro` pages.
- `sites/www/test/pages.test.ts` — the 3 pages: site check, no `<script>`; landing CTA, section order, labelled claims, pre-launch notice.
- `CONTEXT.md` §10.2, `docs/ROADMAP.md` W1.2, `README.md`, `sites/www/README.md` — docs.

**Review findings:** 25 findings (medium 6, low 14, false 5). 6 patch groups applied: offline-verification rewording (hero + meta), present-tense → designed-to copy (incl. per-event billing wrap), qualified market claims, `/capabilities/` added to page tests, "target certification floor", grid layout for about one scroll. 0 deferred. Rejected with reasons in the triage log: CTA label, stub indexable, repeated renders, spec test-count typo, container-only no-JS check, conversion/vendor-approval scope; 5 `false` rows refuted.

**Follow-up review recommended: true.** Patched by verdict: medium 3 groups, low 3. Unverified risk: the patch pass rewrote most body copy and the layout (grids, smaller text), and that rewritten copy has not had its own honesty or accessibility review beyond the automated guard and Lighthouse.

**Verification:** `pnpm --filter www build` exit 0, 3 pages, 0 violations, `dist/index.html` and `dist/early-access/index.html` present; `pnpm --filter www test` 119/119; `pnpm --filter www typecheck` 0 errors; `pnpm --filter www lighthouse` performance 100 / accessibility 100 on all 3 pages; an implementer-side probe with a bare "erases drives" line failed both test and build (removed).

**Residual risks:** copy awaits vendor sign-off (done checkpoint); "one scroll" is judged by eye, not tested; a config-injected same-origin script would not be caught by test or build; the problem section's market statements are unguarded prose by design of the checker.
