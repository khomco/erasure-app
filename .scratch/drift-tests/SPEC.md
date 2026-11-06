---
id: SPEC-drift-tests
companions: [../../docs/ROADMAP.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# Code-derived reference cannot drift silently

## Why

A pain to solve. 91a0c98 fixed an API route that existed in handlers but was missing from the router; the docs-site drift gates (ROADMAP W2.1/W2.2/W2.6, L-R8) are blocked on ADR-0006 and CI (D1), but the checks themselves need neither. Golden-file tests under `cargo test` give the protection now and become the site's inputs later.

## Capabilities

- **CAP-1**
  - **intent:** The set of HTTP routes is pinned, and the frontend client covers it.
  - **success:** A test enumerates the router's routes into a committed golden file and fails on any difference; a second check fails if a route has no `client.ts` function or vice versa.
- **CAP-2**
  - **intent:** CLI help output is pinned per subcommand.
  - **success:** Golden files for `--help` of every `wipestation` subcommand; a test fails on difference.
- **CAP-3**
  - **intent:** Every licensing `Feature` variant is accounted for.
  - **success:** A test fails when a `Feature` variant is added without a row in a committed entitlement listing.
- **CAP-4**
  - **intent:** A known v2 certificate keeps verifying.
  - **success:** A committed signed v2 certificate fixture with its public key verifies under `cargo test`; a format change that breaks it fails the build.
- **CAP-5**
  - **intent:** Regenerating goldens is one command.
  - **success:** An env var (e.g. `UPDATE_GOLDENS=1 cargo test`) rewrites the files; documented in AGENTS.md "Running and verifying".

## Constraints

- Goldens live in a neutral `docs/generated/`; no Astro, no `sites/`, no xtask, no CI config (those are D1).
- Never pipe `cargo test` through grep/awk (AGENTS.md).

## Non-goals

- The documentation site; publishing the generated files.

## Success signal

- Deleting a `.route(...)` line, or adding one without a client function, fails `cargo test --workspace`.
