---
title: 'Evaluation and attestation on the Certificate page'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '5790732b482d6a11f58615a47b6cb26844c2f74d'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/docs/adr/0005-licensing-and-attestation.md']
warnings: ['oversized']
deferred:
  - summary: >-
      Nothing ties the TS licence-chain mirror and test fixture to the Rust serde output, so a renamed field would turn every licensed certificate into "Licence attached but unreadable".
    evidence: |-
      certificate.test.ts builds the chain by hand; no shared Rust-generated licensed-certificate fixture exists (crates/wipe-cert/tests/fixtures holds none with an attestation, and those files are frozen). Closing it needs a wipe-license test that writes a fixture the vitest suite imports, as catalog.test.ts does with catalog.json. Same root as story 1's TS-drift deferral.
    location: >-
      apps/desktop/src/api/certificate.test.ts
    severity: low
---

<intent-contract>

## Intent

**Problem:** Every certificate says inside its signed payload whether it is an **Evaluation certificate**, and a licensed one carries the **Attestation chain** naming the customer, but the TS `SignedCertificate` has neither field, so the Certificate page shows nothing: an operator cannot tell an evaluation certificate from a licensed one (SPEC CAP-2, ROADMAP O-R9; audit 2026-11-05 §(d)).

**Approach:** Add `certificate.evaluation` and an optional top-level `attestation` (a hand-mirror of `wipe_license::AttestationChain`) to the TS `SignedCertificate`; add a pure `licenceSummary(cert)` helper in `api/certificate.ts` that classifies the certificate the way `verify-cert` does without a vendor root; render it as a header pill and a "Licence" field on `CertificatePage`.

## Boundaries & Constraints

**Always:** The page never claims the chain is verified: the browser has no vendor root, so a licensed certificate is shown as "licensed to X per the attached licence", with a pointer to `verify-cert --vendor-root-b64`. The evaluation reading comes from `certificate.evaluation` (signed), not from the absence of a chain. A malformed `attestation` never crashes the page. CONTEXT §10 licensing row and README licensing paragraph and test table updated in the same commit; AGENTS.md vitest count updated.

**Never:** No Rust change (the serde shape already carries both fields). No chain verification in TypeScript (verification stays offline in `verify-cert`, ROADMAP §0.3). No licence banner (story 3), no install (story 4). No gating or warning-blocking of anything (ROADMAP §0.2). No hardware-sanitization claim. Do not edit `crates/wipe-cert/tests/fixtures/*`.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Evaluation | `evaluation: true`, no `attestation` | kind `evaluation`; pill "Evaluation"; field says produced without a valid licence, erasure and signature still valid | none |
| Licensed | `evaluation: false`, chain with `customer_name` "Acme", `customer_id` "c-1", `license_id` "lic-1" | kind `licensed`; field "Licensed to Acme (c-1)", detail names licence `lic-1` and that the chain is not checked here (verify with `--vendor-root-b64`) | none |
| Contradiction | `evaluation: true` and a chain | kind `inconsistent`; field warns the certificate is marked evaluation yet carries a licence | warning text, no throw |
| Unmarked | `evaluation: false`, no chain (pre-licensing cert) | kind `unmarked`; field says no licence attached and the certificate does not mark itself evaluation | none |
| Unreadable chain | `attestation` present but lacking `license.body.entitlements.customer_name` (string) | kind `unreadable`; field says a licence is attached that this page cannot read; verify offline | no throw |
| `evaluation` missing | older JSON without the key | treated as `false` | none |

## Autonomous decisions

1. **How strongly may the page state "licensed"?** *What:* the operator opens a certificate on the bench. *Problem:* the chain is signed by the vendor root, which the UI does not hold; showing "Licensed" flatly would claim a verification that did not happen. *Options:* (a) verify in TS with a bundled root; (b) show the customer named in the attached licence, labelled unchecked, pointing to `verify-cert`; (c) show nothing for licensed certs. *Choice:* (b). *Why:* it delivers CAP-2 ("the licensed customer") without overclaiming; (a) widens scope and duplicates verification that ADR-0005 places in the offline verifier.
2. **Which states to distinguish.** *Choice:* the four no-root outcomes `verify-cert` already prints (evaluation, chain present, both, neither) plus an unreadable chain. *Why:* mirrors existing, reviewed CLI wording; the contradictory cases are findings the CLI already flags and an auditor would want surfaced.
3. **Wording.** Operator-facing labels paraphrase `verify-cert`'s existing output and the CONTEXT §14 **Evaluation certificate** entry; no new product claim, so not a copy decision needing the owner.

