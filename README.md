# Wipestation — v0.1

A NIST SP 800-88 Rev. 2 / IEEE 2883-2022 data-sanitization tool with a
local web UI, an HTTP API, and per-station mDNS fleet discovery. The
station is one binary (`wipestation`) serving its API and UI from one
origin, and signs command-level evidentiary certificates that anyone can
verify offline. The vendor-side hub (`wipe-hub`) is a separate binary that
no station talks to.

> **Status:** v0.1 vertical slice with a mock device backend, plus the
> ADR-0001 outer-Job model landed in v0.2. No real hardware ioctl yet —
> replace `wipe-engine-mock` with `wipe-engine-linux` (planned) when a
> Linux box with real drives is available. **Nothing in this repo has
> ever erased a real drive**; the mock backend synthesises the command
> evidence it reports. Licensing by attestation (`wipe-license`) and a
> separate vendor hub (`wipe-hub`, with its portal) also exist; no station
> talks to the hub.

## Domain model (ADR-0001)

A **Job** is the outcome-bearing unit: process one Asset to a terminal
disposition. It composes a `Vec<JobActivity>` — `Diagnostic`,
`HealthCheck`, `Erasure`, `Verification`, `Destruction` — and signs one
`Certificate` covering the whole evidence chain.

| Layer | State machine |
| --- | --- |
| Outer `Job` | `Queued → InProgress → (Erased \| PendingDestruction → Destroyed)`; a scanned drive's Job is created directly in `PendingDestruction`; `Aborted` from any non-terminal state (including `Queued` and `PendingDestruction`, where an already-signed cert remains); `Quarantined` is declared but nothing transitions into it yet. `PendingDestruction` serializes as `pending_destruction` and still accepts the pre-ADR-0011 name `pending_co_sign` |
| Inner `ErasureEvent` | `Queued → Probing → (Unfreezing) → Confirming → Running → Completed`; `Failed` when a command poll reports failure, `Aborted` on operator abort. A backend error from the capabilities probe, unfreeze or issue leaves the event in its last state |

The inner machine ends at `Completed`. After it, the runner appends a
sibling `JobActivity::Verification` to the Job (sampled reads), and the
certificate is signed by `wipe-server` when the Job reaches `Erased` or
`PendingDestruction` — neither verification nor signing is an inner state. If
the sampled reads fail, the `ErasureEvent` stays `Completed`, the failing
Verification activity is recorded, and the Job stays `InProgress`.

Today a Job holds at most one `ErasureEvent` (none if it is recorded for
destruction or aborted from `Queued`): there is no retry or
method-fallback route, so a failed attempt leaves the Job `InProgress`
until the operator aborts it or records it for destruction. The
`Job.activities` list is shaped to hold more attempts later. Recorded for
destruction, the Job moves to `PendingDestruction`, is rolled into a `DestructionManifest`
(a destruction run), and reaches `Destroyed` when the run's completion is confirmed (and
concurred with, when the station's concurrence policy requires it). See
[ADR-0001](docs/adr/0001-job-as-outcome-bearing-composition.md).

> Pre-ADR-0001 the type called `Job` meant "one attempted erasure". That
> type is now `ErasureEvent`, and the old `JobEvent` stream is now
> `JobUpdate`. Both renames have shipped — the code and the CONTEXT.md
> glossary agree.

## What's in the box

