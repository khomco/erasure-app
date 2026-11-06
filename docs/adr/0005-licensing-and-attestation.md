# Licensing by attestation: a signature chain to a vendor root, not a metering server

**Status:** accepted (2026-08-06)

Extends the Ed25519 certificate model in `wipe-cert` and reuses the
control-plane client seam from [ADR-0003](0003-station-config-persistence.md).

## Context

Two requirements point in opposite directions.

**Compliance** wants certificates that verify offline, forever, against a
published key — CONTEXT §3 differentiator #3, and the reason air-gapped and
PXE stations are first-class. **Commerce** wants to know how many erasures a
customer performed, which normally means a metering server the station must
reach.

Every mid-market competitor resolves this by making the vendor a dependency:
Blancco bills a credit when an erase *starts* and validates certs by lookup
against its own database. CONTEXT §1 lists both as defects we exist to fix,
and §3 #5 commits to per-success licensing where failures and re-runs are free.

So the question is not "how do we meter" but: **what can a signature prove
without a server, and what genuinely cannot be proved that way?**

The honest answer is that a signature chain proves *authenticity and
entitlement* perfectly offline, and cannot prove *consumption* at all. A
station with no network can always under-report. Any design claiming
otherwise is lying, and a compliance product that lies about its own
enforcement is worse than one with documented limits.

## Decision

**Attestation is the product. Metering is an optional add-on.** Four layers,
strongest first, each honest about what it does not do.

### 1. The attestation chain (core, fully offline, cryptographically strong)

```
  Vendor root key           offline, never distributed, air-gapped
        |  signs
        v
  License certificate       binds a credential to a customer + entitlements
        |  names
        v
  Instance signing key      lives on the station; signs erasure certs
        |  signs
        v
  Erasure certificate       the artefact an auditor reads
```

Every erasure certificate carries the license certificate inline, so the
whole chain travels with the document. An auditor with **only our published
root public key** can establish, with no network:

- this cert was signed by a key the vendor licensed,
- to *this named customer*, under *these entitlements*,
- and the payload has not been altered since.

That is a stronger claim than any vendor-DB lookup, because it survives us
going out of business.

Concretely, in `wipe-license`:

- `VendorRoot` — the root keypair. Production practice is that the private
  half never leaves an offline signer; nothing in this codebase needs it
  except tests and the issuance tool.
- `LicenseCertificate` — vendor-signed, carrying `Entitlements`, the licensed
  `instance_public_key_id`, and validity dates. Signed over canonical bytes
  with the same deterministic serialization the erasure cert already uses.
- `AttestationChain` — the license plus the root key id, embedded in
  `SignedCertificate` as an optional field.
- `verify_chain(signed, root_keys)` — one call, returning a typed
  `ChainVerdict` rather than a bool, because "unlicensed" and "tampered" are
  wildly different findings and a caller must not be able to conflate them.

**The instance key signs the erasure cert; the vendor key signs the license.**
The vendor never signs erasure certs and never sees them. That is what keeps
the model offline.

### 2. Entitlements (vendor-signed, customer-uneditable)

Inside `LicenseCertificate`, therefore covered by the vendor signature:

| Field | Meaning |
| --- | --- |
| `customer_id`, `customer_name` | who this is licensed to; appears on the cert |
| `quota` | `Unlimited` or `Count { erasures }` |
| `scope` | `Machine { fingerprint }` or `Site { site_id }` |
| `not_before` / `not_after` | the lease window (§3a) |
| `features` | allowed feature flags, e.g. enterprise mode, hub sync |
| `allowed_methods` | `All`, or an explicit `Method` discriminant allow-list |
| `machine_binding` | optional fingerprint the station must match |
| `issued_at`, `license_id` | provenance and revocation handle |

Editing any of these invalidates the vendor signature. That is the entire
enforcement mechanism for entitlement *content*, and it is airtight — unlike
consumption, which is not.

`allowed_methods` is deliberately an allow-list over method discriminants
rather than free text, so a license cannot accidentally permit a method the
build does not implement.

### 3. Offline enforcement levers, with their limits stated

**(a) Time-limited leases — implemented.** `not_after` is checked against
the station clock at signing time. Works with no network.

*Weakness, stated plainly:* rolling the clock back defeats it. We mitigate
partially, not completely, with a **monotonic time watermark**: the station
persists the highest wall-clock time it has ever observed (through the
ADR-0003 tiered store) and refuses to accept a clock earlier than that
watermark minus a small skew allowance. This costs nothing, catches casual
rollback, and does **not** stop an attacker who also edits the watermark file.
It is a speed bump; the ADR says so rather than implying otherwise.

