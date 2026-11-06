---
id: SPEC-offerings-and-payments
companions: [../../docs/adr/0006-web-presence-and-local-first-verification.md, ../../docs/adr/0007-hub-accounts-payments-persistence.md, ../../docs/adr/0005-licensing-and-attestation.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# The vendor defines what is for sale, and customers buy it

## Why

An opportunity to capture: revenue. ADR-0007 §4 makes offerings data the vendor manages, and fulfilment webhook-driven through Stripe, with a fake provider so everything is testable locally without a Stripe account.

## Capabilities

- **CAP-1**
  - **intent:** A vendor admin creates, edits, publishes and retires offerings.
  - **success:** Admin pages for offerings: name, description, price (minor units + currency), interval (one-off|annual), grants (licence scope machine|site, seats, quota or unlimited, `wipe_license::Feature` entitlements, term). Only published offerings are visible to customers; a retired offering keeps existing orders intact.
- **CAP-2**
  - **intent:** A verified customer buys an offering.
  - **success:** Checkout creates a pending order and redirects to the provider; the order becomes `paid` only when a verified provider event arrives; the customer sees orders and their status in the portal.
- **CAP-3**
  - **intent:** Payment events are trustworthy and processed exactly once.
  - **success:** Stripe webhook signature verified; events stored by id and processed idempotently; replaying an event changes nothing; out-of-order events handled; tests drive these through the `Fake` provider and through recorded Stripe payload fixtures.
- **CAP-4**
  - **intent:** A paid order grants what the offering promised.
  - **success:** Fulfilment creates an entitlement for the organization (seats, scope, features, quota, term start/end); annual renewals extend the term; a refund or cancellation is recorded and stops renewal (existing licences are not silently killed — ROADMAP §0.2).
- **CAP-5**
  - **intent:** Customers manage billing themselves.
  - **success:** Portal "Billing" opens the provider's customer portal (Stripe) or a local stub (Fake) for cards and invoices.
- **CAP-6**
  - **intent:** The marketing site shows live published prices.
  - **success:** `GET /api/offerings` (public, published only); the site's pricing page reads it at build time and the build fails if the hub is unreachable.
- **CAP-7**
  - **intent:** A developer can run real Stripe test mode locally if they want.
  - **success:** Documented: test keys in env, `stripe listen --forward-to localhost:<port>/api/payments/webhook`; nothing in tests needs it.

## Constraints

- No card data touches the hub; Stripe Checkout only.
- Admin offering changes and every order state change are audit-logged.
- Purchase requires a verified email.

## Non-goals

- Tax/VAT (Stripe Tax later), coupons, usage-based billing (ROADMAP L4), invoices by purchase order.

## Success signal

- Locally, with the fake provider, a customer buys "Station — annual, 1 seat" and their organization shows one available licence seat.
