---
title: 'Fulfilment into entitlements, renewal and cancellation'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: 'dd4ee36d1a399f57682a1fb56749b2ffec068460'
review_loop_iteration: 2
followup_review_recommended: false
context:
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
  - '{project-root}/.scratch/offerings-and-payments/SPEC.md'
  - '{project-root}/.scratch/offerings-and-payments/stories/3-stripe-provider-and-verified-idempotent-webhooks.md'
warnings: [oversized]
deferred:
  - summary: >-
      An offering's integer minor units are sent to Stripe as `unit_amount` while the hub and portal display them with ISO 4217 / CLDR digits, and these conventions disagree for some currencies, so a price can display differently from what Stripe charges.
    evidence: |-
      Review pass 1 (Blind). Stripe treats MGA as zero-decimal (ISO says 2) and has special rules for ISK, HUF, TWD and UGX; CLF, UYW and UYI are not Stripe currencies. Offerings accept any `^[A-Z]{3}$` code (story 1), and story 3 sends `price_minor` as `unit_amount` unchanged, so this predates story 4; the story 4 notice's Rust table (ISO) and the portal's Intl `formatPrice` (CLDR) also differ for a few codes such as IQD and MGA. Settled by restricting offering currencies to a supported list or by one currency-exponent table shared by admin input, display and Stripe.
    location: >-
      crates/wipe-hub/src/offering_admin.rs (currency validation); crates/wipe-hub/src/stripe.rs (unit_amount)
    severity: low
  - summary: >-
      A subscription checkout paid by a delayed method (ACH or SEPA debit) may have its subscription events answered 503 for longer than Stripe's 3-day retry window while the order is still pending.
    evidence: |-
      Review pass 2 (Blind), maybe-false. Decision 7 keeps `NotReady` while the named order is `pending`, and a delayed payment keeps it pending for days. Whether Stripe sends `customer.subscription.updated` during that window (rather than only at the incomplete → active change, which coincides with `checkout.session.async_payment_succeeded`) is unverified. Settled by recording a real test-mode ACH subscription checkout's event sequence with `stripe listen --print-json`; if updates do arrive early, store them `unmatched` while the order is pending and the event changes nothing (`renews` true) instead of answering 503.
    location: >-
      crates/wipe-hub/src/payment_events.rs (waits_for_its_order)
    severity: medium (unverified)
---

## Owner decisions

Decided 2026-11-08, answering the halt recorded below. These are binding intent: carry them into `<intent-contract>` when planning.

- **Renewal: (B) Stripe subscriptions** (DECISIONS D15).
  - Annual offerings check out with `mode=subscription` and a yearly price. One-off offerings keep `mode=payment`.
  - Each renewal payment Stripe reports (`invoice.paid` for a renewal) extends the entitlement's term by 12 months from its current end, exactly once per invoice. These events may arrive out of order relative to the checkout events.
  - An owner cancels in Stripe's billing portal. The term runs to its end and is not renewed again.
  - A failed renewal charge is handled by Stripe's retries and dunning emails. If Stripe then cancels the subscription, that is recorded like a cancellation. Neither ever touches a licence already issued.
  - The Fake provider gets "renew now" and "cancel" actions so local tests cover the whole path.
