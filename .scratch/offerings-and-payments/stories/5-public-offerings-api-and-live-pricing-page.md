---
title: 'Public offerings API and live pricing page'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '0964fa0543fbb38fee0bd715e5ef49f1b285781f'
review_loop_iteration: 0
followup_review_recommended: false
context:
  - '{project-root}/docs/adr/0006-web-presence-and-local-first-verification.md'
  - '{project-root}/.scratch/offerings-and-payments/SPEC.md'
  - '{project-root}/sites/www/README.md'
warnings: [oversized]
deferred:
  - summary: >-
      AGENTS.md's marketing-site line reads as if PRICING_HUB_ORIGIN must be exactly http://127.0.0.1:7900; any bare http(s) hub origin works.
    evidence: |-
      The added AGENTS.md sentence says "lists live prices only when built with `PRICING_HUB_ORIGIN=http://127.0.0.1:7900`". parseHubOrigin accepts any bare http(s) origin. The fix edits an agent-context file, so it is deferred by rule.
    location: >-
      AGENTS.md:58
    severity: low
  - summary: >-
      /pricing/ formats prices with Intl's (CLDR) minor-unit digits while the hub's renewal headline uses ISO 4217 digits, so for a few currencies (e.g. IQD, MGA) the card's price and its headline disagree.
    evidence: |-
      sites/www/src/lib/offerings.ts copies the portal's formatPrice (decision 5), which reads Intl maximumFractionDigits; crates/wipe-hub entitlements uses an ISO table. Pre-existing: story 4 deferred the same ISO/CLDR/Stripe minor-unit mismatch for the portal. Common currencies (USD, EUR, GBP, JPY) agree.
    location: >-
      sites/www/src/lib/offerings.ts:fractionDigits
    severity: low
---

<intent-contract>

## Intent

**Problem:** The marketing site's `/pricing/` page says "Early-access pricing on request" and shows no prices, and the hub's `GET /api/offerings` answers only signed-in users (story 2, decision 2), so the site cannot read published prices. CAP-6 and ADR-0006 §1 want the pricing page to show the hub's published offerings, read at build time, with the build failing rather than publishing stale prices.

**Approach:** Make `GET /api/offerings` public: same path, same JSON shape, published offerings only, no session. Give the static site an opt-in build-time variable, `PRICING_HUB_ORIGIN`. When it is set, `/pricing/` fetches the catalog during the build and lists each published offering, and any fetch or shape failure fails the build. When it is unset, the page stays exactly as today.

## Boundaries & Constraints

**Always:**
- `GET /api/offerings` needs no session and ignores any cookie (a stale or invalid session cookie does not turn it into a 401). It answers the same `{offerings: [...]}` body, in the same order (lower-cased name, then id), to anonymous and signed-in callers. Only `status = 'published'` rows appear. It writes no audit row (the customer read never did).
- The site reads `PRICING_HUB_ORIGIN` at build time only, through `import.meta.env` (so `.env` files count). It must be a bare http(s) origin, validated by the same rule as `PUBLIC_HUB_ORIGIN` (`parseHubOrigin`), and the error names the variable. The variable never reaches the browser: it is not `PUBLIC_`-prefixed and is never rendered.
- With `PRICING_HUB_ORIGIN` set, the build fetches `<origin>/api/offerings` once, with a 10 s timeout. A network error, a timeout, a non-2xx status or a body that fails the strict shape check throws. The message names `PRICING_HUB_ORIGIN`, the URL and the cause, and `astro build` exits non-zero.
- The shape check is strict and fails the build on any violation. It requires the top-level `offerings` array. Each offering needs:
  - string `id`, `name` and `description`;
  - `price_minor`, a positive safe integer;
  - `currency` matching `^[A-Z]{3}$`;
  - `interval`, either `one_off` or `annual`;
  - `grants` with `scope` (`machine` or `site`), integer `seats` ≥ 1, `quota_erasures` (null or an integer ≥ 1), a `features` string array and integer `term_months` ≥ 1;
  - `renewal_notice`, null or an object with a string `headline`. It must be non-null exactly when `interval` is `annual`.
- With offerings, each one shows:
  - the name and the description (Astro-escaped text, line breaks kept);
  - the price, formatted like the portal's `formatPrice` (`1,200.00 USD`), followed by "per year" (annual) or "one-off payment";
  - seats ("1 seat" / "N seats"), scope ("Machine licence" / "Site licence"), quota ("Unlimited erasures" / "N erasures", grouped), and the term ("12-month term");
  - for annual offerings only, the hub's `renewal_notice.headline` verbatim.

  The "not on sale yet" block is replaced. The "How licensing is designed to work" section stays unchanged.
