---
title: 'Destruction schema, techniques and identity assurance'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '1fb16d6626858586ea89ae98c7dfa3eeb1db7ee8'
review_loop_iteration: 0
followup_review_recommended: false
context:
  - '{project-root}/docs/adr/0011-destruction-by-operator-attestation.md'
  - '{project-root}/.scratch/destruction-attestation/research.md'
warnings: [oversized]
deferred:
  - summary: >-
      The escalate dialog's technique label and shred/pulverize caution have no render test; only the destruction.ts helpers are tested.
    evidence: |-
      No desktop test renders JobDetailPage or CertificatePage, and apps/desktop has no component-test toolchain (no @testing-library, jsdom or happy-dom). Passing a wrong classification to EscalateButton, or removing the caution block, would leave every test green. Adding a render-test toolchain is outside this story.
    location: >-
      apps/desktop/src/pages/JobDetailPage.tsx:141
    severity: low
  - summary: >-
      A station built before Feature::OperatorSso rejects any licence that grants operator_sso, because Feature is a closed enum.
    evidence: |-
      entitlement.rs makes Feature closed on purpose ("so a license cannot name a feature this build does not understand"), so this is the existing design, and every new Feature variant behaves the same way. Under ROADMAP §0.2 an unreadable licence still erases, with certificates marked evaluation. It matters once deployed stations exist and the hub can grant features those stations do not know; the fix is a version-aware grant rule or a tolerant reader.
    location: >-
      crates/wipe-license/src/entitlement.rs:65
    severity: medium
---

<intent-contract>

## Intent

**Problem:** ADR-0011 needs records the code cannot hold yet: an operator's "recorded for destruction" attestation, a run's "destruction completed" confirmation, an optional concurrence, and the signer's identity assurance. The technique list also only has the five NIST Destroy techniques, so a bench that crushes, punches, degausses or drills drives cannot record what it actually did, and nothing tells the operator which techniques are not NIST Destroy.

**Approach:** Add the ADR-0011 record types to `wipe-common`, with constructors that validate input and render the research.md statements word for word. Add optional slots for them on `DestructionEvent` and `DestructionManifest`. Grow `DestructMethod` to nine techniques, with an honest standard label and the shred/pulverize caution. Add `Feature::OperatorSso`, and mirror all of it in TypeScript. Show the honest label and the caution wherever the desktop UI names a technique today. Nothing in this story produces an attestation, completion or concurrence; stories 2–4 do.

## Boundaries & Constraints

**Always:**
- New `DestructionEvent` fields are `#[serde(default, skip_serializing_if = "Option::is_none")]`. `DestructionEvent` is inside v2 signed certificates, and `verify` re-canonicalizes, so an existing certificate must serialize to the same bytes.
- Statement text is produced only by the Rust constructors, from the research.md wording: "I, [name, title], …" renders as `I, {full_name}, {title}, …`; the date/time renders as RFC 3339 UTC; the "Video reference: [ID]." sentence is left out when there is no video reference.
- The technique label is exactly `NIST SP 800-88 Destroy` for shred, disintegrate, incinerate, pulverize and melt, and exactly `physical damage — not NIST Destroy` for crush, punch, degauss and drill. Rust and TS pin the same literals in tests.
- A concurrence's kind fixes its statement, so a reviewer's concurrence can never carry the witness wording.
- Rust shapes and `apps/desktop/src/api/types.ts` change together. The `Feature` serde names stay mirrored in `wipe-hub` `FEATURES`, portal `adminOfferings.ts` and `wipe-cli` `parse_feature`.
- Destruction never depends on licence state (ROADMAP §0.2). `OperatorSso` gates nothing in this story.

**Never:**
- No runner transition, route, station-config wiring of the concurrence policy, cert-schema change or `PendingCoSign` rename (stories 2–4).
- No edit to how `Method::Destroy` is selected: `select_method` still produces only `Disintegrate`.
- No SSO sign-in. Only `self_asserted` is constructed.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Attestation | name "Ana Ruiz", title "Bench lead", Shred, equipment "SHR-2", location "Bay 3", scanned asset tag | statement starts `I, Ana Ruiz, Bench lead, recorded the storage device(s) listed above for destruction by Shred using SHR-2 at Bay 3.`; assurance `{"level":"self_asserted"}` | — |
| Blank input | blank name, title, equipment, location, or neither asset tag nor serial | no record is created | `Err(DestructionRecordError::…)` naming the field |
| Completion unverified | `remnants_inspected` or `equipment_cycle_completed` false, or blank criterion | no record is created | `Err` |
| Completion, no video | `video_ref: None` | statement has no "Video reference" sentence | — |
| Concurrence | kind Review vs Witness | `…reviewed the destruction records and verification evidence for manifest {id}…` vs `…witnessed the destruction for manifest {id}…` | — |
| Old event | `DestructionEvent` JSON with no `attestation` key | deserializes to `None`, reserializes without the key | — |
| Caution | Shred or Pulverize at Moderate/High | caution text returned/shown | none at Low or for other techniques |

