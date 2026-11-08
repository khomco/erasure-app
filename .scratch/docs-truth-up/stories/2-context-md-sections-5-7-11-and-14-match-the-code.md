---
title: 'CONTEXT.md sections 5, 7, 11 and 14 match the code'
type: 'chore'
created: '2026-11-08'
status: 'done'
baseline_revision: '68387f379e7cfb184d7f78a87052531e5b081a49'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/.scratch/audit-2026-11-05/shipped-claims.md']
warnings: ['oversized']
deferred:
  - summary: >-
      CONTEXT.md §12 still contradicts the corrected §5: its resolved Verification item says the inner `Verifying` state runs the sampled reads, and its Lead question quotes §5 wording this story replaced.
    evidence: |-
      CONTEXT.md §12 (~line 951 "The inner `Verifying` state runs the sampled reads"; Lead question quoting "holds canonical config, audit aggregation, license state"). ErasureEventState has no Verifying variant (crates/wipe-common/src/erasure_event.rs:50-59). §12 is outside SPEC CAP-2's sections; the Verifying sentence was already false before this story.
    location: >-
      CONTEXT.md §12
    severity: medium
  - summary: >-
      Escalating a Queued Job to Destroy can race the runner's start: run_job sets InProgress unconditionally, so a PendingCoSign Job may revert to InProgress and then Erased, overwriting its Destroyed certificate.
    evidence: |-
      crates/wipe-engine/src/runner.rs:303-311 sets InProgress without checking the current state; escalate_to_destroy (runner.rs:178-210) accepts Queued. Unverified: would be settled by a test that escalates between start() and run_job's first mutation. Code defect, not docs.
    location: >-
      crates/wipe-engine/src/runner.rs:303
    severity: medium (unverified)
  - summary: >-
      CONTEXT.md §14 Lapse policy says "stations enforce it", but no station code reads LapsePolicy (enforcement is licence-visibility story 5, unbuilt).
    evidence: |-
      grep -i lapse in crates/wipe-server, wipe-engine and wipe-cli finds no enforcement. The entry is a hub-side §14 entry this story's boundaries leave untouched.
    location: >-
      CONTEXT.md §14 Lapse policy
    severity: low
---

<intent-contract>

## Intent

**Problem:** CONTEXT.md §5 lists an inner `Verifying → GeneratingCert → Signing` ErasureEvent path, retries, a station id on events, an "independent" co-signature and a Lead that holds config and licence state; §7's PXE discipline reads as built (certs streamed, keys off-station, licence fetched at boot); §11 calls items shipped without their gaps; §14 calls `wipe-hub` the control plane and lacks the licensing, catalog and store terms the other docs use. Agents read CONTEXT.md as the domain truth.

**Approach:** Edit only those four sections so each present-tense sentence matches code at 68387f3; keep target designs, but label them as target with a "Today:" statement; add the eight missing §14 terms.

## Boundaries & Constraints

**Always:** Only `CONTEXT.md` §5, §7, §11, §14 (plus this story file). Every corrected sentence traceable to a Code Map anchor. "Licence" in prose, flag/type names verbatim. ROADMAP §0.1: no claim that a drive has really been sanitized. ADR-0011 (destruction by attestation) and ADR-0012 enforcement described as accepted/not built. Keep §11 item numbering and strike-throughs.

**Never:** Change code, tests, ADRs (story 3), `docs/design/*` or UI copy (story 4), README/ARCHITECTURE (story 1), AGENTS.md, CONTEXT §10.2 (SPEC non-goal), or the hub-side §14 entries (Registered station … Email-send throttle), which were written with their stories and are not in the audit. Delete a target design: mark it as target instead.

</intent-contract>

## Code Map