- With no offerings (variable unset, or the hub publishes none), the page renders byte-for-byte as today.
- The pricing page with offerings still passes `checkPage`: an overclaim in a vendor-entered name or description fails `astro build`, and it is fixed by editing the offering in the admin area. Name and description stay editable while published.
- The page ships no JavaScript in either mode.

**Never:**
- No CORS on `/api/offerings`. The fetch happens at build time in Node, and AGENTS.md limits `--site-origin` CORS to two routes.
- No client-side fetch of prices, and no prices typed into the site.
- Do not list `grants.features` on the page. See decision 2: the tier→feature matrix is ROADMAP W3.3.
- Do not change `src/data/capabilities.ts` statuses, the consent wording, `privacyNoticePublished`, or the `PUBLIC_HUB_ORIGIN` guard.
- No new marketing prose beyond the functional labels above and the meta description in decision 3.
- Do not change the catalog's JSON shape or the hub's renewal-notice wording.
- No network in `pnpm --filter www test` or `cargo test` beyond the compose services and local listeners.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Anonymous catalog | `GET /api/offerings`, no cookie; hub has published, draft and retired offerings | 200 `{offerings}` with published ones only, in the existing order and shape | — |
| Signed-in catalog | the same request as an owner or a member | body identical to the anonymous one | — |
| Stale cookie | an invalid or expired session cookie | 200, same body | — |
| Default build | `PRICING_HUB_ORIGIN` unset | `/pricing/` identical to today, no fetch made | — |
| Priced build | set; hub publishes an annual and a one-off offering | both listed with name, description, price + interval label, seats, scope, quota and term; headline on the annual one only | — |
| Empty catalog | set; `{offerings: []}` | the default page | — |
| Hub down | set; connection refused | build fails: `pricing: could not read http://127.0.0.1:7900/api/offerings (PRICING_HUB_ORIGIN): …` | throw |
| Hub slow | no answer within 10 s | build fails, naming the timeout | throw (abort) |
| Bad status | 401, 404 or 500 | build fails, naming the status | throw |
| Bad body | not JSON, `offerings` missing, or one offering malformed (e.g. `currency: "usd"`, annual without a notice) | build fails, naming the first bad field and the offering's index | throw |
| Bad variable | `localhost:7900`, `http://h/path` or `ftp://h` | build fails: `PRICING_HUB_ORIGIN "…" must be an http(s) origin…` | throw |
| Overclaim in vendor text | a description saying "wipes NVMe drives" | `checkPage` reports a guarded-phrase violation; `astro build` fails | site check |
| Markup in vendor text | a description containing `<script>alert(1)</script>` | rendered as text; the page has no `<script>` | escape |

## Autonomous decisions

1. **Prices are fetched only when the build is given a hub (`PRICING_HUB_ORIGIN`).**
   - *About:* the vendor building the marketing site.
   - *Problem:* ADR-0006 §1 and CAP-6 say the build fails if the hub is unreachable. If every build needed a hub, `pnpm --filter www build`, the Lighthouse script and every developer build would need Postgres and a running hub. That breaks ADR-0006 §3's local-first rule and the AGENTS.md verification steps.
   - *Options:* (a) always fetch, from a default `http://localhost:7900`; (b) fetch only when `PRICING_HUB_ORIGIN` is set, and otherwise ship today's no-prices page.
   - *Choice:* (b). It follows the lead forms' precedent (`PUBLIC_HUB_ORIGIN`). It is separate from that variable because the privacy-notice gate concerns browsers posting to the hub, and a build-time read sends no visitor data.
   - *Why it does not contradict the ADR:* every price the page shows comes from the hub at build time, and a build asked for prices fails if it cannot read them. The ADR's concern, stale prices, cannot arise, because without the variable no price is published at all.
2. **The page does not list licence features.**
   - *Problem:* the hub's feature names (Enterprise mode, Hub sync, ERP integration, PDF certificates, Certificate anchoring) have no entry in the site's capability list, apart from ERP integration. Listing them would assert features without status badges, and choosing their statuses is a vendor sign-off.
   - *Choice:* omit them. The generated tier→feature matrix is ROADMAP W3.3, which depends on L1. The portal's `/buy` page still shows every grant before purchase.
