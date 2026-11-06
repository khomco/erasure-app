---
title: 'Product, standards, segments, pricing, contact and legal pages'
type: 'feature'
created: '2026-11-06'
status: 'done'
baseline_revision: '102ecbedacdb6d0f64374b6fc259282be65c34f2'
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

**Problem:** `sites/www` has a landing page, `/capabilities/` and an `/early-access/` stub. Buyers by segment and auditors have no page that speaks to them, there is no pricing, contact, privacy or terms page, and the early-access CTA leads to a page with no form (SPEC CAP-2).

**Approach:** Add product, standards, segment (index + ITAD, enterprise IT, government), pricing, contact, privacy and terms pages on the existing `Base` layout, and replace the `/early-access/` stub with a static sign-up form that posts nowhere. Wire the new pages into the header nav and footer. Every guarded claim is wrapped in `<Capability>`. Where a page needs a claim the capability list lacks, add a new non-`available` capability entry, so every pre-launch claim carries a visible badge (CAP-3).

## Boundaries & Constraints

**Always:**
- New pages: `/product/`, `/standards/`, `/segments/`, `/segments/itad/`, `/segments/enterprise-it/`, `/segments/government/`, `/pricing/`, `/contact/`, `/privacy/`, `/terms/`; `/early-access/` rewritten. Each has an `h1`, title and description, and is readable with JavaScript disabled (no `<script>`, no islands).
- Copy says what the product is *designed* to do (ADR-0006 §2), draws on CONTEXT §1–§4, §9, §13, and never names competitors. Title, description and OG text carry no guarded phrase.
- Standards page: NIST SP 800-88 Rev. 2 and IEEE 2883-2022 posture (conformance self-attestation, wrapped in `nist-ieee-conformance`), R2v3 alignment, and a "Certification roadmap" section whose heading says it is future. Each certification target (ADISA AL5, Common Criteria EAL 2, R2v3/e-Stewards recognition, NATO NIAPC) is its own `<Capability>` with a Planned badge and its target year from CONTEXT §13.
- Pricing page states "Early-access pricing on request", shows no amount or currency, and links to `/early-access/`. It may present pricing principles only as wrapped capabilities.
- Early-access and contact forms share one component. Field names match `lead-capture` CAP-1: early access has `name`, `email`, `company`, `role`, `segment` (`itad`, `enterprise-it`, `government`, `other`), `message` (optional), `consent`; contact has `name`, `email`, `company`, `message`, `consent`. Every control has a visible `<label>`. The form has no `action`, its submit button is `disabled`, and visible text says sign-up is not open yet and nothing is sent. In development builds (`import.meta.env.DEV`) an extra notice says the form posts nowhere until `lead-capture` adds `POST /api/leads`.
- Privacy and terms are marked as placeholders to be published before launch. Privacy states only what is true of the site today: no cookies, no analytics, no third-party resources, forms send nothing.
- New capability entries are `in-development` or `planned`, follow the file's rules (assertion-shaped patterns, each matching the entry's own label or summary), and appear on `/capabilities/` automatically. Expected: `fleet-coordination` (mDNS discovery and lead election), `air-gap-operation`, `verification-portal` (customer-facing, planned), `itam-integration` (planned), `licence-never-blocks-erasure` (ROADMAP §0.2).

**Never:**
- No change to existing capability statuses or patterns, or to `src/lib/site-check.ts` rules.
- No form `action`, `fetch`, `mailto:` or third-party form service; no email address, phone or postal address (brand and domain are undecided).
- No prices, tier names presented as offered products, testimonials, customer logos, or hardware-sanitization/certification claim as current fact.
- No refurbisher segment page (SPEC names three segments), no docs/blog pages, no analytics.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Every page passes the guard | Each new page and `/early-access/` rendered | `checkPage` returns `[]`, no `<script` | Test and build fail naming the page |
| Form posts nowhere | Rendered `/early-access/` and `/contact/` | One `<form>` without `action`; its only submit button is `disabled`; text "not open yet" | Test fails |
| Field contract | Rendered forms | Field `name`s exactly the lists above; every input/select/textarea has a matching `<label for>` | Test fails |
| Dev notice | Form component rendered with dev on / off | Notice naming `lead-capture` present / absent | Test fails |
| Pricing honesty | Rendered `/pricing/` | Contains "Early-access pricing on request"; no `[$€£]\s?\d` or ISO currency amount | Test fails |
| Certification roadmap | Rendered `/standards/` | Roadmap section heading contains "future"; contains `adisa-al5`, `common-criteria-eal2`, `r2v3-recognition`, `nato-niapc`, each `data-status="planned"` | Test fails |
| Findable | Rendered `Base` | Header nav links `/product/`, `/standards/`, `/segments/`, `/pricing/`, `/capabilities/`; footer links `/contact/`, `/privacy/`, `/terms/`; `/segments/` links the three segment pages | Test fails |

