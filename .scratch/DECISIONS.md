# Decisions that block the backlog

Ordered by how much each unblocks. Each has the smallest question that unblocks it. When one
is decided, record it (an ADR for D1, D2, D4, D5, D6; a CONTEXT §12 resolution otherwise),
strike it here, and add the newly unblocked epic to `QUEUE`.

| # | Decision | Smallest question | Unblocks |
|---|---|---|---|
| D1 | **ADR-0006 site architecture + CI host** | Accept ROADMAP §1.2 as written (one in-repo Astro site at `sites/www`, generated reference + drift gates, WASM verify)? And which CI — GitHub Actions? | All of W; every "CI fails when…" gate |
| D2 | **ADR-0009 operator identity** | At a PXE station with no display, where does identity come from: site token + PIN, tablet handoff, or YubiKey? | O1, v0.2 #6/#7, attribution, A3, A4.1, L2.3 |
| D3 | **Ratify the evaluation stance + pick the tier axis** | Is ADR-0005 §5 the published stance? Is the entitlement-matrix "tier" eval / machine / site (ROADMAP §4.1) or Station / Cloud (CONTEXT §9)? | L1.1, L1.2, W3.3 |
| D4 | **ADR-0010 site-licence semantics** | One licence certificate listing many instance keys, or per-station licences sharing a `site_id`? (Today `Scope::Site` binds one key.) | L2, L3.4, A3.3 |
| D5 | **ADR-0007 hub persistence + vendor-root custody** | Postgres on the hub, separate from any station-side SQLite? Where does the vendor root key live (HSM/KMS vs offline file)? | All of A; A1.5 |
| D6 | **ADR-0008 station ↔ hub protocol** | REST? What exactly does a station send, and what does it do if the hub is unreachable forever? | A2, L3 |
| D7 | **Certificate versioning stance** (CONTEXT §12) | Must every certificate ever issued keep verifying under every later release? (v1 already does not.) | W4.3; protects old certs |
| D8 | **Diagnostic timing** (CONTEXT §12) | Do diagnostics run always, or only when a SanitizationProfile asks? What does a Critical finding do to the Job? | v0.2 #11 |
| D9 | **Certifying destruction** | (a) A drive destroyed with no erasure attempt: issue a destruction-only certificate, or refuse to destroy through the app? (b) The PendingCoSign cert is signed with `Destroyed` before the supervisor acts: sign at co-sign time instead, or add a pending disposition? (c) Supervisor co-sign is by the station key: acceptable until D2, or block "co-signed" wording until per-person keys exist? | `destruction-cert-integrity` story 4 |
| D10 | **Exhausted-attempts routing** (CONTEXT §12) | After N failed erasures, route to Destroy automatically or ask a supervisor? | O2.2, v0.3 #13 |
| D11 | **Enterprise data model** | May an operator override a WorkOrder's classification? Is Simple mode a strict subset of Enterprise? | v0.2 #4/#5, O4.1, O4.2 |
| D12 | **Hardware access for `wipe-engine-linux`** | Which drives (NVMe, SATA, SAS) and which machine, and who runs the tests? | v0.2 #1, W3.4, real TPM counter, real fingerprint |
| D13 | **`allowed_methods` outside the licence** | A cert whose method the licence does not allow: mark it `evaluation`, or ignore the restriction? (Never block the wipe either way — ROADMAP §0.2.) | the last open piece of `licensing-as-built` |
