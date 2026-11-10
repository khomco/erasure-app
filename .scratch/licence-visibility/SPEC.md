---
id: SPEC-licence-visibility
companions: [../audit-2026-11-05/shipped-claims.md, ../../docs/adr/0005-licensing-and-attestation.md, ../../docs/adr/0013-licensing-controls-every-erasure.md]
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
  - **intent:** A station decides at the start of each Job whether its licence covers it, refuses a start it does not cover with a clear reason, and certifies every covered Job as licensed however long it runs (ADR-0013 §1, §2; replaces the ADR-0012 opt-in).
  - **success:**
    - Starting a Job (the `JobRunner` path behind `start_job`) and recording drives for destruction (device or scanned) go through one start gate. The rule lives in Rust alone, in `LicenseContext`.
    - The gate refuses, on a real device backend, when: there is no licence; the licence has expired or is not yet valid; it belongs to another machine; the clock moved back before the watermark; the profile's method is outside `allowed_methods`; or the licence counts credits and none is left, or the reservation cannot be saved. The refusal (409) names the reason and the fix, the Job stays `Queued`, and nothing is broadcast.
    - The simulated backend (`wipe-engine-mock`) is never refused; its certificates stay `evaluation` when there is no licence.
    - A start that is allowed records its decision on the Job (the licence chain, and one reserved credit per drive on a licence that counts credits, saved before the Job leaves `Queued`). The certificate uses that recorded decision, not the licence state at signing: a Job started before expiry is certified as licensed after it.
    - A reserved credit is spent when the certificate is signed and returned when the Job fails or is aborted. This uses the licence's own `Count` quota; the organization pool and credit grants are `controlled-licensing`.
    - A running Job is never interrupted. An unreachable hub never refuses. Installing a valid licence (CAP-4) lifts a refusal without a restart. The licence's `lapse_policy` field is ignored.
    - `GET /api/license` reports the refusal text (`new_jobs_refused`), and the CAP-3 banner shows it on every page. CAP-3's expiry warnings, and its banner for an expired or exhausted licence, say that new jobs will be (or are) refused, not that certificates are marked evaluation.
    - Integration tests cover each refusal reason, the mock exemption, a Job spanning expiry, credit reserve, spend and return, and install lifting a refusal.

## Constraints

- Only CAP-5's start gate may refuse an erasure (ROADMAP §0.2 as reversed by ADR-0013): at the start only, never mid-run, never because of connectivity, with no override. Banners inform only.
- Licence hot-swap writes through the shared cell (see `AppState::with_license` comment).
- Rust serde shapes and TS mirrors change in the same commit.

## Non-goals

- Authentication on these endpoints (D2), online activation (A2), site scope (D4).

## Success signal

- An operator at an unlicensed bench sees "evaluation" before starting the first job, and on every certificate it produces.