## Autonomous decisions

1. **The `PendingCoSign` rename waits for story 2.** *About:* the Job state between escalation and destruction, which ADR-0011 renames "pending destruction". *Problem:* the SPEC assigns the rename to no story, and doing it here means rewriting the CONTEXT, README and ARCHITECTURE entries for that state now, then again when story 2 changes the transition into it. *Options:* (a) rename here, behaviour unchanged; (b) rename in story 2, together with the new "record for destruction" transition. *Choice:* (b). It is safe to defer because nothing changes behaviour in the meantime, and the docs then change once, in step with the code.
2. **Records are validated where they are constructed.** *About:* the attestation, completion and concurrence statements an operator signs. *Problem:* a statement saying "the remnants were inspected" must never be recorded when they were not. *Options:* validate in each later route, or in the constructors. *Choice:* the constructors return `Result`, refusing blank required fields and a completion whose verification booleans are not both true. This is the safe default for the audit trail, and routes in stories 2–3 must build records only through them.
3. **The technique is not repeated inside the attestation.** It stays as `DestructionEvent::method`, and `DestructionAttestation::new` takes it only to render the statement, so the two can never disagree. The equipment ID for completion goes on `DestructionVerification`, because the completion statement names it.
4. **The concurrence policy type is added, but its station-config wiring waits for story 3**, which owns the policy behaviour (SPEC CAP-3).
5. **ADR-0001 gets an addendum now that points to ADR-0011's three amendments.** It records which ADR governs, not built behaviour, so it is true already.
6. **Statement wording.** The technique renders by its human name (for example "Shred"). The concurrence statement's "[ID]" is the manifest UUID.

</intent-contract>

## Code Map