</intent-contract>

## Code Map

- `crates/wipe-cert/src/schema.rs:99-101` -- `Certificate.evaluation: bool`, `#[serde(default)]`, always serialized. Read-only.
- `crates/wipe-cert/src/sign.rs:65-81` -- `SignedCertificate.attestation: Option<serde_json::Value>`, skipped when `None`. Read-only.
- `crates/wipe-license/src/chain.rs:74-118,246-252` -- `LicenseBody` (`license_format_version, license_id, root_key_id, instance_public_key_id, entitlements, issued_at` RFC 3339, `lapse_policy` omitted when evaluation), `LicenseSignature`, `AttestationChain {license, root_key_id}` -- shape to mirror.
- `crates/wipe-license/src/entitlement.rs:14-140` -- `Entitlements` (`customer_id, customer_name, quota, scope, not_before, not_after, features, allowed_methods, machine_binding?`); `Quota`, `Scope`, `AllowedMethods` are tagged enums -- mirror these as `unknown` to avoid inventing tag spellings; the page reads only customer/licence ids.
- `crates/wipe-cli/src/main.rs:725-810` -- `report_attestation`: the wording and four-way classification to mirror.
- `apps/desktop/src/api/types.ts:326-353` -- `LapsePolicy`, `LicenseCustomer` already exist (story 1); `:445-499` `SignedCertificate` -- add `evaluation: boolean` inside `certificate`, `attestation?: AttestationChain` at top level.
- `apps/desktop/src/api/certificate.ts` -- pure view helpers; add `licenceSummary`.
- `apps/desktop/src/api/certificate.test.ts` -- `cert()` fixture builder; add tests here.
- `apps/desktop/src/pages/CertificatePage.tsx:78-125` -- header and field grid; add pill next to the title and a "Licence" `Field`. `pill-warning`/`pill-danger` classes exist (`DispositionPill`).
- `CONTEXT.md:918`, `README.md:485-491`, `README.md:408` (vitest table, `certificate` (6)), `AGENTS.md:54` (256 tests).

## Tasks & Acceptance

**Execution:**
- `apps/desktop/src/api/types.ts` -- add `LicenseBody`, `LicenseSignature`, `LicenseCertificate`, `LicenceEntitlements` (quota/scope/allowed_methods `unknown`), `AttestationChain`; extend `SignedCertificate` -- TS mirror of the serde shape.
- `apps/desktop/src/api/certificate.ts` -- `licenceSummary(cert): { kind: "evaluation"|"licensed"|"inconsistent"|"unmarked"|"unreadable"; title: string; detail: string }`, defensive against malformed attestation -- testable view logic.
- `apps/desktop/src/api/certificate.test.ts` -- one test per I/O matrix row.
- `apps/desktop/src/pages/CertificatePage.tsx` -- header pill ("Evaluation" warning tone for `evaluation`/`inconsistent`/`unreadable`, "Licensed" for `licensed`, none for `unmarked`) and a "Licence" field showing title and detail.
- `CONTEXT.md`, `README.md`, `AGENTS.md` -- certificate page shows the marking; banner still pending; test counts.

**Acceptance Criteria:**
- Given a certificate signed by an unlicensed station, when the operator opens its Certificate page, then "Evaluation" is visible in the header without scrolling.
- Given the TS mirror, when `pnpm typecheck` runs, then `SignedCertificate` exposes `certificate.evaluation: boolean` and `attestation?: AttestationChain` and the page compiles.
- Given any certificate JSON, when `licenceSummary` runs, then it returns one of the five kinds and never throws.

## Spec Change Log

