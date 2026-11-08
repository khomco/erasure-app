---
id: SPEC-licence-visibility
companions: [../audit-2026-11-05/shipped-claims.md, ../../docs/adr/0005-licensing-and-attestation.md, ../../docs/adr/0012-opt-in-hard-stop-on-licence-lapse.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# Licence state is visible at the bench

## Why

A pain to solve (ROADMAP L1.4, L1.5, O3.5, O-R9). Licensing marks certificates `evaluation` but nothing in the UI says so: there is no licence route, and the TS `SignedCertificate` lacks `evaluation` and `attestation`. An operator can run a whole shift producing evaluation certificates without knowing.

## Capabilities

- **CAP-1**
  - **intent:** Clients can read the station's licence state.
  - **success:** `GET /api/license` returns status, operator message (`LeaseStatus::operator_message`), customer, expiry, instance key id and fingerprint; mirrored in `client.ts`/`types.ts`; HTTP test.
- **CAP-2**
  - **intent:** A certificate's evaluation marking and attestation are visible on the Certificate page.
  - **success:** TS `SignedCertificate` gains `evaluation` and `attestation`; the page shows an evaluation badge or the licensed customer; vitest covers the view logic.
- **CAP-3**
  - **intent:** Whenever the licence is not valid, every page says so.
  - **success:** A persistent Layout banner appears for any non-`Valid` status with the operator message. It warns ahead of expiry at 30, 7 and 1 days, held in named constants. Under a `stop` lapse policy the warning says that new jobs will be refused at expiry.
- **CAP-4**
  - **intent:** An operator can install a licence file without restarting.
  - **success:** `POST /api/license` validates with the existing install checks and hot-swaps via `with_license`; a bad file is rejected with the reason and leaves the old state.
- **CAP-5**
  - **intent:** A station whose licence carries the customer's `stop` lapse policy refuses to start new jobs once that licence has lapsed (ADR-0012).
  - **success:**
    - Starting a job (the `JobRunner` path behind `start_job`) refuses, with an error naming the expiry or revocation and how to install a licence offline, when the installed licence's `lapse_policy` is `stop` and its status is expired, revoked or clock-rolled-back.
    - A job already running when the licence lapses runs to completion and is certified as the licence state at signing dictates.
    - An unreachable hub or licence server never causes a refusal.
    - Enumeration, certificate viewing and verification, and licence install (CAP-4) all keep working.
    - Installing a valid licence lifts the refusal without a restart.
    - Under `evaluation`, or with no field, behaviour is unchanged.
    - Integration tests cover each of these cases.

## Constraints

- Nothing here may gate, delay or warn-block an erasure (ROADMAP §0.2); banners inform only. The one exception is CAP-5's refusal to start new jobs under the customer's opt-in `stop` policy (ADR-0012). It has no override, it never interrupts a running job, and it never triggers on connectivity.
- Licence hot-swap writes through the shared cell (see `AppState::with_license` comment).
- Rust serde shapes and TS mirrors change in the same commit.

## Non-goals

- Authentication on these endpoints (D2), online activation (A2), site scope (D4).

## Success signal

- An operator at an unlicensed bench sees "evaluation" before starting the first job, and on every certificate it produces.
