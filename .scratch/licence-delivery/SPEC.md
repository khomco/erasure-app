---
id: SPEC-licence-delivery
companions: [../../docs/adr/0006-web-presence-and-local-first-verification.md, ../../docs/adr/0007-hub-accounts-payments-persistence.md, ../../docs/adr/0005-licensing-and-attestation.md, ../../docs/adr/0012-opt-in-hard-stop-on-licence-lapse.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# A purchase becomes a licence on the customer's station

## Why

The delivery half of the sale. ADR-0005 already issues licences offline from a vendor root via the CLI; customers need to get one themselves, within what they paid for, with a record of who issued what (ROADMAP A1.2, A1.4, L1.3).

## Capabilities

- **CAP-1**
  - **intent:** A customer registers a station and downloads its licence.
  - **success:** Portal "Stations": the customer pastes the station's instance public key id (and optional label/site); if a seat is free the hub issues a licence through `wipe-license` matching the entitlement (scope, quota, features, expiry = term end) and offers the licence file for download; the station accepts it with `wipestation serve --license` and `--vendor-root-b64`.
- **CAP-2**
  - **intent:** Seats are enforced at issuance, never at the bench.
  - **success:** Registering beyond the entitlement's seats is refused with a clear message; releasing a station frees its seat and records the old licence as superseded.
- **CAP-3**
  - **intent:** Renewal gives every station a fresh licence without re-registration.
  - **success:** When an entitlement's term is extended, the portal offers renewed licence files for every registered station; old files keep working until their own expiry.
- **CAP-4**
  - **intent:** Vendor staff can issue, inspect and revoke licences by hand.
  - **success:** Admin pages to issue a licence for any organization (e.g. evaluation or a trial), inspect any licence (decode and verify), and mark one revoked with a reason; every action is audit-logged.
- **CAP-5**
  - **intent:** Customers can tell a station is genuinely theirs.
  - **success:** The portal shows how to read the instance key id on a station (`GET /api/public_key`, startup log) and the vendor root public key to publish to auditors.
- **CAP-6**
  - **intent:** A customer can choose that its stations stop rather than issue `evaluation` certificates once their licence lapses (ADR-0012).
  - **success:**
    - `wipe-license`'s signed licence body gains `lapse_policy`: `evaluation` | `stop`. It has a serde default of `evaluation`, so existing licence files still verify and behave as before.
    - An organization owner sets the organization's policy on the portal's Stations page. The page explains what `stop` does, that a running job always finishes, and that existing licence files must be reinstalled to pick up a change.
    - The change is audited (`org.lapse_policy_changed`, old and new value).
    - Every licence issued or reissued afterwards carries the policy, and the portal offers reissued files for every registered station.
    - Members and vendor admins can see the policy but cannot change it for a customer.

## Constraints

- The hub never gates erasure: a revoked or expired licence only changes how future certs are marked (ROADMAP §0.2). The one exception is the customer's own opt-in `stop` lapse policy (CAP-6, ADR-0012), which the hub only writes into licences and never enforces itself. Revocation distribution to stations is out of scope and the portal says so.
- Vendor root is file-held in development with a warning on every start (ADR-0007, not decided for production).
- Licence issuance uses `wipe-license` functions directly; no copy of the signing logic.

## Non-goals

- Online activation, usage reporting, revocation lists (ROADMAP A2/L3, ADR candidate 0008), site-wide multi-key licences (D4).

## Success signal

- A customer who bought one seat registers a running local station, installs the downloaded licence, and that station's next certificate is licensed to their organization instead of `evaluation`.
