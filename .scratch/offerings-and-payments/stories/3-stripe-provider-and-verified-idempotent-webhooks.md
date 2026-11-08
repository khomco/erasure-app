---
title: 'Stripe provider and verified idempotent webhooks'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '135ab208385e92626a7d085735b342d8007d101e'
review_loop_iteration: 2
followup_review_recommended: false
context:
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
  - '{project-root}/.scratch/offerings-and-payments/SPEC.md'
  - '{project-root}/.scratch/offerings-and-payments/stories/2-payment-provider-trait-fake-provider-and-checkout.md'
warnings: [oversized]
deferred:
  - summary: >-
      A trailing-slash path under an existing /api route (POST /api/checkout/, /api/payments/webhook/) gets the portal fallback, not the JSON 404 AGENTS.md promises.
    evidence: |-
      Pre-existing router behaviour found by the story 3 implementer; the /api/*rest catch-all does not match a trailing slash after a defined route. A Stripe endpoint registered with a trailing slash would get a non-JSON 404 and retries.
    location: >-
      crates/wipe-hub/src/lib.rs
    severity: low
  - summary: >-
      The story 3 edit to the AGENTS.md hub bullet left one line wrapped at about 150 columns.
    evidence: |-
      Review pass 2 (Blind). Cosmetic; the fix edits an agent-context file, so it is deferred by rule.
    location: >-
      AGENTS.md
    severity: low
---

<intent-contract>

## Intent

**Problem:** `--payment-provider stripe` starts, but checkout and billing answer 503. No HTTP route accepts provider events, and nothing stores them. So a real payment can never mark an order paid, and a replayed or reordered event has no defence beyond the order's status guard (story 2).

**Approach:** Add a `Stripe` implementation of `payments::PaymentProvider`. It calls the Stripe REST API (Checkout Sessions and billing portal sessions) over `reqwest` with a timeout, and it verifies `Stripe-Signature`. Add `POST /api/payments/webhook`, which has no CSRF check: the configured provider verifies the raw body, then one transaction stores the event by `(provider, event id)` and applies it through `orders::apply_event`. A repeated event id changes nothing. The Fake pay and abandon routes go through the same store. Tests drive Stripe through a local mock of the Stripe API and through Stripe-shaped payload fixtures signed in the test. Document real test mode with `stripe listen` (CAP-7).

## Boundaries & Constraints

**Always:**
- `config`: `--payment-provider stripe` refuses to start without both `--stripe-secret-key`/`WIPE_HUB_STRIPE_SECRET_KEY` and the existing webhook secret (`ConfigError::MissingStripeSecretKey`, naming the env var). `--stripe-api-base`/`WIPE_HUB_STRIPE_API_BASE` defaults to `https://api.stripe.com`, and anything but https, or http on a local host, refuses to start (`BadStripeApiBase`). Both secrets are trimmed, `hide_env_values`, and redacted in `Config`'s `Debug`.
- `Stripe` sends `Authorization: Bearer <key>`, `Stripe-Version: 2024-06-20` and form bodies. One `reqwest::Client` with a 5 s connect and 15 s total timeout. Checkout sends `Idempotency-Key: wipehub-checkout-<order id>`. A non-2xx answer, a timeout or a reply without a usable `id`/http(s) `url` is `ProviderError::Failed`, logged without the key, and 502 to the client with nothing written (as in story 2).
- Checkout session form: `mode=payment`, `client_reference_id=<order id>`, `success_url` and `cancel_url` = `{public_base_url}/orders/<order id>` (story 2's `page_url`), one line item with `quantity=1` and `price_data[currency]` (lower-case), `price_data[unit_amount]=price_minor`, `price_data[product_data][name]=offering_name`, `metadata[wipehub_order_id]`, `metadata[wipehub_organization_id]`, `metadata[wipehub_hub]` (this hub's `public_base_url` origin, `scheme://host[:port]`; decision 11). Plus `customer=<id>` when the organization has a stored Stripe customer, else `customer_creation=always`. No email or other personal data is sent. If Stripe answers 400 with `error.code=resource_missing` and `error.param=customer` to a request that sent a stored customer, the request is sent once more with `customer_creation=always` instead of `customer` and `Idempotency-Key: wipehub-checkout-<order id>-new-customer` (decision 10). Any other answer to either request is handled as above.
- Signature (Stripe's documented scheme): the header holds `t=<unix>` and one or more `v1=<hex>`. The expected value is hex HMAC-SHA256 over `"{t}.{raw body}"`, keyed with the whole webhook secret string. Any matching `v1` passes, compared in constant time, and `|now − t|` must be ≤ 300 s (`STRIPE_SIGNATURE_TOLERANCE`). Anything else is `Rejected`. A public `stripe::signature_header(secret, t, body)` helper signs for tests.
- Event mapping (parse only `id`, `type` and `data.object.{id, payment_status, amount_subtotal, currency, customer, client_reference_id, metadata.wipehub_order_id}`, tolerating unknown fields):
  - `checkout.session.completed` with `payment_status=paid`, or `checkout.session.async_payment_succeeded`, maps to `CheckoutCompleted`.
  - `checkout.session.expired` or `checkout.session.async_payment_failed` maps to `CheckoutExpired`.
  - Every other type, including `completed` not yet paid, maps to `Ignored`.
  - A `checkout.session.*` event without `data.object.id` is `Rejected`.
- `VerifiedEvent` gains private `customer_id`, `amount: Option<(i64, String)>` (minor units, upper-case currency) and `order_ref: Option<Uuid>` (the `metadata[wipehub_order_id]` value only, when it is a UUID and `metadata[wipehub_hub]` equals this hub's origin; `client_reference_id` is sent but never read back, decisions 9 and 11), with getters. Fake leaves them `None`, and it still has no public constructor.
- Migration `0012_payment_events.sql` (new; never edit 0011):
  - `payment_events`: `provider`, `event_id` (non-blank, ≤255), `event_type`, `session_id` null, `payload_sha256` (64 hex), `outcome` null or one of `applied`, `unchanged`, `ignored`, `unmatched`, `mismatch`, and `received_at`. Primary key `(provider, event_id)`. A trigger refuses DELETE and any UPDATE except setting `outcome` once from NULL.
  - `billing_customers`: `organization_id` → organizations, `provider`, `customer_id`, `created_at`, `updated_at`. Primary key `(organization_id, provider)` and UNIQUE `(provider, customer_id)`. A trigger refuses DELETE and any UPDATE except one that changes `customer_id` and `updated_at` only (decision 10).
- `payment_events::receive(conn, &VerifiedEvent, raw_body)` is the only entry from a verified event to `apply_event`, and both the webhook and the Fake pay/abandon routes use it:
  1. `INSERT … ON CONFLICT DO NOTHING`. A conflict is `Duplicate`, which writes nothing more, and concurrent duplicates serialise on the key.
  2. Otherwise run `apply_event`.
  3. Set `outcome` once.
- `apply_event` (still the only `UPDATE orders SET status`) returns an outcome:
  - `Ignored` for the `Ignored` kind.
  - `Unmatched` when no order has the session.
  - `Unchanged` when the order is not pending.
  - `Mismatch` when a completed event's `amount` differs from the order's snapshot price/currency, or its `order_ref` differs from the order id. The order stays pending and an error is logged.
  - Otherwise `Applied`, with story 2's audit row unchanged. A paid transition that carries `customer_id` stores it in `billing_customers`: insert, or, when the organization's stored customer differs, replace it (logged at info with both ids). An unchanged customer writes nothing.
- A `CheckoutCompleted` that is `Unmatched` but carries an `order_ref` (the hub's own order, not committed yet or lost) makes `receive` return `NotReady`. The handler rolls back and answers 503 `this payment event cannot be applied yet; retry later`, logged at error, so Stripe retries. Every other outcome commits and answers 200 `{"received":true}`.
- `billing_portal(organization_id, customer_id: Option<&str>)`. Stripe with no stored customer, or whose stored customer Stripe answers `resource_missing`/`param=customer` (decision 10), answers `ProviderError::NoBillingAccount`, which is 409 `billing opens after your first payment`. With a customer it calls `POST /v1/billing_portal/sessions` with `customer` and `return_url={public_base_url}/orders`. Fake ignores the customer.
- `csrf::require` exempts exactly `POST /api/payments/webhook`. The provider signature is that route's credential.

**Never:**
- No fulfilment, entitlements, subscriptions, renewal, refunds or new order statuses (story 4). No public catalog (story 5). No portal pages change except to show the new 409 text if they do not already.
- No raw event payload stored (only its SHA-256), no card data, no Stripe secret in logs, errors, `Debug` or audit rows.
- Nothing in `cargo test` reaches the network beyond 127.0.0.1. No `sqlx::query!` macros. Never edit an applied migration.
- No order status change outside `verify_event` → `receive` → `apply_event`. The browser's return to `success_url` changes nothing.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Stripe checkout | verified owner, published offering, mock API answers `{id:"cs_test_…",url:"https://checkout.stripe.com/…"}` | 201 `{order, checkout_url}`, order `pending` with `provider=stripe` and that session id; the mock saw the form, headers and idempotency key above | — |
| Stripe API down | mock answers 500, or stays silent beyond the timeout (provider unit test with a short timeout) | 502 `the payment provider could not be reached; try again later` | no order, no audit |
| Paid webhook | signed `checkout.session.completed` fixture (paid, matching amount and order) | 200; order `paid`; one `order.paid` audit row (actor `provider:stripe`, `{event_id}`); event row `applied`; `billing_customers` row | — |
| Replay | the same event again, sequentially or concurrently | 200 each time; still one audit row and one event row; order unchanged | — |
| Out of order | `expired` after `paid`; `completed` (unpaid) then `async_payment_succeeded`; `async_payment_failed` on pending | `unchanged` / pending then `paid` / `expired` with `order.expired` | — |
| Event before its order | paid event whose session has hub metadata but no committed order | 503, nothing stored; after checkout creates that session, the same event → 200 `applied` | Stripe retries |
| Foreign or other event | session without hub metadata, unknown session that expired, `payment_intent.succeeded` | 200; stored `unmatched` / `ignored`; no order change | — |
| Mismatch | paid event with a different `amount_subtotal`, currency, or `wipehub_order_id` | 200; stored `mismatch`; order stays `pending`; error logged | — |
| Bad signature | wrong secret, tampered body, `t` older or newer than 300 s, missing or malformed header | 400 `{error}` | nothing stored |
| Webhook without CSRF | no cookie and no `X-CSRF-Token` | accepted (other POSTs still 403) | — |
| Fake through webhook | Fake-signed event posted to the webhook on a local hub | applied like Stripe's; non-local hub → 503 | — |
| Billing | owner, Stripe, no customer yet / after a paid order | 409 `billing opens after your first payment` / 200 `{url}` from the mock (`customer`, `return_url` sent) | member 403 as in story 2 |
| Stale customer | stored customer `cus_old`; mock answers the first checkout request 400 `{error:{type:"invalid_request_error",code:"resource_missing",param:"customer"}}`, then succeeds; the paid event names `cus_new` | 201; the second request has `customer_creation=always`, no `customer`, and the `-new-customer` idempotency key; after the paid event `billing_customers` holds `cus_new` and billing sends `cus_new` | any other 400 → 502 with no retry |
| Foreign UUID reference | paid event for an unknown session with a UUID `client_reference_id` and no `wipehub_order_id` metadata | 200; stored `unmatched`; nothing else | — |
| Another hub's event | paid event with `wipehub_order_id` metadata for an unknown session, whose `wipehub_hub` is another origin or absent | 200; stored `unmatched`; nothing else (never 503) | — |
| Billing, customer deleted | stored customer; mock answers the billing-portal request 400 `resource_missing`/`param=customer` | 409 `billing opens after your first payment` | any other 400 → 502 |
| Expired with a customer | `async_payment_failed` carrying a `customer`, for a pending order of an organization with no stored customer | order `expired`; no `billing_customers` row; billing still 409 | — |

## Autonomous decisions

1. **Annual offerings are charged once (`mode=payment`) in this story, not as Stripe subscriptions.** *About:* an owner buying an annual offering through Stripe. *Problem:* a subscription would auto-renew and charge the customer next year, but renewal fulfilment is story 4. *Options:* subscription now, or a single payment now. *Choice:* a single payment, which never charges for something the hub cannot yet fulfil. Story 4 decides the recurring mechanism.
2. **Stripe invoices are not turned on.** Stripe bills post-payment invoices for one-off payments as a paid feature, so turning them on is a cost decision for the owner. Stripe's billing portal still manages cards and payment history, and Stripe emails receipts. Invoices arrive with story 4's renewal mechanism. The rest of CAP-5 "invoices" is left as a residual.
3. **The event store keeps event metadata and the SHA-256 of the payload, not the payload.** Stripe payloads carry buyers' names, emails and addresses. Keeping only what idempotency and forensics need is data minimisation (GDPR Art. 5(1)(c), the rule to collect no more personal data than needed), and the Stripe Dashboard retains the full event.
4. **Billing before the first payment is 409.** The hub learns the Stripe customer from the paid event, so it never creates empty Stripe customers and never sends an email address to Stripe.
5. **Unknown sessions.** A paid event for the hub's own order that cannot be found is 503, so Stripe retries for up to 3 days and money never goes unrecorded silently. Every other unmatched event is stored and acknowledged, so foreign events on a shared Stripe account do not retry for days.
6. **Paid events are checked against the order's snapshot** (subtotal, currency, order reference). A mismatch leaves the order pending for a human to look at, rather than granting a different purchase.
7. **The fixtures are hand-built from Stripe's documented event shapes, not recorded.** Recording needs a Stripe account, which is outside the repo. The fixtures live in `tests/fixtures/stripe/` with placeholders, and the README says how to re-record them with `stripe listen --print-json`.
8. **`--stripe-api-base` exists** so tests can point the provider at a local mock. Only https, or http on a local host, is accepted, so a typo cannot send the secret key over plain http.
9. **Only the hub's own metadata marks an event as one of the hub's orders** (review pass 1). *About:* the vendor's Stripe account receiving checkout events. *Problem:* if the account is shared with another app whose checkouts use a UUID as `client_reference_id`, reading that field made every one of that app's paid events a "not ready" 503. Stripe then retries for 3 days and can disable a webhook endpoint that keeps failing, which would cut the hub off from its own payments. *Options:* keep the fallback; read only `metadata[wipehub_order_id]`. *Choice:* metadata only. The hub sets it on every session it creates, so nothing of the hub's is lost, and foreign events are stored as `unmatched` and acknowledged.
10. **A deleted Stripe customer does not lock an organization out** (review pass 1). *About:* an owner buying again, or opening billing, after the vendor deleted that customer in Stripe (for example with "Delete all test data" in test mode). *Problem:* with an insert-only mapping, every later checkout sent the deleted customer, Stripe refused, and the owner got a 502 for good, recoverable only by editing the database. *Options:* leave it; let a newer paid event replace the stored customer and let checkout fall back to a new customer when Stripe says the stored one is missing. *Choice:* the fallback and the replacement. Both are driven only by Stripe's own answer or a verified paid event for that organization's order, so nothing untrusted can rewrite the mapping. Billing for a customer Stripe no longer knows answers the same 409 as before the first payment.
11. **Each checkout names the hub that created it** (review pass 2). *About:* several hubs (for example local development and staging) on one Stripe test account. *Problem:* every hub receives every hub's checkout events, so a paid event for another hub's order looked like this hub's own uncommitted order and got 503 for 3 days, which can get the endpoint disabled. *Options:* a time window on the 503; tag each session with the creating hub's public origin. *Choice:* the tag, `metadata[wipehub_hub]`. The origin is already public, no personal data is sent, and an event from another hub is acknowledged as `unmatched`.

</intent-contract>

## Code Map

- `crates/wipe-hub/src/payments.rs` -- trait (:131), `CheckoutOrder` (:59; add `customer_id: Option<String>`), `ProviderError` (:80; add `NoBillingAccount`), `VerifiedEvent` (:107; add fields and getters), `provider_for` (:157; Stripe replaces `Unavailable`, which goes along with its test and `MSG_PAYMENTS_UNAVAILABLE` if unused), `page_url` (:165), `fake_mac`/HMAC pattern, `pub use crate::orders::apply_event`.
- `crates/wipe-hub/src/orders.rs` -- `create` (:283; look up `billing_customers` for `(org, provider.name())` into `CheckoutOrder`), `apply_event` (:362; return an outcome enum; amount, currency and order-ref checks; customer insert), `lock_by_session` (:249).
- `crates/wipe-hub/src/order_routes.rs` -- `routes()` (:53; add the webhook), `From<ProviderError> for ApiError` (:83; map `NoBillingAccount` to 409), `billing_portal` (:183; read the stored customer first), `fake_finish` (:211; call `payment_events::receive` instead of `apply_event`, keeping the 409 pre-check).
- `crates/wipe-hub/src/csrf.rs:84` -- `require`: the exemption, plus a unit test.
- `crates/wipe-hub/src/config.rs` -- `Args` (:47), `Config` (:144), `ConfigError` (:162), `from_args` (:191), the `Debug` impl (:262), the field-list test (:640-650), the Stripe tests (:432-452). Use the `STRIPE_WEBHOOK_SECRET_ENV` pattern for the new env-name constants. `auth::is_local_host` decides the api-base rule.
- New `crates/wipe-hub/src/stripe.rs` (flat file: `tests/isolation.rs`'s route scanner and `tests/orders.rs`'s status-update guard read `src/*.rs` non-recursively), `src/payment_events.rs`, and `lib.rs` module lines.
- `crates/wipe-hub/Cargo.toml` -- move `reqwest = { workspace = true }` into `[dependencies]` (0.12.28 is already locked; `.form()` and `.json()` are available). Keep it in dev-deps only if needed.
- `Args { … }` literal initializers that need the two new fields: `tests/support/hub.rs:48`, `tests/{team,orders,health,auth,portal}.rs`, `src/config.rs` tests.
- `crates/wipe-hub/tests/orders.rs` -- story 2's "Provider unavailable" Stripe row and the `apply_event` grep guard. Re-point the row to Stripe with a failing mock (502).
- `crates/wipe-hub/tests/isolation.rs:40-145` -- add `("POST", "/api/payments/webhook", Anonymous("the provider's signature is the credential; answers only {received}"))`, and add `billing_customers` to `rows_of` (:262).
- `crates/wipe-hub/tests/support/hub.rs` -- `spawn_hub_args`, `published_offering` (:290), `audited`, `count`.
- `apps/portal/src/pages/OrdersPage.tsx:66` -- `BillingButton` must render a 409's error text (check, and change only if it does not).
- Docs: `README.md:48` (the hub row: Stripe, webhook, event store; a "Stripe test mode" how-to: keys in env, `stripe listen --forward-to localhost:7900/api/payments/webhook`, using the CLI's `whsec_`, re-recording fixtures), `CONTEXT.md` §10.2 (new row; edit the hub row's "`stripe` starts but answers 503"), §14 (**Payment event** entry; **Payment provider** names Stripe), `docs/ROADMAP.md:319` (A1.3 note), `AGENTS.md` (the hub bullet: Stripe needs both secrets).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0012_payment_events.sql` -- the two tables and the trigger above.
- `crates/wipe-hub/src/config.rs` -- the new flags, errors, redaction and tests: missing key, a bad api base (`http://api.example.com`, `ftp://…`), and http on 127.0.0.1 accepted.
- `crates/wipe-hub/src/stripe.rs` -- the `Stripe` provider, signature verification, `signature_header`, event mapping. Unit tests cover: round trip; any of several `v1` matches; wrong secret; tampered body; stale and future `t`; a missing, duplicated or malformed header; each mapped type; unpaid completed → `Ignored`; a `checkout.session.*` without an id → `Rejected`; a timeout against a silent local listener → `Failed`.
- `crates/wipe-hub/src/payments.rs`, `orders.rs`, `payment_events.rs`, `order_routes.rs`, `csrf.rs`, `lib.rs` -- as in the contract.
- `crates/wipe-hub/tests/fixtures/stripe/*.json` -- `checkout.session.completed` (paid), `checkout.session.completed.unpaid`, `async_payment_succeeded`, `async_payment_failed`, `checkout.session.expired`, `payment_intent.succeeded`, with `__EVENT_ID__`, `__SESSION_ID__`, `__ORDER_ID__`, `__ORGANIZATION_ID__`, `__AMOUNT__` and `__CURRENCY__` placeholders.
- `crates/wipe-hub/tests/stripe.rs` -- a mock Stripe API (axum on `127.0.0.1:0` that records method, path, headers and form, with scripted replies), a hub spawned with Stripe pointed at it, and one test per matrix row. Assert responses, stored order, `payment_events` and `billing_customers` rows, and audit rows (count, action, actor, detail).
- `crates/wipe-hub/tests/orders.rs`, `isolation.rs`, `support/hub.rs`, other `Args` literals -- the updates above.
- Docs as in the Code Map.
- Review pass 1 additions:
  - `crates/wipe-hub/src/stripe.rs` -- `Session::order_ref` reads `metadata[wipehub_order_id]` only. On a `resource_missing`/`param=customer` 400, `create_checkout` retries once as the contract says. That needs `post` to expose Stripe's `error.code` and `error.param` to its caller, still never its message. A known-answer unit test: `verify_signature("t=1700000000,v1=13941114bb88ac44a76abcfddea5b92aa6182a4b63d8be3aae908a616083bd7e", br#"{"id":"evt_test"}"#, "whsec_test_secret", 1700000000)` is `Ok`. That value was computed with `openssl dgst -sha256 -hmac`, independently of the hub's code. The module doc must not claim the idempotency key makes the hub's retries safe (the hub does not retry, except the one customer fallback); say what it does: a repeated identical request returns the same session.
  - `crates/wipe-hub/migrations/0012_payment_events.sql` and `src/orders.rs` `apply_event` -- `billing_customers.updated_at` and the replace-only trigger; the upsert `INSERT … ON CONFLICT (organization_id, provider) DO UPDATE SET customer_id = EXCLUDED.customer_id, updated_at = now() WHERE billing_customers.customer_id <> EXCLUDED.customer_id`.
  - `crates/wipe-hub/tests/stripe.rs` -- the three new matrix rows, plus a stripe.rs unit test for `order_ref` that ignores `client_reference_id`. The "Expired with a customer" row runs in a fresh organization, so the assertion can fail. The `out_of_order_events_are_handled` assertion of attempt 1 could not fail, because the organization already had `cus_test_1`.
  - `README.md` "Stripe test mode" -- run `stripe listen` in a second terminal (it stays in the foreground), or capture the secret first with `export WIPE_HUB_STRIPE_WEBHOOK_SECRET=$(stripe listen --print-secret)`. Add a short "production webhook" paragraph: register `https://<hub>/api/payments/webhook`, enable only the four `checkout.session.*` events the hub reads, set the endpoint's API version to `2024-06-20`, and use that endpoint's `whsec_` secret.
  - `CONTEXT.md` §14 **Payment provider** and the `PaymentProvider::billing_portal` doc comment -- "cards and payment history", not "cards, invoices" (decision 2).

- Review pass 2 additions:
  - `crates/wipe-hub/src/stripe.rs` -- `metadata[wipehub_hub]` in the checkout form (the `public_base_url` origin, `Url::origin().ascii_serialization()`); `parse_event` gets this hub's origin, so `order_ref` is `None` unless it matches; `billing_portal` maps `is_missing_customer()` to `NoBillingAccount`. Unit tests: `order_ref` with a matching, a different and a missing `wipehub_hub`; a paid event whose `customer` is blank or longer than 255 characters still verifies with `customer_id() == None`.
  - `crates/wipe-hub/tests/fixtures/stripe/*.json` -- `"wipehub_hub":"__HUB__"` in the metadata; `tests/stripe.rs` substitutes the spawned hub's origin and adds the two new matrix rows.
  - `CONTEXT.md` §10.2 row -- the lock residual says checkout can hold the offering's share lock and a pool connection across two sequential Stripe calls (up to about 30 s with the customer fallback). Drop the "orphaned session's paid event 503s" residual: a checkout whose transaction fails never returns its URL, so the buyer cannot pay that session.

**Acceptance Criteria:**
- Given a hub on Stripe pointed at the mock, when an owner checks out and the paid fixture is delivered twice concurrently, then the order is `paid` with exactly one `order.created` and one `order.paid` audit row and one `payment_events` row.
- Given any `src/` file, when grepped, then `UPDATE orders SET status` appears only in `apply_event`, and `apply_event` is called only from `payment_events::receive`.
- Given `cargo test -p wipe-hub` with compose up, when `isolation` runs, then the webhook route is classified and organization B's rows (including `billing_customers`) are byte-for-byte unchanged.
- Given `wipe-hub serve --payment-provider stripe` with only a webhook secret, when it starts, then it exits non-zero naming `WIPE_HUB_STRIPE_SECRET_KEY`.

## Spec Change Log

- 2026-11-08, implementation choices within the contract (no matrix behaviour changed):
  - `VerifiedEvent` also keeps the provider's own `event_type` (stored in `payment_events.event_type`, e.g. `checkout.session.completed`) and gains `session_id()`/`event_type()` getters. Providers build it through a `pub(crate) VerifiedEvent::new`, which rejects (400) an event id, type or session id that is blank or over 255 characters, so an oversized signed event can never hit the 0012 CHECKs as a 500. Fake events get the same checks.
  - `orders::apply_event` returns `ApplyOutcome::{Applied(Order), Unchanged(Order), Ignored, Unmatched, Mismatch(Order)}`; `payment_events::receive` returns `Receipt::{Duplicate, NotReady, Recorded(Box<ApplyOutcome>)}`. Mismatch is checked only on completed events, and only for the fields the event carries (a Stripe completed event without `amount_subtotal`/`currency` is not amount-checked).
  - `payments::provider_for` returns `anyhow::Result` (building the `reqwest` client can fail); `stripe::Stripe::with_timeouts` exists for the silent-listener unit test. `Unavailable` and `MSG_PAYMENTS_UNAVAILABLE` are removed.
  - `--stripe-api-base` also refuses a user name/password, query or fragment. On a Stripe non-2xx answer only Stripe's `error.type`/`error.code` are logged, never its message (which can quote part of a key).
  - The Stripe mock lives in `tests/support/stripe_mock.rs`, shared by `tests/stripe.rs` and `tests/orders.rs`. The CSRF exemption's unit test builds its near-miss paths from `PAYMENT_WEBHOOK_PATH`, because `tests/isolation.rs` treats every `"/api…"` literal in `src/` as a route.
  - Found, not changed (pre-existing, out of scope): a trailing-slash path under an existing route (`POST /api/checkout/`, `/api/payments/webhook/`) misses the `/api/*rest` JSON 404 and gets the portal fallback (plain-text 404 when the portal is unbuilt). Nothing runs, but it contradicts AGENTS.md's "unknown `/api/*` paths are JSON 404s".
- 2026-11-08, review pass 1 loopback (bad_spec, `review_loop_iteration` 1):
  - **Triggering findings:**
    - `order_ref` fell back to `client_reference_id`, so a shared Stripe account's foreign paid sessions got 503 for 3 days (Blind, Edge Case Hunter).
    - The insert-only `billing_customers` locked an organization out of checkout and billing for good once its Stripe customer was deleted (Blind, Edge Case Hunter).
  - **Amended:** inside the contract, decisions 9 and 10 and only the rules they touch (the `order_ref` source, the checkout customer fallback, `billing_customers` replaceable, the customer upsert), plus three matrix rows. Outside it, the "Review pass 1 additions" tasks, which fold in this pass's trivial fixes: README commands, production webhook notes, idempotency-key wording, "cards and payment history", the expired-with-customer test and a known-answer signature test.
  - **Known-bad states avoided:**
    - a foreign event answered 503 until Stripe disables the endpoint;
    - an organization that can never buy again without database surgery;
    - a test whose assertion cannot fail.
  - **KEEP:**
    - Attempt 1 is saved as `_bmad-output/implementation-artifacts/offerings-and-payments-story-3-attempt-1.patch` and applies cleanly to `baseline_revision` with `git apply`. Start from it.
    - Its code, tests, fixtures and docs met every other rule: 513 workspace tests, clippy `-D warnings` and rustfmt all clean, and every original matrix row covered.
    - Keep its structure: `stripe.rs`, `payment_events.rs`, `ApplyOutcome`/`Receipt`, `PAYMENT_WEBHOOK_PATH` and the CSRF exemption test, `tests/support/stripe_mock.rs`, `VerifiedEvent::new` with its 255-character checks, and redaction of `error.type`/`error.code` only. Also keep the Spec Change Log choices above.
    - Change only what this entry amends.

- 2026-11-08, attempt 2 implementation choices within the amended contract:
  - Before the decision-10 upsert, `orders::store_customer` checks whether the paid event's customer is already stored for another organization (the `(provider, customer_id)` UNIQUE). If so it leaves both rows alone and logs at error, rather than failing the event with a unique violation (a 500 that Stripe would retry for days). Tested by `a_customer_stored_for_another_organization_is_left_alone`.
  - The 0012 trigger allows an UPDATE only when `customer_id` actually changes and `organization_id`, `provider` and `created_at` stay; `updated_at` may change with it.
  - The fallback logs the stale customer id at warn; Stripe's error message is still never logged.

- 2026-11-08, review pass 2 loopback (bad_spec, `review_loop_iteration` 2):
  - **Triggering finding:** with several hubs on one Stripe account, another hub's paid event names an order (`wipehub_order_id`) this hub never had, so it answered 503 for days (Edge Case Hunter).
  - **Amended:** inside the contract, decision 11 and the rules it touches (the `wipehub_hub` metadata, `order_ref` requires it), the billing fallback that decision 10 already promised, and two matrix rows. Outside it, the "Review pass 2 additions" tasks, which fold in this pass's trivial fixes: billing on a deleted customer, the two CONTEXT residuals, and the oversized-customer test.
  - **Known-bad states avoided:**
    - a staging hub's events 503ing on a development hub until Stripe disables the endpoint;
    - billing answering 502 "could not be reached" when Stripe answered that the customer is gone.
  - **KEEP:**
    - Attempt 2 is saved as `_bmad-output/implementation-artifacts/offerings-and-payments-story-3-attempt-2.patch` and applies cleanly to `baseline_revision`. Start from it (`git apply`).
    - It met every other rule: 519 workspace tests, clippy and rustfmt clean, every earlier matrix row covered.
    - Keep everything in it, including its Spec Change Log choice that a customer already stored for another organization is left alone. Change only what this entry amends.

- 2026-11-08, attempt 3 implementation choices within the amended contract:
  - `stripe::hub_origin(&Url)` (public, `Url::origin().ascii_serialization()`, so a default port is omitted) computes the `wipehub_hub` value once in `Stripe::with_timeouts`; `parse_event` compares it byte for byte, so a trailing slash, another port or a non-string value is another hub.
  - A billing-portal `resource_missing`/`param=customer` answer is logged at warn with the stale customer id (never Stripe's message) and changes no row; the next paid event replaces the customer.
  - README "Stripe test mode" says each hub sharing a Stripe account needs a distinct public origin, and lists the `__HUB__` fixture placeholder.

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 28 findings — high 0, medium 4, low 18, false 6, maybe-false 0
- findings:
  - `[low]` `[reject]` A mismatched paid order is only logged, not audited or listed anywhere (Blind) — the spec chose an error log because a mismatch changes no order state. A Stripe-signed event that disagrees with the hub's own price_data needs Dashboard tampering, and the event row (`mismatch`) records it.
  - `[medium]` `[bad_spec]` Insert-only `billing_customers` blocks an organization for good after its Stripe customer is deleted (Blind) — real: every later checkout sends the dead id and gets a 502. Amended: decision 10, checkout fallback and replaceable customer.
  - `[false]` `[reject]` The webhook lets anyone mark any Fake order paid with no session (Blind) — a Fake event must name the checkout session id, 128 random bits shown only to the buyer, and Fake still refuses non-local hubs. Without an owner session there is no new reach beyond story 2's recorded residual.
  - `[medium]` `[bad_spec]` `order_ref` falls back to `client_reference_id`, so foreign paid sessions get 503 for days (Blind) — real on a shared Stripe account, where Stripe can disable the endpoint. Amended: decision 9, metadata only.
  - `[low]` `[bad_spec]` The idempotency-key comment promises retry safety the hub never uses (Blind) — the doc wording is folded into the review pass 1 tasks.
  - `[low]` `[bad_spec]` README runs `stripe listen` in the foreground, so the next commands never run (Blind) — folded into the review pass 1 tasks.
  - `[low]` `[bad_spec]` No production webhook setup notes: events, endpoint API version (Blind) — folded into the review pass 1 tasks.
  - `[low]` `[reject]` A buyer who cancels on Stripe sees "pending" for up to 24 h; no `expires_at` (Blind) — the fix needs a product choice of session length. The order expires through `checkout.session.expired`, as the Fake abandon path did in story 2.
  - `[low]` `[reject]` `apply_event` is still `pub` and the "only `receive` calls it" rule is a grep (Blind) — integration tests call it directly, so `pub(crate)` breaks them. The grep is the spec's acceptance check.
  - `[low]` `[reject]` The webhook is not throttled and keeps axum's default 2 MB body limit (Blind) — the HMAC over at most 2 MB is cheap, and a small limit risks refusing large legitimate events that Stripe then retries forever.
  - `[low]` `[reject]` The `billing_customers` FK and trigger block deleting an organization (Blind) — `orders` (0011) already blocks it identically, and no organization deletion exists.
  - `[low]` `[reject]` A Stripe completed event without `amount_subtotal`/`currency` is not amount-checked (Blind) — every Stripe Checkout Session carries both in payment mode, and the signature proves the payload is Stripe's.
  - `[low]` `[bad_spec]` "cards, invoices" wording is stale given decision 2 (Blind) — folded into the review pass 1 tasks.
  - `[low]` `[reject]` Untested: NotReady via async_succeeded, mismatch then expired, over-long Stripe ids, deleted customer (Blind) — the first three share code paths that are already tested. The last is covered by the new stale-customer matrix row.
  - `[false]` `[reject]` A buyer is charged with no order when the checkout transaction fails after Stripe creates the session (Edge Case Hunter) — the checkout URL is returned only after commit, so a buyer of a rolled-back checkout never reaches Stripe's page.
  - `[medium]` `[bad_spec]` A shared account's UUID `client_reference_id` gets 503 forever (Edge Case Hunter) — same root cause as the Blind `order_ref` row; decision 9.
  - `[low]` `[reject]` A paid event without an amount or order reference is not checked (Edge Case Hunter) — same as the Blind amount row; Fake events legitimately carry neither.
  - `[low]` `[reject]` Two checkouts before the first payment create two Stripe customers, with no log (Edge Case Hunter) — needs two paid checkouts racing before either is applied. Decision 10's replacement now keeps the most recent customer.
  - `[medium]` `[bad_spec]` A deleted or invalid stored customer makes every checkout 502 (Edge Case Hunter) — same root cause as the Blind `billing_customers` row; decision 10.
  - `[low]` `[reject]` An offering below Stripe's minimum charge gets a misleading 502 (Edge Case Hunter) — a vendor-admin configuration error (prices under about 50 cents), and the logged Stripe `error.code` names it.
  - `[false]` `[reject]` An unhandled `checkout.session.*` type without `data.object.id` gets 400 forever (Edge Case Hunter) — Stripe has exactly the four `checkout.session.*` types the hub maps, and each carries the session object.
  - `[low]` `[bad_spec]` The expired-path "no customer stored" assertion cannot fail, because the organization already holds `cus_test_1` (Verification Gap, pre-verified) — a new matrix row runs it in a fresh organization.
  - `[low]` `[bad_spec]` Fixtures are self-signed by the hub's own signer, with no known-answer vector (Intent audit) — an openssl-computed vector is now a task.
  - `[low]` `[reject]` The CAP-5 portal path is not browser-tested and invoices are off (Intent audit) — decision 2 is recorded, and the portal's Billing button is unchanged from story 2.
  - `[false]` `[reject]` The 409 before the first payment is customer-visible behaviour CAP-5 did not state (Intent audit) — decision 4 is recorded, and the error copy follows the pattern of prior stories.
  - `[false]` `[reject]` Replay is detected only by event id (Intent audit) — a different event id for the same session reaches `apply_event` and is `unchanged` by the status guard, which is the intended second line of defence.
  - `[false]` `[reject]` CAP-7 is documentation only (Intent audit) — the SPEC says "Documented … nothing in tests needs it".
  - `[low]` `[reject]` Out-of-order cases are tested only through Stripe fixtures, not Fake (Intent audit) — Fake has no async or out-of-order event types; its repeat path is story 2's tested status guard over the same `apply_event`.

### 2026-11-08 — Review pass (loop 1)
- verdicts: 24 findings — high 0, medium 1, low 17, false 6, maybe-false 0
- findings:
  - `[low]` `[patch]` Billing has no deleted-customer fallback and answers a misleading 502 (Blind) — decision 10 promised it. Folded into the pass 2 tasks: `is_missing_customer()` → 409.
  - `[false]` `[reject]` A race in `store_customer` turns a paid event into a 500 (Blind) — it needs two organizations' paid events naming one Stripe customer, which the hub never creates (each session names its own organization's customer or a new one). Even then, Stripe's retry succeeds.
  - `[low]` `[reject]` Mismatched or unmatched paid events cannot be recovered and nothing lists them (Blind) — carried: pass 1 rejected the same claim (the mismatch is only logged and stored as `mismatch`).
  - `[low]` `[patch]` The CONTEXT lock residual understates two sequential Stripe calls (Blind) — folded into the pass 2 tasks.
  - `[low]` `[patch]` The CONTEXT "orphaned session 503s" residual contradicts pass 1's refutation (Blind) — folded into the pass 2 tasks: the residual is dropped.
  - `[low]` `[reject]` `payload_sha256` is never compared on a duplicate (Blind) — the digest serves forensics against Stripe's retained event; comparing would add a branch for redeliveries Stripe never sends with a different body.
  - `[low]` `[reject]` A failed payment and an expiry look the same in the audit log (Blind) — `payment_events.event_type` keeps the distinction under the `event_id` in the audit detail; portal wording would be a copy decision.
  - `[low]` `[reject]` The Stripe secret key's prefix and mode are not checked (Blind) — a wrong key fails the first checkout with Stripe's logged `error.type`. Prefix rules add validation for a rare operator mistake.
  - `[low]` `[reject]` The concurrent-replay test does not force the two inserts to overlap (Blind) — the primary key guarantees it either way; forcing the overlap needs a two-connection harness.
  - `[low]` `[reject]` The "only `receive` calls `apply_event`" grep can be fooled (Blind) — carried: pass 1 rejected the `pub` and grep-guard claim.
  - `[low]` `[defer]` The trailing-slash `/api` route violation is recorded only in the story (Blind) — pre-existing, not caused by this story; added to `deferred`.
  - `[low]` `[reject]` README test-mode gaps (portal build, base URL, CLI login, endpoint disabling) (Blind) — the README's hub section already covers the portal build; the defaults make `success_url` localhost; the Stripe CLI prompts for login itself.
  - `[low]` `[defer]` AGENTS.md line wrap (Blind) — the fix edits an agent-context file; added to `deferred`.
  - `[medium]` `[bad_spec]` Another hub's paid event on a shared Stripe account 503s for days (Edge Case Hunter) — amended: decision 11, `metadata[wipehub_hub]`.
  - `[false]` `[reject]` Adaptive Pricing makes `currency`/`amount_subtotal` differ, so paid orders show as mismatched (Edge Case Hunter) — Stripe's docs: for sessions created since 2025-03-30 the top-level `currency` and `amount_subtotal` stay in the integration currency, and the local amount is in `presentment_details` (docs.stripe.com/payments/currencies/localize-prices/adaptive-pricing, docs.stripe.com/changelog/basil/2025-03-31/add_presentment_details).
  - `[low]` `[patch]` Billing returns 502 when the stored customer was deleted (Edge Case Hunter) — same root cause as the Blind billing row; folded into the pass 2 tasks.
  - `[false]` `[reject]` The `store_customer` unique race (Edge Case Hunter) — same refutation as the Blind row.
  - `[low]` `[patch]` Dropping an oversized or blank Stripe customer id is untested (Verification Gap, pre-verified) — folded into the pass 2 tasks as a unit test.
  - `[low]` `[reject]` Fixtures are synthetic; only one known-answer signature anchor (Intent audit) — carried: decision 7, and pass 1 added the openssl vector.
  - `[low]` `[reject]` The Stripe API is exercised only through the mock (Intent audit) — CAP-7: "nothing in tests needs" a Stripe account; decision 7.
  - `[low]` `[reject]` The CAP-5 portal surface is untested and invoices are off (Intent audit) — carried: decision 2, the Billing button is unchanged.
  - `[false]` `[reject]` Fake covers less of CAP-3 than the Stripe fixtures do (Intent audit) — carried: Fake has no async or out-of-order event types.
  - `[false]` `[reject]` Repeats are detected by event id only (Intent audit) — carried: the status guard is the intended second line.
  - `[false]` `[reject]` Additions beyond the SPEC (decisions 1, 5, 6, 9, 10) (Intent audit) — recorded autonomous decisions, not defects.

### 2026-11-08 — Review pass (loop 2)
- verdicts: 30 findings — high 0, medium 0, low 24, false 6, maybe-false 0
- findings:
  - `[low]` `[patch]` Leaving Stripe invoices off is a cost decision recorded only in the story, not for the owner (Blind) — fixed: `.scratch/DECISIONS.md` D16, in the AGENTS question shape. It defers rather than contradicts ADR-0007 §4: the billing portal ships, and invoices come with story 4's renewal.
  - `[low]` `[patch]` Charging annual offerings once is a customer-visible product choice with no question raised (Blind) — fixed: `.scratch/DECISIONS.md` D15.
  - `[low]` `[patch]` Live payments are documented with no sales-tax caveat (D14) (Blind) — fixed: one sentence in the CONTEXT row and the README production paragraph.
  - `[low]` `[patch]` README does not mention saving the Stripe billing portal's configuration in the Dashboard (Blind) — fixed: a README "Billing portal" paragraph.
  - `[low]` `[reject]` No runbook for `mismatch` or long-running `NotReady` events (Blind) — carried: the same visibility claim was rejected in passes 1 and 2. The residuals are listed in CONTEXT.
  - `[low]` `[patch]` Stale CONTEXT residuals: "no payments", and pending "until story 3's expiry events" (Blind) — fixed: both rows corrected.
  - `[low]` `[patch]` No §14 glossary term for the provider customer (Blind) — fixed: a **Billing customer** entry, used in the §10.2 row and the README.
  - `[low]` `[defer]` The trailing-slash `/api` bug is tracked only in the story (Blind) — carried defer, already in `deferred`.
  - `[low]` `[defer]` AGENTS.md line wrap (Blind) — carried defer, already in `deferred`.
  - `[low]` `[reject]` Story-file drift: a misplaced triage log, and the Design Notes fixture lacks `wipehub_hub` (Blind) — the fix edits this build's spec. The log placement was corrected as workflow bookkeeping, and the fixtures themselves carry `__HUB__`.
  - `[low]` `[reject]` `payment_events` has no `order_id` column or `session_id` index (Blind) — forensic lookups are rare and join through `orders.provider_session_id`; the fix adds schema.
  - `[low]` `[reject]` Configuration refusals (an unsupported currency, three-decimal amounts) read as "could not be reached" (Blind) — a broader form of pass 1's minimum-charge row; Stripe's `error.code` is logged.
  - `[low]` `[patch]` ROADMAP A1.3 overstates Stripe readiness (Blind) — fixed: mock-only and tax caveats added.
  - `[low]` `[reject]` A paid event for this hub's order lost in a database reset gets 503 until Stripe gives up (Edge Case Hunter) — it needs a development database reset during a live retry window; the order-exists-with-another-session case is impossible because each order has a single idempotency key.
  - `[false]` `[reject]` An expired or failed event arriving before its order commits is lost (Edge Case Hunter) — expiry takes at least 30 minutes and a failure follows completion; checkout commits in milliseconds after the session is created.
  - `[false]` `[reject]` `store_customer` unique race (Edge Case Hunter) — carried false (pass 2).
  - `[low]` `[reject]` A mismatch writes no audit row (Edge Case Hunter) — carried reject (pass 1).
  - `[low]` `[reject]` TRUNCATE is not refused on `payment_events`/`billing_customers` (Edge Case Hunter) — it needs the table owner running TRUNCATE by hand, and the fix adds statement triggers.
  - `[low]` `[reject]` A secret key with internal whitespace starts but fails at runtime (Edge Case Hunter) — keys are trimmed, and Stripe keys have no internal whitespace; the 502 is logged.
  - `[low]` `[reject]` A Fake hub refuses to start on an invalid, unused `WIPE_HUB_STRIPE_API_BASE` (Edge Case Hunter) — it needs an operator to set a Stripe-only variable wrongly on a Fake hub; the contract validates it unconditionally, and the error names it.
  - `[low]` `[reject]` Claim: the "append-only" comment ignores TRUNCATE (Edge Case Hunter) — same as the TRUNCATE row.
  - `[false]` `[reject]` Claim: a customer stored for another organization 500s under concurrency (Edge Case Hunter) — carried false (pass 2).
  - `[low]` `[patch]` The expanded `customer` object branch is untested (Verification Gap, pre-verified) — fixed: a test case asserts `cus_expanded`.
  - `[low]` `[reject]` A Fake hub validates the Stripe API base (Verification Gap, other) — same as the Edge Case Hunter row.
  - `[low]` `[reject]` Stripe's wire format is checked only against self-authored fixtures and the mock (Intent audit) — carried: decision 7, plus the openssl known-answer vector.
  - `[low]` `[reject]` The portal's Billing UI is untested and invoices are off (Intent audit) — carried, now also D16.
  - `[false]` `[reject]` Out-of-order handling covers only the checkout-session family (Intent audit) — refunds and renewals are story 4 (CAP-4), and other types are stored `ignored`.
  - `[false]` `[reject]` Fake covers less of CAP-3 (Intent audit) — carried false.
  - `[low]` `[reject]` CAP-7 steps are unverified (Intent audit) — carried: "nothing in tests needs it".
  - `[false]` `[reject]` Scope beyond the SPEC, and decision 1 against ADR-0007's annual interval (Intent audit) — carried: recorded decisions. The offering's annual interval and 12-month term are unchanged; only the renewal mechanism is story 4's, now D15.

## Design Notes

Webhook handler, in sketch: `event = state.payments.verify_event(&headers, &body)?; tx = begin; match payment_events::receive(&mut tx, &event, &body).await? { Receipt::NotReady => { rollback; 503 } _ => { commit; 200 {"received":true} } }`.

Fixture shape (abridged): `{"id":"__EVENT_ID__","object":"event","api_version":"2024-06-20","type":"checkout.session.completed","data":{"object":{"id":"__SESSION_ID__","object":"checkout.session","mode":"payment","payment_status":"paid","status":"complete","amount_subtotal":__AMOUNT__,"amount_total":__AMOUNT__,"currency":"__CURRENCY__","customer":"cus_test_1","client_reference_id":"__ORDER_ID__","metadata":{"wipehub_order_id":"__ORDER_ID__","wipehub_organization_id":"__ORGANIZATION_ID__"}}},"livemode":false}`. `__CURRENCY__` is lower-case, as Stripe sends it.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including `stripe`, `orders` and `isolation`.
- `cargo test --workspace` -- expected: all pass.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.
- `pnpm --filter portal typecheck && pnpm --filter portal test` -- expected: pass (only if the portal was touched).

## Auto Run Result

Status: done

**Summary:** The hub can now take payments through Stripe. With `--payment-provider stripe` (which now needs both `WIPE_HUB_STRIPE_SECRET_KEY` and `WIPE_HUB_STRIPE_WEBHOOK_SECRET`), checkout creates a Stripe Checkout Session, a single payment priced from the order's snapshot that sends no personal data. Billing opens Stripe's billing portal for the organization's **billing customer**, which is learnt from its first paid event (409 before that). Stripe's events arrive at `POST /api/payments/webhook`, the one route without a CSRF token, where the provider's signature is the credential. Each event is signature-checked (300 s window), stored once by provider and event id (metadata and the payload's SHA-256 only), and applied in the same transaction. So a replayed event changes nothing, and events arriving out of order are handled by the order's status guard. The Fake provider's pay and abandon go through the same event store.

Further safeguards:
- A paid event that disagrees with its order (amount, currency, order id) leaves the order pending and is logged.
- A paid event for this hub's order that is not committed yet gets 503, so Stripe retries.
- Events from another hub or app on the same Stripe account are acknowledged as `unmatched`. Checkout tags each session with the hub's origin, `metadata[wipehub_hub]`.
- A Stripe customer deleted at Stripe falls back to a new customer at checkout, and billing answers 409.

The README documents CAP-7: `stripe listen`, production webhook setup, the billing portal configuration and re-recording fixtures.

**Files changed:**
- `crates/wipe-hub/migrations/0012_payment_events.sql`: `payment_events` (append-only, outcome set once) and `billing_customers` (replace-only), each with its trigger.
- `crates/wipe-hub/src/stripe.rs`: the Stripe provider (API calls with a 5 s connect / 15 s total timeout, idempotency keys, the customer fallback), signature verification and event mapping.
- `crates/wipe-hub/src/payment_events.rs`: `receive`, the only path from a verified event to `apply_event`.
- `crates/wipe-hub/src/payments.rs`, `orders.rs`, `order_routes.rs`, `csrf.rs`, `config.rs`, `lib.rs`, `Cargo.toml`:
  - the trait and event fields, `ApplyOutcome`, billing-customer storage and the mismatch checks;
  - the webhook route and its CSRF exemption;
  - the new Stripe flags;
  - `reqwest` as a runtime dependency (already in the lock file).
- `crates/wipe-hub/tests/stripe.rs`, `tests/support/stripe_mock.rs`, `tests/fixtures/stripe/*.json`: 20 HTTP tests against a local Stripe mock, plus six Stripe-shaped fixtures.
- `crates/wipe-hub/tests/{orders,isolation,cli,auth,health,portal,team}.rs`, `tests/support/{hub,mod}.rs`: the `Args` fields, the webhook route's classification, and `billing_customers` added to the isolation check.
- Docs: `README.md`, `CONTEXT.md` (§10.2 row; §14 **Payment event** and **Billing customer**), `docs/ROADMAP.md` A1.3, `AGENTS.md` (the hub's Stripe flags), and `.scratch/DECISIONS.md` (D15, D16).
- `_bmad-output/implementation-artifacts/offerings-and-payments-story-3-attempt-{1,2}.patch`: the two re-derived attempts.

**Review:** three passes; 82 findings in total, triaged row by row in the log above.
- **Passes 1 and 2:** two spec loopbacks (bad_spec).
  - Pass 1 led to decisions 9 and 10: only hub metadata identifies an order, and a deleted Stripe customer is recoverable.
  - Pass 2 led to decision 11: the hub-origin tag on every session.
  - Each loop also folded in that pass's trivial fixes.
- **Pass 3:** 8 low findings patched, all docs except one test:
  - D15 and D16;
  - the tax caveat and the billing portal configuration step;
  - stale CONTEXT residuals and a new glossary term;
  - the ROADMAP caveat;
  - the expanded-customer test.
- **Deferred (2, low):**
  - a trailing-slash `/api` path gets the portal's 404 (pre-existing);
  - an overlong AGENTS.md line.
- Everything else was rejected, with its reason in the log.

**Follow-up review recommended:** false. The final pass patched 0 high and 0 medium findings (8 low), and the medium findings from passes 1 and 2 were resolved by re-deriving from the amended spec, which pass 3 then reviewed.

**Verification:**
- `cargo test --workspace` (compose up): exit 0, 524 passed, 0 failed.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean.
- `rustfmt --edition 2021 --check` on the touched `.rs` files: clean.
- Matrix test audit: all 15 rows are covered by tests that ran and passed.
- The portal is unchanged (its Billing button already shows the 409 text).

**Residual risks:**
- **Never run against real Stripe.** The fixtures are hand-built from Stripe's documented shapes, and the API is a local mock. Only the signature scheme has an independent anchor, an openssl known-answer vector. Run the README's test-mode steps once before relying on it.
- **Live payments wait on tax (D14).** Annual offerings are charged once with no renewal (D15), and invoices are off (D16). Both await the owner.
- **Checkout holds a lock during Stripe calls.** It holds the offering's share lock and a database connection across up to two Stripe calls (about 30 s at worst).
- **Mismatches and stuck events are logged, not shown.** A paid event that disagrees with its order, and a 503 `NotReady` that never resolves, are visible only in the error log and in `payment_events`.
- **Hubs sharing a Stripe account need distinct public origins.**
