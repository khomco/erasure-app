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
| T0.2 ADR-0006 site + drift gates | not started — D1 |
| T0.3 ADR-0007 hub persistence + tenancy | not started — D5 |
| T0.4 ADR-0009 identity / auth / RBAC / PXE | not started — D2 |
| T0.5 evaluation stance | default decided **and implemented** by ADR-0005 §5; needs ratification — D3 |
| (new) CI | none exists — D1 |

## Surfaces

Key: **R** an unattended agent can build it from a spec today · **D*n*** blocked on a decision
in `DECISIONS.md` · **HW** needs real hardware · **→x** waits for item x.

| Surface | Started? | Buildable now | Blocked |
|---|---|---|---|
| **W** website + docs | no | route/CLI/feature drift tests without a site (`drift-tests`) | everything else on D1; W3.4 on HW |
| **A** control plane | no (seams: `ControlPlaneStore`, `HttpLicenseClient` stubs) | — | D5, D6 |
| **O** operator portal | partly (single-select bay map, per-card attention states) | O1.4 batch start, O2.1 exception inbox, O3.3 scanner input (`bench-throughput-inbox`); O3.5 licence status (`licence-visibility`) | O1.1–O1.3 on D2; O2.2 on D10; O4 on D11 |
| **L** licensing tiers | partly (chain, entitlements, offline lease) | lease persistence, FileCounter, free-tier count (`licensing-as-built`); bench status + expiry warning (`licence-visibility`) | L1.1/L1.2 on D3; L2 on D4; L3 on D5/D6 |

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

1. `docs-truth-up` — make README / ARCHITECTURE / ADRs / design docs say what the code does
2. `destruction-cert-integrity` — finish the Destroy path's correctness gaps (one story waits on D9)
3. `licensing-as-built` — make ADR-0005's lease and counting claims true
4. `desktop-parity` — the Tauri shell gets what the CLI station has
5. `licence-visibility` — licence state and `evaluation` visible at the bench
6. `bench-throughput-inbox` — batch start, exception inbox, scanner input
7. `drift-tests` — routes, CLI help, entitlements and a pinned cert fixture as golden tests

Everything after these needs at least one decision from `DECISIONS.md`.
