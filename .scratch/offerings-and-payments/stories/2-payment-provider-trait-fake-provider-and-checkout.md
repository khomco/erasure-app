---
title: 'PaymentProvider trait, Fake provider and checkout'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '313883878415352a7f984e40c73376796b60acab'
review_loop_iteration: 0
followup_review_recommended: false
context:
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
  - '{project-root}/.scratch/offerings-and-payments/SPEC.md'
warnings: [oversized]
deferred: []
---

<intent-contract>

## Intent

**Problem:** Vendor admins can publish offerings (story 1), but no customer can buy one: the hub has no orders, no `PaymentProvider` (ADR-0007 §4) and no checkout, and the portal has no buy, orders or billing pages.

**Approach:** Add a `payments` module with the `PaymentProvider` trait and its in-process `Fake` implementation, an `orders` table and data layer, and customer routes to list published offerings, check out, list and view orders, open billing, and drive the Fake provider's hosted-page stand-in. An order becomes `paid` or `expired` only by applying a `VerifiedEvent`, which only `PaymentProvider::verify_event` can construct. Portal pages `/buy`, `/orders`, `/orders/$id`, `/checkout/fake` and `/billing/fake` complete the flow locally.

## Boundaries & Constraints

**Always:**
- The trait lives at `wipe_hub::payments::PaymentProvider` (`Send + Sync`, `axum::async_trait`): `create_checkout(&CheckoutOrder) -> CheckoutSession {session_id, url}`, `billing_portal(organization_id) -> Url`, `verify_event(&HeaderMap, &[u8]) -> VerifiedEvent`. `AppState` gains `payments: Arc<dyn PaymentProvider>`, chosen by the existing `config::PaymentProvider` enum.
- `VerifiedEvent {id, kind: CheckoutCompleted{session_id} | CheckoutExpired{session_id} | Ignored{event_type}}` has private fields and no public constructor, so nothing but `verify_event` can produce one. `payments::apply_event` is the only code that changes an order's status.
- Fake signs `{"id","type","session_id"}` JSON with HMAC-SHA256 under the public constant `FAKE_WEBHOOK_SECRET`, sent in a `fake-signature: <hex>` header. Verification is constant-time, and anything else is refused.
- Checkout is for a session user who is an `owner` with a verified email, and only for a `published` offering. Checkout reads the offering `FOR SHARE`, so a concurrent retire serialises with it. It calls the provider first, then inserts the order with the session id and the audit row in one transaction. A provider error writes nothing.
- An order snapshots the offering's name, price, currency, interval and grants. Its lifecycle is `pending → paid` or `pending → expired`, and both are terminal. Every state change (creation included) writes one audit row in the same transaction: `order.created` (actor `user:<id>`), then `order.paid` or `order.expired` (actor `provider:<fake|stripe>`, detail `{event_id}`). The subject is `order:<id>`.
- Every order query takes the session's organization id first. Another organization's order id or session id is a 404 that changes and audits nothing.
- Money stays integer minor units. Timestamps are RFC 3339 UTC strings formatted in SQL (`admin::RFC3339_UTC`).

**Never:**
- No HTTP webhook route, no event store and no Stripe implementation (story 3). No fulfilment, entitlements, renewal or refunds (story 4). No public or marketing-site read (story 5).
- No browser redirect or client call that marks an order paid except through `verify_event` → `apply_event`.
- No card data, no floats for money, no `sqlx::query!` macros, and no editing an applied migration.
- The Fake provider never completes a checkout on a hub whose `public_base_url` is not a local host.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Catalog | any signed-in user, `GET /api/offerings` | `{offerings:[{id,name,description,price_minor,currency,interval,grants}]}` published only, by lower(name), id | no session → 401 |
| Checkout | verified owner, `POST /api/checkout {offering_id}` | 201 `{order, checkout_url}`; order `pending`, terms snapshotted; Fake url `{public_base_url}/checkout/fake?session=fake_cs_…`; audit `order.created {offering_id, price_minor, currency, provider}` | — |
| Checkout refused | member or vendor admin / unverified owner / unknown, draft or retired offering, non-UUID / bad body | 403 `only an organization owner can buy` / 403 `verify your email address before buying` / 404 `this offering is not for sale` / 400 `{error}` | no row, no audit |
| Provider unavailable | `--payment-provider stripe` (story 3 not built); or Fake with non-local base URL | checkout and billing 503 `payments are not available on this hub yet` / `the fake payment provider works only on a local hub` | nothing written |
| Orders | any role, `GET /api/orders`, `GET /api/orders/:id` | own organization's orders, newest first / one order | other org's or unknown id → 404 `no such order` |
| Fake page read | owner, `GET /api/payments/fake/sessions/:id` | `{order}` | member 403; other org/unknown/provider not fake → 404 `no such checkout` |
| Fake pay | owner, `POST …/sessions/:id/pay` on pending order | signs `checkout.completed`, `verify_event`, `apply_event` in one transaction → 200 `{order}` `paid`, `paid_at`; audit `order.paid {event_id}` | order not pending → 409 `this checkout has already ended`, nothing written |
| Fake abandon | owner, `POST …/sessions/:id/abandon` on pending order | same with `checkout.expired` → `expired`, `expired_at`; audit `order.expired` | as above |
| Bad event | tampered payload, wrong or missing signature | `verify_event` returns `Err` | unit-tested; no order change |
| Event not applicable | completed or expired event for an unknown session or a non-pending order; `Ignored` kind | `apply_event` returns the unchanged order (or none), audits nothing | — |
| Billing | owner, `POST /api/billing/portal` | 200 `{url}`: Fake `{public_base_url}/billing/fake` | member 403 `only an organization owner can manage billing` |

