# The hub: accounts, payments, and licence delivery on Postgres

**Status:** accepted (2026-11-05)

Introduces `wipe-hub`, the vendor-side service that the portal
([ADR-0006](0006-web-presence-and-local-first-verification.md)) talks to, and
that ADR-0003 (`ControlPlaneStore`) and ADR-0005 (`HttpLicenseClient`) already
anticipate. This ADR covers the commercial side only; station ↔ hub sync stays
out of scope (ROADMAP ADR candidate 0008).

## Context

To sell the product the vendor needs, in order: people who register, offerings
they can buy, a way to pay, and delivery of what they bought — software
downloads and licences. ADR-0005 already produces licences offline from a
vendor root. What is missing is everything commercial around that signing step,
with an audit trail, and with no dependency that stops a station working.

## Decision

### 1. One Rust service, `crates/wipe-hub`

Axum, like `wipe-server`, so the vendor signing code in `wipe-license` is a
function call, not a network hop. It serves `/api/*` and the built portal from
one origin. It depends on `wipe-license` and `wipe-cert`; it must **never** be a
dependency of any station crate. A station never needs the hub to erase, sign
or verify (ROADMAP §0.2, §0.3).

### 2. Postgres from day one

- `sqlx` with migrations in `crates/wipe-hub/migrations`, applied at start-up.
- Local development: `docker compose up` runs Postgres (and the dev mail
  catcher, §3) from `compose.yaml` at the repo root.
- Tests: each test gets its own disposable database on the local server
  (created and dropped by the test harness), so tests run in parallel and never
  share state. A test that needs Postgres and cannot reach it **fails** with a
  message to run `docker compose up`; it never silently skips.
- Money is stored as integer minor units plus an ISO currency code. Timestamps
  are UTC.

### 3. Built-in accounts and sign-in

- **Tenancy:** an *organization* is the customer; *users* belong to
  organizations with a role: `owner`, `member` (customer side) or
  `vendor_admin` (our staff, on the vendor organization). Every customer-side
  query is scoped by organization; a test asserts one organization cannot read
  another's records.
- **Sign-in:** email + password (Argon2id) and an emailed magic link. Email
  verification before purchase. Password reset by email.
- **Sessions:** server-side, in Postgres; an `HttpOnly`, `Secure`
  (outside localhost), `SameSite=Lax` cookie; rotated on sign-in; CSRF token
  on every state-changing request; sign-in and reset rate-limited.
- **Email:** SMTP. Locally, [Mailpit](https://mailpit.axllent.org) in the
  compose file catches everything, and tests read mail through its API.
- The first `vendor_admin` is created by a CLI command (`wipe-hub admin create`),
  never by a web form.
- **Single sign-on** (OIDC/SAML) for customer portal users is a later,
  licence-tier feature. Operator identity at the bench is a separate decision
  (ROADMAP ADR candidate 0009); this ADR does not decide it.

### 4. Offerings are data, payments go through Stripe

- **Offerings** are created by vendor admins in the portal: name, description,
  price, billing interval (one-off or annual), and what each offering grants:
  licence scope, seat count, quota, entitlements (`wipe_license::Feature`),
  term. Pricing tiers therefore become **configuration**, not code. The
  open question of which tier axis to sell (ROADMAP D3) is answered by what the
  vendor publishes, not by a release.
- **Payments:** behind a `PaymentProvider` trait with two implementations:
  `Stripe` (Checkout for payment, webhooks for fulfilment, the Stripe customer
  portal for invoices and cards) and `Fake` (deterministic, in-process, used by
  every automated test and by the local end-to-end script). No card data ever
  touches the hub.
- **Fulfilment is driven by verified webhooks, not by the browser redirect.**
  Webhook signatures are verified; events are stored and processed
  idempotently by event id; an order is fulfilled exactly once.
- Locally, real Stripe is optional: test-mode keys plus `stripe listen`
  forwarding to the hub. Nothing in the repo requires a Stripe account to build
  or test.

### 5. Delivery: licences and downloads

- **Licences:** a customer registers a station by its instance public key id
  (shown on the station; ADR-0005), within the seats their orders granted. The
  hub issues the licence through `wipe-license` and offers it as a file to
  download and install offline. That is the existing ADR-0005 activation path,
  joined to a purchase. Revocation is recorded in the hub; distributing it to
  stations is ADR candidate 0008/0010 work.
- **Downloads:** vendor admins publish releases (version, notes, artifacts with
  SHA-256). Customers with an active entitlement can download them. Artifacts
  are stored behind an `ArtifactStore` trait (local directory now, object
  storage later). Every download is logged.

### 6. Everything is audited

Every vendor-admin action, licence issuance, revocation, order state change and
download is written to an append-only audit table with actor, time and subject.
ROADMAP A1.4 is part of this ADR, not a later phase.

## Consequences

- ROADMAP §2 (A) is re-scoped: A1 (vendor issuance) and parts of A2 (seat
  allocation, activation by file) and L1 are built inside the hub's commercial
  scope. The fleet console (A3) remains later.
- `rusqlite`, declared in the workspace but unused, is unrelated to this ADR.
  The Enterprise data model's storage (CONTEXT §11 #4) stays undecided.

## Not decided here

- **Vendor root custody (ROADMAP A1.5).** In development the hub reads the
  root seed from a file and logs a warning on every start that this is not
  production custody. **Production must not launch on a file-held root.** HSM
  or KMS is to be decided before any real licence is sold.
- Sales tax and VAT (Stripe Tax is the likely path; a decision before selling
  outside the vendor's home jurisdiction).
- Station ↔ hub protocol, online activation and usage reporting (ADR candidate
  0008).
