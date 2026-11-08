# Decisions that block the backlog

Ordered by how much each unblocks. Each has the smallest question that unblocks it. When one
is decided, record it (an ADR for D1, D2, D4, D5, D6; a CONTEXT §12 resolution otherwise),
strike it here, and add the newly unblocked epic to `QUEUE`.

| # | Decision | Smallest question | Unblocks |
|---|---|---|---|
| ~~D1~~ | ~~ADR-0006 site architecture~~ | **Decided 2026-11-05** — ADR-0006 (static Astro marketing site + hub-served portal, local-first verification) and ADR-0007 (Rust `wipe-hub`, Postgres, built-in sign-in, Stripe). Still open but not blocking: CI host, hosting, domain/brand. | — |
| D2 | **ADR-0009 operator identity** | At a PXE station with no display, where does identity come from: site token + PIN, tablet handoff, or YubiKey? | O1, v0.2 #6/#7, attribution, A3, A4.1, L2.3 |
| ~~D3~~ | ~~Ratify the evaluation stance~~ | **Decided 2026-11-08 — ADR-0012.** The ADR-0005 §5 default is ratified. A customer may opt in to a `stop` lapse policy, carried in the signed licence, with these safeguards: no effect on a running job, never triggered by an unreachable hub, warnings at 30/7/1 days, and no override. Built by `licence-delivery` story 4 and `licence-visibility` story 5. | — |
| D4 | **ADR-0010 site-licence semantics** | One licence certificate listing many instance keys, or per-station licences sharing a `site_id`? (Today `Scope::Site` binds one key.) | L2, L3.4, A3.3 |
| D5 | **Vendor-root custody** *(persistence decided: Postgres, ADR-0007)* | Where does the vendor root key live in production — HSM/KMS or an offline signer? Development uses a file with a loud warning. | Selling real licences (not local development) |
| D6 | **ADR-0008 station ↔ hub protocol** | REST? What exactly does a station send, and what does it do if the hub is unreachable forever? *(Not needed for the first commercial release: licences are delivered as files, ADR-0007 §5.)* | Online activation, usage reporting, revocation distribution, and a connected station fetching its auto-renewed licence by itself (owner request, 2026-11-08) |
| D7 | **Certificate versioning stance** (CONTEXT §12) | Must every certificate ever issued keep verifying under every later release? (v1 already does not.) | W4.3; protects old certs |
| D8 | **Diagnostic timing** (CONTEXT §12) | Do diagnostics run always, or only when a SanitizationProfile asks? What does a Critical finding do to the Job? | v0.2 #11 |
| ~~D9~~ | ~~Certifying destruction~~ | **Decided 2026-11-05** — ADR-0011: operator attestation in two steps (recorded for destruction → completed), one certificate signed at `Destroyed`, concurrence optional by site policy, identity assurance recorded (SSO-verified when licensed). Built by `destruction-attestation`. | — |
| D10 | **Exhausted-attempts routing** (CONTEXT §12) | After N failed erasures, route to Destroy automatically or ask a supervisor? | O2.2, v0.3 #13 |
| D11 | **Enterprise data model** | May an operator override a WorkOrder's classification? Is Simple mode a strict subset of Enterprise? | v0.2 #4/#5, O4.1, O4.2 |
| D12 | **Hardware access for `wipe-engine-linux`** | Which drives (NVMe, SATA, SAS) and which machine, and who runs the tests? | v0.2 #1, W3.4, real TPM counter, real fingerprint |
| D13 | **`allowed_methods` outside the licence** | A cert whose method the licence does not allow: mark it `evaluation`, or ignore the restriction? (Never block the wipe either way — ROADMAP §0.2.) | the last open piece of `licensing-as-built` |
| D14 | **Sales tax / VAT** | Sell only in your home jurisdiction at first, or enable Stripe Tax before launch? | Taking real payments outside the home jurisdiction |
| ~~D15~~ | ~~Annual offerings through Stripe: one charge or a subscription~~ | **Decided 2026-11-08: (B) Stripe subscriptions.** Annual offerings check out with `mode=subscription` and renew automatically until cancelled. Each paid renewal extends the entitlement's term by 12 months. Cancelling lets the term run to its end. A failed renewal charge stops renewal but never touches licences already issued. Checkout shows an auto-renewal notice, its wording approved by the owner on 2026-11-08 and shipped verbatim on `/buy`, the test checkout and Stripe's checkout page. Built by offerings-and-payments story 4. | — |
| ~~D16~~ | ~~Stripe invoices~~ | **Settled 2026-11-08 by D15 (B).** Annual subscriptions get Stripe invoices with no add-on fee. One-off purchases keep receipts only (D16's default (A)). Turning on paid invoices for one-off purchases remains an owner cost choice if a buyer asks. | — |