- 2026-11-08 (implementation): the header pill reads "Evaluation" for `evaluation` and `inconsistent` (both carry the signed `evaluation: true`) but "Licence unreadable" for `unreadable`, still in warning tone: labelling an `evaluation: false` certificate "Evaluation" would contradict the Always rule that the evaluation reading comes from the signed marker. "Licensed" uses the neutral `pill-info` tone, not success green, because the chain is not verified on the page. An `attestation: null` is treated as no chain, as ADR-0005 §5 describes unlicensed certificates.

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 23 findings — high 0, medium 4, low 12, false 7, maybe-false 0
- findings:
  - `[low]` `[reject]` Blind: a licence copied from another station shows "Licensed" (no instance-key-id vs signature key-id check) — the page only shows certificates this station signed with its own loaded licence, so a foreign chain is not reachable in everyday use; the fix adds a new branch. The detail already says the chain is unchecked.
  - `[false]` `[reject]` Blind: page drops verify-cert's "licence expired at signing" note — `LicenseContext::signing_decision` attaches a chain only when the lease is valid; an expired licence yields `evaluation: true` with no chain, so the case cannot reach the page.
  - `[low]` `[reject]` Blind: a partial chain (string `customer_name`, no signature/root id) reads as licensed — the server serializes a whole `AttestationChain`, so a partial chain does not arise in everyday use; the fix adds guards.
  - `[false]` `[reject]` Blind: the story misstates verify-cert's no-root classification — true of the spec's wording, not the code; the fix would edit this build's spec.
  - `[low]` `[patch]` Blind: the unmarked detail explains the case away as pre-licensing — reworded plainly: no licence and no evaluation mark; pre-licensing certificates look like this; check any other offline.
  - `[false]` `[reject]` Blind: TS types overstate (`evaluation` required, `attestation` never null) — the wire matches the types: `evaluation` is always serialized and `attestation` is skipped when `None`, never `null`; `licenceSummary` is only defensive.
  - `[low]` `[patch]` Blind: the "Verify offline" command lacks `--vendor-root-b64` while the Licence field points to it — the command adds a `--vendor-root-b64 <vendor root public key>` line when the certificate carries an attestation.
  - `[medium]` `[patch]` Blind: nothing tests the header pill — grouped with the verification-gap and intent rows below; the kind → pill/tone mapping moved to a pure helper in `api/certificate.ts` with a five-row test.
  - `[low]` `[patch]` Blind: untested branches (evaluation + `null` attestation; inconsistent + malformed chain) — tests added.
  - `[low]` `[patch]` Blind: `LicenceEntitlements` mixes spelling with the `License*` mirrors — renamed `LicenseEntitlements`.
  - `[low]` `[patch]` Blind: `features` optional though Rust always serializes it — made required.
  - `[low]` `[patch]` Blind: inconsistent detail repeats the offline-check sentence; unmarked detail is a double negative — duplicate sentence dropped; unmarked reworded (same patch as the unmarked row).
  - `[false]` `[reject]` Blind: ROADMAP not updated; triage log empty — O-R9/L1.4 are not fully shipped until story 3's banner, so nothing to strike; the triage log is written by this pass.
  - `[low]` `[reject]` Edge: partial chain shows Licensed — duplicate of the Blind partial-chain row; same reasoning.
  - `[low]` `[reject]` Edge: empty `customer_name` gives "Licensed to " — the hub issues licences with an organization name; not met in everyday use, and the fix adds a guard.
  - `[false]` `[reject]` Edge: unreadable pill reads "Licence unreadable", not "Evaluation" as the task line says — the code follows the intent contract's Always rule (evaluation only from the signed marker), logged in the Spec Change Log; the fix would edit this build's spec.
  - `[false]` `[reject]` Edge: page and verify-cert verdicts differ (no-root branch) — the divergence is the recorded autonomous decision 2 (surface the contradiction); the remaining claim is the spec's wording, whose fix edits this build's spec.
  - `[medium]` `[patch]` Verification gap: header pill and Licence field tone untested — same group as the Blind pill row; helper and tests added.
  - `[low]` `[defer]` Verification gap: TS chain fixture is hand-written, not tied to Rust serde output — needs a Rust-generated licensed-certificate fixture; the story rules out Rust changes and fixtures are frozen; deferred.
  - `[medium]` `[patch]` Intent: the rendered page has no test — same group as the pill row; the page's decisions now live in the tested helper (no DOM harness exists in `apps/desktop`).
  - `[medium]` `[patch]` Intent: label/tone decisions sit in the untested `.tsx` — same group; moved to the helper.
  - `[low]` `[defer]` Intent: tests feed hand-built JSON, not Rust-signed output — same root as the verification-gap fixture row; deferred.
  - `[false]` `[reject]` Intent: scope goes beyond "badge or licensed customer" — the extra states are autonomous decision 2, within CAP-2's "evaluation marking and attestation are visible".

