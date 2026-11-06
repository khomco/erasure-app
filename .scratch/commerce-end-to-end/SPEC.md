---
id: SPEC-commerce-end-to-end
companions: [../../docs/adr/0006-web-presence-and-local-first-verification.md, ../../docs/adr/0007-hub-accounts-payments-persistence.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# The whole customer journey, proven on a laptop

## Why

A mandate from ADR-0006 §3: with no CI and no deployment, one script must prove the commercial platform works end to end, the way `scripts/demo.sh` proves the station.

## Capabilities

- **CAP-1**
  - **intent:** One command proves the customer journey.
  - **success:** `scripts/commerce-demo.sh` (requires `docker compose up -d`) starts the hub with the Fake provider and a station, then: creates a vendor admin and a published offering; signs up a customer; verifies email via Mailpit; buys the offering; publishes and downloads a release and checks its SHA-256; registers the station's instance key; downloads and installs the licence; runs an erasure; verifies the certificate offline with the vendor root and asserts it is **not** evaluation and names the customer; exits non-zero with a clear message on any failed step.
- **CAP-2**
  - **intent:** A new developer can bring the platform up from the README.
  - **success:** README and `docs/agents/` describe the local stack (compose, hub, portal, site, optional Stripe test mode) and the demo; AGENTS.md lists the commands.
- **CAP-3**
  - **intent:** Nobody mistakes the local stack for production.
  - **success:** The portal shows a persistent "development" banner whenever the payment provider is Fake or the vendor root is file-held.

## Constraints

- No network beyond localhost; no third-party accounts.

## Non-goals

- Deployment, CI configuration.

## Success signal

- `docker compose up -d && scripts/commerce-demo.sh` passes on a fresh clone.
