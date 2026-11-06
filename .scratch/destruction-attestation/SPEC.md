---
id: SPEC-destruction-attestation
companions: [../../docs/adr/0011-destruction-by-operator-attestation.md, ../../docs/adr/0001-job-as-outcome-bearing-composition.md, research.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# An operator attests destruction, and the certificate tells the truth about it

## Why

A mandate to meet, and a pain to solve. Destroyed drives are what an ITAD's customers pay to have proven. ADR-0011 replaces the mandatory supervisor co-sign with the record the standards actually describe: an operator's attestation, often made before the physical act, then a completion confirmation, certified once.

## Capabilities

- **CAP-1**
  - **intent:** An operator records drives for destruction by scanning them, with or without any erasure attempt.
  - **success:** From a Job (any non-terminal state) or by scanning asset tag/serial at a "Record for destruction" screen, the operator picks technique, equipment ID and location, reads the attestation statement (research.md wording), confirms "I attest", and the typed full name + title is recorded as the signature; the Job becomes *pending destruction*; identifiers record whether they were scanned or read from the device.
- **CAP-2**
  - **intent:** Completion is confirmed once per destruction run.
  - **success:** A destruction run (`DestructionManifest`) groups pending drives; completing it records time, verification (remnants inspected against a named criterion, equipment cycle completed), optional video reference, and the confirmer's attestation; all its Jobs become `Destroyed` unless concurrence is required.
- **CAP-3**
  - **intent:** A site may require review or witness concurrence.
  - **success:** Site policy `none|review|witness` (default `none`) in station configuration; with `review`/`witness`, Jobs wait for a concurrence on the run, worded distinctly per research.md; a reviewer can never be recorded as a witness.
- **CAP-4**
  - **intent:** Each destroyed drive has exactly one certificate, signed once, containing everything.
  - **success:** The certificate is produced on the transition into `Destroyed`; attestation, completion and concurrence are inside the signed bytes; `verify-cert` checks them; a drive never erased is certifiable; cert schema v3, and v2 certificates still verify (test with a pinned v2 fixture).
- **CAP-5**
  - **intent:** The technique is named honestly.
  - **success:** Techniques include crush, punch, degauss, drill alongside the five NIST Destroy techniques; the certificate and UI say "NIST SP 800-88 Destroy" only for disintegrate/incinerate/melt/pulverize/shred, otherwise "physical damage — not NIST Destroy" with the verification criterion; the shred/pulverize caution appears when classification is above low.
- **CAP-6**
  - **intent:** Identity assurance is recorded, ready for SSO.
  - **success:** Every attestation carries `identity_assurance: self_asserted | verified { issuer, subject }`; only `self_asserted` is produced today; a `Feature::OperatorSso` entitlement exists in `wipe-license`; the certificate page shows the assurance level.

## Constraints

- ADR-0011 is the design; where it amends ADR-0001, follow ADR-0011 and add an addendum to ADR-0001 saying so.
- Destruction never depends on licence state (ROADMAP §0.2).
- Only `JobRunner` mutates Jobs; certificates only on `JobStateChanged` into `Erased`/`Destroyed` — update AGENTS.md's rule in the same commit.
- Keep c779361's guarantee in the new form: no Job reaches `Destroyed` without a certificate.
- `PendingCoSign` is renamed with the old serde name accepted on input.
- Rust shapes and `types.ts`/`client.ts`/`ws.ts` mirrors change together; CONTEXT §5/§14 gain the new terms.

## Non-goals

- The SSO sign-in itself (ROADMAP ADR candidate 0009), video storage, photo capture, remote/async concurrence (Tier-2 cloud).

## Success signal

- An operator scans three dead drives, attests, runs them through the shredder, confirms completion, and each drive has one verifiable certificate that states who attested, when, how it was verified, and that it was self-asserted.
