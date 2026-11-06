# Web presence: a static marketing site, and a portal served by the hub — all verifiable locally

**Status:** accepted (2026-11-05)

Replaces the proposal in [ROADMAP](../ROADMAP.md) §1.2, which assumed a
documentation-and-marketing site only. Pairs with
[ADR-0007](0007-hub-accounts-payments-persistence.md), which owns the service
behind the portal.

## Context

The product is pre-launch, and will run against the simulated backend for some
time (ROADMAP §0.1). The vendor wants to market it now: a landing site that
draws traffic and captures interested parties. Behind that, the vendor needs to
sell and deliver: manage offerings, take payments, and distribute the software
and licences to registered customers, who sign in to a portal to manage their
account.

Two constraints shape every choice below:

1. **There is no CI and no deployment environment yet** (ROADMAP §0.5). Every
   surface must be buildable, runnable and verifiable on a laptop, end to end,
   before either exists.
2. **The site must not overclaim.** A marketing page saying the product
   sanitizes real drives would be false while the only backend is the mock,
   and false in exactly the way the product exists to attack (CONTEXT §1).

## Decision

### 1. Two web surfaces, two lifecycles

| Surface | Where | What it is | Why separate |
|---|---|---|---|
| **Marketing site** | `sites/www` | Static [Astro](https://astro.build) site: landing, product, standards posture, pricing, early-access sign-up, contact, legal pages | Changes with positioning, not with code; static is fast, cheap to host and cannot leak customer data |
| **Portal** | `apps/portal` | React + Vite app (same stack as `apps/desktop`), served by `wipe-hub` | Authenticated, transactional, and needs the hub's licence-signing code; one origin with its API |

The portal has two audiences behind one sign-in, separated by role
(ADR-0007 §3): **customers** (account, team, orders, invoices, stations,
licences, downloads) and **vendor admins** (offerings, customers, orders,
licences, releases, leads).

The marketing site's forms (early access, contact) post to the hub's public
API; the pricing page reads published offerings from the hub at build time, so
prices are never typed twice. If the hub is unreachable at build time, the
build fails rather than publishing stale prices.

### 2. Honest-claims discipline is enforced, not remembered

Every capability the marketing site mentions carries a status —
`available`, `in-development`, or `planned` — in page frontmatter or a shared
capability list. A test fails the site build if a page describes hardware
sanitization, certification, or a compliance claim as `available` while the
capability list says otherwise. Pre-launch copy may describe what the product
is *designed* to do; it may not say it *does* it.

### 3. Local-first verification is the contract

Each surface ships with:

- one command to run it locally (`docker compose up` for dependencies, then the
  hub and both web apps in dev mode);
- tests that run with `cargo test --workspace` and `pnpm test` and need no
  network and no third-party account;
- an end-to-end script, like `scripts/demo.sh` for the station, that walks the
  whole customer journey against local services and exits non-zero on any
  failure: sign up → verify email → buy (fake payment provider) → register a
  station → receive a licence → install it on a running station → produce a
  *licensed* certificate → verify it offline.

Anything that would need CI to be checked is written as a test that CI will
later simply run.

## Consequences

- Two new pnpm packages (`sites/www`, `apps/portal`); a pnpm workspace file at
  the root ties them to `apps/desktop`.
- The portal reuses `apps/desktop` conventions (TanStack Router/Query,
  Tailwind) but is a separate app: an operator at the bench and a buyer in the
  portal are different users with different trust levels.
- Documentation pages (ROADMAP W2) later join the marketing site; the
  generated-reference drift tests in `.scratch/drift-tests` are its inputs.
- Hosting is deliberately undecided. Static output and a single hub binary keep
  every reasonable option open.

## Not decided here

- CI host and deployment target.
- Analytics and cookie consent (needed before real traffic; a placeholder hook
  exists, defaulting to no tracking).
- The final brand, domain and copy, which the vendor signs off before anything
  is published.
