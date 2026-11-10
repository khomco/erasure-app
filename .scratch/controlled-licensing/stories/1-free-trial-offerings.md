---
title: 'Free trial offerings'
type: 'feature'
created: '2026-11-10'
status: 'done'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '{project-root}/AGENTS.md'
  - '{project-root}/docs/adr/0013-licensing-controls-every-erasure.md'
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
warnings: ['oversized']
deferred:
  - summary: >-
      The portal's Start free trial button on /buy has no interactive test of its success state, cache refresh or 409 message.
    evidence: |-
      BuyPage.test.ts renders TrialCard statically only; startTrial is tested at the API-wrapper level and the hub tests pin the effect end to end. The portal has no DOM test setup (testing-library), which this story does not add.
    location: >-
      apps/portal/src/pages/BuyPage.tsx (TrialCard)
    severity: low
baseline_revision: '0d75acf0063ceaf03dd49b65d880e7ae6228b952'
---

<intent-contract>

## Intent

**Problem:** ADR-0013 §2 makes evaluation a free trial licence, but the hub can only sell offerings with a price of at least 1 minor unit through a payment provider, so a prospect has no way to get a licence without paying.

**Approach:** Offerings gain a `trial` flag (price zero, one-off, a required erasure quota). A signed-in owner with a verified email takes a published trial with `POST /api/trials`; in one transaction the hub writes an already-`paid` order (provider `trial`, no checkout session) and its entitlement, audited, once per organization per trial offering. The catalog, portal `/buy` and site `/pricing/` show a trial as "Free trial" with a "Start free trial" action instead of a price.

## Boundaries & Constraints

**Always:**
- A trial offering is `trial = true`, `price_minor = 0`, `interval = one_off`, and `grants.quota_erasures` a count (never `null`/unlimited). A non-trial keeps today's rules (price 1..99,999,999). Scope, seats, features, term are as for any offering. The admin validator refuses violations with a 400 naming the field; migration CHECKs repeat each rule.
- `trial` is a commercial term: frozen at publication like price (extend the `offerings_lifecycle` trigger and `same_terms`/`changes`).
- Taking a trial: owner only (403 `MSG_OWNER_ONLY_BUY` wording reused), verified email (403, `MSG_UNVERIFIED_BUYER` reused), CSRF as every state-changing route. Offering must be published and a trial, else 404/409 as in the matrix.
- Order snapshot as checkout writes it, `price_minor = 0`, `status = 'paid'`, `paid_at = now()`, `provider = 'trial'`, `provider_session_id = 'trial:<order id>'`; entitlement `provider = 'trial'`, no subscription, no payment ref, `renews = false`, term from `paid_at` for `term_months`. Once-per-organization is enforced by a partial unique index on `orders (organization_id, offering_id) WHERE provider = 'trial'`, with the unique violation mapped to the 409 (so concurrent takes give exactly one 201).
- Audit in the same transaction, actor `user:<id>`: `order.trial_taken {offering_id}` (subject `order:<id>`) and `entitlement.granted {order_id, term_end}` (subject `entitlement:<id>`).
- A trial entitlement behaves like any other: seats register stations, licences carry its quota, downloads are allowed while current.
- Rust serde shapes and TS mirrors change in the same commit: `Offering`/`OfferingRequest` (admin), `CatalogOffering` (hub, portal `api/orders.ts`, site `lib/offerings.ts`), `Order.provider` (`fake`|`stripe`|`trial`).
- New migration `crates/wipe-hub/migrations/0020_trial_offerings.sql`; never edit an applied one.

