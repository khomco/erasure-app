---
id: SPEC-hub-foundation
companions: [../../docs/adr/0006-web-presence-and-local-first-verification.md, ../../docs/adr/0007-hub-accounts-payments-persistence.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# A vendor service that runs on a laptop

## Why

Foundation for the website's commercial side (ADR-0006/0007). Everything after it (accounts, payments, licence delivery, downloads) needs a running service, a database and a test harness, all working locally with no CI and no cloud account.

## Capabilities

- **CAP-1**
  - **intent:** A developer starts every dependency with one command.
  - **success:** `compose.yaml` at the repo root runs Postgres and Mailpit; `docker compose up -d` then `cargo run -p wipe-hub -- serve` serves `GET /api/health` reporting database and mail reachability.
- **CAP-2**
  - **intent:** The hub's schema is versioned and applied automatically.
  - **success:** sqlx migrations in `crates/wipe-hub/migrations` apply at start-up and in tests; a migration test applies them to an empty database.
- **CAP-3**
  - **intent:** Tests get isolated databases and real email capture with no shared state.
  - **success:** A test-harness module creates and drops a database per test and exposes a Mailpit client; when Postgres is unreachable, tests fail with "run docker compose up" (never skip); `cargo test --workspace` runs them.
- **CAP-4**
  - **intent:** Every vendor-side action can be audited from day one.
  - **success:** An append-only `audit_log` table and a single `audit::record` function; a test proves rows cannot be updated or deleted through the hub's data layer.
- **CAP-5**
  - **intent:** The hub serves the portal app from the same origin as its API.
  - **success:** An empty `apps/portal` React+Vite package (TanStack Router/Query, Tailwind, Vitest — same conventions as `apps/desktop`) builds; the hub serves it with SPA fallback; a root `pnpm-workspace.yaml` lists `apps/*` and `sites/*`.
- **CAP-6**
  - **intent:** Configuration is explicit and safe by default.
  - **success:** Config from env/flags (database URL, SMTP, public base URL, payment provider `fake|stripe`, vendor root seed path); the hub refuses to start with `stripe` selected and no webhook secret; it logs a warning on every start when the vendor root is file-held.

## Constraints

- `wipe-hub` may depend on `wipe-license`, `wipe-cert`, `wipe-common`; no station crate may depend on `wipe-hub`.
- Rust MSRV 1.80, edition 2021; workspace dependency style.
- AGENTS.md "Running and verifying" gains the hub's commands in the same commit.

## Non-goals

- Accounts, payments, licences, downloads (later spec folders).

## Success signal

- On a fresh clone: `docker compose up -d && cargo test --workspace` passes, and the hub's health endpoint is green.