## Autonomous decisions

1. **Only an organization owner may buy or open billing.** *About:* a customer team in the portal. *Problem:* SPEC says "a verified customer buys", but a member committing the organization's money is the riskier reading. *Options:* any verified user; owners only. *Choice:* owners only, matching the existing owner-only team management. This is the safe default, and widening it later is non-breaking. Every role can see orders.
2. **Story 2 adds `GET /api/offerings` for signed-in users only.** *Problem:* customers need a list to buy from, while story 5 owns the public API and the pricing copy sign-off. *Choice:* the same path and shape, published only, behind a session. Story 5 drops the session requirement. This keeps prices non-public until the marketing story.
3. **Fake events are applied in-process.** The HTTP webhook route needs a CSRF exemption, and that, the event store and replay-safety are story 3 (CAP-3). In story 2 the Fake pay and abandon routes sign an event, run it through the same `verify_event` and `apply_event` that the webhook will use, and rely on the status guard (only `pending` changes) for repeats.
4. **The Fake provider refuses non-local hubs at request time.** *Problem:* Fake is the default provider, and on a deployed hub any owner could mark an order paid for free. *Options:* warn only; refuse startup (which would break many existing configs and tests that use non-local base URLs with the default provider); refuse checkout, billing and pay at request time. *Choice:* refuse at request time with 503 when `!auth::is_local_host(public_base_url)`, plus a `startup_warnings` line whenever the provider is Fake.
5. **`--payment-provider stripe` still starts** (the documented behaviour). Until story 3 its provider is `Unavailable`, which answers 503 and verifies nothing.
6. **Order statuses are `pending`, `paid` and `expired` only.** Story 4 adds refund and cancellation states through its own migration.

</intent-contract>

## Code Map