## Design Notes

Read the chain defensively — it is opaque JSON on the Rust side:

```ts
const body = (cert.attestation as any)?.license?.body;
const name = body?.entitlements?.customer_name;
if (typeof name !== "string") return unreadable;
```

## Verification

**Commands:**
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: clean; new `licenceSummary` tests pass.
- `cargo test -p wipe-cert -p wipe-license` -- expected: pass (no Rust change; sanity).

## Auto Run Result

Status: done

**Summary:** The Certificate page now shows each certificate's licence marking. The TS `SignedCertificate` gains `certificate.evaluation` and an optional `attestation` (a hand-mirror of `wipe_license::AttestationChain`). `licenceSummary` sorts a certificate into evaluation, licensed, inconsistent, unmarked or unreadable from the signed marker and the attached chain. `licencePill`/`licenceIsWarning` turn that into a header pill ("Evaluation", "Licence unreadable", neutral "Licensed", or none) and a Licence field. A licensed certificate names the customer from the attached licence and says the chain is not checked here. When a chain is attached, the "Verify offline" command adds a `--vendor-root-b64` line. No Rust change.

**Files changed:**
- `apps/desktop/src/api/types.ts` -- `LicenseEntitlements`, `LicenseBody`, `LicenseSignature`, `LicenseCertificate`, `AttestationChain`; `evaluation` and `attestation` on `SignedCertificate`.
- `apps/desktop/src/api/certificate.ts` -- `licenceSummary`, `licencePill`, `licenceIsWarning`.
- `apps/desktop/src/api/certificate.test.ts` -- 13 new tests (each I/O row, null/malformed combinations, the pill per kind).
- `apps/desktop/src/pages/CertificatePage.tsx` -- header pill, Licence field, vendor-root line in the offline command.
- `CONTEXT.md`, `README.md`, `AGENTS.md` -- §10 licensing row, licensing paragraph, test table and counts (269).

**Review:** 23 findings (high 0, medium 4, low 12, false 7). Patched: one medium entry (pill and tone logic untested, from four rows) and six low entries (unmarked and inconsistent wording, the offline command's vendor-root line, two untested branches, the `LicenseEntitlements` name, required `features`). Deferred: the Rust↔TS fixture drift (two rows). Rejected: a copied foreign licence, a partial chain (two rows) and an empty customer name, all unreachable from a station's own certificates and needing new guards; expired-at-signing, which cannot occur (an expired lease gives an evaluation certificate with no chain); three findings whose fix would only edit this spec's wording; types overstating the wire (they match it); ROADMAP not struck (O-R9 needs story 3); scope beyond "badge or customer" (autonomous decision 2). Patched entries by verdict: high 0, medium 1, low 6. Follow-up review: not recommended (no high patched, only one medium).

**Verification:** `pnpm typecheck` clean; `pnpm test` 269 passed. `cargo test -p wipe-cert -p wipe-license` passed (no Rust change). The page has not been rendered in a browser: `apps/desktop` has no DOM test harness, so the page's choices are tested through the pure helpers.

**Residual risks:** The page reads the chain without verifying it, and says so; a forged or foreign chain would show its customer name labelled unchecked. The TS mirror can drift from Rust unnoticed (deferred). The station-wide banner (story 3) is still missing, so an operator sees "evaluation" only on a certificate, not before the first job.

