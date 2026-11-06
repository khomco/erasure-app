# Destruction is recorded by operator attestation, in two steps, and certified once

**Status:** accepted (2026-11-05)

**Amends [ADR-0001](0001-job-as-outcome-bearing-composition.md)** on three
points, stated explicitly: supervisor co-sign is no longer mandatory on the
Destroy path; the destruction certificate is signed when destruction is
*complete*, not at `PendingCoSign`; and a drive that was never erased can be
certified. The batched `DestructionManifest` from ADR-0001 stays, as the batch
record. Numbered 0011 because 0008–0010 are reserved for ROADMAP §6 candidates.

## Context

The 2026-11-05 audit found two defects on the Destroy path. A Job escalated
before any erasure could reach `Destroyed` with no certificate (refused since
c779361, but still not certifiable). And the certificate was signed claiming
`Destroyed` before anyone had confirmed anything.

The vendor's field experience is that destruction is recorded as an
**operator's attestation**, and often *before* the physical act: the operator
scans the asset tag, then the drive goes into the shredder or punch, and after
that nothing can be read from it.

Research into the standards (summary in `.scratch/destruction-attestation/research.md`)
found:

- **NIST SP 800-88 Rev. 2 §4.6** recommends a certificate of sanitization per
  device. It may be electronic, captured by scanning bar codes, and signed by
  the person performing it with a statement: *"I attest that the information
  provided on this statement is accurate to the best of my knowledge."* A
  second signer ("Concurrence") is **optional**. Verification of Destroy means
  inspecting the remnants and identifying the equipment.
- **NAID AAA (i-SIGMA) §4.3** requires hard-drive serials to be **recorded
  before destruction** and the log returned to the customer afterwards. CCTV
  covers the process. Witnessing is an option the customer may request, not a
  requirement.
- **R2v3** requires destruction per NIST 800-88 with a defined verification
  process (Core 7) and video records (Appendix B). From secondary sources, it
  has no second-person requirement.
- **NIST Rev. 2 §3.1.3**: Destroy techniques are disintegrate, incinerate,
  melt, pulverize and shred. Degaussing alone, bending, cutting and drilling
  are **not** Destroy.

No body found requires a two-person rule. Pre-destruction scanning is normal.
What none of them accept is presenting the scan as the destruction.

## Decision

### 1. Two attested steps

| Step | Who | What is recorded |
|---|---|---|
| **Recorded for destruction** | The operator, per drive or for a scanned group | Identifiers captured from the device (scanned asset tag and serial, or the enumerated device snapshot, with which one it was); technique; equipment ID; location; date/time; operator identity and title; the attestation statement, verbatim; the operator's signature |
| **Destruction completed** | The operator, or whoever ran the equipment, for the `DestructionManifest` (one destruction run) | Completion time; verification (remnants inspected against a named criterion, equipment cycle completed); optional video/CCTV reference; the same identity and signature fields |

Between the two, the Job is **pending destruction**, and the UI says exactly
that, never "destroyed". `PendingCoSign` is renamed accordingly, with the old
serde name still accepted.

### 2. One certificate, signed once, after completion

The certificate for a destroyed drive is produced when the Job reaches
`Destroyed`. Every attestation, the completion, and any concurrence (§3) are
**inside the signed bytes**. Nothing material sits outside the signature, and
co-signatures are no longer appended after the fact. A drive that was never
erased is certified from its recorded identifiers, and the certificate says
whether those were scanned or read from the device. AGENTS.md's
certificate-trigger rule changes from `Erased`/`PendingCoSign` to
`Erased`/`Destroyed`.

### 3. Concurrence is optional site policy

A site chooses `none` (the default, matching the standards), `review` or
`witness`. The wording keeps them distinct: a reviewer *"reviewed the
destruction records and verification evidence and concurs"*, while a witness
*"witnessed the destruction"*. A reviewer is never recorded as a witness.
Concurrence is given per manifest, not per drive. Where a site requires it, the
Job reaches `Destroyed` only after concurrence.

### 4. Identity: self-asserted now, verified when the licence allows

Every attestation records the signer's **identity assurance**:

- `self_asserted`: the typed full name and title from the station session,
  plus an explicit "I attest" confirmation of the displayed statement. The
  typed name is the signature. This meets every standard reviewed.
- `verified`: the identity comes from the customer's identity provider (issuer
  and subject). It is available when the station's licence includes a new
  `OperatorSso` entitlement. Building the SSO sign-in itself waits for ROADMAP
  ADR candidate 0009. The schema and the certificate carry the field from now
  on, so verified attestations need no format change later.

### 5. Name the technique honestly

The technique list grows to include crush, punch, degauss and drill alongside
the five NIST Destroy techniques. The certificate states NIST 800-88
**Destroy** only for those five. For the others it records the technique and
the criterion it was verified against (for example "platters will not engage",
NAID §4.3), and says it is physical damage, not NIST Destroy. NIST Rev. 2's
caution about shred and pulverize above the lowest security categories is
printed when the Job's classification is above low.

### 6. Unchanged

Destruction never depends on licence state (ROADMAP §0.2). A
`DestructionManifest` still groups N drives. Certificates of erased drives are
unaffected. Certificates already issued keep verifying (cert schema version
bump; the v2 verifier is retained).

## Consequences

- Schema change across `wipe-common` (attestation, completion, concurrence,
  assurance, techniques), `wipe-cert` (schema v3, signed once at `Destroyed`),
  the runner (new transitions), the routes, and the TypeScript mirror.
- Manifests UI becomes "destruction runs". Assemble, attest completion,
  optional concurrence.
- CONTEXT §5/§14 gain the terms *recorded for destruction*, *destruction
  completed*, *concurrence*, *identity assurance*.
- Resolves `.scratch/DECISIONS.md` D9.