- `crates/wipe-common/src/erasure_event.rs:40-59` -- `ErasureEventState` = Queued, Probing, Unfreezing, Confirming, Running, Completed, Failed, Aborted; doc comment says Verifying/GeneratingCert/Signing moved out. `:131,148` + `crates/wipe-engine/src/runner.rs:540` -- `station_id` field exists, always `None`.
- `crates/wipe-engine/src/runner.rs:125,295,356` -- comments cite `retry_erasure`, which does not exist: at most one ErasureEvent per Job. Failed event ⇒ Job stays InProgress; operator may escalate to destroy or abort. `:186-205` escalate allowed from any non-terminal state (incl. Queued, no erasure).
- `crates/wipe-common/src/job.rs` -- `JobState::Quarantined` declared, never entered (`grep Quarantined crates`).
- `crates/wipe-server/src/app.rs:200-275` -- cert generator: on `Erased`/`PendingCoSign`; skips Jobs with no erasure activity; PendingCoSign cert signed with a synthesized `Destroyed` disposition before any supervisor acts; licensing decides `evaluation` vs attestation (`:246-269`).
- `crates/wipe-server/src/handlers.rs:403-427` -- manifests held in the assembling station's RAM (`state.manifests`); `manifest_ref` never stamped; co-sign uses the station's own key, supervisor is an `OperatorRef` label. `:62-65` Lead is only reported (`/api/fleet/lead`); no config, audit or licence role.
- `crates/wipe-server/src/store.rs:37-56,170-214` -- `StoreTier` = LocalFile | ControlPlane | Ephemeral; tier 3 is `EphemeralStore` with `awaiting_decision`; `ControlPlaneStore::probe` always errors (no HTTP client).
- `crates/wipe-cli/src/main.rs:687-689` -- signing key from `--key-path` file or fresh each start, held by the station; `:735-755` licence loaded from `--license` file, checked against `--vendor-root-b64`, `LeaseState::new(now)` each start (not persisted). `:496-498` simulator (identify mode) wired by the CLI only.
- `crates/wipe-license/src/chain.rs:1-17,85-87` -- attestation chain: vendor root → `LicenseCertificate` → `instance_public_key_id` → erasure cert; `LapsePolicy` carried, enforcement is the station's (unbuilt). `lease.rs:19-35` -- `LeaseState { time_watermark, erasures_used, counter_is_monotonic }`; doc claims persistence falsely; `FileCounter` named but absent; `TpmCounter` placeholder. `crates/wipe-server/src/licensing.rs:73-92` -- `record_erasure` in RAM; `permits_licensed_signing`.
- `crates/wipe-cert/src/schema.rs:77-79` -- `certificate.evaluation: bool` inside signed payload.
- `crates/wipe-common/src/catalog.rs:180-205,281` -- `EnclosureModel` (`art: Option<String>` = key into frontend shell registry), bundled `data/catalog.json`. `apps/desktop/src/bench/shells/registry.ts` -- shell registry, `GenericShell` fallback labelled generic.
- `apps/desktop/src-tauri/src/lib.rs:117-147` -- desktop shell: no store, licence or simulator.

## Tasks & Acceptance

**Execution:**
- `CONTEXT.md` §5 -- JobState note Quarantined never entered; PendingCoSign: cert signed at PendingCoSign already claiming Destroyed, co-sign adds a block signed with the station key (not independent), no cert for a Job with no erasure activity, ADR-0011 replaces this (accepted, not built); ErasureEvent: at most one per Job today, `station_id` always empty, no retry; ErasureEventState real states, Failed ⇒ escalate or abort; VerificationEvent: no inner Verifying state, the runner appends it after Completed; DestructionEvent: escalation from any non-terminal state; DestructionManifest: held in the RAM of the station it was assembled on, not the lead; Lead: elected and reported only today.
- `CONTEXT.md` §7 -- keep each PXE item as the target and add a "Today:" line: no PXE image; certs in station RAM, served by `GET /api/jobs/:id/certificate`, lost on restart, not streamed; licence from a `--license` file, lease reset each start; signing key on the station (`--key-path` file or fresh per start); tier 2 is a stub so a PXE station lands on tier 3/4.
- `CONTEXT.md` §11 -- #2 add not-built remainder (retry, Quarantine, station id, independent co-sign); #8 list what is not built (lease persistence, `FileCounter`, `allowed_methods`, root-only verification, lapse-policy enforcement); #10 note tier 2 stub and identify mode/topology store wired by the CLI only; #13 `wipe-hub` exists (ADR-0007) but serves neither seam.
- `CONTEXT.md` §14 -- fix ErasureEvent, PendingCoSign, DestructionManifest, Control plane entries; add evaluation certificate, attestation chain, licence certificate, instance key, lease / watermark, enclosure model / catalog, shell, store tier.