</intent-contract>

## Code Map

- `sites/www/src/layouts/Base.astro` -- shell; `nav` array (one entry today) and footer. Extend nav (keep `aria-current` logic, allow wrapping on narrow screens) and add footer links. Props `title`, `description` only.
- `sites/www/src/components/Capability.astro` -- `id`, `as`, `badge: start|end`, `class`, slot overrides label. Card pattern: `<Capability id as="li" badge="start" class={item}><h3 class={h3}>…</h3><p class={body}>…</p></Capability>` (see `src/pages/index.astro` for the `grid`/`item`/`h3`/`body` class constants and tone).
- `sites/www/src/data/capabilities.ts` -- append new entries only; reuse the `GAP` helper for gap patterns. Pattern traps for copy: `certifi(ed|cation)`, `webhooks`, `every attempt`, `Ed25519`, `NIST Clear/Purge`, `wipe/erase/sanitize … drives`, `shred`, `chain of custody`, `JSON-LD`, `HPA`/`DCO`, `per-success`, `failures … free`, `R2v3`, `e-Stewards`, `ADISA`, `AL5`, `Common Criteria`, `EAL 2`, `NIAPC`, `conformance/compliance … NIST/IEEE/2883` (both orders), `self-attest`, `Secure Boot`, `mobile … erasure`, `Validate step`.
- `sites/www/src/pages/early-access.astro` -- stub to replace with copy + form.
- `sites/www/test/pages.test.ts` -- container render (`render(page, path)`), `pages` table runs the site check and no-script test over each page; add every new page there plus the matrix tests. A component can be rendered the same way with `container.renderToString(Component, { props })`.
- `sites/www/test/site-check.test.ts:299` -- per-capability "guarded phrase matches own label or summary" test and `:307` the whole list rendered labelled; both cover new entries automatically.
- `.scratch/lead-capture/SPEC.md` CAP-1 -- the field contract the form mirrors.
- `CONTEXT.md` §2 (segments), §4 and §13 (standards, roadmap), §9 (pricing posture), §10.2 marketing-site row; `docs/ROADMAP.md:185` W1.3 (strike); `README.md:49` layout row and `:172` test row; `sites/www/README.md` Pages list -- docs to update.

## Tasks & Acceptance

**Execution:**
- `sites/www/src/data/capabilities.ts` -- append the new entries named in Boundaries -- every claim the new pages make gets a badge.
- `sites/www/src/components/LeadForm.astro` -- props `kind: "early-access" | "contact"`, `devNotice = import.meta.env.DEV`; renders the fields, consent checkbox (text says details are used only to follow up on early access and links `/privacy/`), disabled submit, "not open yet" text, dev-only notice -- one form, two pages, ready for `lead-capture` to wire.
- `sites/www/src/pages/early-access.astro` -- what early access means (simulated drives), the form -- CTA target.
- `sites/www/src/pages/contact.astro` -- contact copy + contact form; no address.
- `sites/www/src/pages/product.astro` -- how it is designed to work: bench to job to certificate to verification, the destroy path, fleet and integration, licensing that never blocks erasure; all claims wrapped.
- `sites/www/src/pages/standards.astro` -- per Boundaries.
- `sites/www/src/pages/segments/index.astro`, `segments/itad.astro`, `segments/enterprise-it.astro`, `segments/government.astro` -- per CONTEXT §2; each segment page ends with a secondary link to `/early-access/`.
- `sites/www/src/pages/pricing.astro`, `privacy.astro`, `terms.astro` -- per Boundaries.
- `sites/www/src/layouts/Base.astro` -- nav and footer links.
- `sites/www/test/pages.test.ts` -- add every new page to `pages`; add the matrix tests.
- `CONTEXT.md` §10.2, `docs/ROADMAP.md` W1.3, `README.md`, `sites/www/README.md` -- docs in the same commit.