3. **Copy in the priced state is functional only.**
   - The new copy is the labels in the Always rules and one meta description: "Published prices for Wipestation offerings."
   - The existing "Early-access pricing on request" block shows only when there are no offerings, because "not on sale yet" would be false next to a price list.
   - All of it stays under ADR-0006's rule that the vendor signs off final copy before anything is published.
   - The page has no buy link, because the site does not know the portal's public URL (a residual).
4. **Annual offerings show the first sentence of the owner-approved auto-renewal notice**, taken from the hub's `renewal_notice.headline` so the wording is never retyped (story 4, decision 3). The second sentence is omitted because it names the first year's end, which depends on the purchase date.
5. **Price formatting copies the portal's `formatPrice` algorithm** (Intl fraction digits, string arithmetic, no floats) into `sites/www`. The two apps share no package, and copying it keeps `/pricing/` and `/buy` showing the same price string.

</intent-contract>

## Code Map

- `crates/wipe-hub/src/order_routes.rs:5` (module doc) and `:168` `catalog`: drop the `_user: CurrentUser` extractor; the doc line becomes "anyone, no session: published offerings".
- `crates/wipe-hub/src/orders.rs:1-10` (module doc, the "Catalog" bullet), `:60` `CatalogOffering`, `:263` `catalog`: unchanged logic; the doc says the catalog is public.
- `crates/wipe-hub/tests/orders.rs:147` `the_catalog_lists_published_offerings_to_signed_in_users`: rename it and assert anonymous 200 with the same body as owner and member, plus a bogus `Cookie` → 200.
- `crates/wipe-hub/tests/isolation.rs:124` `("GET", "/api/offerings", Customer)`: becomes `Anonymous("published offerings only; the public price list, no organization's records")`. Check that the sweep logic needs nothing else for an Anonymous GET.
- `crates/wipe-hub/tests/support/hub.rs` provides `published_offering` and `published_one_off_offering`; `Hub::get(path, None)` makes the anonymous call.
- `apps/portal/src/api/orders.ts:214` `fetchCatalog` doc says "(signed in)"; make it "(public)". No behaviour change.
- `apps/portal/src/api/adminOfferings.ts:150-175`: `minorToDecimal` and `formatPrice`, the algorithm to copy.
- `sites/www/src/lib/lead-submit.ts:26` `parseHubOrigin(value)`: add an optional variable-name parameter (default `"PUBLIC_HUB_ORIGIN"`) used in both error messages; existing callers and tests stay unchanged.
- `sites/www/src/config.ts:36` `hubOrigin()`: add `pricingHubOrigin()` beside it.
- `sites/www/src/pages/pricing.astro`: the page to extend.
- `sites/www/src/lib/ui.ts`: shared class strings (`grid`, `item`, `h3`, `body`, …) to reuse.
- `sites/www/src/components/Capability.astro`: unchanged; the page's existing wrapped capabilities stay.
- `sites/www/src/lib/site-check.ts` `checkPage`: run over the priced render in tests.
- `sites/www/test/pages.test.ts:26-80`: container render helper and `pages` table; `:148-165` shows the `vi.stubEnv` pattern (restore in `afterEach`).
- Docs:
  - `CONTEXT.md` §10.2: add a row after the story 4 row (line ~702).
  - `CONTEXT.md` §14 **Offering** (:1086): published offerings are public through `GET /api/offerings` and the pricing page.
  - `README.md` hub section: the route list.
  - `sites/www/README.md`: the `/pricing/` bullet and a `PRICING_HUB_ORIGIN` note.
  - `docs/ROADMAP.md:199-200`: W-R5 note, live prices shipped, feature matrix still W3.3.
  - `AGENTS.md`, the marketing-site bullet: `PRICING_HUB_ORIGIN` makes `build` need a reachable hub.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/src/order_routes.rs`, `src/orders.rs`: make the catalog public as above, with doc updates.
- `crates/wipe-hub/tests/orders.rs`: the catalog test covers the anonymous, owner, member and stale-cookie rows, with identical bodies.
- `crates/wipe-hub/tests/isolation.rs`: reclassify the route as `Anonymous`.
- `apps/portal/src/api/orders.ts`: doc comment only.
- `sites/www/src/lib/lead-submit.ts`: the named-variable `parseHubOrigin`.
- `sites/www/src/config.ts`: `pricingHubOrigin()`.
- `sites/www/src/lib/offerings.ts` (new):
  - `CatalogOffering` type;
  - `parseCatalog(json): CatalogOffering[]`, which throws `"offerings[i].field: …"`;
  - `loadOfferings(origin, {fetchImpl, timeoutMs = 10000})`, which throws the matrix messages;
  - `formatPrice`, `intervalLabel`, `seatsLabel`, `scopeLabel`, `quotaLabel`, `termLabel`.
- `sites/www/src/components/PriceList.astro` (new): renders `offerings` as a list (`<ul>`, one `<li data-offering>` each) with the fields above.
- `sites/www/src/pages/pricing.astro`: `const origin = pricingHubOrigin(); const offerings = origin ? await loadOfferings(origin) : [];`. With offerings, it renders `PriceList` and the decision 3 meta description in place of the no-prices block. Otherwise it renders today's markup unchanged.
- `sites/www/test/offerings.test.ts` (new):
  - `parseCatalog`: a valid catalog, plus each invalid field named in the Always rules;
  - `loadOfferings`: a fake fetch for each matrix row (refused, abort or timeout with a short `timeoutMs`, 401/500, bad JSON) asserting the message names `PRICING_HUB_ORIGIN` and the URL;
  - label and price formatting (USD, JPY, seat and quota plurals and grouping);
  - `parseHubOrigin` with the pricing variable name.
- `sites/www/test/pages.test.ts`: `/pricing/` with `PRICING_HUB_ORIGIN` stubbed and a stubbed global `fetch` that returns an annual and a one-off offering. Assert:
  - it passes `checkPage`, ships no `<script>`, and shows each name, price string, interval label and the annual headline (not on the one-off);
  - an escaped `<script>` description renders as text;
  - a "wipes NVMe drives" description makes `checkPage` report a violation;
  - an empty catalog renders the default copy.
- Docs as in the Code Map.

**Acceptance Criteria:**
- Given compose up, when `cargo test --workspace` runs, then everything passes, including `isolation` with `/api/offerings` classified `Anonymous`.
- Given no `PRICING_HUB_ORIGIN`, when `pnpm --filter www build` runs with no hub running, then it succeeds and `dist/pricing/index.html` still says "Early-access pricing on request".
- Given a local hub with one published annual offering, when `PRICING_HUB_ORIGIN=http://127.0.0.1:7900 pnpm --filter www build` runs, then it succeeds and `dist/pricing/index.html` lists the offering with its price and renewal headline. With the hub stopped, the same command exits non-zero naming `PRICING_HUB_ORIGIN`.
- Given the www and portal packages, when `pnpm --filter www typecheck && pnpm --filter www test && pnpm --filter portal typecheck && pnpm --filter portal test` run, then all pass.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 19 findings — high 0, medium 0, low 15, false 4, maybe-false 0
- findings:
  - `[low]` `[patch]` (Blind) The README and CONTEXT say stale prices are never published, but the static page is a snapshot until the next build. — Reworded to "never publishes prices it could not read"; README and the CONTEXT gaps column now say prices are as of the last build, so rebuild after publishing, editing or retiring an offering.
  - `[low]` `[patch]` (Blind) The CONTEXT orders row says "any signed-in user, public since story 5", and its gaps column still says a session is needed. — The description now says public with no session; the gap now says the catalog was signed-in only until story 5.
  - `[low]` `[patch]` (Blind) With prices shown, the page jumps from `h1` to each offering's `h3` (heading order). — Offering names are now `<h2>`; tests updated; the long doc-comment line is wrapped.
  - `[low]` `[reject]` (Blind) The priced page has no call to action once the early-access block is replaced. — Rejected: adding a link is new customer-visible copy (decision 3 records no buy link as a residual), and the priced page is not published without vendor sign-off.
  - `[low]` `[reject]` (Blind) `PRICING_HUB_ORIGIN` accepts plain http to a remote host, so someone on the network path could alter the published prices. — Rejected: it needs the vendor to configure http to a remote hub and an attacker on that path, and checkout still charges the hub's real price. The fix adds a validation branch the contract does not ask for. Recorded as a residual.
  - `[low]` `[reject]` (Blind) The page shows amounts with no tax note. — Rejected as out of scope: the epic SPEC lists tax/VAT as a non-goal. The open D14 tax decision is recorded as a residual for publishing prices.
  - `[low]` `[reject]` (Blind) Offerings that differ only in features look identical. — Rejected: features are omitted by decision 2 (ROADMAP W3.3), the vendor's description can state them, and the fix would be new copy.
  - `[false]` `[reject]` (Blind) The renewal headline is reused on the marketing page without a recorded sign-off. — Disproved: decision 4 records the reuse, the sentence is the owner's own approved wording, and ADR-0006 requires vendor sign-off of all site copy before publishing, so nothing unapproved goes live.
  - `[low]` `[patch]` (Blind) No test checks that the public route sends no CORS header and sets no cookie. — Added assertions to `the_catalog_lists_published_offerings_to_anyone`: no `access-control-allow-origin` and no `set-cookie`.
  - `[false]` `[reject]` (Blind) `parseCatalog` accepts empty names and duplicate ids. — Disproved: the hub refuses blank offering names (`offering_admin` validation and CHECKs), and ids are primary keys, so a real catalog never carries either.
  - `[low]` `[patch]` (Blind) Missing tests: a timeout while reading the body, CRLF descriptions, non-ASCII names. — Added the body-stall timeout test and a `\r\n` render test. Non-ASCII names have no special code path (Astro escapes all text).
  - `[low]` `[defer]` (Blind) The AGENTS.md line reads as if one exact `PRICING_HUB_ORIGIN` value is required. — Deferred, because the fix edits an agent-context file (frontmatter `deferred`).
  - `[low]` `[defer]` (Edge Case Hunter) Intl and the hub's ISO table disagree on minor-unit digits for IQD, MGA and similar currencies. — Pre-existing: the convention is copied from the portal (decision 5), and story 4 already deferred the same mismatch. Recorded in frontmatter `deferred`.
  - `[low]` `[reject]` (Edge Case Hunter) The anonymous catalog has no throttle, so anyone can force repeated database queries. — Rejected: it is one indexed read of a small table. Anonymous `/api/health` already queries Postgres and SMTP per request, and any free sign-up could already call the catalog without limit. Rate limiting is a deployment decision (ROADMAP §0.5). Recorded as a residual.
  - `[low]` `[patch]` (Verification Gap) A timeout while reading the body is untested. — Grouped with the Blind missing-tests finding; the body-stall test is added.
  - `[low]` `[reject]` (Intent Alignment) The build-failure behaviour is tested only at function and render level, never through `astro build`'s exit status. — Rejected: verified by hand twice (implementer and orchestrator): a build against a closed port exits 1 with the named message. The render test proves the page throws, and a full-build test would need a real build per case.
  - `[low]` `[reject]` (Intent Alignment) The diff takes the conditional reading (R2) and leaves ADR-0006 §1's unconditional sentence as written. — Rejected: decision 1 explains why this does not contradict the ADR (no prices are published without the variable), and every edited doc states the conditional behaviour.
  - `[false]` `[reject]` (Intent Alignment) No release build sets `PRICING_HUB_ORIGIN`, so visitors see no prices. — Disproved as a defect: the repo has no release build or CI at all (ADR-0006 "Not decided here", ROADMAP §0.5). The variable is the deploy-time switch.
  - `[false]` `[reject]` (Intent Alignment) No test feeds the hub's real JSON into the site's parser. — Not a defect here: TypeScript shapes are hand-mirrored by repo convention (AGENTS.md), the Rust test pins the exact JSON, and a priced build against a running hub was checked by hand.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: exit 0, no failures (never piped through grep).
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.
- `pnpm --filter www typecheck && pnpm --filter www test && pnpm --filter www build` -- expected: all pass with no hub running.
- `pnpm --filter portal typecheck && pnpm --filter portal test` -- expected: pass.