**Acceptance Criteria:**
- Given CONTEXT.md, when grepping §5/§14 for `Verifying`, `GeneratingCert`, `independently verifiable`, `retries`, then no match asserts a current feature.
- Given §7, when reading each PXE item, then it states what happens today (certs not streamed; signing keys on the station; licence not fetched; tier 2 unavailable).
- Given §14, when searching for each of the eight terms, then each has an entry consistent with its Code Map anchor, and "Control plane" no longer equates to `wipe-hub` as built.
- Given `cargo test --workspace` and `cd apps/desktop && pnpm typecheck && pnpm test`, then both pass (no code changed).

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 29 findings — high 0, medium 12, low 14, false 2, maybe-false 1
- findings:
  - `[medium]` `[defer]` (blind) §12 Verification item still says the inner `Verifying` state runs the sampled reads — pre-existing false text in §12, outside CAP-2's sections; recorded in `deferred`.
  - `[low]` `[defer]` (blind) §12 Lead question quotes the §5 wording this story replaced — same root (§12 not in scope); recorded with the §12 deferral.
  - `[medium]` `[patch]` (blind) §5/§14 Job "signs one Certificate" unqualified; Aborted Jobs and Jobs with no erasure get none — Today: qualifier added to both entries.
  - `[low]` `[patch]` (blind) Evaluation certificate "never whether the erasure runs" ignores ADR-0012 `stop` — exception named, marked not yet enforced.
  - `[low]` `[reject]` (blind) "Licence certificate" duplicates the hub "Licence" entry — SPEC CAP-2 requires the term; it names the signed type and cross-links the hub entry (the stored file/row), so the two are distinct and linked.
  - `[medium]` `[patch]` (blind) "Shell" clashes with "desktop shell" used in §6/§10.1/AGENTS.md — entry now defines both senses (enclosure shell; desktop shell with its limits).
  - `[medium]` `[patch]` (blind) Co-sign entries omit that signer/role sit outside the signed bytes and `verify-cert` ignores co-signatures — added in §5 PendingCoSign/DestructionManifest and §14 DestructionManifest.
  - `[medium]` `[patch]` (blind) DestructionEvent omits re-escalation from PendingCoSign and escalation mid-erasure — grouped with edge findings 3 and 5; clauses added.
  - `[low]` `[patch]` (blind) JobState diagram lacks Queued → PendingCoSign — added to its Today: line with abort from any non-terminal state.
  - `[low]` `[patch]` (blind) DestructionEvent `station_id` always empty (handlers.rs:300) unmentioned — added.
  - `[medium]` `[patch]` (edge) Backend error mid-attempt leaves the ErasureEvent non-terminal, never Failed (runner.rs `?`) — added to ErasureEventState.
  - `[low]` `[patch]` (edge) `verify()` error appends no VerificationEvent (runner.rs:526-533) — clause added, with "only when the spec asks".
  - `[medium]` `[patch]` (edge) Escalating while Running doesn't cancel the erasure; cert signed while the drive is written (poll loop honours only Aborted, runner.rs:447) — grouped with blind DestructionEvent finding; added.
  - `[maybe-false]` `[defer]` (edge) Escalate-from-Queued race with run_job's unconditional InProgress — code race, would be settled by a targeted test; recorded in `deferred` as medium (unverified).
  - `[medium]` `[patch]` (edge) Re-escalating a PendingCoSign Job re-signs and replaces the stored cert — grouped with blind DestructionEvent finding; added.
  - `[low]` `[patch]` (edge) Abort allowed from PendingCoSign leaves the Destroyed-claiming cert servable (runner.rs:153-170) — clause added.
  - `[medium]` `[patch]` (edge) No reject route; `ManifestState::Rejected` never set, so a refused manifest blocks its Jobs (handlers.rs:376-387) — added to DestructionManifest.
  - `[low]` `[patch]` (edge) Evaluation cert "verifies offline like any other" omits the needed station public key, which changes per start without `--key-path` — reworded.
  - `[low]` `[patch]` (edge) `--key-path` to a missing file generates and writes a key (main.rs:701-704) — §7 item 4 and Instance key corrected.
  - `[medium]` `[defer]` (edge) §12 contradiction — duplicate of the blind §12 finding; deferred with it.
  - `[low]` `[defer]` (edge) §14 Lapse policy "stations enforce it" while nothing enforces it — pre-existing hub-side entry outside this story's boundaries; recorded in `deferred`.
  - `[low]` `[patch]` (edge) Co-signature block does carry `manifest_ref` (handlers.rs:479-485) though Jobs are unstamped — stated.
  - `[medium]` `[patch]` (edge) "desktop app neither saves a layout" wrong: default ephemeral store accepts saves for the session (app.rs:92-95) — §11 #10 corrected.
  - `[low]` `[patch]` (edge) Instance key "not a machine name" wrong: `required_fingerprint` via `machine_binding`/`Scope::Machine` (entitlement.rs:148-154) — replaced.
  - `[medium]` `[patch]` (intent) R3: §14 Job still "signs one Certificate" — duplicate of the blind Job finding; same patch.
  - `[false]` `[reject]` (intent) R4(a) vs (b) for PXE items — CAP-2 asks that the items "describe what happens today"; each now does in a Today: line, and keeping the target is a recorded decision (Design Notes).
  - `[low]` `[patch]` (intent) §11 #10 omits that the desktop skips tier detection — grouped with the desktop-save finding; the corrected text says saves happen without the tier-3 prompt.
  - `[low]` `[reject]` (intent) Verification is lexical and nothing traces sentences to anchors — the fix edits this build's spec; every finding above was checked against the cited code.
  - `[false]` `[reject]` (intent) Contract written by the same change — SPEC CAP-2 and the stories.yaml entry are fixed upstream; all CAP-2 success items are met.