| Crate | Purpose |
| --- | --- |
| `wipe-common` | Shared domain types (`Device`, `Method`, `Capabilities`, `Job`, `JobActivity`, `ErasureEvent`, `DestructionManifest`, `StationInfo`, …) and the NIST R2 method selector |
| `wipe-engine` | `DeviceBackend` trait + `JobRunner`; runs the outer-Job and inner-ErasureEvent state machines and broadcasts `JobUpdate` records |
| `wipe-engine-mock` | Fake fleet of 4 drives (NVMe x2, SATA SSD, HDD) with simulated progress, failure injection, synthesised evidence |
| `wipe-cert` | JSON-LD certificate schema (v2 — carries the activity chain) + canonical serialization + detached Ed25519 sign / verify + co-signature blocks (the block's signer, role and `manifest_ref` sit outside the signed bytes) |
| `wipe-license` | Licensing by attestation (ADR-0005): vendor root → licence → station instance key → certificate chain, entitlements, offline lease checks and the vendor-side licence issuing used by `wipestation license` and `wipe-hub`. Licences verify against vendor root public keys supplied at run time (`--vendor-root-b64`), not keys built into the binary; licensing never stops an erasure, and lease state (anti-rollback watermark, quota count) is held in memory and resets on restart |
| `wipe-fleet` | mDNS service advertise / browse + deterministic lead election |
| `wipe-server` | Axum REST API + WebSocket event stream for one station: health, station info, public key, fleet peers/lead, devices and capabilities, bay topology (get, save, saved config, dry-run resolve, store tier, ephemeral acknowledge), enclosure catalog, Jobs (create, list, get, start, abort, record-for-destruction, certificate), record a scanned group for destruction and preview the attestation statement, destruction runs (manifests: create, list, get, confirm completion, concur, preview either statement) and the read-only concurrence policy, simulator hot-plug (`/api/sim/devices[/attach\|/detach]`, always routed, 404 when the backend has no simulator) and `/api/events` (WS). Auto-signs certs when a Job reaches `Erased` or `PendingDestruction` and keeps them in RAM only |
| `wipe-cli` | The `wipestation` station binary: `serve` (`--addr --station-id --no-fleet --key-path --fast --static-dir --bay-topology --bay-profile --control-plane-url --ephemeral-config --license --vendor-root-b64 --concurrence-policy none|review|witness`; without `--key-path` a fresh signing key is generated on every start), `bay-presets [--dump <id>]`, `verify-cert`, `inspect`, and the vendor-side `license new-root / issue / inspect` |
| `wipe-hub` | Vendor-side hub (ADR-0007), `wipe-hub serve`: Axum + Postgres (sqlx), flag/env config validated at start-up, `GET /api/health` probing Postgres and SMTP, `GET /api/deployment` (no session) reporting its payment provider, how its vendor root is held and whether it is a development hub (Fake provider or file-held root), which drives the portal's development banner, serving the built portal (`apps/portal/dist` or `--portal-dir`) with SPA fallback while unknown `/api/*` paths stay JSON 404s, an append-only audit log (`audit::record`; UPDATE/DELETE/TRUNCATE rejected by database triggers), and accounts: organizations and `owner`/`member`/`vendor_admin` users, sign-up with an emailed verification link (`--mail-from`, plain SMTP), Argon2id password sign-in and server-side sessions under `/api/auth/*` (double-submit CSRF token on every state-changing `/api` request, per-email and per-IP sign-in lockout, session rotation on sign-in, sign out everywhere), emailed single-use magic (sign-in) and password reset links, profile and password change, organization rename and team management (owners invite by emailed single-use link, change roles between `owner` and `member`, and remove members; an organization always keeps an owner), a per-address and per-IP limit on every email it sends (5 per address, 20 per IP per hour; invitations also 20 per organization per hour), every auth event audited, and `wipe-hub admin create --email <address>`, which creates a vendor admin in the one vendor organization and emails them a set-password link (never a password on the command line), for an admin area under `/api/admin/*` (search customer organizations and users, view one organization, and list, view, set the status of, annotate, export as CSV and erase sales leads, and create, edit, publish and retire offerings (price in integer minor units, one-off or annual, and grants: licence scope, seats, erasure quota, `wipe_license::Feature` entitlements, term), whose price and grants freeze once published; vendor admins with a verified email only, and every search, view, export and change audited, with ids only and never a sales lead's personal data), and sales leads from the marketing site's forms (`POST /api/leads` behind credentialed CORS for the one `--site-origin` and the usual CSRF token, with a per-IP limit of 10 an hour, a honeypot, one lead per address, the exact consent text stored with every submission, a confirmation email whose link unsubscribes via the portal's `/unsubscribe` page, and lead audit rows that hold no personal data). `tests/isolation.rs` checks that one organization's owner and member never see or change another organization's records through any customer route, and fails when a new `/api` route is left unclassified. Customers buy through a `PaymentProvider` seam (`--payment-provider fake|stripe`): a public catalog of published offerings (`GET /api/offerings`, no session; the marketing site's pricing page reads it at build time), checkout by an organization owner with a verified email (`POST /api/checkout`, creating a `pending` order that snapshots the offering's terms), orders for every role (`GET /api/orders[/{id}]`) and billing for owners (`POST /api/billing/portal`); an order becomes `paid` or `expired` only by applying a provider event whose signature the provider verified, and every order state change is audited. The `fake` provider (the default) works only on a local `--public-base-url` and drives the portal's test checkout page; `stripe` (which needs `WIPE_HUB_STRIPE_SECRET_KEY` and `WIPE_HUB_STRIPE_WEBHOOK_SECRET`) creates Stripe Checkout Sessions (a single payment for a one-off offering, a yearly subscription for an annual one; no email or other personal data sent) and opens the Stripe billing portal once the organization's first payment has told the hub its billing customer (its Stripe customer; 409 before that). Provider events arrive at `POST /api/payments/webhook` (no CSRF token: the `Stripe-Signature`, or Fake's signature on a local hub, is the credential); each is stored once by provider and event id in an append-only `payment_events` table (metadata and the payload's SHA-256 only, never the payload) and applied in the same transaction, so a replayed event changes nothing, and a paid event that disagrees with its order's price, currency or order id leaves the order pending. Fulfilment: the paid transition creates the order's one **entitlement** in the same transaction (its seats, scope, quota and features read from the order's snapshot, a term from the payment for the offering's term), listed with the organization's available licence seats at `GET /api/entitlements` (any role; seats of entitlements not refunded and whose term has not ended). An annual offering is a subscription: each paid renewal invoice (`invoice.paid`, `subscription_cycle`) extends the term by 12 months, once per invoice; an owner cancelling at period end in the billing portal stops renewal (`customer.subscription.updated`), Stripe ending the subscription marks the entitlement `cancelled` with its term running to its end (`customer.subscription.deleted`), and a full refund (`charge.refunded`) marks the entitlement and its order `refunded`, which withdraws its seats; none of these touches an issued licence. The owner-approved auto-renewal notice is built by the hub and shown on `/buy`, the test checkout and Stripe's checkout page. The Fake provider adds owner-only test actions, `POST /api/payments/fake/entitlements/{id}/renew` and `/cancel`. See [Stripe test mode](#stripe-test-mode). Licence delivery: with a vendor root (`--vendor-root-seed` / `WIPE_HUB_VENDOR_ROOT_SEED`, a 32-byte seed file; without one registration is 503 and the hub warns at start), an owner registers a station (`POST /api/stations`: its instance key id, the entitlement whose seat it takes, the station's `--station-id` for a machine-scoped entitlement, an optional label and site) and the hub signs its licence with `wipe_license::VendorRoot::issue` (customer = the organization, quota, features and `not_after` = the entitlement's term end, machine scope bound to `machine_fingerprint(station_id)`, site scope to the entered site or the organization id), refusing it when no seat is free (the entitlement row is locked `FOR UPDATE`, so concurrent registrations cannot exceed the seats); every role lists stations (`GET /api/stations`), downloads the stored licence file byte for byte (`GET /api/licences/{id}/download`, audited) and reads the vendor root public key (`GET /api/licensing/root`); an owner releases a station (`POST /api/stations/{id}/release`), freeing its seat and marking its licence superseded, though the file on the station keeps working until it expires. After a paid renewal moves an entitlement's term end past a station's licence, the station lists `renewal_available: true` and an owner issues it a renewed licence (`POST /api/stations/{id}/reissue`, which also issues an updated licence after a lapse policy change): the same key, scope, quota and features to the new term end, the old licence superseded (its file still valid offline until its own expiry) and the seat kept; nothing is reissued by the payment itself. Vendor admins also issue a licence to any customer organization outside entitlements and seats, for a trial, evaluation or goodwill grant, with a mandatory reason and at most 1095 days (`POST /api/admin/organizations/{id}/licences`), list an organization's licences (`GET /api/admin/organizations/{id}/licences`), view one with its decoded file and whether it verifies against the hub's root (`GET /api/admin/licences/{id}`), download it (`GET /api/admin/licences/{id}/download`), decode and verify a pasted file (`POST /api/admin/licences/inspect`), and revoke one (`POST /api/admin/licences/{id}/revoke {reason}`), all audited. Revocation is recorded in the hub only: stations are not told and the file keeps verifying offline until its own expiry; the customer sees the station as revoked (no reason), cannot download or renew that licence, and can still release the station. Lapse policy (ADR-0012): every role reads the organization's policy (`GET /api/organization/lapse-policy`, `evaluation` or `stop`) and an owner changes it (`POST /api/organization/lapse-policy {lapse_policy}`, audited); every licence the hub signs (register, reissue, vendor issue) carries the policy in its signed body (omitted when `evaluation`, so earlier files are unchanged), and a station holding a licence with another policy lists `lapse_policy_update_available: true` until an owner reissues it. The hub only writes the policy; it enforces nothing. Releases (ADR-0007 §5): vendor admins create draft releases (`POST /api/admin/releases {version, notes, channel}`: a SemVer version without build metadata, never reused, even after withdrawal; `stable` or `preview`), upload one file per platform as the raw request body (`POST /api/admin/releases/{id}/artifacts?platform=&filename=`, `application/octet-stream`, at most 1 GiB; the hub computes its SHA-256 and size while streaming it into an `ArtifactStore`, a local directory set by `--artifact-dir` / `WIPE_HUB_ARTIFACT_DIR`, default `.wipe-hub/artifacts` under the working directory and created at start-up), remove a draft's files, publish (`…/publish`, needs a file; files freeze) and withdraw (`…/withdraw`, for good; files kept), list and view releases (`GET /api/admin/releases[/{id}]`), all audited; `wipe-hub release publish --version 0.2.0 --notes "…" [--channel preview] --file linux-x86_64=<path> …` does create, upload and publish in one step against the hub's database and artifact directory, printing each file's SHA-256. Downloads (ADR-0007 §5, §6): any user of an organization with a current entitlement (not refunded, term not ended; never the vendor organization) lists the published releases of both channels with each file's size and SHA-256 (`GET /api/downloads`; 403 otherwise) and asks for a signed link to one file (`POST /api/downloads/{artifact_id}/link`: `{url, expires_at}`, good for five minutes, HMAC-signed with a key the hub draws at start, so a restart voids outstanding links); fetching the link needs no session (`GET /api/downloads/{artifact_id}/file?user=…&expires=…&sig=…`), re-checks the user and their organization's entitlement, records the download (user id, organization, file, time; no email is copied) in an append-only `release_downloads` table and the audit log (`release.downloaded`), then streams the stored file as an attachment (`GET` only: a `HEAD` probe is 405 and records nothing); vendor admins list each release's downloads with each user's current email, or none once the user has been removed (`GET /api/admin/releases/{id}/downloads`, audited). Depends only on `wipe-common`/`wipe-cert`/`wipe-license`; no station crate depends on it |
| `apps/desktop` | Vite + React + TanStack Router + Tailwind frontend, plus the Tauri 2 shell (package `wipestation-desktop`, `apps/desktop/src-tauri`) |
| `apps/portal` | Vite + React + TanStack Router/Query + Tailwind customer and vendor portal, served by `wipe-hub` from the same origin as its API (ADR-0006 §1). Sign-up, sign-in, email-verification, sign-in-link (magic link), password-reset, accept-invitation and sales-lead unsubscribe (`/unsubscribe`, acting only on a button press) pages, an account page (name, password change), an organization page (rename, members, invitations, roles and removal for owners), an admin area for vendor admins (`/admin`: organization and user search, run only on submit since each is audited; `/admin/organizations/$id`: one organization and its users; `/admin/leads`: sales leads filtered by segment, status and submission date, with CSV export; `/admin/leads/$id`: one sales lead's submissions and consent text, status, notes and erasure; `/admin/offerings`: every offering and a new-draft form; `/admin/offerings/$id`: edit, publish and retire one offering; an organization's licences and an issue form on its page; `/admin/licences/$id`: one licence's decoded file, verification, download and revocation; `/admin/licences/inspect`: decode and verify a pasted licence file; `/admin/releases`: every release and a new-draft form; `/admin/releases/$id`: notes, files with their full SHA-256, upload and remove while draft, publish and withdraw, and who downloaded it, on a press), buying (`/buy`, the offerings for sale with a Buy button for verified owners and an annual offering's auto-renewal notice; `/orders` with Billing for owners; `/orders/$id`, re-read every 3 s while pending; `/seats`, "Licence seats": the organization's available seats and entitlements; `/stations`: registered stations with licence download and, for owners, a register form, Release and, once an entitlement renews, "Issue renewed licence" (members see that an owner can issue it), and "Revoked on <date>" with no download for a licence the vendor revoked, plus how to read a station's instance key id and the vendor root public key; `/downloads`: the published releases with each file's platform, size and full SHA-256 and a Download button for organizations with a current entitlement, else a link to buy; the test provider's `/checkout/fake`, labelled "Test payment: no money moves", and `/billing/fake`, which lists renewing subscriptions with "Renew now (test)" and "Cancel (test)"), a persistent, non-dismissible development banner above the header on every page while the hub's `GET /api/deployment` says it is a development hub (Fake payment provider or file-held vendor root; nothing shown if the hub cannot say), and a home page showing the hub's status and the signed-in account (with an unverified-email notice and resend, sign out and sign out everywhere, and a link to the admin area for vendor admins); its API client sends the hub's CSRF token on every POST |
| `sites/www` | Static Astro + Tailwind marketing site (pre-launch): landing, product, standards, segment (ITAD, enterprise IT, government), pricing, contact, privacy and terms pages, `/capabilities`, and early-access/contact forms that stay closed (no script, disabled button) unless the build is given `PUBLIC_HUB_ORIGIN`, when a small script posts them to the hub's `POST /api/leads` (`astro build` refuses that while the privacy notice is a placeholder); a build-time check fails `astro build` on any capability overclaim, missing meta/OG tag, missing sitemap, or resource loaded from another origin (ADR-0006 §2). See [`sites/www/README.md`](sites/www/README.md) |

## Quick start

The whole commercial platform on one laptop (compose, hub, portal, marketing site,
optional Stripe test mode, the commerce demo, teardown) is walked through step by step in
[`docs/agents/local-stack.md`](docs/agents/local-stack.md).

```bash
# Prereqs: rustup-installed stable toolchain, pnpm, jq
# (the CI/demo scripts source ~/.cargo/env automatically).

# Start the hub's local dependencies (Postgres on 127.0.0.1:55432, Mailpit
# SMTP on :1025 with its UI on http://127.0.0.1:8025). The hub's tests need them:
docker compose up -d

# Run all Rust tests (696 across the workspace):
cargo test --workspace

# Build the portal the hub serves at / (root pnpm workspace):
pnpm install && pnpm --filter portal build

# Run the vendor hub (it applies its database migrations before listening, and
# exits if Postgres is unreachable) and check its dependencies are reachable:
cargo run -p wipe-hub -- serve &
curl 127.0.0.1:7900/api/health   # {"status":"ok","database":"up","mail":"up"}

# Build + typecheck + test the frontend:
cd apps/desktop && pnpm install && pnpm build && pnpm typecheck && pnpm test && cd ../..

# Marketing site (root pnpm workspace; each package keeps its own lockfile):
pnpm install && pnpm --filter www build && pnpm --filter www test

# Run the two-station demo end-to-end (mDNS discovery + erase + cert verify):
./scripts/demo.sh

# Run the commerce demo end-to-end (needs `docker compose up -d`; uses its own
# throwaway database and ports 7910/7890, overridable with HUB_PORT/STATION_PORT):
# vendor admin + offering, customer sign-up + email verification, Fake payment,
# release download + SHA-256 check, station licence, licensed cert verified offline.
./scripts/commerce-demo.sh
```


## Stripe test mode

Nothing in the repo needs a Stripe account: every automated test runs the hub
against a local mock of the Stripe API (`crates/wipe-hub/tests/support/stripe_mock.rs`)
and Stripe-shaped event fixtures signed in the test. To try real Stripe test
mode locally (optional):

```bash
# Test-mode keys only (sk_test_… from the Stripe Dashboard). Keep them in your
# shell or an untracked env file, never in the repo.
export WIPE_HUB_PAYMENT_PROVIDER=stripe
export WIPE_HUB_STRIPE_SECRET_KEY=sk_test_...

# The webhook signing secret of `stripe listen` (whsec_…), not a Dashboard
# endpoint's, is the one the hub must verify with. It stays the same for your
# account, so it can be captured before forwarding starts:
export WIPE_HUB_STRIPE_WEBHOOK_SECRET=$(stripe listen --print-secret)

cargo run -p wipe-hub -- serve
```

`stripe listen` stays in the foreground, so forward events from a second
terminal:

```bash
stripe listen --forward-to localhost:7900/api/payments/webhook
```

Buy an offering from the portal's `/buy` as a verified owner, pay on Stripe's
page with a test card (`4242 4242 4242 4242`), and the order turns `paid` when
`checkout.session.completed` arrives, not when the browser returns; `/seats`
then shows its seats. An annual offering is a Stripe subscription; cancel it
from Billing to see renewal stop. To see a renewal without waiting a year,
either use the Fake provider's "Renew now (test)" on `/billing/fake`, or use a
[test clock](https://docs.stripe.com/billing/testing/test-clocks). A test clock
moves only customers created on it, and the hub checks out with the
organization's stored billing customer (or a new one), never a clock's. So
create a customer on a clock in the Dashboard, make it the organization's
billing customer in your local database after one purchase (`UPDATE
billing_customers SET customer_id = 'cus_…', updated_at = now() WHERE
organization_id = '…' AND provider = 'stripe'`), buy the annual offering again,
then advance the clock past a year.
`--stripe-api-base` / `WIPE_HUB_STRIPE_API_BASE` (default
`https://api.stripe.com`) accepts only https, or http on a local host.

Several hubs (for example local development and staging) may share one Stripe
test account: each checkout carries `metadata[wipehub_hub]`, the creating hub's
`--public-base-url` origin, and a hub treats an event from another hub's
checkout as not its own (stored as `unmatched` and acknowledged). Give each hub
a distinct public origin.

**Production webhook.** In the Stripe Dashboard, register
`https://<hub>/api/payments/webhook` as an endpoint, enable only the eight
events the hub reads (`checkout.session.completed`,
`checkout.session.async_payment_succeeded`,
`checkout.session.async_payment_failed`, `checkout.session.expired`,
`invoice.paid`, `customer.subscription.updated`,
`customer.subscription.deleted`, `charge.refunded`), set the
endpoint's API version to `2024-06-20` (the version the hub pins and parses),
and give the hub that endpoint's own `whsec_…` signing secret as
`WIPE_HUB_STRIPE_WEBHOOK_SECRET`. Other event types are stored as `ignored` and
acknowledged. Live keys (`sk_live_…`) should wait for the open sales-tax/VAT
decision (`.scratch/DECISIONS.md` D14, ADR-0007 "Not decided here").

**Billing portal.** Save the billing portal's default configuration in the
Stripe Dashboard (Settings → Billing → Customer portal), once in test mode and
once in live mode, before anyone presses Billing: until then Stripe refuses
`POST /v1/billing_portal/sessions` and Billing answers 502. In that
configuration, allow cancelling subscriptions **at the end of the billing
period only** (never immediately, which would end a year the customer paid
for), and do not allow switching plans or changing quantities: the hub grants
exactly what the order bought.

**Renewal reminders.** The auto-renewal notice customers see promises "We
email you before each renewal". Turn on Settings → Billing → Subscriptions and
emails → "Send emails about upcoming renewals" in both modes; the hub sends no
such email itself. Leave Stripe's retries and failed-payment emails on: a
failed renewal charge is handled by Stripe, and if Stripe then cancels the
subscription the hub records it like a cancellation.

**Failed payments.** Under Settings → Billing → Subscriptions and emails →
"Manage failed payments", set what happens after all retries fail to **cancel
the subscription**, in both modes. Otherwise the subscription stays `past_due`
or `unpaid`, no `customer.subscription.deleted` arrives, and the hub keeps
showing the entitlement as renewing although nobody pays for it.

**Mismatched checkouts.** A paid subscription checkout whose amount, currency
or order disagrees with the hub's order is stored `mismatch` (logged at error)
and grants nothing, but Stripe has already started the subscription. Cancel
that subscription in the Dashboard (and refund it if appropriate), or it keeps
charging; each later renewal is logged at error as a paid renewal that matches
no entitlement.

**Refunds.** Refund in the Stripe Dashboard. Only a full refund of a charge
(`refunded=true`) is acted on: it marks the entitlement and its order
`refunded` and withdraws its seats, while a partial refund is a goodwill credit
the hub ignores. Refunding any one payment of a subscription (its first
payment or any renewal) refunds the whole entitlement and its order. When
refunding any payment of a subscription, also cancel the subscription:
otherwise Stripe keeps charging, and the hub logs each later renewal of a
refunded or cancelled entitlement at error without extending it, for you to
refund. A lost dispute (chargeback) is not read by the hub: after losing one,
refund or cancel in Stripe yourself. No refund or cancellation ever revokes or
shortens a licence already issued.

**Known gaps.**
- A renewing entitlement is not current (its seats are not counted) between
  its `term_end` and Stripe's renewal charge: about an hour after the new
  period starts, longer while Stripe retries a failed charge.
- Orders paid before migration 0013 have no entitlement (no backfill; no hub
  took live payments before it).
- Renewals are read in both invoice shapes: API version `2024-06-20`
  (`invoice.subscription`) and newer ones
  (`invoice.parent.subscription_details`). Refund matching of subscription
  payments needs `charge.invoice`, which newer versions drop, so keep the
  webhook endpoint at `2024-06-20` as above, and run `stripe listen` on an
  account whose default API version is `2024-06-20` (pass `--latest` only
  knowing that subscription refunds will then be stored `unmatched`).

The fixtures in `crates/wipe-hub/tests/fixtures/stripe/` are hand-built from
Stripe's documented event shapes (API version `2024-06-20`) with `__EVENT_ID__`,
`__SESSION_ID__`, `__ORDER_ID__`, `__ORGANIZATION_ID__`, `__HUB__`, `__AMOUNT__`,
`__CURRENCY__`, `__SUBSCRIPTION_ID__`, `__INVOICE_ID__`, `__PAYMENT_INTENT__`,
`__CANCEL_AT_PERIOD_END__`, `__CREATED__`, `__REFUNDED__` and
`__AMOUNT_REFUNDED__` placeholders (`invoice.paid.parent.json` is a renewal
invoice in the newer `parent.subscription_details` shape). To re-record one, run `stripe listen --print-json`
while completing a test checkout (or `stripe trigger checkout.session.completed`),
copy the event, and replace those values with the placeholders. Remove anything
else that identifies a real account.

## Operating modes (one binary, one origin)

The same Axum server serves **both** `/api/*` and the React UI at `/`. That
means a browser, a tablet, or the Tauri window can all point at the same
URL — there's no "API port" and "UI port" split.

| Mode | Invocation | What you get |
| --- | --- | --- |
| **Standalone (browser)** | `wipestation serve` then open `http://127.0.0.1:7878` | UI + API on one port; mDNS-advertised so tablets discover this station |
| **Headless / air-gap** | `wipestation serve --no-fleet` | Same as above, mDNS off |
| **Remote tablet console** | open `http://<station-ip>:7878` from another device | Same React UI, talks to the remote station's API |
| **Fast demo mode** | `wipestation serve --fast` | Erasures complete in ~1s for live demos |
| **Native desktop window** | `cargo run -p wipestation-desktop` | Tauri window over an in-process API on `127.0.0.1:7878` (`WIPESTATION_API_ADDR` moves the API, but the window URL stays `http://127.0.0.1:7878`), same UI as the browser path. Apart from that variable the shell takes none of `serve`'s options: no bay-topology store (bench setup is not saved), no licence (every cert is evaluation), no hot-plug simulator, default (slow) mock timing, and a fresh signing key and random station id on every launch, so certs from one launch do not verify against the next launch's key. It advertises on mDNS, so each launch appears to other stations as a new peer. If the port is taken, the bind error is only logged and the window shows whatever answers there. Its `api_url_cmd` Tauri command exists but the frontend never calls it |
| **Inspect catalog** | `wipestation inspect` | Prints the mock device catalog as JSON |
| **Offline cert verify** | `wipestation verify-cert path.json --public-key-b64 <key> [--vendor-root-b64 <root>] [--require-licensed]` | Checks the primary signature against the station key and, given a vendor root, the ADR-0005 attestation chain; `--require-licensed`, which takes effect only together with `--vendor-root-b64`, fails an evaluation cert. Does not check co-signatures. No network needed |

### Important: build the frontend first

The `wipestation` binary auto-detects the frontend bundle at one of:

* `--static-dir <path>` (explicit override)
* `$WIPESTATION_STATIC_DIR`
* `apps/desktop/dist` relative to the CWD
* `apps/desktop/dist` relative to the binary

If none of these are present you'll see an "API only" landing page that
tells you exactly how to fix it. To build the frontend:

```bash
cd apps/desktop && pnpm install && pnpm build && cd ../..
cargo build -p wipe-cli
./target/debug/wipestation serve --fast
# open http://127.0.0.1:7878
```

The Tauri window uses the same in-process API and serves the same UI — no
Vite dev server required for production use. If you want hot reload while
hacking on the frontend, run `pnpm dev` separately at `apps/desktop` (Vite
on :5173, which proxies `/api/*` to :7878).

## End-to-end flow

### Erase path

1. `POST /api/jobs` with a `JobSpec` (device, classification, intent, operator).
2. `POST /api/jobs/:id/start` — the Job moves `Queued → InProgress` and the
   runner appends an `ErasureEvent`, driving it through Probing →
   (Unfreezing) → Confirming → Running → Completed and emitting a
   `JobUpdate` at every step. After `Completed` it runs the sampled read
   check, appends a `JobActivity::Verification`, and the Job reaches
   `Erased`; `wipe-server` then signs the certificate.
3. `GET /api/jobs/:id/certificate` — returns a `SignedCertificate` containing:
   * The job spec, the operator (with email — required by R2), and asset/ticket linkage
   * The full `activities` chain, and within each `ErasureEvent` the captured
     `CommandEvidence` (interface, opcode, action, raw CDB, log pages) for
     every command issued
   * The `VerificationReport` (sampled reads, SHA-256, entropy)
   * The resolved `AssetDisposition`, stated explicitly so an auditor need
     not re-derive it from the chain
   * A detached Ed25519 signature over the canonical (sorted-keys) JSON
4. Anyone with the station's public key can `wipestation verify-cert` it offline — no vendor lookup, no online check; add `--vendor-root-b64` to also check who the station was licensed to (`--require-licensed` takes effect only with it).

### Destroy path

The operator records a drive for destruction (ADR-0011 §1, first step) with
an attestation: technique, equipment ID, location, full name and title, and
the identifiers, then reads the statement the server renders and ticks
"I attest". `POST /api/jobs/:id/record-for-destruction` records an existing
Job (from any Queued or InProgress Job, not only after a failed erasure):
the server reads the serial from the device (`source: device`, beside the
Job's asset tag) unless the request carries scanned `identifiers`
(`source: scanned`), and refuses (422) a device-sourced record whose serial
cannot be read. `POST /api/destruction/records` records a scanned group of
drives that never enumerate, creating one Job per drive (intent `destroy`,
device id `scanned:<serial or tag>`), all-or-nothing: a repeated identifier
is 422, and a drive matching an open Job is 409 naming that Job. Either way
the Job moves to `PendingDestruction` — at which point, if it has an
erasure activity, its certificate is generated, with
`media_status.operational = false`. A Job with no erasure activity
(recorded from `Queued`, or scanned) gets no certificate, so it cannot join
a destruction run until ADR-0011 story 4. The old unattested `escalate-to-destroy` route is
gone (unknown `/api/*` paths are JSON 404s). The Job is
then rolled into a destruction run (a `DestructionManifest`) via
`POST /api/manifests`, which refuses (409, naming them) Jobs with no
certificate, records the concurrence the station's policy requires
(`required_concurrence`), and stamps the run id on each member Job
(`manifest_id`) and its latest `DestructionEvent` (`manifest_ref`) without
re-signing the certificate; a Job on a run can no longer be recorded for
destruction again. The operator then confirms the run's completion
(`POST /api/manifests/:id/completion`): when destruction completed (local
time on the page, sent in UTC; the server's now if omitted), remnants
inspected, equipment cycle completed, equipment ID, the criterion met, an optional video reference, and
their full name and title, after reviewing the server-rendered statement and
ticking "I attest". With the default policy (`wipestation serve
--concurrence-policy none`) every member Job becomes `Destroyed` at once.
With `review` or `witness` the run waits for a concurrence of that kind
(`POST /api/manifests/:id/concurrence`) from someone other than the person
who confirmed completion, and only then are its Jobs destroyed. That
different-signer check compares the self-asserted operator and typed name,
so it does not prove a second person was involved until sign-in and roles
exist. Moving the
Jobs is all-or-nothing, and a refused request changes neither the run nor
any Job. The policy is set at startup only (env
`WIPESTATION_CONCURRENCE_POLICY`) and readable at `GET
/api/destruction/policy`. Completion changes no certificate and appends no
co-signature; the supervisor co-sign route is gone. Certificates co-signed
before ADR-0011 keep their co-signature blocks (made with the station's own
key, signer and role outside the signed bytes), which `verify-cert` does
not check. [ADR-0011](docs/adr/0011-destruction-by-operator-attestation.md)
ends with one certificate signed once at `Destroyed`, carrying the
attestation, completion and concurrence inside the signed bytes; that is
story 4 and not built yet.

| Endpoint | Purpose |
| --- | --- |
| `POST /api/jobs/:id/record-for-destruction` | Record an existing Job for destruction (attested) → `PendingDestruction` |
| `POST /api/destruction/records` | Record a scanned group of drives; one new `PendingDestruction` Job each → `{job_ids}` |
| `POST /api/destruction/statement` | Render the attestation statement for review → `{statement}` |
| `GET /api/manifests` | List destruction runs |
| `POST /api/manifests` | Assemble a run from N certified `PendingDestruction` Jobs |
| `GET /api/manifests/:id` | Fetch one run |
| `POST /api/manifests/:id/completion/statement` | Render the completion statement and the time it names → `{statement, completed_at}` |
| `POST /api/manifests/:id/completion` | Confirm completion (attested) → `completed` (Jobs `Destroyed`) or `awaiting_concurrence` |
| `POST /api/manifests/:id/concurrence/statement` | Render the concurrence statement for the run's required kind → `{statement}` |
| `POST /api/manifests/:id/concurrence` | Concur (attested, a different signer) → `completed`, Jobs `Destroyed` |
| `GET /api/destruction/policy` | The station's concurrence policy → `{concurrence_policy}` |

## Design highlights

- **NIST 800-88 Rev. 2 aware** — method selector in [`wipe-common/src/method.rs`](crates/wipe-common/src/method.rs) follows the R2 decision flow (intent → classification → media → capability), prefers NVMe Sanitize Crypto Erase on SED-provisioned NVMe, ATA Secure Erase Enhanced on SATA SSDs, single-pass overwrite on HDDs.
- **Command-level evidence** — every command the backend issues produces a [`CommandEvidence`](crates/wipe-common/src/evidence.rs) record (opcode, action, status, log page bytes). Auditors read the actual proof, not a marketing summary.
- **Offline-verifiable certs** — JSON-LD payload, canonicalized via deterministic key-sorting, signed with Ed25519. Embedded `canonical_sha256_hex` defends against silent payload modification.
- **mDNS fleet discovery** — every station advertises `_wipestation._tcp.local.` with TXT metadata. Tablets/operators discover and select stations from the LAN. Lead election is deterministic by `(started_at, id)` — no Raft round needed for small fleets.
- **PXE-ephemeral by design** — **no persistent local state for evidence**: certs are signed in RAM, held in the station's memory and served by `GET /api/jobs/:id/certificate`, never written to local storage; they are lost when the station restarts, so download them before then. Nothing ships them to a lead station, hub or cloud yet. *Configuration* (the bay topology) is a different matter and does persist where the station has somewhere to put it — see [ADR-0003](docs/adr/0003-station-config-persistence.md) for the tiered store and why the distinction is drawn.
- **One station binary, one engine** — the Tauri window and the Axum HTTP API (for browsers, tablets and automation) wrap the same engine; a Ratatui TUI is future work.

## Test inventory (791 Rust + 757 TypeScript, measured 2026-11-08)

| Crate / file | Tests | Covers |
| --- | --- | --- |
| `wipe-common/tests/method_selection.rs` | 7 | R2 decision flow across NVMe/SATA/HDD; destroy intent; frozen device handling; evidence serde round-trip |
| `wipe-common/tests/bay_topology.rs` | 36 | Bay grid construction and vendor numbering runs (row/column-major, four origins, label offsets); bay→device resolution across every binding kind; declared bindings beating enumeration fill; no device placed twice; disabled bays skipped; overflow reported; generated-fallback flagging; serde round-trip and a hand-written config |
| `wipe-common/tests/catalog.rs` | 17 | Bundled enclosure catalog (ADR-0004): schema version matches the Rust constant; rejects empty banks, zero-bay banks and duplicate ids; every model expands to a topology whose `bay_count` matches; `capabilities` requires `verified_by`; "unverified" must stay absent rather than `false` |
| `wipe-cert/src/canonical.rs` (inline) | 4 | Canonical JSON: sorted keys, nested sort, finite-only numbers, integer preservation |
| `wipe-cert/tests/sign_verify.rs` | 8 | sign+verify happy path, unknown-key rejection, tamper detection, JSON round-trip, activity chain carries erasure + verification, a co-signature block verifies against the key that made it (a separate key in the test; the station uses its own key), deterministic public-key-id, verifying-key round-trip |
| `wipe-engine-mock/tests/end_to_end.rs` | 31 | NVMe crypto-erase happy path reaches `Erased`; SATA failure keeps the outer Job `InProgress` for an operator decision; enumerate; broadcast stream observes outer *and* inner transitions; simulated hot-plug detach/attach (four tests); manifest linking stamps the Job and its latest DestructionEvent without a broadcast, refuses (stamping nothing) a Queued, unknown, already-linked or twice-listed Job, and a linked Job cannot be recorded for destruction again (six tests); `mark_destroyed` moves a whole run's Jobs to `Destroyed` with one broadcast each, and refuses all-or-nothing (moving and broadcasting nothing) on an unknown, twice-listed or not-pending Job (two tests); runner refusals (six tests): a second `start` is refused and one erasure runs, `start` moves the Job to `InProgress` before returning, a Job recorded for destruction cannot be started, aborting an `Erased` Job is refused while aborting an `Aborted` one is a no-op, and an unknown id is `JobNotFound` on start, abort, record-for-destruction and mark-destroyed; an abort or record for destruction landing right after `start` runs no erasure (two tests); record for destruction (seven tests): a record with no attestation is refused, a scanned group creates one pending Job per drive with intent `destroy` and broadcasts each, and a group repeating a serial, matching an open Job's asset tag, attached serial or erasure-snapshot serial, or empty creates nothing, while a match with a terminal Job does not block |
| `wipe-fleet/tests/two_instance.rs` | 2 | Solo lead election, two-instance mutual discovery + cross-station election agreement |
| `wipe-server/tests/http_e2e.rs` | 46 | Full HTTP flow → signed cert → offline verify; aborted job emits no cert; destruction runs (seventeen tests): with policy `none` completing a run of three erased drives destroys every Job, stores the previewed statement and time, and leaves each certificate unchanged, without a co-signature and verifying offline, and a destroyed Job cannot be aborted; with `witness` the run awaits a witness and no Job is destroyed until a second operator concurs, recorded as `witness`; with `review` the concurrence is a review whatever kind the client sends; the completion's signer (same operator id, or same name ignoring case and spacing) cannot concur; a concurrence not needed, too early or repeated, and a repeated completion, are 409 and change nothing; an unverified or unattested completion is 422 and changes nothing; the completion time defaults to now and is 422 in the future or before assembly; an unknown run is 404 on completion, concurrence and both previews; blank preview fields are 422; assembling a never-erased Job is 409 naming it; a run member that lost its certificate blocks completion and concurrence (409 naming it), as does one aborted after assembly; previews are 409 when the run is not in the state their action accepts; a completion time sent as `+02:00` is stored in UTC, exactly as the statement prints it; the default policy is `none` and runs assembled under it require no concurrence; the removed co-sign route is a JSON 404; a Job cannot be on two runs; a refused assembly stamps no Job; re-recording a Job on a run is refused with 409; record for destruction (thirteen tests): a Queued Job's record reads the serial from the device beside its asset tag and stores the previewed statement, scanned identifiers are recorded as `scanned`, a detached drive's serial comes from its erasure snapshot, a GET on the record route stays 405, an unreadable drive is 422 asking for a scan, an unattested or blank-field record is 422 and records nothing, an Erased Job is 409, the removed `escalate-to-destroy` route is 404, a scanned group creates one pending Job per drive sharing one statement, equal to the preview, a group that is empty, repeats an identifier, has an unidentified drive or is unattested is 422 and creates nothing, a group matching an open Job is 409 naming it, a non-NIST technique is recorded as such, and a blank preview field is 422; runner refusals map to exact statuses (seven tests): an unknown Job id is 404 on start, abort and record-for-destruction, a malformed id stays 400, a second start is 409 with one erasure, starting a Job recorded for destruction is 409, recording an aborted Job is 409, aborting an erased Job is 409 and aborting an aborted Job 200; assembly waits for the certificate and a certificate signed after linking omits the manifest (proved deterministically against `AppState` by `certificate_signed_after_linking_still_omits_the_manifest`; the HTTP companion only checks the wire shape); an unsaved topology resolves as a dry run (asserting the resolved topology came back) |
| `wipe-server/tests/topology_store.rs` | 16 | All four persistence tiers (ADR-0003), with the control-plane tier reporting unavailable rather than pretending to save; the write probe against a read-only directory; corrupt-config recovery; save-survives-restart; a licence installed after construction reaches the cert generator; and that a path binding learned by identify mode keeps its bay when a *different* drive is swapped into that port |
| `wipe-license/tests/attestation.rs` | 28 | ADR-0005 chain: vendor root → licence → instance key → cert; unknown roots, edited entitlements and swapped keys rejected; expiry, quota, machine scope and fingerprints; lease watermark and clock-rollback detection; counter seams that do not claim hardware backing; features need a grant and a live lease; install only on the named station |
| `wipe-license/tests/end_to_end.rs` | 6 | Licensed certs verify on both axes; evaluation certs are valid and marked; the evaluation marker cannot be stripped; a stolen licence cannot launder another station's cert; attaching a chain leaves the erasure signature intact |
| `wipe-license/tests/lapse_policy.rs` | 8 | ADR-0012 lapse policy in the signed licence body: older files still verify as `evaluation`, adding or removing `stop` is tampering, wire spelling |
| `wipe-hub` (unit + `tests/*.rs`) | 550 | 167 unit and 383 integration tests against the compose Postgres and Mailpit: migrations, audit log, accounts and sessions, team, admin, leads, offerings, orders and Stripe/Fake payments, entitlements, stations and licences, releases and downloads, portal serving, deployment banner, cross-organization isolation, CLI |
| `apps/desktop` (`pnpm test`) | 251 | `topologyEdit` (17): numbering runs across every order/origin/offset, grid rebuilds preserving operator edits by position, binding rules, validation. `identify` (12): hot-plug diffing — a reused port is a *different* drive. `shells/catalog` (8): catalog↔artwork drift in both directions, and one bundled model stays on the generic shell so the fallback stays exercised. `shells/contract` (163): every registered shell built at four content sizes, asserting the bay slot equals the content and the legibility rules hold, self-tested with deliberately broken shells. `recordForDestruction` (13): adding a scanned drive trims and refuses an empty or repeated one, the statement can be previewed only with every field it names, a stale preview is detected and blocks recording until re-reviewed and attested, and the request omits identifiers when they are read from the device. `manifestAssembly` (21): the Destruction runs page offers only `PendingDestruction` Jobs with an erasure activity and no `manifest_id` that are on no pending, awaiting or completed run, lists the never-erased ones apart, and submits only selected Jobs still offered. `destructionRuns` (12): run status labels ("Pending completion", "Awaiting review/witness", "Completed"), the equipment ID prefilled only when every member's attestation names the same one, each member drive listed by its recorded asset tag and serial (else short Job id), the local completion time converted to RFC 3339 UTC and part of the preview key, the completion's signer (operator id, or name ignoring case and spacing) refused as the concurring signer, completion and concurrence readiness (verified, signed, attested against a current statement) and their requests, which send back the reviewed completion time and never a concurrence kind. `destruction` (5): technique labels and identity assurance |
| `apps/portal` (`pnpm --filter portal test`) | 240 | API client modules (auth with the CSRF token, team, admin, leads, offerings, licences, releases, orders, stations, downloads, deployment, health) against mocked `fetch`, and page logic for buy, seats, stations, downloads, the Fake billing page, admin licence and release pages, and the development banner |
| `sites/www` (`pnpm --filter www test`) | 266 | The real pages rendered with the Astro container (`test/pages.test.ts`): all 13 pages pass the site check, ship no `<script>`, and have one `h1`, a title and a description; the landing page has exactly one primary CTA to `/early-access/`, its sections in order, and labelled claims in every claim section; closed (default), the early-access and contact forms have no `action`, a disabled submit, "not open yet" text, no script and exactly the `lead-capture` field names each with a `<label for>`, and the consent label is exactly the text the hub stores; open (`PUBLIC_HUB_ORIGIN`), they carry `data-hub-origin`, a script, a `<noscript>` note and an off-screen, `aria-hidden`, untabbable honeypot; the dev-only notice follows `import.meta.env.DEV`; the segment select starts on an empty, disabled placeholder; pricing shows no amount, currency or per-unit price anywhere in the document; the standards roadmap is labelled future with each target planned; header, footer and `/segments/` link every new page. The build-time site check (`src/lib/site-check.ts`) against fixture HTML: labelled vs bare overclaims (body text, read-aloud attributes, `<title>`, description, `og:*`), invisible characters splitting words, fake or mismatched badges, `available` capabilities unable to own shared phrases, unknown ids, stale statuses, missing/hidden/wrong-text badges, missing meta, third-party scripts, images, frames, media, icons, `style` and CSS `url()`/`@import`; `available` items allowed bare; the real capability list against sample copy; the `astro:build:done` hook run over temp dirs (recursion, CSS scan, missing sitemap); the build-only guard refusing `PUBLIC_HUB_ORIGIN` while the privacy notice is unpublished; the form script (`test/lead-submit.test.ts`): request body, CSRF fetch and one retry, and the 202/400/429/network messages (a 429 in the hub's own words; success never claims an email was sent); and `initLeadForms` in a DOM (`test/lead-form-dom.test.ts`, happy-dom): enables only open forms, always prevents the native submit, posts to the hub and resets only on success |

Plus the `scripts/demo.sh` end-to-end shell drives two real station processes, mDNS discovery, a real cert issuance, and three negative tests (correct key passes, wrong key rejected, tampered cert rejected).

## Frontend surfaces

| Page | What it shows |
| --- | --- |
| **Devices** | Bench view, in two modes — a **bay map** that mirrors the station's physical drive bays (see below), and a **card grid** with one card per attached device. Both join `/api/devices` against `/api/jobs` by `device_id` and colour-code by slot status (empty / idle / wiping / erased / failed / pending destruction / destroyed / quarantined / aborted), with a "safe to disconnect" affordance on Erased and a "needs attention" affordance on Failed |
| **Jobs** | All Jobs with outer state and activity counts |
| **Job Detail** | Live activity timeline over the `JobUpdate` WebSocket stream, with each destruction record's attestation; on a Queued or InProgress Job, a "Record for destruction" dialog (technique, equipment, location, name, title, identifiers from the device or scanned, "Review statement", "I attest") |
| **Record for destruction** | Scan screen for drives that cannot be read: classification and the attestation fields, a scan row (asset tag, serial, Enter adds) building a drive list, the reviewed statement and "I attest"; submitting records the group and links the created Jobs |
| **Certificate** | Signed-cert viewer, including the activity chain, and the co-signature block a pre-ADR-0011 destruction certificate may carry |
| **Destruction runs** | Assemble Jobs pending destruction into a run (never-erased Jobs are listed apart, with why), confirm each run's completion against the reviewed statement, give the concurrence the station policy requires, and read the recorded statements |
| **Fleet** | mDNS-discovered peers and the elected lead |

## Bay map — mirroring the physical bench

A station can declare the drive bays it physically has, so the Devices page
renders a layout the operator recognises instead of a grid of cards in
enumeration order. The hierarchy is `BayTopology → Enclosure → Bank → Bay`;
everything is parameterised (grid shape, tray orientation, form factor,
numbering run, bay→device binding) rather than templated per chassis. See
[ADR-0002](docs/adr/0002-configurable-bay-topology.md).

```bash
# List the built-in presets
wipestation bay-presets

# Run against one
wipestation serve --fast --bay-profile arma-4u-32

# Or dump a preset as a starting point for your own bench and edit it
wipestation bay-presets --dump arma-4u-32 > bench.json
wipestation serve --bay-topology bench.json
```

| Preset | Shape |
| --- | --- |
| `arma-4u-32` | 4U rackmount: two banks of 2×8 front-loading trays either side of a ventilation column |
| `dock-2bay` | Two-bay top-loading hot-swap dock |
| `nvme-carrier-8` | Eight-socket M.2 NVMe carrier |

`GET /api/bay-topology` returns the geometry with each bay's `device_id`
already resolved — the frontend renders, it never re-implements the binding
rules. Bindings resolve `by` SES device slot, `/dev` path, serial, WWN or
explicit device id; unbound bays fall back to enumeration order unless
`auto_fill_unbound` is off. Devices that no bay claimed are reported in
`unplaced_devices` and surfaced in the UI rather than silently dropped.

**A station with no bay config does not get a plausible-looking chassis.** It
gets a single auto-sized bank flagged `generated: true`, and the UI says the
positions are enumeration order, not physical bays.

## Not implemented yet

- Real Linux ioctl backend (`wipe-engine-linux` — drop in alongside the mock).
- `DiagnosticEvent` and `HealthCheckEvent` are **schema-only**: the types and
  the `JobActivity` variants exist and serialise into the cert, but the runner
  never emits them. `Erasure`, `Verification` and `Destruction` are live.
- `DestructionEvent.photo_refs` is a schema slot; there is no photo-capture UX.
- Retry and method fallback: one `ErasureEvent` per Job, no retry route.
  `Quarantined` is declared on `JobState` but nothing moves a Job into it.
- Destruction under ADR-0011: recording, completion and concurrence are
  built (see [Destroy path](#destroy-path)), but the certificate is still
  signed at `PendingDestruction` and carries neither the completion nor
  the concurrence, the run id stamped on Jobs at assembly sits outside the
  signed certificate, a never-erased drive gets no certificate (so it
  cannot join a run), and the concurrence policy is set at startup only;
  ADR-0011's single certificate at `Destroyed` is story 4.
- Licence lapse policy on the station: the hub writes the ADR-0012 policy
  into licences, but no station code reads it, so `stop` is not enforced.
- Station ↔ hub connection: `wipe-hub` exists and issues licences, but no
  station talks to it. Licences reach a station as a file passed to
  `serve --license`; certs are not uploaded anywhere.
- Control-plane configuration tier (ADR-0003 tier 2): `--control-plane-url`
  is accepted, but the probe always fails, so the station falls back to the
  ephemeral tier.
- Licence enforcement is partial: the chain and the `evaluation` marker are
  real, but lease state (watermark, quota count) is not persisted and resets
  on restart, and allowed-method and feature checks are not wired in.
  Licensing never blocks an erasure (ADR-0005).
- RBAC, SAML/OIDC on the station, customer cert portal.
- PDF/A-3 cert rendering (JSON-LD only for now; PDF wraps the JSON later).
- Bootable ISO + UEFI Secure Boot signing chain.
- Mobile / iOS-Android erasure (separate product surface).