**Manual checks:**
- Run `cargo run -p wipe-hub -- serve` with compose up. Insert or publish one annual offering (through `psql` against `127.0.0.1:55432`, or the admin API). Then build `www` with `PRICING_HUB_ORIGIN=http://127.0.0.1:7900`: it exits 0 and `dist/pricing/index.html` shows the offering. Stop the hub and build again: it exits non-zero with the matrix message. `curl 127.0.0.1:7900/api/offerings` without a cookie answers 200.

## Auto Run Result

Status: done

**Summary:** Anyone can now read the hub's published offerings at `GET /api/offerings`, with no session, and the marketing site can list them. The route returns the same body as before. A stale cookie no longer turns it into a 401. It writes no audit row and sends no CORS headers. On the site, a build given `PRICING_HUB_ORIGIN` reads the catalog once at build time (10 s timeout) and checks its shape strictly. It then lists each published offering on `/pricing/`: name, description, price ("1,200.00 USD per year" or "one-off payment"), seats, scope, quota, term, and for annual offerings the owner-approved renewal headline. Any failure to read the catalog (hub down, timeout, non-2xx status, bad body) fails `astro build` with a message naming `PRICING_HUB_ORIGIN`, the URL and the cause. Without the variable, or with an empty catalog, the page is byte-for-byte unchanged and the build needs no hub. Vendor-entered names and descriptions go through the site's overclaim check like any other copy.

