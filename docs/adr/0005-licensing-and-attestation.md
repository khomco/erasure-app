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

> **Ratified and amended by [ADR-0012](0012-opt-in-hard-stop-on-licence-lapse.md)
> (2026-11-08).** What follows remains the default. An organization may opt in to a `stop`
> lapse policy, carried in its signed licence, under which a lapsed station refuses to start
> new jobs instead of producing `evaluation` certificates.

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

## As built (2026-11-05)

The decision stands. The points below are where the code differs from what
is written above; they are recorded here rather than edited above, so the
original intent stays readable. Taken from the 2026-11-05 shipped-claims
audit and re-checked against the code on 2026-11-08. None of them changes
how a certificate is marked when the station is unlicensed.

1. **§3a, the lease is not persisted.** Both station start-up paths build a
   fresh lease: `build_license_context` in `crates/wipe-cli/src/main.rs`
   passes `LeaseState::new(now)` for a licensed station, and
   `LicenseContext::unlicensed` (`crates/wipe-server/src/licensing.rs`) does
   the same. The watermark and `erasures_used` are held in memory and reset
   on every start, so a restart defeats rollback detection and resets the
   local count a `Count` quota is checked against. The doc comment on
   `LeaseState` (`crates/wipe-license/src/lease.rs`) says it is stored
   through the ADR-0003 tiered store; it is not.
2. **§3b, counters.** The `MonotonicCounter` trait exists (`lease.rs`), but
   `FileCounter` is named only in a comment and does not exist. `TpmCounter`
   returns `Unsupported` from `read` and `increment`, as written, but its
   `is_hardware_backed()` returns `true`: a counter that cannot count
   reports itself hardware-backed, against §3b's "nothing pretends to
   succeed".
   *Resolved 2026-11-08 (licensing-as-built story 1):* `FileCounter` now
   exists in `lease.rs` — a decimal count in one file, written by temp
   file and rename, rewindable by anyone who can edit that file, and so
   reporting `is_hardware_backed() == false`. `TpmCounter` still returns
   `Unsupported` and now reports `false` as well, until it really counts.
   Nothing constructs a `FileCounter` yet, and lease persistence remains
   item 1.
3. **§2, `allowed_methods` and `features` are not enforced.**
   `AllowedMethods::permits` (`crates/wipe-license/src/entitlement.rs`) is
   tested (`crates/wipe-license/tests/attestation.rs`) and called by no
   server, engine or CLI code. The allow-list is over `MethodClass`, not over
   `Method` discriminants as §2 describes. Likewise `feature_available`
   (`lease.rs`) is called only from that test file, so no feature flag in a
   licence turns anything on or off.
4. **§1, root-only verification is not possible yet.** The licence
   certificate carries the instance key's id (`instance_public_key_id`,
   `crates/wipe-license/src/chain.rs`), which `verify_chain` compares with
   the id of the key that signed the certificate, but not the instance
   public key itself. An auditor therefore needs the station's public key as
   well as the vendor root: `wipestation verify-cert` requires
   `--public-key-b64` (`VerifyCert` in `crates/wipe-cli/src/main.rs`), and
   `--vendor-root-b64` alone cannot check the certificate signature.
5. **§5, the free-tier count is not tracked; §2, destroy-path
   certificates consume a `Count` quota.** The certificate hook in
   `crates/wipe-server/src/app.rs` calls `record_erasure` only when the
   certificate is not `evaluation`, so an unlicensed station counts nothing,
   and no API route or UI surfaces a count (§5). The same hook signs a
   certificate when a Job enters `PendingCoSign` (the destroy path), and
   under a valid licence that certificate is counted against §2's
   `quota: Count { erasures }` like an erasure, although under
   [ADR-0011](0011-destruction-by-operator-attestation.md) a destruction is
   an operator attestation, not an erasure.
6. **§2, `scope`: stations can share a fingerprint, and site scope is not
   checked.** `machine_fingerprint` (`crates/wipe-license/src/lib.rs`)
   hashes the station id, and `wipestation serve` defaults the station id to
   `standalone` (`crates/wipe-cli/src/main.rs`), so every station started
   without `--station-id` has the same fingerprint. `--station-id` does not
   cover every station: `AppState::with_static_dir`
   (`crates/wipe-server/src/app.rs`) starts from
   `LicenseContext::unlicensed("standalone", ..)`, and the desktop shell
   calls it, has no `--station-id` and installs no licence. For site scope,
   `evaluate` (`lease.rs`) checks only `required_fingerprint()`, which is
   `None` for `Scope::Site`, and nothing compares `site_id` with anything,
   so a site licence validates on any station.
7. **§4, the client seam is never constructed.** `HttpLicenseClient`
   (`crates/wipe-license/src/client.rs`) always reports `Unreachable`, as
   "seam only" intends, and nothing outside its own module constructs it.
8. **§5 as amended by [ADR-0012](0012-opt-in-hard-stop-on-licence-lapse.md),
   the lapse policy is not read by the station.** `lapse_policy` is carried
   in the signed licence (`LicenseCertificate`'s body in `chain.rs`), but
   no code in `wipe-server`, `wipe-cli` or `wipe-engine` reads it, so a
   `stop` policy does not stop a lapsed station from starting new jobs.
   ADR-0012 post-dates the 2026-11-05 audit; this item was added at the
   2026-11-08 re-check.

Conforms as written: licensing never stops a job from running
(`crates/wipe-server/src/licensing.rs` and the signing decision in `app.rs`);
`evaluation` is inside the signed certificate payload
(`crates/wipe-cert/src/schema.rs`); `verify-cert` prints EVALUATION for an
evaluation certificate; §6's `CertAnchor` has only the failing
`UnconfiguredAnchor` (`crates/wipe-license/src/anchor.rs`).