**(b) TPM 2.0 / secure-element monotonic counter — seam only.** Where
hardware offers a monotonic counter, the watermark and the erasure count can
be anchored somewhere the customer cannot rewind. `MonotonicCounter` is
defined now with a `FileCounter` implementation (honest: rewindable, and
labelled so) and a `TpmCounter` variant that returns `Unsupported` until a
Linux backend can reach `/dev/tpmrm0`. Same discipline as
`ControlPlaneStore`: the seam is real, nothing pretends to succeed.

**(c) Detectable prepaid credits (Chaum-style e-cash) — documented, not
built.** Blind-signed single-use tokens would make double-spend *detectable
after the fact* on reconciliation, without a server at spend time. It is the
only known way to get offline metering with real teeth. Out of scope now;
recorded so the option is not lost.

### 4. Online reconciliation (client seam only)

Hard counts require a server. `LicenseClient` mirrors `ControlPlaneStore`:
config-first endpoint, boring wire contract
(`POST {base}/api/licenses/{license_id}/activate`,
`POST {base}/api/licenses/{license_id}/checkin`), and it **fails visibly**
when unreachable. We are not building the server.

**Default (documented, overridable): paid tiers remain fully functional
offline.** A `Count` quota is enforced against locally-observed usage and
reconciled when a licensing server is configured and reachable. We do not
require online activation, because a station that refuses to wipe drives in an
air-gapped facility is worthless to exactly our best-fit customer (CONTEXT §2
#2). The commercial risk of under-reporting is accepted and is a smaller
problem than being unusable.

### 5. Free tier (default chosen, flagged for override)

**A station with no license still erases, and still produces a fully valid,
offline-verifiable certificate — signed by its own self-generated key, with
no attestation chain.** Such certs carry `attestation: null` and an explicit
`evaluation: true` marker in the cert body, so they are trivially
distinguishable from vendor-chained ones by a verifier and by eye.

Rationale: refusing to sign would destroy evidence the operator may need, and
silently emitting an unmarked cert would let unlicensed output masquerade as
licensed — the one outcome that damages the vendor *and* the auditor. The
distinction is machine-checkable: `verify_chain` returns `Unlicensed` rather
than an error, and `wipestation verify-cert` prints it prominently.

Free-tier volume limits (the "N erasures" idea) are **not** enforced
cryptographically. A count is tracked and surfaced, and exceeding it changes
nothing but the warning text. Anything stronger would be theatre given §3a's
limits.

### 6. Blockchain — optional anchoring only, explicitly not licensing

An optional, online, *additive* seam: publish `canonical_sha256_hex` of a
signed cert to a public chain to obtain third-party-verifiable existence-by-a-
certain-time. It answers "this cert existed on date X and has not changed",
which a signature alone cannot (our own timestamp is only as trustworthy as
our clock).

It is **not** used for licensing, metering, entitlement, or revocation, and a
cert's validity never depends on it — an air-gapped station must remain
first-class. Defined as `CertAnchor` with no implementation.

## Considered and rejected

- **Online activation required for paid tiers.** Standard industry answer,
  and it breaks the air-gapped/PXE customer we are explicitly targeting.
- **Vendor signs each erasure certificate.** Would make consumption
  provable, and requires the station to reach us for every wipe — the exact
  vendor-DB dependency CONTEXT §1 lists as a competitor defect.
- **Embed a shared secret / obfuscated licence check in the binary.** Broken
  by one determined customer, and the breakage is silent. A signature chain
  fails loudly and provably instead.
- **Refuse to operate without a license.** Rejected under the same reasoning
  as ADR-0003's ephemeral tier: a sanitization tool that will not sanitize is
  the worst possible failure mode, and destroys evidence the operator needs.
- **Blockchain for metering/licensing.** Slow, costs money per erasure, leaks
  customer activity volume publicly, and still cannot prevent an offline
  station from under-reporting. Anchoring is the only defensible use.
- **Claiming offline count enforcement is secure.** It is not, and a
  compliance product that overstates its own controls has a credibility
  problem far more expensive than the licence revenue at stake.

## Consequences

- `SignedCertificate` gains an optional `attestation` field. Existing certs
  deserialize unchanged and verify exactly as before — additive, like the
  co-signature field before it.
- We must operate a root key with real ceremony (offline, backed up, ideally
  HSM/YubiKey). Losing it means no new licenses; leaking it means anyone can
  mint them. This is now the highest-value secret in the product.
- The published root public key becomes a permanent commitment. Rotation
  needs a cross-signed successor, or every historical cert becomes
  unverifiable — the transitional design is out of scope but the field
  `root_key_id` exists so a verifier can select among multiple roots.
- Revocation is not solved offline. `license_id` gives a handle for an
  online CRL later; a revoked license stays cryptographically valid to an
  offline verifier until its `not_after` passes. Short lease windows are the
  only offline mitigation, and they trade against air-gap usability.
- Marking free-tier certs as `evaluation` is a compatibility commitment: the
  marker must remain stable, or old evaluation certs become
  indistinguishable from licensed ones.