**Acceptance Criteria:**
- Given `pnpm --filter www build`, when it finishes, then it exits 0 with zero site-check violations, and `dist/` contains every page above and `sitemap-0.xml` lists them.
- Given `pnpm --filter www test` and `pnpm --filter www typecheck`, when run, then both pass.
- Given `pnpm --filter www lighthouse` with local Chrome, when run, then performance and accessibility are ≥ 90 on every page.
- Given `pnpm --filter www dev`, when `/early-access/` is opened, then the dev notice is shown; given the production build, when `dist/early-access/index.html` is read, then it has no dev notice but still says sign-up is not open yet.

## Spec Change Log

## Review Triage Log

### 2026-11-06 — Review pass
- verdicts: 30 findings — high 0, medium 9, low 18, false 3, maybe-false 0
- findings:
  - `[medium]` `[patch]` Blind: segment `<select>` pre-selects "ITAD operator", so `required` never fires and a skipped segment would be stored as `itad` once `lead-capture` wires the form — empty disabled "Choose one" first option added; test updated.
  - `[low]` `[reject]` Blind: no hidden field carrying the consent wording — `lead-capture` CAP-1 defines the field list and stores the consent text server-side; a client-sent wording field is new contract surface that story owns.
  - `[low]` `[reject]` Blind: privacy page's "no analytics/no cookies" is not tied to `site.analytics.enabled` — turning analytics on needs a vendor and consent decision (ADR-0006 "Not decided here") that rewrites the privacy page anyway; the fix adds a config branch for a change nobody makes in everyday use.
  - `[medium]` `[patch]` Blind: enterprise-IT "A bench anyone on the team can run" card is an unbadged pre-launch claim — new `in-development` bench-workflow capability added and the card wrapped in it, as the spec's "every claim wrapped" rule requires.
  - `[low]` `[patch]` Blind: "Several benches, one view" claims fleet visibility beyond `fleet-coordination` — heading retitled to discovery/lead election only.
  - `[medium]` `[patch]` Blind: government heading "Destruction with a second person" implies a two-person rule, contradicting ADR-0011's optional concurrence — heading retitled.
  - `[false]` `[reject]` Blind: R2v3/e-Stewards recognition shown as "Year 2" but CONTEXT §13 says customer-side — CONTEXT §13 lists "Year 2 | R2v3 / e-Stewards vendor recognition"; "Customer-side" is §4's row for operator certifications.
  - `[low]` `[patch]` Blind: "makes no pre-launch claim without a badge" test counts `available` capabilities and accepts a nested capability's badge — test deleted; `checkPage` in the same suite already enforces badges with nearest-capability semantics.
  - `[medium]` `[patch]` Blind: present-tense claims ("a licence credit is spent", "still runs every erasure", "stations find each other", "anyone checks a certificate", "every attempt composes") break ADR-0006 §2's designed-to rule — rephrased as designed-to copy, still inside each capability.
  - `[low]` `[reject]` Blind: `r2v3-alignment` badged twice (heading and paragraph) and the roadmap heading carries a badge — both badges are accurate labels; removing them means rewording headings around guard patterns, which the site README forbids.
  - `[medium]` `[patch]` Blind: production default of the dev notice, sitemap and Lighthouse not tested — same group as the VerificationGap row; sitemap (13 URLs) and Lighthouse (100/100 on 13 pages) are verification commands, run and recorded.
  - `[low]` `[patch]` Blind: ROADMAP W1.3 note says W-R5 shipped though its tier→feature matrix is unbuilt — note reworded: W-R5 only partly met, matrix waits for `offerings-and-payments`.
  - `[low]` `[reject]` Blind: mixed British/American spelling — the repo's own source (CONTEXT, capabilities.ts) already mixes "licence"/"defense"/"programme"; no reader harm, and choosing a convention is a vendor copy decision.
  - `[low]` `[patch]` Blind: `licence-never-blocks-erasure` pattern misses "doesn't/won't stop" phrasings — widened (same group as the Edge pattern rows).
  - `[medium]` `[patch]` Edge: segment select has no empty placeholder — same group as the first Blind row.
  - `[low]` `[reject]` Edge: privacy page vs analytics flag — same as the Blind privacy row.
  - `[low]` `[reject]` Edge: `r2v3-alignment` (in-development) can own bare "R2v3", so a recognition claim wrapped in it would get the wrong badge — needs an author to deliberately mislabel a recognition claim (same class as story 1's rejected wide-wrapper row); narrowing the pattern forces alignment copy under the planned recognition badge, mislabelling it the other way.
  - `[low]` `[patch]` Edge: new pattern misses contractions, curly apostrophes and -ing — widened.
  - `[low]` `[patch]` Edge: `air-gap-operation`/`verification-portal` miss "airgapped", "air-gaps", "portals" — widened.
  - `[low]` `[patch]` Edge: badge test fails on `available` capabilities — same group as the Blind badge-test row (test deleted).
  - `[low]` `[patch]` Edge: badge test accepts a nested badge — same group (test deleted).
  - `[low]` `[patch]` Edge: no nav item is current on segment subpages — prefix match for non-root hrefs; test added.
  - `[low]` `[patch]` Edge: pricing test misses bare "50 per drive" and spelled currencies — assertions widened.
  - `[medium]` `[patch]` VerificationGap: LeadForm's `import.meta.env.DEV` default is never exercised; a wrong default would ship the developer notice on public pages — test renders both pages with DEV stubbed off and on.
  - `[medium]` `[patch]` Intent: dev/production notice checked only through an explicit prop — same group as the VerificationGap row.
  - `[false]` `[reject]` Intent: build, sitemap and Lighthouse have no test — the spec verifies them as commands, not tests; they were run (13 pages, 0 violations, 13 sitemap URLs, Lighthouse 100/100).
  - `[medium]` `[patch]` Intent: tests prove "wrapped implies badged", not "claim implies wrapped"; the enterprise-IT bench card is unwrapped — same group as the Blind bench-card row.
  - `[low]` `[reject]` Intent: `r2v3-alignment` changes how the R2v3 guard behaves — same as the Edge `r2v3-alignment` row.
  - `[low]` `[patch]` Intent: pricing test reads only `main`, not title/description — same group as the Edge pricing row; now checks the whole document.
  - `[false]` `[reject]` Intent: the government page's procurement targets are untested — `checkPage` runs over that page in `describe.each(pages)` and fails on a missing, stale or hidden badge, so the targets are tested for their labels.

## Design Notes

"Posts nowhere" is implemented as a form with no `action` and a disabled submit button. A form with no `action` but an enabled button would submit to the page URL and put the visitor's email in a query string or a POST to a static host. With the default button disabled, implicit submission (Enter in a field) does nothing either. `lead-capture` later adds `action`/`method` and enables the button.

New capability entries are a vendor sign-off like the statuses; this story's `done_checkpoint` is that sign-off. Example entry shape:

```ts
{ id: "air-gap-operation", label: "Full operation air-gapped", status: "in-development",
  summary: "Designed so an air-gapped station keeps full product value with no connection to the vendor ever.",
  guardedPhrases: [/\bair[- ]gap(ped)?\b/i] },
```

## Verification

**Commands:**
- `pnpm --filter www build` -- expected: exit 0, 0 violations, 13 pages.
- `pnpm --filter www test` and `pnpm --filter www typecheck` -- expected: pass.
- `pnpm --filter www lighthouse` -- expected: all scores ≥ 90.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: unchanged, pass.

## Auto Run Result

Status: done

**Summary:** Ten new static pages on the `Base` layout: `/product/`, `/standards/`, `/segments/` with `itad`, `enterprise-it` and `government`, `/pricing/` ("Early-access pricing on request", no amounts), `/contact/`, `/privacy/` and `/terms/` (placeholders). `/early-access/` is rewritten around a shared `LeadForm` that uses the `lead-capture` CAP-1 field names. The form posts nowhere: it has no `action`, its submit button is disabled, it says "not open yet", and development builds add a notice. The header nav and footer link every page, and the nav marks segment subpages as current. Every claim sits in a `<Capability>` with a visible badge and is written as designed-to copy. Seven non-`available` capability entries were appended. Existing statuses, patterns and site-check rules are unchanged.

**Files changed:**
- `sites/www/src/components/LeadForm.astro`: one form for early access and contact; posts nowhere.
- `sites/www/src/data/capabilities.ts`: appended `r2v3-alignment`, `fleet-coordination`, `air-gap-operation`, `licence-never-blocks-erasure` and `operator-bench-workflow` (in development), and `verification-portal` and `itam-integration` (planned).
- `sites/www/src/layouts/Base.astro`: header nav (Product, Standards, Who it is for, Pricing, Capabilities) with prefix-based `aria-current`, and footer links (early access, contact, privacy, terms).
- `sites/www/src/lib/ui.ts`: shared class strings.
- `sites/www/src/pages/{product,standards,pricing,contact,privacy,terms,early-access}.astro` and `src/pages/segments/{index,itad,enterprise-it,government}.astro`: the pages.
- `sites/www/test/pages.test.ts`: every page goes through the site check, the no-script check, and checks for one h1, a title and a description. Further tests cover the form contract (no action, disabled submit, field names, labels, segment placeholder), the dev notice with DEV stubbed on and off, the whole-document pricing check, the standards roadmap labelled future and planned, and findability including `aria-current` on segment subpages.
- `CONTEXT.md` §10.2, `docs/ROADMAP.md` (W1.3 struck; W-R4 met, W-R5 partly), `README.md`, `sites/www/README.md`: docs.

**Review findings:** 30 findings (medium 9, low 18, false 3), in 11 patch groups, all applied:
- segment placeholder option;
- bench-workflow card wrapped in a new capability;
- fleet heading retitled;
- destruction heading retitled (ADR-0011);
- present-tense copy rewritten as designed-to;
- redundant badge test deleted;
- dev-notice default tested;
- ROADMAP W-R5 wording;
- new guard patterns widened;
- nav `aria-current` on subpages;
- pricing test widened.

0 deferred. Rejected, with reasons in the triage log:
- client-side consent-wording field (owned by `lead-capture`);
- privacy text tied to the analytics flag (needs a vendor/consent decision first);
- double R2v3 badge;
- mixed spelling;
- `r2v3-alignment` able to own bare "R2v3" (needs deliberate mislabelling; narrowing it mislabels the other way);
- government targets untested (false: `checkPage` covers them).

The other 2 `false` rows are refuted in the log.

**Follow-up review recommended: true.** Patched by entry verdict: medium 5, low 6. The unverified risk is that the patch pass rewrote copy on five pages and added an eighth capability entry (`operator-bench-workflow`). That rewritten copy, and the two capability entries the spec did not name (`r2v3-alignment`, `operator-bench-workflow`), have had no honesty review beyond the automated guard.

**Verification:**
- `pnpm --filter www build`: exit 0, 13 pages, 0 violations; `sitemap-0.xml` lists 13 URLs; built `/early-access/` and `/contact/` contain no dev notice and still say "not open yet".
- `pnpm --filter www test`: 180/180.
- `pnpm --filter www typecheck`: 0 errors.
- `pnpm --filter www lighthouse`: performance and accessibility 100 on all 13 pages.
- `apps/desktop`: typecheck OK, 200/200 tests.

**Residual risks:**
- All copy and the seven new capability entries await vendor sign-off (done checkpoint).
- The forms only become real when `lead-capture` adds `action`/`method` and enables the button.
- Privacy and terms are placeholders.
- Brand and origin are placeholders.
- No CI runs the build (ROADMAP §0.5).