**Files changed:**
- `crates/wipe-hub/src/order_routes.rs`, `src/orders.rs`: the catalog handler no longer requires a session; module docs updated.
- `crates/wipe-hub/tests/orders.rs`: the catalog test covers anonymous, owner, member and stale-cookie callers (identical bodies), no audit row, and no CORS or `Set-Cookie` header.
- `crates/wipe-hub/tests/isolation.rs`: the route is now classified `Anonymous`.
- `apps/portal/src/api/orders.ts`: a doc comment only.
- `sites/www/src/lib/offerings.ts` (new): the catalog shape check, the build-time fetch, a copy of the portal's price formatter, and the labels.
- `sites/www/src/components/PriceList.astro` (new): the price list.
- `sites/www/src/pages/pricing.astro`: chooses between the price list and the existing no-prices block.
- `sites/www/src/config.ts`: `pricingHubOrigin()`.
- `sites/www/src/lib/lead-submit.ts`: `parseHubOrigin` names the variable in its errors.
- `sites/www/test/offerings.test.ts` (new), `test/pages.test.ts`: the matrix cases.
- `CONTEXT.md`, `README.md`, `sites/www/README.md`, `docs/ROADMAP.md`, `AGENTS.md`: docs updated (a §10.2 row, the Offering glossary entry, W-R5).