- `crates/wipe-common/src/method.rs:77-85` -- `DestructMethod` (5 variants, snake_case serde). Add `Crush, Punch, Degauss, Drill` after `Melt`, plus `ALL`, `is_nist_destroy`, `human_name`, `standard_label`, `caution(Classification) -> Option<&'static str>`. `Method::Destroy`/`category()` (`:65`, `:98`) stay as they are; `select_method` (`:125`) only builds `Disintegrate`.
- `crates/wipe-common/src/job.rs:171-194` -- `DestructionEvent`: add `attestation: Option<DestructionAttestation>`. `:285-327` `DestructionManifest` + `new`: add `completion`, `concurrence` (`#[serde(default)]`, `None` in `new`).
- `crates/wipe-common/src/lib.rs` -- add `pub mod destruction; pub use destruction::*;`.
- `crates/wipe-common/src/operator.rs` -- `OperatorRef`, reused inside the signer.
- `crates/wipe-common/Cargo.toml` -- `thiserror` is available for the error enum.
- Struct literals that need `attestation: None`: `crates/wipe-server/src/handlers.rs:290`, `crates/wipe-server/tests/http_e2e.rs:1065`, `crates/wipe-engine-mock/tests/end_to_end.rs:255`.
- `crates/wipe-license/src/entitlement.rs:54-65` -- `Feature`; add `OperatorSso` last.
- `crates/wipe-hub/src/offering_admin.rs:52-60,646-672` -- `FEATURES` + exhaustive `name_of` / mirror test.
- `crates/wipe-cli/src/main.rs:166-167,897-906` -- `--feature` help text and `parse_feature`.
- `apps/portal/src/api/adminOfferings.ts:24-39` + `adminOfferings.test.ts:190-198` -- `FEATURES`, `FEATURE_LABELS`, mirror test.
- `apps/desktop/src/api/types.ts:69-74,252-263,282-291` -- `DestructMethod`, `DestructionEvent`, `DestructionManifest`.
- `apps/desktop/src/pages/JobDetailPage.tsx:170-260` -- `EscalateButton` technique picker (hard-coded five); `:322-340` destruction activity card.
- `apps/desktop/src/pages/CertificatePage.tsx:118-125` -- Destruction field.
- `CONTEXT.md:327-394` (DestructionEvent, DestructionManifest entries), `:1100+` §14 glossary; `docs/adr/0001-job-as-outcome-bearing-composition.md` (addendum).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-common/src/destruction.rs` (new) -- `IdentityAssurance` (`#[serde(tag = "level", rename_all = "snake_case")]`: `SelfAsserted`, `Verified { issuer, subject }`); `AttestationSigner { operator, full_name, title, identity_assurance }` + `self_asserted(..)`; `IdentifierSource { Scanned, Device }`; `RecordedIdentifiers { asset_tag, serial, source }`; `DestructionAttestation { at, equipment_id, location, identifiers, signer, statement }` + `new(technique, ..)`; `DestructionVerification { remnants_inspected, equipment_cycle_completed, equipment_id, criterion }`; `DestructionCompletion { completed_at, verification, video_ref, signer, statement }` + `new`; `ConcurrenceKind { Review, Witness }`; `ConcurrencePolicy { None (default), Review, Witness }` + `required_kind()`; `Concurrence { kind, manifest_id, at, signer, statement }` + `new`; `DestructionRecordError` (thiserror). Constructors trim inputs and return `Result`. Unit tests cover every I/O matrix row, the JSON shapes, and the default policy -- this is the ADR-0011 data model.
- `crates/wipe-common/src/method.rs` -- techniques, labels and caution as in the Code Map, with tests pinning the labels, `ALL` order and caution cases -- CAP-5.
- `crates/wipe-common/src/job.rs`, `lib.rs`, and the three literal sites -- slots and fixtures; add a test that an event without an attestation serializes with no `attestation` key -- keeps v2 certs byte-stable.
- `crates/wipe-license/src/entitlement.rs`, `crates/wipe-hub/src/offering_admin.rs`, `crates/wipe-cli/src/main.rs`, `apps/portal/src/api/adminOfferings.ts` (+ test; label `Operator SSO`) -- `operator_sso` -- CAP-6.
- `apps/desktop/src/api/types.ts` -- mirror every new type; `attestation?: DestructionAttestation` (it is omitted when absent); `completion`/`concurrence: … | null`.
- `apps/desktop/src/api/destruction.ts` (new) + `destruction.test.ts` -- `DESTRUCTION_TECHNIQUES` (Rust `ALL` order), `isNistDestroy`, `techniqueName`, `techniqueStandardLabel`, `techniqueCaution(technique, classification)`, `identityAssuranceLabel` ("Self-asserted" / "Verified by {issuer}"), with the same literals the Rust tests pin.
- `apps/desktop/src/pages/JobDetailPage.tsx` -- the picker lists all nine techniques, grouped under the two labels; the selected technique's label shows, and the caution shows for the Job's classification. The activity card shows the technique name and label, and the identity assurance when there is an attestation.
- `apps/desktop/src/pages/CertificatePage.tsx` -- the Destruction field shows the technique name and label, and "Identity: {assurance}" when there is an attestation -- CAP-6's certificate page.
- `CONTEXT.md` -- §5 Job orchestration: new entries for **Recorded for destruction**, **Destruction completed**, **Concurrence** (and its policy), **Identity assurance** and **Destruction technique**, each with a **Today:** line saying schema only (nothing produces it); DestructionEvent/Manifest entries name the new slots. §14: matching one-liners.
- `docs/adr/0001-job-as-outcome-bearing-composition.md` -- addendum (2026-11-08): ADR-0011 amends it on three points, and those govern.

