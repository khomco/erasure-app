# Where Wipestation is — 2026-11-05

Source: the 2026-11-05 audit (`audit-2026-11-05/shipped-claims.md`) on `main` after every
feature branch was merged. Update this file when an epic in `QUEUE` finishes or a decision in
`DECISIONS.md` lands; CONTEXT §10 remains the record of what shipped.

## One paragraph

The station engine and operator app are the mature part: job composition, the Destroy path
with manifest co-sign, certificate schema v2, configurable bay topology with a tiered config
store, a bench-setup builder, the enclosure catalog, and a licensing attestation chain — 138
Rust and 200 frontend tests. Several of those are **partial as built** (see CONTEXT §10.2,
"As-built gaps"), the desktop shell exposes less than the CLI, and the only device backend is
the mock, which synthesises evidence. **None of the four product surfaces in ROADMAP (W site,
A control plane, O operator portal, L licensing tiers) has started**, and most of their
phases are blocked on decisions rather than on code.

## Foundations (ROADMAP §5 T0)

| Item | State |
|---|---|
| T0.1 merge branches | ✅ done 2026-11-05 (c9a9214) |
| T0.2 ADR-0006 site | ✅ decided 2026-11-05 (ADR-0006, ADR-0007) |
| T0.3 ADR-0007 hub persistence + tenancy | ✅ decided 2026-11-05 (Postgres, organizations); vendor-root custody still open — D5 |
| T0.4 ADR-0009 identity / auth / RBAC / PXE | not started — D2 |
| T0.5 evaluation stance | ✅ decided 2026-11-08 by ADR-0012: ADR-0005 §5 ratified, plus a customer opt-in `stop` lapse policy |
| (new) CI | none exists; not blocking — ADR-0006 makes every check runnable locally |

## Surfaces

Key: **R** an unattended agent can build it from a spec today · **D*n*** blocked on a decision
in `DECISIONS.md` · **HW** needs real hardware · **→x** waits for item x.

| Surface | Started? | Buildable now | Blocked |
|---|---|---|---|
| **W** website + docs | no | marketing site + lead capture (`marketing-site`, `lead-capture`); drift tests | docs pages after the site exists; W3.4 on HW |
| **A** control plane | no | the commercial hub: accounts, offerings, Stripe, licence delivery, downloads (ADR-0007); station enrollment from the bench and licence fetch (ADR-0008, `station-enrollment`) | fleet console, usage reporting and revocation distribution on D6; production keys on D5 |
| **O** operator portal | partly (bay map, per-card attention states; O1.4 batch start, O2.1 exception inbox, O3.3 scanner input and O1.5 grouped Jobs list (in part; no throughput/ETA) shipped 2026-11-09, `bench-throughput-inbox` stories 1–4) | O3.5 licence status (`licence-visibility`) | O1.1–O1.3 on D2; O2.2 on D10; O4 on D11 |
| **L** licensing tiers | partly (chain, entitlements, offline lease) | lease persistence, FileCounter, free-tier count (`licensing-as-built`); bench status + expiry warning + runtime licence install (`POST /api/license`) + the start check refusing uncovered starts on a real backend, with per-drive credit reservations (`licence-visibility`, story 5 2026-11-11) | L2 on D4; L3 on D5/D6; licensing controls every erasure (ADR-0013, 2026-11-09, replacing ADR-0012); revocation is not yet a refusal reason (D6); trials, the credit pool, the renewal grace (ADR-0013 §4, amended 2026-11-16) and retiring the lapse-policy setting are `controlled-licensing` |

## Engine (CONTEXT §11 v0.2)

| # | Item | State |
|---|---|---|
| 1 | Real Linux ioctl backend | not started — HW (D12) |
| 2 | Job composition | shipped, partial (no retries) |
| 3 | Bench status overlay | shipped |
| 4 | Enterprise data model | not started — D11 |
| 5 | ERP integration | not started — →#4 |
| 6/7 | Operator auth / RBAC | not started — D2 |
| 8/15 | Licensing | chain shipped; tiers not — see L |
| 9 | PDF/A-3 certs | not started — needs a spec |
| 10 | Bay topology | shipped |
| 11 | Diagnostic/HealthCheck runtime | schema only — D8 |

## The queue the driver runs (`QUEUE`)

Re-planned 2026-11-05 after ADR-0006, ADR-0007 and ADR-0011.

1. **Pre-launch marketing:** `marketing-site`.
2. **Commercial platform**, all verifiable on a laptop: `hub-foundation`, `portal-accounts`, `lead-capture`, `offerings-and-payments`, `licence-delivery`, `software-downloads`, `commerce-end-to-end`.
3. **Station trust:** `docs-truth-up`, `destruction-cert-integrity`, `destruction-attestation`, `licensing-as-built`, `licence-visibility`, `station-enrollment` (ADR-0008), `controlled-licensing` (ADR-0013).
4. **Station usability:** `desktop-parity`, `bench-throughput-inbox`, `drift-tests`.
5. **Hardening before real hardware:** `hardening` (13 stories from the 2026-11-16 assessment, `.scratch/hardening/assessment.md`; owner decisions D18–D20 split out).
6. **Buildable without decisions** (specced 2026-11-16): `cert-verification` (root-only verification, closing ADR-0005 gap 4; a shared `wipe-verify` core built for the browser; the `/verify/` page, ROADMAP W3.2), `docs-site` (W1.5, W2.1–W2.6, W-R13), `bench-floor` (O2.3 restart notice, O2.4 labels with an offline QR, O3.1 wall-board, O3.2 gloved hands and high contrast, O3.4 accessibility), `certificate-pdf` (O4.3 PDF/A-3b). Wording stories stop for owner approval.

Everything after these needs at least one decision from `DECISIONS.md`.