**Review:** one pass, 19 findings: 15 low, 4 false.
- **Patched (5 entries, all low):**
  - the docs no longer claim prices are always current: the page is a snapshot, so rebuild after a catalog change;
  - the orders row in `CONTEXT.md` reads consistently;
  - offering names are `h2`, so the page's headings no longer skip a level;
  - the public route is tested to send no CORS header and no cookie;
  - new tests for a hub that stalls while sending the body, and for CRLF line breaks.
- **Deferred (2, low):** the AGENTS.md wording, and the currency minor-unit mismatch between Intl and ISO, which predates this story.
- **Rejected (12):** with reasons in the Review Triage Log:
  - no call to action on the priced page;
  - plain http accepted for a remote hub;
  - no tax note;
  - offerings that differ only in features look identical;
  - the renewal headline reused on the marketing page;
  - empty names and duplicate ids accepted by the shape check;
  - no rate limit on the public route;
  - build failure not tested through a real `astro build`;
  - the conditional reading of ADR-0006;
  - no release build sets the variable;
  - no test feeds the hub's real JSON to the site's parser;
  - non-ASCII names untested.

**Follow-up review recommended:** false. Patched by verdict: high 0, medium 0, low 5.

**Verification:**
- `cargo test --workspace` with compose up: exit 0, 563 passed, 0 failed.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean. rustfmt `--check` on the touched `.rs` files: clean.
- www: typecheck 0 errors, 266 tests pass, and the build passes with no hub (site check: 13 pages, 0 violations).
- Portal: typecheck passes and 130 tests pass.
- Manual:
  - `PRICING_HUB_ORIGIN=http://127.0.0.1:7999 pnpm --filter www build` (nothing listening) exits 1 with `pricing: could not read http://127.0.0.1:7999/api/offerings (PRICING_HUB_ORIGIN): fetch failed: connect ECONNREFUSED …`.
  - The implementer ran a priced build against a live hub on a scratch database: it exits 0 and lists the offering with its renewal headline.
- Matrix audit: every row is covered by a test that ran and passed.

**Residual risks:**
- **Copy needs your sign-off.** The page's wording in the priced state (labels and the meta description) awaits vendor sign-off before publishing, like all site copy.
- **No buy link, no feature list.** The priced page has no buy link, and licence features are not listed (ROADMAP W3.3).
- **Prices are a snapshot.** They are as of the last build; the site must be rebuilt after a catalog change.
- **Plain http to a remote hub is accepted.** `PRICING_HUB_ORIGIN` takes plain http to any host. Use https, or a hub on the build machine, for a production build.
- **The public catalog has no rate limit.**
- **Tax is undecided.** Publishing prices should wait on the open tax decision (D14), like live payments.
- **The two deferred items above.**