## Design Notes

Autonomous decisions:
- **Target vs today in §7.** The PXE discipline is a design target the audit says is unbuilt. Options: delete it, rewrite it as as-built, or keep it and add "Today:". Chosen: keep + "Today:" — the target is still the product direction (ROADMAP), and deleting design intent is not a docs-truth fix.
- **§11 scope.** SPEC CAP-2 names §11 without listing items. Chosen: correct only items whose shipped/decided wording overstates the code (#2, #8, #10, #13); leave purely future items alone.

## Verification

**Commands:**
- `grep -nE 'Verifying|GeneratingCert|independently verifiable|retries' CONTEXT.md` -- expected: only denials or future-tense matches.
- `cargo test --workspace` and `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: pass.

## Auto Run Result

Status: done

**Summary.** CONTEXT.md §5, §7, §11 and §14 now match the station as built at 68387f3. §5: the real inner ErasureEvent states (no Verifying/GeneratingCert/Signing), at most one ErasureEvent per Job and no retry, `station_id` empty, `Quarantined` never entered, escalation and abort paths, the PendingCoSign certificate signed before any supervisor acts, a co-signature made with the station key (signer outside the signed bytes, unchecked by `verify-cert`), manifests in the assembling station's RAM with no reject route, and a Lead that is only elected. ADR-0011 is described as accepted, not built. §7: each PXE item kept as a target with a Today: line (no PXE image, certs not streamed, licence from a file, signing key on the station, tier 2 a stub). §11: #2, #8, #10 and #13 name what is not built. §14: ErasureEvent, PendingCoSign, DestructionManifest, Lead, Hub and Control plane corrected, and new entries for store tier, evaluation certificate, attestation chain, licence certificate, instance key, lease / watermark, enclosure model / catalog, and shell (both senses).

**Files changed**
- `CONTEXT.md` — §5 Job orchestration and Fleet, §7 PXE discipline, §11 #2/#8/#10/#13, §14 glossary.
- `.scratch/docs-truth-up/stories/2-context-md-sections-5-7-11-and-14-match-the-code.md` — this story spec.

**Review.** 29 findings (high 0, medium 12, low 14, false 2, maybe-false 1). 16 patch entries applied (7 medium, 9 low), covering 21 finding rows. 3 deferred: §12 contradictions (outside CAP-2's sections), an escalate-from-Queued race in the runner (code, unverified), and the §14 Lapse policy entry saying stations enforce it. Rejected: the "Licence certificate" term duplicating "Licence" (SPEC requires it; the two are cross-linked and distinct); lexical-only verification (fix would edit this spec); 2 false (see triage log).

**Follow-up review recommended:** false. This pass patched seven mediums, but every one is a docs-only wording change checked against the code it describes, and no unverified risk can be named.

**Verification.** `cargo test --workspace`: 696 passed, 0 failed (compose up), before and after patches. `apps/desktop`: `pnpm typecheck` ok, `pnpm test` 200 passed. The stale-claim grep for `Verifying|GeneratingCert|independently verifiable` finds no match in §5/§7/§11/§14; the only remaining match is in §12 (deferred).

**Residual risks.** CONTEXT §12 still contradicts §5 until someone fixes it. §14 "Activation" and "Revocation list" describe unbuilt flows in the present tense; they were not in the audit and were left alone.