**Acceptance Criteria:**
- Given a desktop operator escalating a Moderate Job, when they open the escalate dialog, then they can pick any of the nine techniques; choosing Crush shows `physical damage — not NIST Destroy`, and choosing Shred shows `NIST SP 800-88 Destroy` with the shred/pulverize caution.
- Given `POST /api/jobs/:id/escalate-to-destroy` with `"method":"crush"`, when it is handled, then it succeeds and the Job's destruction activity carries `crush`.
- Given a v2 signed certificate produced before this change, when it is verified, then it still verifies (the existing `wipe-cert` and http e2e cert tests pass unchanged).
- Given the hub, portal and CLI, when a licence or offering names `operator_sso`, then it is accepted, and the `Feature` mirror tests pass.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 24 findings — high 0, medium 1, low 17, false 6, maybe-false 0
- findings:
  - `[false]` `[reject]` Completion and concurrence statements say "Destruction" or "destroyed" even for techniques that are not NIST Destroy — research.md's rule covers the NIST category term "Destroy", which neither statement uses. Both statements are the research wording the SPEC adopted word for word.
  - `[false]` `[reject]` A crush Job ends with disposition `Destroyed` and no recorded decision — ADR-0011 §5 decides this: every technique follows the destruction path, and only the "NIST Destroy" claim is limited to the five techniques.
  - `[low]` `[reject]` Records have pub fields and derive Deserialize, so the constructors' validation can be bypassed — the module docs, CONTEXT and autonomous decision 2 all require routes to use the constructors. Enforcing it would mean private fields plus getters on about 20 fields, and nothing bypasses the constructors today.
  - `[low]` `[reject]` `AttestationSigner::checked` does not check that a `Verified` signer has an issuer and subject — nothing constructs `Verified`, which waits for the SSO sign-in. The fix would add a guard for a state nothing can reach.
  - `[low]` `[reject]` The TypeScript technique rules are a hand-kept copy with no test comparing them to Rust — this follows the AGENTS.md hand-mirroring convention (ADR-0002 keeps only bay-topology matching in Rust), and both test suites pin the same literals. Generating a shared fixture would add tooling for little gain.
  - `[low]` `[defer]` The escalate dialog's acceptance criterion has no render test — the repo has no component-test toolchain. The helpers that decide the outcome are tested. Deferred (see `deferred`).
  - `[low]` `[patch]` Rust test gaps: identifier trimming, `IdentifierSource::Device`, concurrence with a blank signer, a manifest round trip with completion and concurrence — fixed: three tests added in `destruction.rs` and `job.rs`.
  - `[false]` `[reject]` The attestation and concurrence statements carry no time — each record's `at` sits in the same record, and story 4 signs the whole record. Checking that the times are in order belongs to story 3's runner transitions.
  - `[false]` `[reject]` Non-NIST techniques at High classification get no caution — CAP-5 asks for the caution only for shred and pulverize, and every non-NIST technique already shows its "not NIST Destroy" label in amber.
  - `[low]` `[patch]` The CONTEXT §14 Identity assurance line lacks "Schema only today" — fixed: appended.
  - `[false]` `[reject]` `Method::Destroy { Crush }` would report category Destroy — no route sets an erasure method. `select_method` builds only `Disintegrate`, and the Rust `JobSpec` has no method override, so nothing can reach this case.
  - `[low]` `[reject]` Verified signer with a blank issuer (edge-case layer) — same claim and same reasoning as the blind-hunter finding above.
  - `[low]` `[reject]` Newlines or control characters inside signer or equipment fields stay in the statement — operators type these into single-line form fields, so it is unlikely, and the fix would add a new validation rule.
  - `[medium]` `[defer]` An older station rejects a licence that grants `operator_sso` — the closed `Feature` enum is the existing, documented design, and every new feature behaves the same way. Deferred (see `deferred`).
  - `[low]` `[reject]` Claim that statements are "rendered only by the constructor" while the fields are public — that is true: nothing else renders a statement. Same root cause as the pub-fields finding above.
  - `[low]` `[defer]` Escalate dialog caution has no render test (verification-gap layer) — grouped with the blind-hunter render-test finding above, on the same reasoning.
  - `[low]` `[patch]` `parse_feature` in the CLI has no test — fixed: added an exhaustive test over every `Feature` serde name, plus a check that an unknown name is refused, in `crates/wipe-cli/src/main.rs`.
  - `[low]` `[reject]` Intent audit (a): the standard label is not inside the signed certificate — the intent's story split gives the certificate schema to story 4. Changing v2 certificate content would break the guarantee that v2 certificates still verify.
  - `[low]` `[reject]` Intent audit (b): the verification criterion is not shown next to the label — no criterion exists until story 3 creates completions.
  - `[low]` `[reject]` Intent audit (c): the caution is not printed on the certificate — that is story 4, the certificate-schema story.
  - `[low]` `[reject]` Intent audit (d): the "Identity" line can never appear yet — nothing produces an attestation until story 2.
  - `[low]` `[reject]` Intent audit (e): "every attestation carries identity_assurance" holds only by construction, because no attestation exists yet — story 2 creates them.
  - `[low]` `[reject]` Intent audit (f): the concurrence policy is not yet a station setting — story 3, titled "…concurrence policy", owns it.
  - `[false]` `[reject]` Intent audit (g): the diff reaches beyond wipe-common, wipe-license and the TS mirror — AGENTS.md requires the Feature mirrors and the doc updates in the same commit, and CAP-5 and CAP-6 name UI surfaces.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass, including the new `wipe-common` tests.