**Never:**
- No payment-provider call, checkout session, Stripe product/price or webhook for a trial; `POST /api/checkout` for a trial offering is refused (409) before the provider is called.
- No credit pool, grants, lapse-policy or renewal work (stories 2–6). No vendor-issued trial file changes (`stations::vendor_issue` already covers air-gapped trials).
- No change to station-side code, the simulated backend, or ROADMAP §0 invariants.
- No retroactive change to existing offerings, orders or entitlements rows (defaults keep them non-trial).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Admin creates trial | `trial: true, price_minor: 0, interval: one_off, quota_erasures: 25` | 201, offering `trial: true` | — |
| Admin bad trial | trial with price 5, or `annual`, or `quota_erasures: null` | 400 naming `price_minor` / `interval` / `grants.quota_erasures` | nothing written |
| Non-trial at price 0 | `trial: false, price_minor: 0` | 400 naming `price_minor` (unchanged rule) | nothing written |
| Body without `trial` | today's admin body | treated as `trial: false` | — |
| Take trial | verified owner, published trial | 201 `{order, entitlement}`; order `paid`, provider `trial`; entitlement active, term `term_months` long; two audit rows | — |
| Take again | same organization, same offering | 409 `Your organization has already taken this free trial.` | nothing written |
| Concurrent takes | 8 parallel takes, one organization | exactly one 201, others 409; one order, one entitlement | — |
| Other organization | second organization takes same trial | 201 | — |
| Unverified owner | email not verified | 403 unverified message | nothing written |
| Member | role `member` | 403 owner-only message | nothing written |
| Signed out | no session | 401 | — |
| Not a trial | published priced offering via `/api/trials` | 409 `This offering is not a free trial.` | nothing written |
| Not for sale | draft, retired or unknown offering id | 404 (same as checkout's not-for-sale) | nothing written |
| Checkout of trial | `POST /api/checkout` with a trial offering | 409 `This offering is a free trial: start it instead of buying it.`; provider not called | nothing written |
| Catalog | published trial | `GET /api/offerings` item has `trial: true`, `price_minor: 0`, `renewal_notice: null` | — |

## Autonomous decisions

- **Recording a trial as a zero-price order.** *About:* the hub's records of what an organization got. *Problem:* every entitlement today belongs to a paid order, and seats, downloads and licences all read the order's snapshot. *Options:* (A) write the trial as an order already `paid`, provider `trial`, price 0, so all existing reads work unchanged and "once per organization" is one unique index; (B) a separate trial table and entitlements without orders, which touches every query that joins orders. *Choice:* A, because it reuses the existing immutable snapshot and lifecycle triggers (the ADR-0007 model) with the smallest change.
- **A trial must carry an erasure quota.** *About:* what a vendor admin can publish as a trial. *Problem:* a trial with unlimited erasures would recreate the uncontrolled station ADR-0013 retires. *Choice:* the hub refuses `quota_erasures: null` on a trial (safe default; ADR-0013 §2 says a trial is "a fixed number of credits").
- **Where the site's "Start free trial" goes.** *About:* a prospect on `/pricing/`. *Choice:* the portal's `/buy` at the same hub origin the build read prices from (`PRICING_HUB_ORIGIN`); the portal serves `/buy` and already prompts a signed-out visitor to sign in. No new site setting.
- **Error wording.** The three new refusal messages in the matrix are drafted here as plain error text, not marketing copy.

</intent-contract>

## Code Map

- `crates/wipe-hub/migrations/0010_offerings.sql`, `0011_orders.sql`, `0013_entitlements.sql` -- read-only: column CHECKs (auto-named `offerings_price_minor_check`, `orders_price_minor_check`, `orders_provider_check`, `entitlements_provider_check`) and lifecycle triggers to amend in 0020 (`CREATE OR REPLACE FUNCTION offerings_lifecycle` adding `trial` to frozen terms; orders/entitlements triggers need no change since `trial` is not an order column — snapshot price 0 is enough).
- `crates/wipe-hub/src/offering_admin.rs:86` `OfferingRequest` (add `#[serde(default)] trial: bool`), `:153` `validate` (trial rules), `:230` `Offering`, `:251` `same_terms`, `:260` `changes`, `:300` `Row`, `select()`/insert/update SQL -- admin data layer; unit tests at bottom of file.
- `crates/wipe-hub/src/orders.rs:61` `CatalogOffering` (+`trial`), `OfferingRow`/`OFFERING_COLUMNS`, `:388` `create` (refuse trial with new `OrderError` variant before `provider.create_checkout`), `:188` `OrderError`; add `take_trial(conn, organization_id, user_id, offering_id)` beside `create`.
- `crates/wipe-hub/src/entitlements.rs:264` `grant` -- takes a `VerifiedEvent`; split the insert into an inner fn taking actor, subscription id, payment ref so the trial path can grant with actor `user:<id>`.
- `crates/wipe-hub/src/order_routes.rs:86` `routes()` (add `POST /api/trials`), `:182` `checkout` (pattern for owner/verified checks, `JsonBody`, tx), `:149` `From<OrderError> for ApiError` (map new variants).
- `crates/wipe-hub/src/stripe.rs`, `payments.rs`, `payment_events.rs` -- unchanged; provider `trial` never reaches them (`lock_by_session` filters by provider).
- `crates/wipe-hub/tests/orders.rs`, `tests/offering_admin.rs`, `tests/isolation.rs`, `tests/migrations.rs`, `tests/support` -- compose-backed HTTP tests; add trial cases here (new `/api/trials` route belongs in isolation's route list).
- `apps/portal/src/api/adminOfferings.ts` (Offering, OfferingInput, guards), `pages/OfferingForm.tsx` (Trial checkbox: when checked price fixed 0 and disabled, interval forced one-off, quota required, defaulting to 25), `AdminOfferingsPage.tsx`/`AdminOfferingPage.tsx` (show "Free trial" instead of a price).
- `apps/portal/src/api/orders.ts` (`CatalogOffering.trial`, `PROVIDERS` + `trial`, `startTrial(offeringId)` → `POST /api/trials`), `pages/BuyPage.tsx:62` `OfferingCard` (trial: "Free trial" in place of price, button "Start free trial", success note linking `/stations` to register a bench), `OrderPage.tsx:72`, `OrdersPage.tsx:118` (provider `trial` → "Free trial", no price).
- `sites/www/src/lib/offerings.ts` `parseOffering` (accept `trial` boolean; trial ⇒ price 0, one-off, quota non-null; non-trial ⇒ price ≥ 1), `components/PriceList.astro` (trial: "Free trial" instead of price, "Start free trial" link to `<origin>/buy`; needs an `origin` prop from `pages/pricing.astro`), `sites/www/test/offerings.test.ts`, `test/pages.test.ts`.
- `CONTEXT.md` §10.2 (add a shipped row), §9 "How the tiers are enforced" (trial now exists), `README.md` hub section if it lists routes.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0020_trial_offerings.sql` -- add `offerings.trial boolean NOT NULL DEFAULT false`; replace price CHECKs on offerings and orders with trial-aware ones (offerings: `trial ⇒ price 0, one_off, quota not null`; `NOT trial ⇒ price 1..99999999`; orders: `price_minor BETWEEN 0 AND 99999999` and `(price_minor = 0) = (provider = 'trial')`); add `'trial'` to orders and entitlements provider CHECKs; partial unique index for once-per-organization; re-create `offerings_lifecycle` with `trial` frozen -- schema backstop.
- `crates/wipe-hub/src/offering_admin.rs` -- `trial` field, validation, freezing, audit diff, unit tests for each matrix admin row.
- `crates/wipe-hub/src/entitlements.rs` -- reusable grant insert; trial grant with actor `user:<id>`.
- `crates/wipe-hub/src/orders.rs` -- `trial` on `CatalogOffering`; checkout refuses trials; `take_trial` (lock offering `FOR SHARE`, insert paid order, grant, audit, map unique violation to `OrderError::TrialTaken`).
- `crates/wipe-hub/src/order_routes.rs` -- `POST /api/trials {offering_id}` (deny unknown fields) → 201 `{order, entitlement}`; error mapping.
- `crates/wipe-hub/tests/*.rs` -- every matrix row through HTTP, audit rows, concurrency, migration CHECKs, isolation entry.
- `apps/portal/src/**` -- types, guards, form, buy card, order views, with vitest for the guards, `startTrial` and the card/form states.
- `sites/www/src/**`, `sites/www/test/**` -- parse, render and test the trial state.
- `CONTEXT.md`, `README.md` -- document what shipped (route, rules, limits) in the same commit.

**Acceptance Criteria:**
- Given a vendor admin publishes a trial offering, when a verified owner opens portal `/buy`, then the offering shows "Free trial" with a "Start free trial" button, and pressing it creates the entitlement and the organization's `/seats` page lists it as current with the trial's seats.
- Given the hub publishes a trial, when `/pricing/` is built with `PRICING_HUB_ORIGIN`, then the trial's card shows "Free trial" and a "Start free trial" link to `<origin>/buy`, and no price or renewal notice.
- Given a trial entitlement, when the owner registers a station against it (`POST /api/stations`), then the issued licence's quota is the trial's `quota_erasures`.
- Given `cargo test --workspace` and the portal and site test suites, when run, then all pass with the new cases included.

## Spec Change Log

## Review Triage Log

### 2026-11-10 — Review pass
- verdicts: 19 findings — high 0, medium 2, low 12, false 5, maybe-false 0
- findings:
  - `[false]` `[reject]` Blind: trial is once per offering, not once per organization — SPEC CAP-1 says "an organization can take each trial offering once", which the index implements; the remaining doc-wording point was patched (CONTEXT limits now say per offering and per organization).
  - `[low]` `[reject]` Blind: /buy does not show a trial already taken — the 409 says so plainly; showing it in advance needs an extra orders query and branch for a rare repeat visit.
  - `[medium]` `[patch]` Blind + Edge: a signed-out visitor following the site's trial link signs in and lands on / instead of /buy (sign-in does link to sign-up, so not a dead end) — `SignInPrompt` takes `next`; /buy passes `/sign-in?next=/buy`; test added.
  - `[false]` `[reject]` Blind: new error messages are capitalised unlike other hub messages — the wording is pinned verbatim in the intent-contract matrix; changing it would edit the spec.
  - `[low]` `[reject]` Blind: trial orders show status "Paid" — accurate to the stored status beside a "Free trial" price; relabelling adds branches for a cosmetic gain.
  - `[low]` `[patch]` Blind: orders.rs audit-rule doc omits `order.trial_taken` — doc updated.
  - `[low]` `[patch]` Blind: no route ends a trial entitlement; new organizations can take trials again — documented as limits in CONTEXT §10.2 (ending trials is not in CAP-1).
  - `[low]` `[reject]` Blind: trial seats/term/features unbounded, form term defaults to 12 — spec makes these admin choices "as any offering"; capping is a product decision.
  - `[low]` `[reject]` Blind: trial downloads untested — downloads use the shared provider-agnostic `CURRENT_SQL`; a dedicated test adds little.
  - `[medium]` `[patch]` Blind + VG: OfferingForm's submitted trial body untested — extracted pure `offeringBody` used by `submit`, tests for trial, locked trial and stale Unlimited.
  - `[low]` `[patch]` Blind + VG: OrderPage duplicates `orderPriceText` and neither is tested — OrderPage uses the helper (`bracketed`), new OrdersPage.test.ts covers it and `PROVIDER_LABELS.trial`.
  - `[low]` `[reject]` Blind: commerce demo and local-stack docs omit trials — not required by CAP-1; hub HTTP tests cover the flow.
  - `[false]` `[reject]` Edge: trial order stores a session-like id — the spec specifies `trial:<order id>` for the NOT NULL column; no checkout session exists.
  - `[low]` `[defer]` VG: TrialCard success/409 states untested interactively — deferred (no DOM test setup; hub tests pin the effect).
  - `[low]` `[patch]` VG: order_routes.rs module doc says orders become paid only through provider events — trial exception added.
  - `[low]` `[patch]` VG: portal api/orders.ts header says the same — trial exception added.
  - `[false]` `[reject]` Intent: CAP-1's wider aim (no unlicensed station, hub-counted credits) not met — owned by licence-visibility story 5 and stories 3–4; the diff and docs say so.
  - `[false]` `[reject]` Intent: a multi-seat trial gives 25 per licence, not 25 in total — per-licence quota is today's model; the organization pool is story 3 (CAP-3).
  - `[low]` `[reject]` Intent: no single test runs admin publish → take → station — each link is tested over HTTP; a chained test adds little.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including new trial tests (do not pipe through grep).
- `cargo test --workspace` -- expected: all pass.
- `cargo clippy -p wipe-hub --all-targets -- -D warnings` -- expected: clean.
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: pass.
- `pnpm --filter www typecheck && pnpm --filter www test && pnpm --filter www build` -- expected: pass.
- `rustfmt --edition 2021 <touched .rs files>` -- only touched files.

## Auto Run Result

Status: done

**Summary.** Vendor admins can publish free trial offerings: price 0, one-off, with a required erasure quota (the portal form suggests 25). A verified owner takes one with `POST /api/trials` and pays nothing. In one transaction the hub writes a paid order (provider `trial`) and its entitlement, audited as `order.trial_taken` and `entitlement.granted`. Each organization can take each trial offering once; a partial unique index enforces this, so concurrent takes give exactly one 201. A checkout of a trial is refused before the payment provider is called. The catalog, portal `/buy` and site `/pricing/` show "Free trial" with a "Start free trial" action instead of a price.

**Files changed.**
- `crates/wipe-hub/migrations/0020_trial_offerings.sql`: adds the `trial` flag, trial-aware price CHECKs, the `trial` provider, the once-per-organization index, and `trial` frozen at publication.
- `crates/wipe-hub/src/offering_admin.rs`: the `trial` field, its validation, freezing and audit diff.
- `crates/wipe-hub/src/orders.rs`: `trial` in the catalog, checkout refuses trials, and the new `take_trial`.
- `crates/wipe-hub/src/entitlements.rs`: one shared grant insert, plus `grant_trial`.
- `crates/wipe-hub/src/order_routes.rs`: the `POST /api/trials` route and the 409 messages.
- `crates/wipe-hub/tests/{orders,offering_admin,migrations,stations,isolation}.rs` and `tests/support/hub.rs`: every matrix row tested over HTTP, plus concurrency, schema CHECKs and a station licence carrying the trial quota.
- `apps/portal/src/api/{adminOfferings,orders}.ts`: `trial` in the types and `startTrial`.
- `apps/portal/src/pages/{OfferingForm,BuyPage,OrdersPage,OrderPage,AdminOffering(s)Page,AccountPage}.tsx` and their tests: the trial checkbox, `offeringBody`, the trial card, "Free trial" labels, and `/sign-in?next=/buy`.
- `sites/www/src/{lib/offerings.ts,components/PriceList.astro,pages/pricing.astro,config.ts}` and `test/*`: parsing and rendering the trial, linking to `<origin>/buy`.
- `CONTEXT.md` (§9 and a new §10.2 row), `README.md`, `docs/ROADMAP.md` (L-R1 note): documentation.

**Review.**
- 19 findings: medium 2, low 12, false 5.
- 8 patched: 2 medium, 6 low, covering the sign-in return to /buy, `offeringBody` with tests, `orderPriceText` reuse with tests, three stale comments, and the CONTEXT limits.
- 1 deferred (low): no interactive test of the `/buy` trial card.
- 10 rejected, each with its reason in the Review Triage Log.

**Follow-up review: recommended.** Two medium patches landed after the review. The `?next=/buy` return path after sign-in and the extracted `offeringBody` are checked only by static-render and unit tests. Nobody has driven them in a browser, so the full journey (site link → sign-in → /buy → start trial) is unverified end to end in the UI.

**Verification.**
- `cargo test --workspace --no-fail-fast`: 1066 passed, 1 failed. The failure is `wipe-hub` `health::silent_smtp_and_database_time_out_within_bound`, which took about 21 s against a 3.5 s limit. That test touches no changed code (health, config, mail and db are untouched), and the implementer saw it fail on the untouched baseline too.
- `cargo clippy -p wipe-hub --all-targets -D warnings`: clean.
- `rustfmt --check` on the touched files: clean.
- Portal: typecheck, 283 tests and build pass.
- Site: typecheck, 274 tests and build pass, site check 0 violations.
- Matrix audit: every row is covered by a hub HTTP test that ran and passed.

**Residual risks.**
- The three new 409 messages are draft text, capitalised unlike other hub errors; the vendor should review them.
- The site's trial link assumes `PRICING_HUB_ORIGIN` also serves the portal.
- No route ends a trial entitlement early.
- A new sign-up, or a newly published trial offering, allows another trial.
- A multi-seat trial gives each licence its own quota until the credit pool lands (story 3).
- Stations still erase without a licence until licence-visibility story 5.
- Nothing was checked in a real browser.