- **Invoices** (D16, settled by D15): subscriptions carry Stripe's invoices at no add-on fee. One-off purchases keep receipts only.
- **The defaults listed below** (fulfilment creates exactly one entitlement; a refund or cancellation never revokes an issued licence; a refunded order's unused seats are withdrawn) are accepted as written.
- **Auto-renewal notice.** Customer-facing copy, shown on the offering's buy page and on the checkout confirmation before payment, verbatim, with `{price}` and `{date}` filled in:

  > **Renews automatically every year at {price} until you cancel.** Your first year runs to {date}. Cancel any time from Billing in your account: your current year still runs to its end, and you will not be charged again. We email you before each renewal.

  Approved by the owner 2026-11-08; ship it verbatim. "We email you before each renewal" depends on Stripe's upcoming-renewal reminder emails, a Stripe dashboard setting that the README's Stripe setup must name.
- **Out of scope here:** a connected station fetching its renewed licence by itself waits on D6 (ADR candidate 0008). Until then, renewed licences reach stations through `licence-delivery` story 2 (renewal reissue). The customer's opt-in hard stop on lapse is ADR-0012, built in `licence-delivery` story 4 and `licence-visibility` story 5.

<intent-contract>

## Intent

**Problem:** A paid order grants nothing: the organization has no record of the seats, features, quota and term it bought, annual offerings are charged once and never renew, and refunds and cancellations are invisible to the hub. The SPEC's success signal ("buys Station — annual, 1 seat … shows one available licence seat") cannot be met, and `licence-delivery` has nothing to issue licences from.

**Approach:** The paid transition creates exactly one **entitlement** for the buying organization in the same transaction. Annual offerings check out as Stripe subscriptions (owner decision D15 (B)); each verified renewal payment extends the term by 12 months, exactly once per invoice; a cancellation (owner cancels in Stripe's portal, or Stripe ends the subscription after failed charges) stops renewal while the term runs out; a full refund is recorded on the order and the entitlement and withdraws its seats. Every new event goes through the existing `verify_event → payment_events::receive → orders::apply_event` path. The Fake provider gains "Renew now" and "Cancel" test actions. The owner-approved auto-renewal notice is shown on the buy page, the Fake checkout page and Stripe's checkout page.

## Boundaries & Constraints

**Always:**
- Migration `0013_entitlements.sql` (never edit 0001–0012):
  - `entitlements`: `id`, `organization_id` → organizations, `order_id` → orders UNIQUE (one per order), `provider`, `subscription_id` null (UNIQUE with `provider`), `payment_ref` null (the first payment's provider reference), `term_start`, `term_end` (> `term_start`), `renews` boolean, `status` `active` | `cancelled` | `refunded`, `renewal_changed_at` null, `cancelled_at`/`refunded_at` tied to status by CHECKs, `created_at`, `updated_at`. Seats, scope, quota, features and term length are read from the order's immutable snapshot, not copied.
  - `entitlement_renewals`: `(provider, invoice_id)` primary key, `entitlement_id`, `previous_term_end`, `term_end`, `created_at`. Append-only (trigger refuses UPDATE and DELETE).
  - An `entitlements` trigger refuses DELETE, any change to `id`, `organization_id`, `order_id`, `provider`, `subscription_id`, `payment_ref`, `term_start`, `created_at`, a `term_end` that decreases, and any status step other than `active → cancelled`, `active → refunded`, `cancelled → refunded`. A `refunded` entitlement never changes again.
  - `orders` gains status `refunded` (`paid → refunded` only, terminal) and `refunded_at` tied to it by a CHECK: drop and re-add the status CHECK, and `CREATE OR REPLACE` the 0011 trigger function with that one extra step.
- **Fulfilment.** When `apply_event` moves an order to `paid` it inserts the entitlement in the same transaction: `term_start = paid_at`, `term_end = paid_at + term_months` (SQL month arithmetic), `subscription_id` and `payment_ref` from the event, `renews = subscription_id IS NOT NULL`, `status = active`, audited `entitlement.granted {order_id, term_end}` (actor `provider:<name>`, subject `entitlement:<id>`). No other path creates an entitlement.
- **Event kinds** (`payments::EventKind` additions; `VerifiedEvent` gains private `subscription_id`, `payment_ref`, `occurred_at: Option<i64>` with getters):
  - `RenewalPaid { subscription_id, invoice_id }`, `RenewalChanged { subscription_id, renews }`, `SubscriptionEnded { subscription_id }`, `Refunded { payment_refs }` (non-empty).
  - Stripe mapping (parse only the fields named, tolerate the rest): `checkout.session.completed`/`async_payment_succeeded` also read `subscription`, `payment_intent` and `invoice` (`payment_ref` = `payment_intent`, else `invoice`). `invoice.paid` with `billing_reason = subscription_cycle` and a subscription id (`subscription`, else `parent.subscription_details.subscription`, decision 9) → `RenewalPaid` with `amount` from `subtotal`/`currency` and `order_ref` from `subscription_details.metadata`, else `parent.subscription_details.metadata` (same `wipehub_hub` rule as decision 11 of story 3); a `subscription_cycle` invoice with neither is `Ignored` and logged at error; any other `invoice.paid` → `Ignored`. `customer.subscription.updated` → `RenewalChanged` with `renews = !(cancel_at_period_end || cancel_at != null)`, `occurred_at` = the event's `created`, `order_ref` from the subscription's `metadata`. `customer.subscription.deleted` → `SubscriptionEnded` (same `order_ref` rule). `charge.refunded` with `refunded = true` → `Refunded` with the charge's non-null `payment_intent` and `invoice`; a full refund naming neither → `Ignored`, logged at warn (decision 11); a partial refund → `Ignored`. A mapped type without its required ids is `Rejected` (400).
  - Fake mapping: `checkout.completed` may carry `subscription_id` (the Fake pay route sends `fake_sub_<uuid>` for an annual order); `subscription.renewed {subscription_id, invoice_id}` → `RenewalPaid`; `subscription.cancelled {subscription_id}` → `RenewalChanged { renews: false }`, `occurred_at` = now.
- **Applying** (`orders::apply_event` stays the only entry from `receive` and the only `UPDATE orders SET status`; entitlement writes live in a new `src/entitlements.rs`, the only `UPDATE entitlements`; outcomes reuse `applied`/`unchanged`/`ignored`/`unmatched`/`mismatch`):
  - `RenewalPaid`: lock the entitlement by `(provider, subscription_id)`. None → `unmatched`. Invoice already in `entitlement_renewals` → `unchanged`. Status not `active` → `unchanged`, logged at error ("a renewal was paid for a cancelled or refunded entitlement; refund it at the provider"). Amount differs from the order snapshot → `mismatch`, logged at error, no change. Otherwise `term_end += 12 months` from its current end, one `entitlement_renewals` row, audited `entitlement.renewed {event_id, invoice_id, previous_term_end, term_end}`.
  - `RenewalChanged`: none → `unmatched`; status not `active`, or `occurred_at` older than `renewal_changed_at` → `unchanged`; `renews` already equal → `unchanged`, but `renewal_changed_at` still advances to the event's time (decision 8; the one write an `unchanged` outcome makes, and it is not audited); otherwise set `renews` and `renewal_changed_at`, audited `entitlement.renewal_stopped` / `entitlement.renewal_resumed {event_id}`.
  - `SubscriptionEnded`: none → `unmatched`; not `active` → `unchanged`; otherwise `status = cancelled`, `renews = false`, audited `entitlement.cancelled {event_id}`. The term runs to its end.
  - `Refunded`: find the entitlement whose `payment_ref` or one of whose renewal `invoice_id`s is among `payment_refs` (provider-scoped). None → `unmatched` (logged at warn). Already `refunded` → `unchanged`. Otherwise entitlement `refunded` (`renews = false`) and its order `refunded`, audited `entitlement.refunded` and `order.refunded {event_id}`.
  - `receive` answers `NotReady` (503, rolled back, provider retries) for `RenewalPaid`/`RenewalChanged`/`SubscriptionEnded` that are `unmatched` but carry an `order_ref`, only while that order can still be paid: no order with that id exists yet, or it is `pending` and no stored payment event for its checkout session has outcome `mismatch` (decision 7). Otherwise (the order `expired`, `refunded`, or pending after a mismatch) the event is stored `unmatched` with 200. Completed checkout events keep story 3's rule.
- **Never touches a licence.** Cancellation, refund and failed renewal change only entitlement and order rows; nothing revokes, shortens or reissues a licence (ROADMAP §0.2).
- **Seats.** `GET /api/entitlements` (any role, own organization only): `{entitlements: [...], available_seats}`. Each entitlement: `id, order_id, offering_name, interval, grants, term_start, term_end, status, renews, current, cancelled_at, refunded_at` (RFC 3339 UTC). `current` = not `refunded` and `term_end > now()`. `available_seats` = the sum of seats over current entitlements (licence registration subtracts from it in `licence-delivery`).
- **Stripe checkout for annual offerings**: `mode=subscription`, `line_items[0][price_data][recurring][interval]=year`, `subscription_data[metadata][wipehub_order_id|wipehub_organization_id|wipehub_hub]` (same values as the session metadata), `custom_text[submit][message]` = the auto-renewal notice (headline + " " + detail), and never `customer_creation` (Stripe refuses it in subscription mode; the story-3 stale-customer fallback for annual drops `customer` only). One-off offerings keep story 3's form unchanged.
- **Auto-renewal notice**, verbatim (owner-approved 2026-11-08): headline `Renews automatically every year at {price} until you cancel.`; detail `Your first year runs to {date}. Cancel any time from Billing in your account: your current year still runs to its end, and you will not be charged again. We email you before each renewal.` One Rust function builds it; `{price}` formatted like the portal's `formatPrice` (`1,200.00 USD`, ISO 4217 minor-unit digits), `{date}` = today (UTC) + 12 months as `8 November 2027`. Annual catalog offerings carry `renewal_notice: {headline, detail}` (null for one-off), and the Fake session read returns `{order, renewal_notice}`; the portal renders the headline in bold on `/buy` and `/checkout/fake` and never composes the copy itself.
- **Fake billing**: `POST /api/payments/fake/entitlements/:id/renew` and `/cancel` (owner, local hub only else 503, 404 under another provider or for another organization's entitlement, 409 `this subscription is not renewing` unless `active` with `renews` and a `subscription_id`) sign the Fake event, verify it and run it through `receive` in one transaction. `/billing/fake` lists the organization's renewing entitlements with "Renew now (test)" and "Cancel (test)" buttons.
- Every new `/api` route is classified in `tests/isolation.rs`, and `entitlements` / `entitlement_renewals` are in its untouched-rows check.

**Never:**
- No licence issuance, station registration, seat consumption, renewed-licence files or lapse policy (`licence-delivery`, D6, ADR-0012). No public catalog (story 5). No tax, coupons, plan switching or quantity changes.
- No hub-initiated Stripe call on an event (no subscription cancel or refund from the hub). No raw payload, card data or secret stored or logged.
- No entitlement change outside a verified event through `receive`. The browser's return from Stripe changes nothing.
- No `sqlx::query!` macros; nothing in `cargo test` reaches beyond 127.0.0.1.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Fake annual purchase | owner buys "Station — annual, 1 seat" and presses Pay | order `paid`; one entitlement, `renews=true`, term 12 months from `paid_at`; `GET /api/entitlements` `available_seats: 1`; `entitlement.granted` audited | — |
| One-off purchase | paid one-off with `term_months=24` | entitlement `renews=false`, term 24 months | — |
| Fake renew now | renewing entitlement; renew pressed twice | each press extends 12 months from the current end (two invoices); a replay of one event changes nothing | not renewing → 409 |
| Fake cancel | renewing entitlement, cancel | `renews=false`, `status` still `active`, seats kept; renew afterwards → 409 | — |
| Stripe annual checkout | annual offering, mock API | form has `mode=subscription`, yearly `recurring`, `subscription_data[metadata]…`, the notice in `custom_text`, no `customer_creation` | stale customer → retried without `customer` |
| Stripe renewal | signed `invoice.paid` (`subscription_cycle`) for a known subscription; then the same invoice under a new event id | term +12 months once; one renewal row; second → `unchanged` | — |
| First invoice | `invoice.paid` with `billing_reason=subscription_create` | `ignored`; no change | — |
| Renewal before checkout | renewal for an unknown subscription whose metadata names this hub's order | 503, nothing stored; after the paid checkout event, the same event → 200, extended | Stripe retries |
| Renewal amount mismatch | `invoice.paid` subtotal ≠ order price | `mismatch`; term unchanged; error logged | — |
| Portal cancel and resume | `subscription.updated` with `cancel_at_period_end` true, then false; then an older `true` event | `renews` false, then true; the older event `unchanged` | — |
| Subscription ended | `customer.subscription.deleted` | `cancelled`, `renews=false`, seats kept until `term_end`; a later renewal for it → `unchanged`, error logged | — |
| Full refund | `charge.refunded` (`refunded=true`) naming the first payment or a renewal invoice | entitlement and order `refunded`; `available_seats` drops by its seats | — |
| Partial or unknown refund | `refunded=false`; or refs matching nothing | `ignored`; `unmatched` (warn) | — |
| Lapsed term | `term_end` in the past | `current=false`, not counted in `available_seats` | — |
| Another organization | member of B reads entitlements; owner of B calls Fake renew or cancel on A's id; member of B calls them | sees only B's; 404; 403 (owner-only route, checked before the lookup, decision 10) | — |
| Subscription event for an order that will never be paid | `customer.subscription.updated`/`deleted` or a renewal naming this hub's order that `expired`, or that stays `pending` after a `mismatch` checkout event | 200; stored `unmatched`; nothing changes | never 503 |
| Stale change after an equal one | `renews=true`; resume at t=200 (`unchanged`), then cancel at t=100 | `renews` stays true; the cancel is `unchanged` | — |
| Newer invoice shape | `invoice.paid` (`subscription_cycle`) with no top-level `subscription` but `parent.subscription_details.{subscription, metadata}` | renewal applied as with the 2024-06-20 shape | neither present → `ignored`, error logged |

## Autonomous decisions

1. **A paid renewal for a cancelled or refunded entitlement does not extend it.** *About:* a subscription that Stripe still charges after the hub recorded its end or refund (for example a vendor refunded a payment in the Dashboard but did not cancel the subscription). *Problem:* extending contradicts the owner-accepted rule that a refund or cancellation stops renewal; ignoring it keeps money for nothing. *Options:* extend; record `unchanged` and log at error for the vendor to refund. *Choice:* the latter, plus README guidance to cancel the subscription whenever refunding one of its payments. It follows the accepted rule and leaves the audit trail intact.
2. **Only a full refund withdraws seats.** A partial refund is a goodwill credit in Stripe's model, so it is stored as `ignored`. The safe default: no customer loses seats over a partial credit.
3. **The hub formats the notice once.** Copy pinned verbatim in one place cannot drift between the portal and Stripe; the portal only renders it.
4. **Portal cancel-at-period-end is tracked as `renews`, ordered by Stripe's event time,** so the portal can say "will not renew" as the notice promises, and a resumed subscription renews again. Only Stripe's subscription end is terminal.
5. **Refund matching uses the stored payment references** (`payment_intent` for one-off, the first invoice and renewal invoices for subscriptions) rather than charge metadata, which Stripe does not reliably copy. A refund arriving before its checkout event is stored `unmatched`: a residual, since a refund needs a paid order first.
6. **The page is "Licence seats" at `/seats`**, linked from home and orders, a functional label in the portal's existing style rather than marketing copy.
7. **A subscription event waits (503) only for an order that can still be paid** (review pass 1). *About:* Stripe sending subscription and renewal events for a hub order. *Problem:* an order that expired after a failed bank payment, or stayed pending after a mismatched checkout, never gets an entitlement, yet its subscription's events named it, so each was answered 503 until Stripe gave up after 3 days, which can get the webhook endpoint disabled (the risk story 3's decisions 9 and 11 removed). *Options:* keep the 503; 503 only while the order is missing or pending without a mismatch. *Choice:* the latter, the safe default: a real out-of-order event still waits for its checkout, and nothing else blocks the endpoint.
8. **A no-op subscription change still advances the ordering clock** (review pass 1). *About:* an owner cancelling and resuming in Stripe's billing portal. *Problem:* a resume that arrives while the hub already shows "renews" changed nothing, so an older cancel arriving afterwards looked newer than anything recorded and stopped renewal, though Stripe is still renewing. *Options:* advance `renewal_changed_at` on every non-stale event; or fetch the subscription from Stripe (a hub-initiated call, which the contract forbids). *Choice:* advance the clock, the standard last-writer-by-event-time rule.
9. **Renewal invoices are read in both Stripe payload shapes** (review pass 1). *About:* the vendor's webhook endpoint, or `stripe listen` in development, rendering events in an API version newer than 2024-06-20. *Problem:* newer versions move an invoice's subscription to `parent.subscription_details`, so a renewal would be ignored silently and the paid year never granted. *Options:* require 2024-06-20 everywhere; read both shapes. *Choice:* read both, and log at error a renewal invoice that has neither, so a lost year is never silent.
11. **A full refund that names no payment is acknowledged, not refused** (review pass 2). *About:* a Stripe account shared with another app that still uses the legacy Charges API. *Problem:* a full refund of such a charge carries neither a payment intent nor an invoice; answering 400 makes Stripe retry it for 3 days and can get the webhook endpoint disabled, the risk story 3's decisions 9 and 11 removed. Every hub checkout produces a payment intent or an invoice, so such a refund can never be the hub's. *Options:* 400; `ignored`. *Choice:* `ignored` with a warn log, as for every other foreign event.
10. **The Fake renew and cancel routes check the owner role before the entitlement** (review pass 1). *About:* a member (not an owner) of organization B calling them on organization A's entitlement. *Problem:* the matrix said 404, but the routes are owner-only, and the project's convention (story 2's Fake checkout routes, `tests/isolation.rs`) refuses a member with 403 before any lookup. *Options:* 404 for members; 403. *Choice:* 403, which reveals nothing about A and matches every other owner-only route; B's owner still gets 404.

</intent-contract>

## Code Map

- `crates/wipe-hub/src/orders.rs` -- `apply_event` (:362; add `paid` → `entitlements::grant`, dispatch new kinds, `Refunded` sets order `refunded`), `ApplyOutcome` (:183), `Order`/`OrderRow`/`select_orders` (:63, :120, :226; add `refunded_at`), `CatalogOffering` (:50; add `renewal_notice`), `catalog` (:241), `create` (:283; pass the notice/annual flag in `CheckoutOrder`).
- `crates/wipe-hub/src/payment_events.rs:44` -- `receive`: generalise the `NotReady` rule to the three subscription kinds.
- `crates/wipe-hub/src/payments.rs` -- `EventKind` (:95), `VerifiedEvent::new` (:131; length checks for the new ids), `CheckoutOrder` (:59), `FakeEvent` (:291) and Fake `verify_event`, `Fake::signed_event` (needs a variant taking the extra fields), Fake constants.
- `crates/wipe-hub/src/stripe.rs` -- `create_checkout` (:247), `Session::from_object` (:486), `parse_event` (:520; add invoice, subscription and charge objects), unit tests module (:567).
- `crates/wipe-hub/src/order_routes.rs` -- `routes()` (:63), `fake_session` (:286), `fake_finish` (:303; send `subscription_id` for annual orders); add `GET /api/entitlements` and the two Fake entitlement routes.
- New `crates/wipe-hub/src/entitlements.rs` (flat file; `tests/isolation.rs` and the `tests/orders.rs` status-update grep scan `src/*.rs` non-recursively), `lib.rs` module line.
- `crates/wipe-hub/migrations/0011_orders.sql` -- read-only reference for the trigger function to replace; `0012_payment_events.sql` style.
- `crates/wipe-hub/tests/support/{hub.rs,stripe_mock.rs}` -- `published_offering` (:290), mock request capture; `tests/stripe.rs`, `tests/orders.rs` (the `UPDATE orders SET status` grep guard; add an `UPDATE entitlements` guard), `tests/isolation.rs:40-145` (route list) and `:279-290` (`rows_of`), `tests/migrations.rs`.
- `crates/wipe-hub/tests/fixtures/stripe/` -- add `invoice.paid.json` (cycle), `invoice.paid.create.json`, `customer.subscription.updated.json`, `customer.subscription.deleted.json`, `charge.refunded.json`, and a subscription-mode completed session, with `__SUBSCRIPTION_ID__`, `__INVOICE_ID__`, `__PAYMENT_INTENT__`, `__CANCEL_AT_PERIOD_END__`, `__CREATED__`, `__REFUNDED__` placeholders beside story 3's.
- `apps/portal/src/api/orders.ts` (+ `orders.test.ts`) -- `Order.status` `refunded`, `refunded_at`, `CatalogOffering.renewal_notice`, fake session shape, `Entitlement` type, guards and calls; `apps/portal/src/pages/{BuyPage,FakeCheckoutPage,FakeBillingPage,OrdersPage}.tsx`, new `SeatsPage.tsx`, `main.tsx` route, `HomePage.tsx` link.
- Docs: `README.md:48` hub row and "Stripe test mode" (:~120-140: the eight webhook events, portal config allowing only cancel-at-period-end, "Send emails about upcoming renewals" under Settings → Billing → Subscriptions and emails, cancel the subscription when refunding), `CONTEXT.md` §10.2 (new row; edit story 2/3 rows' "no fulfilment…" residuals), §14 (**Entitlement**, distinguished from `wipe_license::Feature` entitlements; **Order** gains `refunded`; **Payment event**), `docs/ROADMAP.md:325-327` (note on A1.3), `.scratch/DECISIONS.md` D15 row ("wording for owner approval" → approved).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0013_entitlements.sql` -- tables, CHECKs, triggers and the orders `refunded` step as in the contract.
- `crates/wipe-hub/src/entitlements.rs` -- types, `grant`, renewal/change/end/refund appliers, `list_for_organization` with `available_seats`, the notice builder and price/date formatting with unit tests (USD, JPY, BHD, grouping; the exact notice strings).
- `crates/wipe-hub/src/payments.rs`, `stripe.rs`, `orders.rs`, `payment_events.rs`, `order_routes.rs`, `lib.rs` -- as in the contract. Stripe unit tests: every new mapping, partial refund ignored, missing ids rejected, `order_ref` from `subscription_details.metadata` / subscription `metadata` with the hub check, annual checkout form.
- `crates/wipe-hub/tests/fixtures/stripe/*.json` -- the new fixtures.
- `crates/wipe-hub/tests/entitlements.rs` -- the Fake rows of the matrix and the "Another organization" row through HTTP.
- `crates/wipe-hub/tests/stripe.rs` -- the Stripe rows, asserting entitlement and order rows, `entitlement_renewals`, `payment_events` outcomes and audit rows (count, action, actor, detail).
- `crates/wipe-hub/tests/{orders,isolation,migrations}.rs` -- the grep guard, route classification and row checks, trigger refusals (delete, term decrease, illegal status step, renewal row update).
- `apps/portal/...` -- as in the Code Map; `pnpm --filter portal test` covers the new guards and calls.
- Docs as in the Code Map.
- Review pass 1 additions:
  - `crates/wipe-hub/src/payment_events.rs` -- decision 7: before answering `NotReady` for a subscription kind, read the named order (any organization); `NotReady` only when it is missing, or `pending` with no `payment_events` row for its session whose outcome is `mismatch`. Tests in `tests/stripe.rs`: a `customer.subscription.deleted` and a renewal naming an `expired` order → 200 `unmatched`; one naming a pending order after a `mismatch` checkout event → 200 `unmatched`; the existing "Renewal before checkout" row still 503s.
  - `crates/wipe-hub/src/entitlements.rs` `change_renewal` -- decision 8: when `renews` is already equal and the event is not older, set `renewal_changed_at = greatest(coalesce(renewal_changed_at, event time), event time)` (no audit, outcome `unchanged`); a `RenewalChanged` without `occurred_at` (never from Stripe) leaves `renewal_changed_at` alone rather than stamping `now()`, except the Fake, which always sends one. Test the matrix row "Stale change after an equal one".
  - `crates/wipe-hub/src/stripe.rs` -- decision 9: `invoice.paid` falls back to `parent.subscription_details.subscription` and `.metadata`; a `subscription_cycle` invoice with neither is `Ignored` with an error log. Unit tests for both shapes; a fixture `invoice.paid.parent.json` and a `tests/stripe.rs` test for the matrix row.
  - `crates/wipe-hub/src/entitlements.rs` `refund` -- the `entitlement.refunded` and `order.refunded` audit details carry the matched `payment_ref` (`{event_id, payment_ref}`), so the audit says which payment was refunded.
  - `crates/wipe-hub/migrations/0013_entitlements.sql` -- the entitlements trigger also refuses a `term_end` increase unless `OLD.status = 'active'`, so the schema backs up decision 1; a `tests/migrations.rs` case.
  - `crates/wipe-hub/tests/stripe.rs` -- `a_refund_after_the_subscription_ended_still_refunds`: `customer.subscription.deleted`, then `charge.refunded` naming the first invoice → `applied`, entitlement `refunded` with `cancelled_at` kept, order `refunded`, both audit rows, `available_seats` 0.
  - `apps/portal/src/pages/FakeBillingPage.tsx` -- "Renew now (test)" and "Cancel (test)" only for owners (members see the list only).
  - `apps/portal/src/pages/SeatsPage.test.ts` (new) -- `entitlementState` once per branch (refunded, lapsed, cancelled and current, renewing, active and not renewing), and a server render of the seats summary showing "1" available seat for one current 1-seat entitlement, following `BuyPage.test.ts`.
  - `CONTEXT.md` §10.2 story 4 row residuals and `README.md` "Stripe test mode": a renewing entitlement is not `current` between `term_end` and Stripe's renewal charge (about an hour after the period starts, longer while Stripe retries a failed charge); refunding any one payment of a subscription refunds the whole entitlement and its order (a partial refund keeps the seats); a lost dispute (chargeback) is not mapped, so the vendor refunds or cancels in Stripe after losing one; orders paid before migration 0013 have no entitlement (no backfill; no hub took live payments before it); events rendered in API versions newer than 2024-06-20 are read for renewals (decision 9), but refund matching of subscription payments needs `charge.invoice`, so set the webhook endpoint's API version to 2024-06-20 as story 3's README already says, and run `stripe listen` on an account whose default version is 2024-06-20 or pass `--latest` only knowing that.

- Review pass 2 additions:
  - `crates/wipe-hub/src/stripe.rs` -- decision 11: a full `charge.refunded` with neither `payment_intent` nor `invoice` maps to `Ignored` with a warn log; unit test.
  - `crates/wipe-hub/src/entitlements.rs` -- `renew` logs at error when no entitlement has the subscription ("a paid renewal matches no entitlement; cancel the subscription or refund it at the provider"), so a charged year is never silent; `end` stamps `cancelled_at` from the event's `occurred_at` when present (Stripe's time the subscription ended), else `now()`.
  - `README.md` "Stripe test mode" -- under failed payments, set Stripe's "Manage failed payments" to cancel the subscription once all retries fail (otherwise it stays `past_due`/`unpaid`, no `customer.subscription.deleted` arrives and the hub keeps showing it as renewing); after a `mismatch` on a subscription checkout, cancel that subscription in the Dashboard; test clocks apply only to a customer created on the clock, so either attach a clock customer as the organization's billing customer first or use the Fake provider's "Renew now (test)".
  - `CONTEXT.md` §14 **Entitlement** -- say that `current` (not refunded and the term not ended), not `status = active`, is what makes seats available, so `licence-delivery` gates on it.
  - `crates/wipe-hub/tests/stripe.rs` -- at the end of `a_full_refund_withdraws_the_seats_and_refunds_the_order`, `GET /api/orders/{id}` as a member: `status` `refunded` and a `refunded_at` ending in `Z`.
  - `apps/portal/src/pages/FakeBillingPage.test.ts` (new) -- a server render of one subscription row with `canManage` false shows neither button, and with true shows both (export the row component if needed).

**Acceptance Criteria:**
- Given a local hub on the Fake provider, when an owner buys "Station — annual, 1 seat" and presses Pay, then `/seats` shows one available licence seat and the order's entitlement renews on its term end.
- Given any `src/` file, when grepped, then `UPDATE orders SET status` appears only in `orders::apply_event`, `UPDATE entitlements` only in `entitlements.rs`, and `apply_event` is called only from `payment_events::receive`.
- Given `/buy` with an annual offering, when rendered, then the notice appears verbatim with the price and date filled in; a one-off offering shows none.
- Given compose up, when `cargo test --workspace` runs, then every test passes, including `isolation` with the new routes classified and organization B's entitlement rows unchanged.

## Spec Change Log

- 2026-11-08, implementation choices within the contract (no matrix behaviour changed):
  - Migration 0013 also drops and re-adds `orders_paid_at_check` as `(status IN ('paid', 'refunded')) = (paid_at IS NOT NULL)`: a refunded order keeps its payment time, which 0011's check would have refused. The replaced trigger lets `paid → refunded` change only `status`, `refunded_at` and `updated_at`.
  - Extra entitlement CHECKs beyond the contract's: `renews` only while `active` with a `subscription_id`; `cancelled_at` null while active, set when cancelled, kept by a later refund; the trigger also refuses changing a set `cancelled_at`.
  - `VerifiedEvent::new` takes a `pub(crate) EventFields` struct (customer, amount, order ref, subscription, payment ref, occurred_at) instead of more positional arguments. Kind-required ids (subscription, invoice, payment refs) that are blank or over 255 characters are `Rejected`; the optional `subscription_id`/`payment_ref` on a checkout event that do not fit are dropped, as story 3 does for the customer.
  - `ApplyOutcome::{Applied, Unchanged, Mismatch}` for subscription and refund events carry the entitlement's order (the type is unchanged). A paid `RenewalChanged` with no `occurred_at` stamps `now()`.
  - `CheckoutOrder` gains `renewal_notice: Option<RenewalNotice>`; `CheckoutOrder::is_annual()` selects subscription mode. `Stripe::checkout_form` builds the form so it is unit-tested.
  - The notice's `{price}` uses an ISO 4217 minor-unit table in Rust (0 for JPY and other zero-decimal codes, 3 for BHD/IQD/JOD/KWD/LYD/OMR/TND, 4 for CLF/UYW, else 2). The portal's `formatPrice` uses `Intl`, which agrees for every common currency (CLDR differs from ISO for a few, for example IQD).
  - `wipe-hub` gains `time` (already a workspace dependency, locked) for the notice date.
  - The Fake renew/cancel routes answer `{entitlement}` after the change. Their access order mirrors the Fake checkout routes: owner first (a member gets 403 `only an organization owner can manage billing`, so the matrix's "member of B calls Fake renew on A's id" is 403 and B's owner gets the 404), then 404 under another provider, then 503 off a local hub.
  - Fixtures: story 3's checkout-session fixtures now carry `__PAYMENT_INTENT__` instead of a literal `pi_test_1`; `charge.refunded.json` also has `__AMOUNT_REFUNDED__`. The tests' `fill` helper gives every story 4 placeholder a default.
  - `tests/stripe.rs`'s `shop()` adds a one-off offering: story 3's "documented request" test now checks out the one-off (its form is unchanged), and the stale-customer test runs on the annual offering (only `customer` dropped) with a new one-off variant (`customer_creation=always` added).
  - Portal: the notice is rendered by `RenewalNoticeText`; `BuyPage.test.ts` server-renders `OfferingCard` to check the notice appears verbatim (headline in `<strong>`) for an annual offering and not for a one-off.

- 2026-11-08, review pass 1 loopback (bad_spec, `review_loop_iteration` 1):
  - **Triggering findings:**
    - an equal-state subscription change did not advance the ordering clock, so an older cancel arriving later stopped renewal (Blind, Edge Case Hunter);
    - subscription events naming an order that expired or stayed pending after a mismatch were answered 503 until Stripe gave up (Blind, Edge Case Hunter, Intent Alignment);
    - renewal invoices in API versions newer than 2024-06-20 carry the subscription under `parent`, so renewals would be ignored silently (Edge Case Hunter);
    - the matrix's 404 for a member conflicted with the owner-only convention (Intent Alignment).
  - **Amended:** inside the contract, decisions 7–10 and only the rules they touch (the Stripe `invoice.paid` mapping, the `RenewalChanged` rule, the `NotReady` rule) plus four matrix rows (decide-by-default). Outside it, the "Review pass 1 additions" tasks, which fold in this pass's small fixes: the refund audit's payment reference, the trigger's term rule, owner-only Fake buttons, the refund-after-cancel test, the portal state tests, and the documented residuals.
  - **Known-bad states avoided:**
    - a renewing subscription shown as "not renewing" after an out-of-order delivery;
    - a webhook endpoint disabled by days of 503s for an order that can never be paid;
    - a paid renewal year never granted because Stripe's payload shape moved.
  - **KEEP:**
    - Attempt 1 is saved as `_bmad-output/implementation-artifacts/offerings-and-payments-story-4-attempt-1.patch` and applies cleanly to `baseline_revision` with `git apply`. Start from it.
    - It met every other rule: 556 workspace tests, `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` (the full form fails on a pre-existing `wipe-common` lint) and rustfmt clean, portal typecheck, tests and build passing, every original matrix row covered.
    - Keep its structure and the choices in the Spec Change Log entry above: `entitlements.rs` with `grant`/`renew`/`change_renewal`/`end`/`refund`/`list`, `EventFields`, `Refund::{Done, Order}`, the notice builder and its tests, `Stripe::checkout_form`, `RenewalNoticeText`, `BuyPage.test.ts`, the fixtures and `fill` defaults, the one-off/annual split in `tests/stripe.rs`, and the 0013 migration's extra CHECKs.
    - Change only what this entry amends.

- 2026-11-08, attempt 2 implementation choices within the amended contract (started from the attempt 1 patch):
  - Decision 7: `payment_events::waits_for_its_order` reads the named order by id in any organization and checks for a `mismatch` among the `payment_events` rows with the order's own provider and session. A completed checkout keeps story 3's rule unchanged.
  - Decision 8 supersedes the attempt 1 choice "a paid `RenewalChanged` with no `occurred_at` stamps `now()`": an event without a time now leaves `renewal_changed_at` as it was, both when it applies and when it is equal.
  - Decision 9: when an invoice has both shapes, the top-level `subscription` wins; `order_ref` comes from `subscription_details.metadata` and falls back to `parent.subscription_details.metadata` when the first names no order of this hub. A `subscription_cycle` invoice with neither subscription is `Ignored` (logged at error with the event and invoice ids) rather than `Rejected`, so Stripe does not retry it.
  - Refund audit: `payment_ref` is the entitlement's first payment when that matched, else the earliest matching renewal invoice. `entitlements::Refund::Order` became `{order_id, payment_ref}` so `apply_event` can put the same reference in `order.refunded`.
  - Portal: the available-seats line is a separate exported `SeatsSummary`, so `SeatsPage.test.ts` server-renders it without a router (the entitlement table's links need one). `/billing/fake` reads `me` to show the buttons to owners only.

- 2026-11-08, attempt 2 implementation choices (recorded by the implementer above this entry) are kept.
- 2026-11-08, review pass 2 loopback (bad_spec, `review_loop_iteration` 2):
  - **Triggering finding:** a full `charge.refunded` naming neither a payment intent nor an invoice was refused with 400, so a foreign legacy charge on a shared Stripe account is retried for days (Blind). The contract's "a mapped type without its required ids is Rejected" caused it.
  - **Amended:** inside the contract, decision 11 and the one `charge.refunded` mapping clause (decide-by-default). Outside it, the "Review pass 2 additions" tasks, which fold in this pass's small fixes: the unmatched-renewal error log, the cancellation time, three README paragraphs, the CONTEXT `current` sentence and two tests.
  - **Known-bad states avoided:** a webhook endpoint disabled by days of 400s for another app's refund; a charged renewal year that matches nothing and leaves no trace; a vendor whose failed-payment setting silently keeps a lapsed subscription "renewing".
  - **KEEP:**
    - Attempt 2 is saved as `_bmad-output/implementation-artifacts/offerings-and-payments-story-4-attempt-2.patch` and applies cleanly to `baseline_revision` with `git apply`. Start from it.
    - It met every other rule: 561 workspace tests, `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` and rustfmt clean, portal typecheck, 128 tests and build passing, every matrix row covered (including decisions 7–10).
    - Keep everything in it, including both Spec Change Log entries of implementation choices. Change only what this entry amends.

- 2026-11-08, attempt 3 implementation choices (started from the attempt 2 patch; only the review pass 2 additions changed):
  - Decision 11: the full-refund-without-refs branch logs at warn with the event and charge ids and maps to `Ignored`; `tests/stripe.rs` `a_partial_or_unknown_refund_changes_nothing` now expects 200 `ignored` (it asserted the superseded 400), and a `stripe.rs` unit test covers the mapping.
  - `renew`'s unmatched error log also fires while `receive` answers 503 for a renewal waiting on its order, so each Stripe retry of such an event logs once; `receive` already logs the 503 at error, so nothing new is silent or misleading.
  - `end` stamps `cancelled_at = coalesce(to_timestamp(occurred_at), now())`; Stripe's `customer.subscription.deleted` already carries the event's `created` as `occurred_at`. `a_subscription_stripe_ends_keeps_its_seats_until_the_term_ends` delivers it an hour late and asserts the stored time.
  - README test-clock paragraph: the clock customer is attached by a local `UPDATE billing_customers` (allowed by 0012's replace-only trigger) after one purchase, then the annual offering is bought again; or Fake "Renew now (test)".
  - `FakeBillingPage.tsx` exports `Subscription` for `FakeBillingPage.test.ts`, rendered inside a `QueryClientProvider`.
  - Verified: 562 workspace tests, `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` and rustfmt clean, portal typecheck, 130 tests and build passing.

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 35 findings — high 0, medium 5, low 24, false 6, maybe-false 0
- findings:
  - `[medium]` `[bad_spec]` An equal-state `RenewalChanged` does not advance `renewal_changed_at`, so a later-delivered older opposite event applies (Blind) — real: resume@200 `unchanged`, then cancel@100 passes `coalesce(100 < NULL, false)`. Amended: decision 8, matrix row, task.
  - `[medium]` `[bad_spec]` Subscription events naming an order that expired or stayed pending after a mismatch get 503 forever (Blind) — real: `waits_for_its_order` checks only `order_ref`. Amended: decision 7, matrix row, task.
  - `[low]` `[bad_spec]` A renewing entitlement is not `current` between `term_end` and Stripe's renewal charge (Blind) — real for about an hour per renewal, longer in dunning; a grace rule is a product choice for `licence-delivery`, so documented as a residual in the review pass 1 tasks.
  - `[low]` `[bad_spec]` Refunding one payment refunds the whole entitlement and order, and the audit does not say which payment (Blind) — this is the contract's accepted rule (a partial refund keeps seats); the audit detail gains `payment_ref` and the rule is documented, both in the review pass 1 tasks.
  - `[low]` `[bad_spec]` Lost disputes (chargebacks) are not mapped or documented (Blind) — real gap outside CAP-4's "refund or cancellation"; mapping it widens scope, so README guidance and a residual are in the review pass 1 tasks.
  - `[low]` `[defer]` Currency minor-unit conventions differ between ISO (Rust notice), CLDR (portal) and Stripe (Blind) — the Stripe mismatch predates this story (story 3 sends `price_minor` as `unit_amount`); recorded in `deferred`.
  - `[low]` `[reject]` The notice's `{date}` (today + 12 months) can be earlier than the real `term_end` (paid later) (Blind) — the term is never shorter than promised, only longer; changing the approved copy needs the owner.
  - `[low]` `[reject]` A renewal without an amount extends without a price check; Fake renewals carry none (Blind) — every Stripe invoice carries `subtotal` and `currency`, and Fake is local-only; same reasoning as story 3's rejected amount row.
  - `[low]` `[bad_spec]` The migration does not refuse a `term_end` increase on a cancelled entitlement, nor tie organization/provider to the order (Blind) — the first is a one-line trigger rule folded into the review pass 1 tasks; the ties are unreachable because `grant` copies both from the order in the same statement.
  - `[low]` `[reject]` Orders paid before 0013 never get an entitlement (Blind) — no hub has taken live payments (live keys wait on D14); dev hubs can buy again. Documented as a residual in the review pass 1 tasks.
  - `[low]` `[reject]` `available_seats` sums seats of different scopes and features (Blind) — the contract defines it as a total for the SPEC's success signal, and each entitlement's own seats and grants are listed beside it for `licence-delivery`.
  - `[low]` `[bad_spec]` Untested portal state labels and pages; Fake lacks refund, end and resume actions (Blind) — the label and seats tests are folded in (same as the verification-gap row); extra Fake actions are beyond the owner's "renew now and cancel".
  - `[medium]` `[bad_spec]` Equal-state change does not advance the clock (Edge Case Hunter) — same root cause as the Blind row; decision 8.
  - `[false]` `[reject]` A `RenewalChanged` without `occurred_at` stamps `now()` into the ordering column (Edge Case Hunter) — Stripe events always carry `created` and Fake always sets it; the review pass 1 task still drops the `now()` fallback for clarity.
  - `[medium]` `[bad_spec]` NotReady loop for a mismatched or expired order (Edge Case Hunter) — same root cause as the Blind row; decision 7.
  - `[low]` `[reject]` A paid renewal delivered after the subscription's deletion is not granted (Edge Case Hunter) — needs the renewal event delayed past a later deletion; decision 1's error log tells the vendor to refund, and comparing provider and hub clocks adds state for a rare case.
  - `[low]` `[reject]` A refund arriving before its checkout event stays `unmatched` (Edge Case Hunter) — decision 5's recorded residual: a refund needs a paid order, whose event Stripe delivers within seconds.
  - `[medium]` `[bad_spec]` Newer-version invoices carry the subscription under `parent`, so renewals are silently ignored (Edge Case Hunter) — real for webhook endpoints or `stripe listen` on newer API versions. Amended: decision 9, matrix row, task.
  - `[false]` `[reject]` An oversized checkout `subscription_id` is dropped, making a non-renewing entitlement (Edge Case Hunter) — Stripe subscription ids are about 28 characters; the 255 limit is never reached by a Stripe-signed event.
  - `[low]` `[reject]` The notice date is one day earlier than the actual `term_end` (Edge Case Hunter) — same as the Blind date row: the customer never gets less than promised.
  - `[false]` `[reject]` Claim: the Stripe page shows a date that differs from the granted term (Edge Case Hunter, low confidence) — the date shown is a lower bound of the term, never later than `term_end`.
  - `[low]` `[bad_spec]` `/billing/fake` shows Renew and Cancel to members, which always 403 (Edge Case Hunter) — real cosmetic issue on the local test page; owner-only buttons are in the review pass 1 tasks.
  - `[low]` `[bad_spec]` No webhook test refunds a cancelled entitlement (Verification Gap, pre-verified) — `a_refund_after_the_subscription_ended_still_refunds` is in the review pass 1 tasks.
  - `[low]` `[bad_spec]` `entitlementState` has no test (Verification Gap, pre-verified) — `SeatsPage.test.ts` is in the review pass 1 tasks.
  - `[low]` `[bad_spec]` The acceptance criterion's `/seats` page is tested only through the API (Intent Alignment) — the seats render test is in the review pass 1 tasks.
  - `[false]` `[reject]` The `/buy` notice test feeds text copied into the TS test rather than the hub's output (Intent Alignment) — by decision 3 the portal only renders what the hub sends; the Rust tests pin the copy, and the portal test checks rendering and bolding.
  - `[low]` `[reject]` `/checkout/fake` notice rendering is untested (Intent Alignment) — it reuses `RenewalNoticeText`, which `BuyPage.test.ts` covers; the session API test covers the notice field.
  - `[low]` `[reject]` Notice date versus term start (Intent Alignment) — same as the Blind date row.
  - `[low]` `[bad_spec]` Member gets 403 where the matrix said 404 (Intent Alignment) — the contract conflicted with the owner-only convention; decision 10 and the matrix row settle it.
  - `[low]` `[reject]` The renewal-for-cancelled error log is not asserted (Intent Alignment) — logs are not part of the tested surface elsewhere in the hub; the `unchanged` outcome is asserted.
  - `[false]` `[reject]` The notice's promises are kept only through Stripe dashboard settings (Intent Alignment) — that is the owner's decision: the README must name the setting, and it does.
  - `[low]` `[reject]` Refund semantics follow reading B1 (whole entitlement) (Intent Alignment) — same as the Blind refund row; documented.
  - `[false]` `[reject]` The hub's term can run later than Stripe's period when the checkout event was delayed (Intent Alignment) — renewals extend from `term_end`, so the customer never loses time; nothing goes wrong.
  - `[low]` `[reject]` The Fake routes answer 409 for a non-Fake entitlement where 404 might be implied (Intent Alignment) — "404 under another provider" refers to the hub's configured provider, which is implemented; a Stripe entitlement on a Fake hub is not renewing through Fake, which is what 409 says.
  - `[low]` `[reject]` A subscription event for a never-paid order 503s with no test (Intent Alignment) — same root cause as the Blind NotReady row; decision 7.

### 2026-11-08 — Review pass
- verdicts: 31 findings — high 0, medium 3, low 22, false 5, maybe-false 1
- findings:
  - `[low]` `[bad_spec]` If Stripe's failed-payment setting leaves a subscription `past_due`/`unpaid`, no deletion arrives and the entitlement keeps `renews=true` (Blind) — real, depends on a Dashboard setting the README did not name; the README instruction is in the review pass 2 tasks.
  - `[medium]` `[bad_spec]` A paid renewal matching no entitlement is stored `unmatched` without any log, and a mismatched subscription checkout leaves a live subscription charging (Blind) — real: `renew` returns `Unmatched` silently and decision 7 now answers 200. Error log and README instruction in the review pass 2 tasks.
  - `[low]` `[reject]` `cancel_at` set beyond the current period is shown as "not renewing" (Blind) — the README limits the billing portal to cancel at period end; a custom cancel date is a vendor Dashboard action, the term is still extended by `renew`, and comparing with `current_period_end` adds parsing for a rare case.
  - `[maybe-false]` `[defer]` Delayed-payment (ACH, SEPA) subscription events could be answered 503 beyond Stripe's retry window (Blind) — unverified whether Stripe sends subscription updates before the async payment settles; recorded in `deferred` with how to settle it.
  - `[medium]` `[bad_spec]` A full refund naming neither payment intent nor invoice is refused with 400 and retried for days (Blind) — real for legacy Charges API charges on a shared account; the contract's rejection rule caused it. Amended: decision 11.
  - `[low]` `[bad_spec]` README test-clock advice cannot work as written (Blind) — real: clocks apply only to customers created on them; corrected wording in the review pass 2 tasks.
  - `[low]` `[reject]` `entitlements` and `entitlement_renewals` have no TRUNCATE trigger (Blind) — the same holds for `orders` and `payment_events` (0011, 0012); only the audit log refuses TRUNCATE, and TRUNCATE needs the table owner.
  - `[low]` `[bad_spec]` Whether `status = active` or `current` gates seats is ambiguous for `licence-delivery` (Blind) — a CONTEXT §14 sentence in the review pass 2 tasks.
  - `[low]` `[reject]` `entitlement.granted` has no `event_id` (Blind) — it carries `order_id`, and the same transaction's `order.paid` row carries the `event_id`, so the grant is traceable.
  - `[low]` `[bad_spec]` `end` stamps `cancelled_at = now()` instead of the event's time (Blind) — a late delivery records the wrong cancellation time; the one-expression fix is in the review pass 2 tasks.
  - `[false]` `[reject]` The `paid → refunded` order update is unlocked and its row count unchecked, so an audit row can claim a refund that changed nothing (Blind) — `refund` locks the entitlement `FOR UPDATE` first, and an order whose entitlement is `active` or `cancelled` is always `paid`, so the guarded UPDATE always matches; only this path refunds an order.
  - `[low]` `[bad_spec]` A `past_due`/`unpaid` subscription keeps `renews=true` (Edge Case Hunter) — same root cause as the Blind failed-payment row; README instruction.
  - `[low]` `[reject]` `cancel_at` beyond the next period (Edge Case Hunter) — same as the Blind `cancel_at` row.
  - `[low]` `[reject]` Two opposite changes in the same second, delivered out of order, apply in arrival order (Edge Case Hunter) — Stripe's `created` has one-second resolution and a cancel and resume within one second is not a human action; documented by the implementer.
  - `[medium]` `[bad_spec]` A paid renewal for a non-renewing paid order matches nothing and is not logged (Edge Case Hunter) — same root cause as the Blind silent-renewal row.
  - `[false]` `[reject]` carried: An oversized checkout `subscription_id` is dropped (Edge Case Hunter) — carried from pass 1: Stripe subscription ids are about 28 characters.
  - `[low]` `[reject]` carried: A refund arriving before its checkout event stays `unmatched` (Edge Case Hunter) — carried from pass 1: decision 5's recorded residual.
  - `[low]` `[reject]` An entitlement without a payment reference cannot be refunded and nobody is told (Edge Case Hunter) — every Stripe checkout session carries a payment intent (payment mode) or an invoice (subscription mode); Fake has no refunds. A warn branch adds code for a state Stripe does not produce.
  - `[false]` `[reject]` Two orders' paid events naming one subscription cause a unique violation (Edge Case Hunter) — every subscription-mode Checkout Session creates its own subscription, and Fake ids are random UUIDs, so two orders never share one.
  - `[low]` `[reject]` carried: The notice's date can differ from `term_end` (Edge Case Hunter) — carried from pass 1: never earlier than the real end; copy changes need the owner.
  - `[low]` `[bad_spec]` A refunded order's `status` and `refunded_at` are never read back through `GET /api/orders/:id` (Verification Gap, pre-verified) — test in the review pass 2 tasks.
  - `[low]` `[reject]` carried: `/checkout/fake` notice rendering has no test (Verification Gap) — carried from pass 1's intent-alignment row: the shared `RenewalNoticeText` is covered by `BuyPage.test.ts`.
  - `[low]` `[bad_spec]` The owner-only Fake billing buttons have no test (Verification Gap, pre-verified) — `FakeBillingPage.test.ts` in the review pass 2 tasks.
  - `[low]` `[reject]` The notice reaches Stripe as plain text, so the bold headline is lost (Intent Alignment) — Stripe's `custom_text` takes plain text; the wording is verbatim and the portal pages bold it.
  - `[low]` `[reject]` carried: `/checkout/fake` notice untested (Intent Alignment) — same as the carried verification-gap row.
  - `[low]` `[reject]` carried: Notice date versus the real term end (Intent Alignment) — carried from pass 1.
  - `[false]` `[reject]` carried: "We email you before each renewal" depends on a Stripe setting (Intent Alignment) — carried from pass 1: the owner's decision is that the README names it.
  - `[low]` `[bad_spec]` A failed renewal charge has no event mapping (Intent Alignment) — by the owner's decision only Stripe's cancellation is recorded; the README failed-payments instruction makes that cancellation happen.
  - `[low]` `[reject]` The Fake provider does not drive refunds, subscription end or mismatches (Intent Alignment) — the owner asked for "renew now" and "cancel"; the rest is covered locally through the Stripe mock and fixtures.
  - `[low]` `[reject]` The portal does not show invoices (Intent Alignment) — D16 puts invoices in Stripe's billing portal, which Billing opens; `/billing/fake` correctly says the test provider has none.
  - `[false]` `[reject]` "Never touches a licence" holds only trivially (Intent Alignment) — no licence exists in the hub yet, so nothing can go wrong; `licence-delivery` owns the rule from issuance on.

### 2026-11-08 — Review pass
- verdicts: 30 findings — high 0, medium 0, low 27, false 3, maybe-false 0
- findings:
  - `[low]` `[bad_spec]` carried: A renewing entitlement loses its seats between `term_end` and Stripe's renewal charge (Blind) — carried from pass 1; already documented as a residual in CONTEXT §10.2 and the README, nothing further.
  - `[low]` `[reject]` `/seats` shows "Ended on …" for an active, renewing entitlement past its term end (Blind) — only during the renewal window or Stripe's retries of a failed card; a new label branch adds UI state for a transient case, and the residual is documented.
  - `[low]` `[reject]` Refunds of subscription payments match only through `charge.invoice`, which newer API versions drop (Blind) — documented residual: the README pins the webhook endpoint to 2024-06-20; storing every invoice's payment intent adds schema for a configuration the README forbids.
  - `[low]` `[reject]` carried: Two subscription changes in the same second apply in arrival order (Blind) — carried from pass 2.
  - `[low]` `[reject]` A charged year the hub refuses is only logged, with no admin recovery view (Blind) — vendor-console views are ROADMAP A1.3 and `licence-delivery` CAP-4, outside CAP-4 of this epic; the error logs name the action.
  - `[low]` `[reject]` Vendor admins cannot see an organization's entitlements (Blind) — same: A1.3 / `licence-delivery` CAP-4 admin pages; not in this story's intent.
  - `[low]` `[reject]` carried: `available_seats` sums seats across scopes (Blind) — carried from pass 1.
  - `[false]` `[reject]` `payment_ref` is not unique, so a refund could pick an arbitrary entitlement (Blind) — each entitlement's `payment_ref` is its own checkout's payment intent or first invoice, which Stripe never shares between two Checkout Sessions; Fake sets none.
  - `[low]` `[reject]` `refunded_at` is the hub's receipt time, unlike `cancelled_at` (Blind) — matters only for a refund webhook delivered late; the fix adds parsing and SQL for a timestamp the audit row already records.
  - `[low]` `[reject]` carried: The notice's date versus the real term (Blind) — carried from pass 1.
  - `[low]` `[patch]` The `entitlements.rs` module doc says every change is audited, but advancing the ordering clock is not (Blind) — doc and migration header narrowed to grants, status, term and `renews` changes (implementer re-engaged).
  - `[low]` `[reject]` The Fake provider cannot reach `cancelled` or resume, and `subscription.cancelled` names a renewal stop (Blind) — the Fake actions are the owner's "renew now" and "cancel" (carried from pass 2); the event name is internal and documented at its mapping.
  - `[low]` `[reject]` carried: A refund processed before its checkout event (Edge Case Hunter) — carried from pass 1.
  - `[low]` `[reject]` An annual order's paid event without a subscription id grants a non-renewing entitlement (Edge Case Hunter) — every subscription-mode Checkout Session carries `subscription`; the related oversized-id row is carried false, and its later renewals are now logged at error.
  - `[low]` `[reject]` carried: A renewal invoice without an amount skips the price check (Edge Case Hunter) — carried from pass 1.
  - `[low]` `[reject]` carried: Same-second opposite changes (Edge Case Hunter) — carried from pass 2.
  - `[low]` `[reject]` `/billing/fake` lists Stripe entitlements after a hub switches providers (Edge Case Hunter) — a local development edge; the buttons are refused with 409 and nothing changes.
  - `[low]` `[patch]` No test pins entitlements newest first (Verification Gap, pre-verified) — `entitlements_are_listed_newest_first` added (implementer re-engaged).
  - `[low]` `[reject]` The `/billing/fake` renewing filter is untested (Verification Gap, filed defer) — caused by this story so not deferrable; a local test page whose wrong buttons the hub refuses with 409, and testing it needs extracting the filter.
  - `[low]` `[reject]` carried: `/checkout/fake` and Stripe display of the notice untested (Intent Alignment) — carried from passes 1 and 2.
  - `[low]` `[reject]` carried: The bold headline is lost on Stripe (Intent Alignment) — carried from pass 2.
  - `[low]` `[reject]` carried: Notice date versus term (Intent Alignment) — carried from pass 1.
  - `[false]` `[reject]` carried: Portal configuration and reminder emails rest on README instructions (Intent Alignment) — carried from pass 1.
  - `[low]` `[reject]` Nothing ran against real Stripe (Intent Alignment) — no Stripe account is in the repo (story 3 decision 7); recorded as a residual.
  - `[low]` `[reject]` The success signal is tested through the API and a component render, never joined in a browser (Intent Alignment) — the repo has no browser harness for the portal; recorded as a residual as in stories 2 and 3.
  - `[low]` `[reject]` Pressing the Fake buttons is untested (Intent Alignment) — the routes are covered over HTTP and the buttons only call them.
  - `[false]` `[reject]` carried: "Never touches a licence" holds trivially (Intent Alignment) — carried from pass 2.
  - `[low]` `[reject]` carried: Refunding any payment refunds the whole order (Intent Alignment) — carried from pass 1.
  - `[low]` `[bad_spec]` carried: A failed charge stops renewal only after Stripe cancels (Intent Alignment) — carried from pass 2; the README failed-payments instruction is in place.
  - `[low]` `[reject]` The Stripe checkout test builds its expected notice from the function under test (Intent Alignment) — the unit test `the_notice_is_the_approved_copy` pins the exact words; the integration test checks wiring.

## Design Notes

Event flow for one annual Stripe purchase: `checkout.session.completed` (paid, `subscription=sub_1`, `invoice=in_0`) → order `paid` + entitlement (`payment_ref=in_0`, `renews`). `invoice.paid` `in_0` (`subscription_create`) → `ignored`. A year later `invoice.paid` `in_1` (`subscription_cycle`) → +12 months, renewal row `in_1`. Owner cancels in the portal → `customer.subscription.updated` (`cancel_at_period_end=true`) → `renews=false`. At period end → `customer.subscription.deleted` → `cancelled`. A refund of `in_1` → `charge.refunded` (`invoice=in_1`) → matched through the renewal row → `refunded`.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all tests pass (do not pipe through grep).
- `cargo clippy -p wipe-hub --all-targets -- -D warnings` -- expected: clean.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: pass.

## Earlier halt (2026-11-08, resolved)

Status: blocked (resolved 2026-11-08, see `## Owner decisions`)
Blocking condition: intent gap — the renewal mechanism for annual offerings is open owner decision D15 (`.scratch/DECISIONS.md`), whose "Unblocks" column names this story's renewal design. Choosing whether customers' cards are charged automatically every year is a pricing and product decision, which AGENTS.md and the decide-by-default rule reserve for the owner. Nothing was planned in detail or implemented; the working tree is unchanged apart from this file.

#### Evidence gathered

- SPEC CAP-4: "annual renewals extend the term; a refund or cancellation is recorded and stops renewal (existing licences are not silently killed — ROADMAP §0.2)". "Stops renewal" assumes something renews by itself.
- Story 3, decision 1, and `.scratch/DECISIONS.md` D15: annual offerings are charged once through Stripe (`mode=payment`) "until story 4", and story 4 chooses the recurring mechanism. D15's reason for charging once ("the hub cannot yet grant a renewed year") goes away once this story builds fulfilment, so what is left is a pure product choice.
- `CONTEXT.md` §9 describes Tier 1 as an "annual subscription per licensed wipestation", which points towards automatic renewal but does not decide the billing mechanism.
- D16 (Stripe invoices) is linked: Stripe subscriptions produce invoices with no add-on fee, while one-off payments need the paid invoicing add-on.
- The next epic, `licence-delivery` (CAP-3), expects "when an entitlement's term is extended, the portal offers renewed licence files". It works with either mechanism, as long as a term can be extended.

#### Defaults this story will apply whatever you choose (object if any is wrong)

These follow ROADMAP §0.2 ("never block a wipe on licensing") and need no answer:

- Fulfilment: a paid order creates one **entitlement** for the buying organization. An entitlement is the record of what the organization may license: scope, seats, features, quota, and a term from the payment time to that time plus the offering's term. The paid event creates it exactly once, in the same transaction, and audits it.
- A refund or cancellation is recorded on the order and the entitlement, and is audited. It never revokes or shortens a licence that has already been issued, which keeps working until its own expiry. It stops the term being extended again.
- A refunded order's unused seats stop being available for new stations. A cancelled subscription keeps its seats until the current term ends.

#### Question for the owner

**What it is about.** An organization owner buys an annual offering, such as "Station — annual, 1 seat", in the customer portal. They pay through Stripe Checkout, or through the Fake provider on a local hub. Today the year is charged once, and nothing happens when the year ends. This story makes a paid order grant the organization an entitlement (seats, features, a 12-month term). Licences for stations are issued from that entitlement later, in `licence-delivery`. The story must also decide how the second year is paid for.

**The problem.** Take an ITAD (IT asset disposition) business that buys one Station seat on 1 March. Its licence runs to 1 March next year. When the term ends, the station keeps erasing, but its certificates are marked `evaluation` instead of licensed to the customer. Someone has to decide whether the next year is charged automatically, so that it continues unless the customer cancels, or only when the owner comes back and buys again. The SPEC's wording "a cancellation… stops renewal" fits the first. The decision recorded in D15 kept the second "until story 4".

**The options.**

- **(A) Renew by buying again.** Annual offerings stay a single charge. Buying the same annual offering again while the organization holds an active or lapsed entitlement from it extends that entitlement by 12 months, from its current end or from the payment if it has lapsed.
  - *What customers see:* no card is ever charged without a click. The portal shows "expires on <date>, buy again to renew". There is nothing to cancel.
  - *Cost:* the smallest build. It adds the Stripe refund event (`charge.refunded`) and a Fake refund action.
  - *Rules out:* automatic renewal. Customers who forget to renew lapse to `evaluation` certificates. "Cancellation" in CAP-4 comes down to a refund. Invoices still need the paid Stripe add-on (D16).
- **(B) Stripe subscriptions.** Annual offerings check out as Stripe subscriptions (`mode=subscription`, a yearly price). Each renewal payment Stripe reports (`invoice.paid`) extends the term by 12 months. An owner can cancel in Stripe's billing portal: the term runs to its end and is not renewed. The Fake provider gets "renew now" and "cancel" test buttons.
  - *What customers see:* the year renews and is charged automatically until they cancel, and they get an invoice from Stripe for each payment.
  - *Cost:* a larger build: the subscription and invoice event family, those events arriving out of order relative to the checkout events, and the Fake simulation.
  - *Ongoing:* failed renewal charges (Stripe retries, then cancels) need handling. The buy page and checkout need an "auto-renews yearly until cancelled" notice. Auto-renewal laws in several US states and the EU require that notice, so its wording is copy you sign off.
  - *Rules out:* nothing. One-off offerings stay one charge, and a "buy again" path could still be added later.
- **(C) Split the story.** Build fulfilment and refunds now under the defaults above, with no renewal of either kind. Annual entitlements simply end after 12 months. Renewal becomes a new story once D15 is answered.
  - *What customers see:* the same as today, plus their seats in the portal.
  - *Cost:* the smallest build, and it unblocks `licence-delivery` and the SPEC's success signal (one available seat after buying) at once.
  - *Rules out:* nothing, but CAP-4's renewal and cancellation stay open, and this epic does not finish as specified.

**Recommendation:** B. `CONTEXT.md` §9 already sells Tier 1 as an annual subscription. Automatic renewal is the B2B (business-to-business) norm. It also settles D16, because subscription invoices carry no add-on fee. The reason for charging once in D15, that the hub could not grant a renewed year, is what this story removes. If you are not ready to sign off auto-renewal copy, choose C, which unblocks the next epic without committing to either.

**Default if nobody decides:** story 4 stays blocked, and `licence-delivery`, which needs entitlements, waits behind it. Story 5 (the public offerings API and pricing page) does not depend on story 4 and can run.

Should annual offerings renew by buying again (A), as Stripe subscriptions that renew automatically until cancelled (B), or should this story ship fulfilment and refunds only and leave renewal to a later story (C)?

## Auto Run Result

Status: done (2026-11-08, three review passes, two `bad_spec` loopbacks)

**Summary.** A paid order now creates exactly one entitlement in the same transaction, with seats, scope, quota and features from the order's snapshot and a term from `paid_at`. Annual offerings check out as Stripe subscriptions (`mode=subscription`, yearly price, subscription metadata, and the owner-approved auto-renewal notice as `custom_text`). Each paid renewal invoice extends the term by 12 months, once per invoice. A cancel or resume in Stripe's billing portal sets `renews`, ordered by Stripe's event time. Stripe ending the subscription marks it `cancelled`, and the term runs to its end. A full refund marks the entitlement and its order `refunded` and withdraws the seats. Licences are never touched. The Fake provider gained "Renew now (test)" and "Cancel (test)". The portal gained `/seats` ("Licence seats"), shows the notice on `/buy` and `/checkout/fake`, and lists subscriptions on `/billing/fake`.

**Files changed:**
- `crates/wipe-hub/migrations/0013_entitlements.sql`: the `entitlements` and `entitlement_renewals` tables, their triggers, and the orders `refunded` status.
- `crates/wipe-hub/src/entitlements.rs` (new): `grant`, `renew`, `change_renewal`, `end`, `refund` and `list`, plus the notice builder and price and date formatting.
- `src/payments.rs`, `src/stripe.rs`: the new event kinds and their Stripe and Fake mappings, and the subscription checkout form.
- `src/orders.rs`, `src/payment_events.rs`: event dispatch, the order refund, and the narrowed `NotReady` rule.
- `src/order_routes.rs`, `src/lib.rs`: `GET /api/entitlements` and the Fake renew and cancel routes.
- `tests/{entitlements,stripe,orders,isolation,migrations}.rs`, `tests/support/hub.rs`, `tests/fixtures/stripe/*`: matrix coverage, grep guards, trigger refusals and the new fixtures.
- `apps/portal/src/...`: API types and guards; `SeatsPage`, `RenewalNoticeText`, and the Fake billing, Fake checkout, buy and orders pages; new tests for the buy, seats and Fake billing pages.
- `README.md`, `CONTEXT.md`, `docs/ROADMAP.md`, `.scratch/DECISIONS.md`: Stripe setup, residuals, glossary entries, A1.3, and D15 marked approved.
- `_bmad-output/implementation-artifacts/offerings-and-payments-story-4-attempt-{1,2}.patch`: the saved attempts.

**Review findings:**
- Pass 1: 35 findings. Loopback 1 added decisions 7–10 (a subscription event waits only for an order that can still be paid; a no-op change advances the ordering clock; both Stripe invoice shapes are read; members get 403 on the Fake routes), plus folded fixes.
- Pass 2: 31 findings. Loopback 2 added decision 11 (a refund naming no payment is ignored), plus folded fixes. One maybe-false finding was deferred.
- Pass 3: 30 findings. 2 low findings were patched (the audit doc wording and the newest-first test). The rest were rejected or carried over, each with its reason in the Review Triage Log.
- Deferred, in the frontmatter:
  - minor-unit conventions differ between ISO, CLDR and Stripe (pre-existing, low);
  - delayed-payment subscriptions may get 503s beyond Stripe's retry window (medium, unverified).

**Follow-up review recommended:** false. This pass patched only two `low` entries, so no `high` and fewer than two `medium` (patched: high 0, medium 0, low 2).

**Verification:**
- `cargo test --workspace` (compose up): exit 0, 563 passed, 0 failed.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean. The form without `--no-deps` fails on a pre-existing `large_enum_variant` lint in the untouched `wipe-common`.
- `rustfmt --edition 2021 --check` on the touched files: clean.
- Portal typecheck, 130 tests and build: pass.
- Matrix audit: every row has a covering test that ran.
- Flipping the entitlement sort order to ASC makes the new ordering test fail.

**Residual risks:**
- Nothing has been run against real Stripe; the fixtures are hand-built.
- The portal pages have not been checked in a browser.
- A renewing entitlement is not `current` between `term_end` and Stripe's renewal charge.
- Refund matching for subscription payments needs the webhook endpoint pinned to API version 2024-06-20.
- Lost disputes (chargebacks) are not mapped.
- Orders paid before migration 0013 have no entitlement.
- The two deferred items above.