- `cargo clippy --workspace --all-targets` -- expected: no new warnings in touched files.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: pass.
- `pnpm --filter portal typecheck && pnpm --filter portal test` -- expected: pass.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.

## Auto Run Result

Status: done

**Summary.** Adds the ADR-0011 data model to `wipe-common`: the "recorded for destruction" attestation, "destruction completed", concurrence and its site policy, and identity assurance. Each record has a validating constructor that renders the research.md statement word for word. Optional slots for the records are added to `DestructionEvent` and `DestructionManifest`; the event's slot is left out of the JSON when empty, so v2 certificates keep their bytes. There are now nine destruction techniques, each with an honest standard label, and the shred/pulverize caution applies above Low. `Feature::OperatorSso` is mirrored in the hub, portal and CLI. The TypeScript mirror is updated, and the escalate dialog, Job activity card and certificate page now show each technique's label and the caution. CONTEXT §5 and §14 have entries for the new terms, and ADR-0001 has an addendum.

**Files changed**
- `crates/wipe-common/src/destruction.rs` — new: the record types, constructors, statements and their tests.
- `crates/wipe-common/src/method.rs` — nine techniques, `ALL`, `is_nist_destroy`, `human_name`, `standard_label`, `caution`, and tests.
- `crates/wipe-common/src/job.rs`, `lib.rs` — the new record slots and tests for serialization compatibility.
- `crates/wipe-server/src/handlers.rs`, `tests/http_e2e.rs`, `crates/wipe-engine-mock/tests/end_to_end.rs` — fixtures, plus an e2e test that a `crush` escalation is accepted.
- `crates/wipe-license/src/entitlement.rs`, `crates/wipe-hub/src/offering_admin.rs`, `crates/wipe-cli/src/main.rs` (with a new test), `apps/portal/src/api/adminOfferings.ts` (and its test) — `operator_sso`.
- `apps/desktop/src/api/types.ts`, `api/destruction.ts` (and its test), `pages/JobDetailPage.tsx`, `pages/CertificatePage.tsx` — the TypeScript mirror and the UI.
- `CONTEXT.md`, `docs/adr/0001-job-as-outcome-bearing-composition.md` — docs.

**Review.** 24 findings. 3 were patched (all low): Rust test gaps, a CLI `parse_feature` test, and the CONTEXT §14 status line. 2 were deferred: no render test for the escalate dialog (low), and older stations rejecting a licence that grants `operator_sso` (medium, existing design). 19 were rejected; each reason is in the triage log. Patched by verdict: high 0, medium 0, low 3. No follow-up review is recommended.

**Verification**
- `cargo test --workspace` with compose up: 743 passed, 0 failed on the final run. An earlier full run failed only `wipe-hub/tests/health.rs::silent_smtp_and_database_time_out_within_bound`. That test has a 3.5 s wall-clock limit, and it fails or passes from run to run on both the baseline and this tree. This story does not touch the hub's health code.
- `cargo clippy --workspace --all-targets`: no warnings in touched files.
- rustfmt is clean on the changed lines. The diffs it still reports at `method.rs:45` and `job.rs:16` were already there.
- Desktop: typecheck passes and 222 vitest tests pass. Portal: typecheck passes and 240 tests pass.
- I did not open the escalate dialog in the running app.

**Residual risks**
- Records can still be built without their constructors, because their fields are public. Stories 2 and 3 must create them only through the constructors.
- The shred/pulverize caution wording ("NIST SP 800-88 Rev. 2: shred and pulverize should be avoided for anything but the lowest security categories.") paraphrases NIST. Operators see it, so it is worth a product look.
- The `PendingCoSign` rename is deferred to story 2 (autonomous decision 1).
- The `wipe-hub` health timing test is flaky.