- `crates/wipe-hub/src/lib.rs` -- `AppState` (add `payments`), module list, `router` merge; `pub use config::PaymentProvider` stays (the config enum). Refer to the trait as `payments::PaymentProvider`.
- `crates/wipe-hub/src/config.rs:37-44` -- `enum PaymentProvider {Fake, Stripe}` (add `as_str`); `startup_warnings` at :291 (add the Fake line; update tests at :770-800 that assert exact warning lists).
- `crates/wipe-hub/src/auth.rs` -- `ApiError` (:109; `internal` logs), `JsonBody` (:155), `CurrentUser` (:256, `role`), `actor()`, `is_local_host` (:217), `MSG_NOT_SIGNED_IN`. `accounts::account(pool, org, user)` gives `user.email_verified` (pattern: `team_routes.rs:353-358`).
- `crates/wipe-hub/src/team_routes.rs` -- the customer route template: owner check before body, `path_id` (non-UUID → 404), `bad_request`/`not_found`/`conflict` helpers.
- `crates/wipe-hub/src/offering_admin.rs` -- `Grants` (reuse in the order snapshot and catalog JSON), `Row`/`select()` column pattern, `OfferingError`, `RFC3339_UTC`; `offerings.status = 'published'` is the catalog filter.
- `crates/wipe-hub/src/audit.rs:75` -- `audit::record(executor, &AuditEntry)`.
- `crates/wipe-hub/migrations/0010_offerings.sql` -- the trigger and CHECK backstop style to copy for `0011_orders.sql`.
- `Cargo.lock:1771` -- `hmac 0.12.1` is already in the graph (via sqlx); add `hmac = "0.12"` to `crates/wipe-hub/Cargo.toml` (`sha2` 0.10 is a dependency already).
- `crates/wipe-hub/tests/isolation.rs:40-130` -- `ROUTES`; the sweep at :140-240 substitutes `:id` and expects owner 404 and member 403 for every Customer `:id` path. `tenant()` and `rows_of()` must learn about orders.
- `crates/wipe-hub/tests/support/hub.rs` -- `spawn_hub`, `bootstrap_admin`, `Who`, `assert_error`, `audited`, `count`; `BASE` = `http://127.0.0.1:7900` (local, so Fake works).
- `crates/wipe-hub/tests/offering_admin.rs:20-80` -- fixture and body helpers for creating and publishing an offering.
- `apps/portal/src/api/adminOfferings.ts` -- `Grants`, `isOffering`-style guards, `formatPrice`, `FEATURE_LABELS` to reuse; `auth.ts` `request`/`ensureOk`/`json`/`HubError`/`fetchMe`.
- `apps/portal/src/main.tsx:69-240` -- routes (`validateSearch` pattern at `/verify-email`); `pages/HomePage.tsx` -- links.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0011_orders.sql` -- `orders`: uuid id; `organization_id` → organizations; `offering_id` → offerings; `created_by` → users ON DELETE SET NULL; snapshot `offering_name`, `price_minor`, `currency`, `billing_interval`, `scope`, `seats`, `quota_erasures`, `features`, `term_months` with offerings' CHECKs; `status` in (`pending`,`paid`,`expired`) default `pending`; `provider` in (`fake`,`stripe`); `provider_session_id` text NOT NULL, UNIQUE (provider, provider_session_id); `created_at`, `updated_at`, `paid_at`, `expired_at`, tied to status by CHECKs; index (organization_id, created_at DESC); trigger refusing DELETE, any transition except pending→paid or pending→expired, and changes to the snapshot, provider or session columns -- schema backstop.
- `crates/wipe-hub/src/payments.rs` (or `payments/{mod,fake}.rs`) -- trait, `CheckoutOrder`, `CheckoutSession`, `ProviderError` (`Unavailable(&'static str)`, `Failed`), `VerifiedEvent`/`EventKind`, `Fake` (`FAKE_WEBHOOK_SECRET`, `sign`, session ids `fake_cs_<uuid simple>`, event ids `fake_evt_<uuid simple>`, URLs built from `public_base_url` keeping any base path), `Unavailable`, `provider_for(&Config)`; unit tests: sign/verify round trip, tampered body, wrong and missing header, unknown type → `Ignored`.
- `crates/wipe-hub/src/orders.rs` -- `Order` serde shape, `catalog`, `create` (offering FOR SHARE, provider call, insert, audit), `list`, `get`, `by_session` (org-scoped), `apply_event` (lock by provider+session FOR UPDATE, status guard, audit).
- `crates/wipe-hub/src/order_routes.rs` -- the eight routes in the matrix, `MSG_*` constants, owner and verified checks before the body, Fake-only routes 404 under other providers; `lib.rs` module and `merge`.
- `crates/wipe-hub/src/config.rs` -- `as_str`, the Fake startup warning, test updates.
- `crates/wipe-hub/tests/orders.rs` -- one HTTP test per matrix row, asserting the response, the stored row and audit rows (count, action, actor, detail), plus: retiring the offering after a paid order leaves the order and its snapshot intact; a second pay is 409 with exactly one `order.paid` row; two concurrent pays give one 200 and one 409, never a 500.
- `crates/wipe-hub/tests/isolation.rs` -- classify the routes (all Customer); seed a published offering and an order per tenant; sweep `:id` with B's order or session id; expect member 404 (not 403) on `/api/orders/:id`; add B's order and session ids to the secrets and `orders` to `rows_of`.
- `apps/portal/src/api/orders.ts` + `orders.test.ts` -- mirrored `CatalogOffering`, `Order` types and guards; `fetchCatalog`, `startCheckout`, `fetchOrders`, `fetchOrder`, `fetchFakeCheckout`, `payFakeCheckout`, `abandonFakeCheckout`, `openBillingPortal`; tests for each call's path, method and body, and the guards.
- `apps/portal/src/pages/BuyPage.tsx`, `OrdersPage.tsx`, `OrderPage.tsx`, `FakeCheckoutPage.tsx`, `FakeBillingPage.tsx`; `main.tsx` routes; `HomePage.tsx` links -- Buy shows offerings, and owners see a Buy button (only when verified, else the verify notice) that navigates to `checkout_url` only if it is http(s). The Fake checkout page is labelled "Test payment: no money moves" and has Pay and Abandon, both ending on `/orders/$id`. Orders has a Billing button for owners. A pending order page refetches every 3 s.
- `CONTEXT.md` (§10.2 row; §14 **Order** and **Payment provider** entries; Offering entry notes it is bought through checkout), `README.md` hub section, `docs/ROADMAP.md` A1.3/L notes as applicable -- docs travel with code.

**Acceptance Criteria:**
- Given a verified owner and a published offering on the Fake provider, when they check out and pay through the API, then the order goes `pending` → `paid` with exactly two audit rows (`order.created`, `order.paid`), and `GET /api/orders` shows it to both the owner and a member.
- Given any request, when it reaches the hub, then only the Fake pay and abandon routes change an order's status, and only through `verify_event` → `apply_event`; a grep shows `apply_event` is the only `UPDATE orders SET status`.
- Given `cargo test -p wipe-hub` with compose up, when `isolation` runs, then every new route is classified and swept, and B's orders are byte-for-byte unchanged.
- Given the portal, when `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` run, then all pass.

## Spec Change Log

- 2026-11-08, implementation choices within the contract (no behaviour the matrix fixes changed):
  - `PaymentProvider` also has `fn name(&self) -> &'static str` (`fake`/`stripe`), the `orders.provider` value and the `provider:<name>` audit actor; `VerifiedEvent` carries it privately.
  - `ProviderError` has a third variant, `Rejected(&'static str)`, for a bad signature or payload, so `verify_event` failures are distinct from `Unavailable` (503) and `Failed` (502 `the payment provider could not be reached; try again later`, logged).
  - `CheckoutOrder` carries the order id generated before the provider call (Stripe's `client_reference_id` in story 3); the insert uses it.
  - The Fake provider's `verify_event` also refuses a non-local hub, so story 3's webhook route cannot accept publicly signed Fake events on a deployed hub; pay/abandon check locality first and answer 503 `the fake payment provider works only on a local hub`.
  - `orders::apply_event` holds the only `UPDATE orders SET status` and is re-exported as `payments::apply_event` (a test greps `src/` for it).
  - The 0011 trigger also freezes `organization_id`, `offering_id` and `created_at`, and refuses any status/stamp change once terminal; `created_by` stays clearable by `ON DELETE SET NULL`.
  - The Fake read route (`GET …/sessions/:id`) answers a member 403 `only an organization owner can buy`.

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 29 findings — high 0, medium 0, low 23, false 6, maybe-false 0
- findings:
  - `[low]` `[patch]` `ProviderError::Rejected` mapped to 502 and logged as a provider outage (Blind) — fixed: `Rejected` answers 400 with its message and logs at warn.
  - `[low]` `[reject]` Fake's "local only" check reads `public_base_url`, not the bind address, so `--addr 0.0.0.0` with the default base URL exposes it (Blind) — real, but it needs a misconfigured hub whose emails and links already point at 127.0.0.1, and such a hub can only hold a file-held development root; the fix adds a second guard. Listed as a residual risk.
  - `[low]` `[reject]` A pending Fake order cannot be resumed, and repeated Buy presses pile up pending orders (Blind) — local Fake only; Stripe sessions expire in story 3. A resume link needs the session id in the order API (new surface).
  - `[low]` `[reject]` `OrderPage` refetches a pending order every 3 s forever (Blind) — one cheap GET per 3 s while the page is open; a backoff adds complexity for a local test flow.
  - `[low]` `[reject]` `VerifiedEvent` carries no amount, currency or order reference for mismatch checks (Blind) — no user harm in story 2; the trait is crate-internal and story 3 adds fields with the Stripe payload.
  - `[low]` `[reject]` Nowhere stores a provider customer id for billing (Blind) — the Stripe customer mapping is story 3's (stories.yaml); Fake needs none.
  - `[low]` `[reject]` Guard test `only_apply_event_updates_an_orders_status` is non-recursive and case-sensitive (Blind) — a developer adding a `payments/` directory hits a loud panic in the test; hardening it adds parsing logic.
  - `[low]` `[patch]` Snapshot test cannot fail, because retiring changes no copied column (Blind) — fixed: the test renames the published offering after checkout and asserts the order keeps the original name.
  - `[low]` `[reject]` No test that a failing provider writes nothing (Blind) — the provider is called before any write and its error returns at once, which is plain from the code; a test needs an injectable provider (new surface).
  - `[low]` `[patch]` No test that a concurrent retire serialises with checkout (Blind) — same root cause as the verification-gap row below; fixed there.
  - `[low]` `[reject]` Provider call inside the transaction with no timeout (Blind) — Fake is in-process and instant; the risk arrives with Stripe's network call in story 3 and is already recorded in CONTEXT.md.
  - `[low]` `[patch]` `created_by ON DELETE SET NULL` through the trigger untested (Blind) — same root cause as the verification-gap row below; fixed there.
  - `[low]` `[reject]` Portal shows raw RFC 3339 timestamps (Blind) — matches the existing portal convention (e.g. `AdminLeadPage`); a locale formatter belongs to a portal-wide change.
  - `[low]` `[reject]` `FakeCheckoutPage` and `OrderPage` show a bare "not signed in" error instead of the sign-in prompt (Blind) — the message is accurate; the fix adds a session query to each page.
  - `[low]` `[reject]` New portal pages untested; `isInt`/`oneOf` duplicated in `orders.ts` (Blind) — the portal has no page-test harness (as in prior stories); the duplicates are two tiny guards with no named divergence.
  - `[low]` `[reject]` Provider call has no timeout while it holds the offering lock (Edge Case Hunter) — duplicate of the Blind row above; story 3.
  - `[low]` `[reject]` A provider session is orphaned if the insert or commit fails after `create_checkout` (Edge Case Hunter) — a Fake session is just an id; a Stripe event for an unknown session is ignored by `apply_event` and the session expires. Story 3 can cancel it.
  - `[false]` `[reject]` An unverified second owner can pay through the Fake page and so bypass the verified-buyer rule (Edge Case Hunter) — the purchase (checkout) is still started only by a verified owner. The Fake page stands in for the provider's hosted page, which anyone holding the URL can complete with Stripe too.
  - `[low]` `[patch]` Stale Pay/Abandon buttons after a 409 (Edge Case Hunter) — fixed: on error the page invalidates the fake-checkout query and re-reads the order.
  - `[low]` `[reject]` Guard test can be slipped past (Edge Case Hunter) — duplicate of the Blind row above.
  - `[low]` `[patch]` Removing an owner who bought leaves the trigger's `created_by` path untested (Verification Gap) — fixed: a test where another owner removes the buyer — removal 204, order kept with `created_by` null and status/timestamps unchanged.
  - `[low]` `[patch]` Nothing proves checkout's `FOR SHARE` waits for an uncommitted retire (Verification Gap) — a sale racing a retire would be benign (the snapshot holds), but the spec promises serialisation; fixed: a race test asserting 404 and no order.
  - `[low]` `[reject]` The verified-event boundary is never exercised over HTTP; the hub signs and verifies in one handler (Intent audit) — autonomous decision 3: the inbound webhook route, its CSRF exemption and the event store are story 3 (CAP-3).
  - `[low]` `[reject]` Portal pages are built but not rendered in tests (Intent audit) — same as the Blind portal-tests row; residual risk.
  - `[false]` `[reject]` The `2.spec.approved` marker predates this spec's decisions (Intent audit) — an orchestration marker that build-auto must not read; the decisions are recorded for the owner in the intent contract and the hand-off.
  - `[false]` `[reject]` No "local end-to-end script" (ADR-0007 §4) (Intent audit) — the SPEC's success signal ends in a licence seat, which is story 4's fulfilment; story 2's path is covered end to end by `tests/orders.rs`.
  - `[false]` `[reject]` Owner-only buying narrows "a verified customer" (Intent audit) — autonomous decision 1, recorded.
  - `[false]` `[reject]` The session-gated `GET /api/offerings` takes story 5's path (Intent audit) — autonomous decision 2, recorded; story 5 drops the session requirement.
  - `[false]` `[reject]` Docs say paid is "never because the browser came back", yet the Fake Pay POST triggers it (Intent audit) — the docs statement is about the redirect; the Pay POST is the Fake provider's own hosted-page action, which produces a signed event, as Stripe's page does.

## Design Notes

Order JSON: `{"id","offering_id","offering_name","price_minor":120000,"currency":"USD","interval":"annual","grants":{…story 1 shape…},"status":"pending","provider":"fake","created_at","updated_at","paid_at":null,"expired_at":null}`. The provider session id is not in it. The Fake page is addressed by it in the URL only.

Fake pay handler, in sketch: `tx = begin; order = orders::by_session(tx, org, "fake", sid)?.ok_or(404); if order.status != "pending" { 409 }; (body, sig) = fake::signed_event("checkout.completed", sid); ev = state.payments.verify_event(&headers_with(sig), &body)?; orders::apply_event(tx, &ev)?; commit`. `by_session` locks `FOR UPDATE`, so concurrent pays serialise and the loser sees `paid` → 409.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including `orders` and `isolation`.
- `cargo test --workspace` -- expected: all pass.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: all pass.

## Auto Run Result

Status: done

**Summary:** A verified organization owner can now buy a published offering on the Fake payment provider. Checkout creates a `pending` order that records the offering's name, price and grants as they were at that moment. The owner is then sent to the Fake provider's stand-in checkout page (`/checkout/fake`). Pressing Pay or Abandon there makes the hub sign a provider event, check it through `PaymentProvider::verify_event`, and apply it with `orders::apply_event`, the only code that changes an order's status. The order becomes `paid` or `expired`, and the change is audited. Every role can list and view its organization's orders, and owners can open billing, which leads to the local `/billing/fake` page under Fake. The Fake provider refuses (503) on any hub whose public URL is not local. `--payment-provider stripe` still starts, but checkout and billing answer 503 until story 3.

**Files changed:**
- `crates/wipe-hub/migrations/0011_orders.sql`: the `orders` table and a trigger enforcing its rules (no deletes, pending → paid or expired only, recorded terms never change).
- `crates/wipe-hub/src/payments.rs`: the `PaymentProvider` trait, `VerifiedEvent`, the `Fake` provider (events signed with HMAC-SHA256), the `Unavailable` provider and `provider_for`.
- `crates/wipe-hub/src/orders.rs`: the customer offerings list, checkout, order list and detail, lookup by checkout session, and `apply_event`.
- `crates/wipe-hub/src/order_routes.rs`, `src/lib.rs`: eight customer routes and `AppState.payments`.
- `crates/wipe-hub/src/config.rs`, `Cargo.toml`: `PaymentProvider::as_str`, a start-up warning whenever Fake is in use, and the `hmac` and `hex` dependencies (both already in the lock file).
- `crates/wipe-hub/tests/orders.rs`: 18 HTTP tests covering the matrix, plus a concurrent-pay race, a checkout racing a retire, removing a buyer, a snapshot rename, and a schema check.
- `crates/wipe-hub/tests/isolation.rs`, `tests/support/hub.rs`: the new routes classified and swept with organization B's order and session ids, plus helpers.
- `apps/portal/src/api/orders.ts` (+ test), `pages/BuyPage.tsx`, `OrdersPage.tsx`, `OrderPage.tsx`, `FakeCheckoutPage.tsx`, `FakeBillingPage.tsx`, `main.tsx`, `HomePage.tsx`, `api/adminOfferings.ts`: the portal client and pages.
- `CONTEXT.md`, `README.md`, `docs/ROADMAP.md`: a §10.2 row, glossary entries for **Order** and **Payment provider**, and the A1.3 note.

**Review:** 29 findings, triaged in the log above.
- 6 fixed by patch, all low. They have 5 root causes: the status code for a rejected event, a snapshot test that could not fail, the checkout-versus-retire race test, the buyer-removal test, and stale Fake checkout buttons.
- 0 deferred.
- 23 rejected, with reasons in the log.

Follow-up review recommended: false, because no high or medium finding was patched.

**Verification:**
- `cargo test --workspace` (compose up): exit 0, 481 passed, 0 failed.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean.
- `rustfmt --edition 2021 --check` on the touched Rust files: clean.
- Portal typecheck, tests (116) and build: all pass.
- The matrix test audit found every row covered by a test that ran and passed.

**Residual risks:**
- The Fake provider's local-only check reads `--public-base-url`, not the address the hub listens on. A hub started on `0.0.0.0` with the default base URL would let anyone on the network mark an order paid for free.
- Checkout holds the transaction and the offering's share lock while the provider is called. Story 3 must add a timeout once that call goes over the network to Stripe.
- A pending Fake order cannot be resumed and never expires on its own. Repeated Buy presses leave more pending orders.
- The portal pages have not been run in a real browser.
- Decisions for the owner to check, recorded in the intent contract:
  - only owners may buy and open billing;
  - `GET /api/offerings` requires a signed-in session until story 5;
  - Fake events are applied in-process, and the webhook route is story 3's;
  - the Fake provider is refused on non-local hubs;
  - Stripe answers 503 until story 3.
