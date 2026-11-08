---
title: 'Stamp the manifest on its Jobs'
type: 'bugfix'
created: '2026-11-08'
status: 'done'
baseline_revision: 'b970fc83f43c0e93bb7b446e012223b98d2974ba'
review_loop_iteration: 1
followup_review_recommended: false
context: []
warnings: [oversized]
deferred:
  - summary: >-
      The 409 that create_manifest returns when link_to_manifest refuses (a concurrent assembly linked the Job first) is never exercised by an HTTP test.
    evidence: |-
      Every refused POST /api/manifests in http_e2e.rs is stopped by the handler preflight (PendingCoSign check or stored-manifest scan) before link_to_manifest runs; the runner refusal is covered only by direct runner tests. Swapping the insert before the link, or ignoring the link's error, would fail no test. Pinning it needs concurrent overlapping POST /api/manifests calls asserting exactly one stored manifest and one manifest_id per Job.
    location: >-
      crates/wipe-server/src/handlers.rs:407-416
    severity: low
  - summary: >-
      A PendingCoSign Job already on a manifest can still be aborted, which leaves its manifest permanently un-co-signable until restart (no reject route).
    evidence: |-
      JobRunner::abort (runner.rs:154) has no manifest_id guard; cosign_manifest then refuses the whole manifest ("not in PendingCoSign") and the other member Jobs cannot join another manifest. Pre-existing (the same happened before this story; CONTEXT.md already records abort of PendingCoSign as allowed). Belongs with the ADR-0011 destruction-attestation rework or a reject route.
    location: >-
      crates/wipe-engine/src/runner.rs:154
    severity: medium
  - summary: >-
      The certificate generator overwrites a stored certificate unconditionally, so a lagging PendingCoSign pass (e.g. after a quick double escalation) can replace an already co-signed certificate with one lacking the co-signature.
    evidence: |-
      spawn_cert_generator regenerates on every JobStateChanged into PendingCoSign using the broadcast's state, not the live Job's, and does state.certs.write().insert(job_id, signed) (app.rs:290) without checking for an existing co-signed cert. Needs escalate, escalate, assemble and co-sign to complete within the generator's lag (milliseconds). Pre-existing; ADR-0011's sign-once-at-Destroyed design removes it.
    location: >-
      crates/wipe-server/src/app.rs:290
    severity: medium
---

<intent-contract>

## Intent

**Problem:** `POST /api/manifests` creates a DestructionManifest but never records it on its member Jobs: `Job.manifest_id` and the DestructionEvent's `manifest_ref` stay `null` (a no-op loop in `create_manifest`). The link between a Job, its destruction record and the manifest a supervisor signs exists only inside the co-signature block, and the Manifests page cannot tell which Jobs are already taken (CAP-1, SPEC `destruction-cert-integrity`).

**Approach:** Add one `JobRunner` method that, all-or-nothing under the runner's write lock, stamps a manifest id onto each listed Job (`manifest_id`) and onto that Job's latest DestructionEvent (`manifest_ref`). `create_manifest` calls it before storing the manifest.

## Boundaries & Constraints

**Always:**
- Only `JobRunner` mutates Jobs; the handler calls the runner method, never patches a clone.
- The link method refuses (and changes nothing on any Job) if any listed Job is missing, not in `PendingCoSign`, already has a `manifest_id`, or has no DestructionEvent.
- The link changes no Job state and emits no `JobBroadcast::JobStateChanged` (that broadcast re-signs the certificate).
- The stored certificate is untouched: its signed copy of the DestructionEvent keeps `manifest_ref: null`; co-sign still signs the same bytes and records the manifest in the co-signature block as today.
- Keep c779361's guarantees: the manifest-scan 409 guard stays, and co-sign still refuses a Job without a certificate.

**Never:**
- No change to the certificate schema, `wipe-cert`, or the TypeScript mirror (`manifest_id` / `manifest_ref` already exist in `types.ts`).
- No Manifests page filter change (story 2) and no runner-error status mapping on start/abort/escalate/cosign (story 3).
- No unlink path (there is no reject route; `Rejected` is never set).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Happy path | Job escalated to PendingCoSign; `POST /api/manifests` with its id | 200; `GET /api/jobs/:id` has `manifest_id` = manifest id and the latest `destruction` activity's `manifest_ref` = manifest id; state still `pending_co_sign` | — |
| Second manifest | Same Job, second `POST /api/manifests` | 409, existing manifest unchanged, Job still stamped with the first id | existing guard |
| Mixed batch | One valid PendingCoSign Job plus one Queued Job | 409; neither Job stamped; no manifest stored | preflight / runner refusal |
| Re-escalate after linking | Job on a manifest, `POST /api/jobs/:id/escalate-to-destroy` again | Refused (non-2xx); no DestructionEvent appended; certificate not replaced | runner returns `InvalidState` |
| Co-sign | Linked Job, cosign manifest | Job `destroyed`; `manifest_id` and `manifest_ref` still set; cert co-signature `manifest_ref` = manifest id | — |

## Autonomous decisions

- **Re-escalating a Job already on a manifest.** Today an operator can press "escalate to destroy" again on a PendingCoSign Job; that appends a second DestructionEvent and re-signs, replacing the stored certificate. If the Job is already on a manifest, the new DestructionEvent would not carry the manifest reference and the certificate the supervisor is about to co-sign would be swapped underneath them. Options: (a) leave it, so the link silently breaks; (b) refuse escalation of a Job whose `manifest_id` is set; (c) stamp the new event too. Chosen: **(b)** — it is the safe default for the audit trail (the record a supervisor signs cannot change once assembled), it is one guard in the runner, and Jobs not on a manifest keep today's behaviour. The refusal's HTTP status follows whatever `escalate_to_destroy` maps `InvalidState` to (500 today; story 3 maps it to 409).
- **Which DestructionEvent gets the stamp.** A Job escalated twice before assembly holds two DestructionEvents. Chosen: stamp only the **latest**, the one that put the Job in its current PendingCoSign and that the certificate's disposition rests on; earlier ones are superseded history and stay as recorded.
- **Status when the runner refuses the link inside `create_manifest`.** The handler's preflight already returns 404/409 for every refusal the runner can make, so a runner refusal there can only come from a concurrent request linking the same Job first. Chosen: map it to **409 Conflict** in `create_manifest`, and store the manifest only after the link succeeds, so the race cannot produce two live manifests for one Job.

</intent-contract>

## Code Map

- `crates/wipe-engine/src/runner.rs:179-210` -- `escalate_to_destroy`: add the "already on a manifest" refusal. Today it checks a clone from `self.get(id)` and mutates later via `mutate_job` (a second lock), so a concurrent `link_to_manifest` can slip between them. The terminal check, the new `manifest_id` check and the append + state change must all happen under one `self.inner.write()` guard; send the broadcasts after dropping it.
- `crates/wipe-engine/src/runner.rs:214-236` -- `mark_destroyed`: pattern for a validated runner mutation; new `link_to_manifest(manifest_id: Uuid, job_ids: &[Uuid]) -> WipeResult<()>` goes next to it. Validate every Job and mutate all inside one `self.inner.write()` guard (not `get` + `mutate_job`, which re-locks). `WipeError::InvalidState` is the error used throughout.
- `crates/wipe-common/src/job.rs:186,231` -- `DestructionEvent.manifest_ref`, `Job.manifest_id` (both `Option<Uuid>`, already serialised). Read-only.
- `crates/wipe-server/src/handlers.rs:356-417` -- `create_manifest`: keeps the duplicate/scan/PendingCoSign preflight; replace the no-op loop at 406-415 with the runner call, ordered before `state.manifests.write().insert`; map its error to `StatusCode::CONFLICT`. Update the doc comment at 353-355. The code comment on the 409 must say what is true: the preflight scans stored manifests while the runner checks `Job::manifest_id` (equivalent today because nothing sets `Rejected`), so a runner refusal here means a concurrent request linked the Job first.
- `crates/wipe-server/src/app.rs:199-283` -- cert generator: triggers only on `JobStateChanged` (so linking must not broadcast it), and runs asynchronously, reading `state.runner.get(job_id)` after the escalate request has already returned. A manifest assembled in that gap would be signed into the certificate. In the `job_for_cert` clone (line ~241) clear `manifest_id` and every DestructionEvent's `manifest_ref` before `Certificate::from_job`, so the signed copy never carries the link whatever the timing (intent contract: "its signed copy of the DestructionEvent keeps `manifest_ref: null`").
- `docs/ARCHITECTURE.md:200-203` -- says `manifest_ref` is never stamped on the Job; must change in the same commit.
- `crates/wipe-server/tests/http_e2e.rs:224-405,566-623` -- existing destroy-path and two-manifest tests; `spawn_server()` helper and the `escalate-to-destroy` JSON body to reuse.
- `crates/wipe-engine-mock/tests/end_to_end.rs` -- runner-level tests with `MockBackend` (wipe-engine itself has no test module and cannot dev-depend on the mock).
- `CONTEXT.md:323-333,350-360,750` and `CONTEXT.md` §14 DestructionManifest (~line 1130) -- say `manifest_id`/`manifest_ref` are never stamped and that re-escalation re-signs; must change in the same commit.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-engine/src/runner.rs` -- add `link_to_manifest` (all-or-nothing, no broadcast, stamps `manifest_id` and the latest DestructionEvent's `manifest_ref`; its doc comment lists every refusal it makes, including a Job listed twice); make `escalate_to_destroy` refuse a Job with `manifest_id` set, with check and mutation under one write guard -- the runner is the only Job mutator.
- `crates/wipe-server/src/handlers.rs` -- call `link_to_manifest` from `create_manifest` before storing the manifest, map failure to 409, delete the stale "out of scope" comment -- CAP-1.
- `crates/wipe-server/src/app.rs` -- strip the manifest link from `job_for_cert` before building the certificate -- keeps the signed bytes independent of request timing.
- `crates/wipe-engine-mock/tests/end_to_end.rs` -- runner tests: happy path stamps both fields; mixed batch (one Queued) refuses and stamps nothing; already-linked Job refused; re-escalation of a linked Job refused and activity count unchanged; unknown id, an id listed twice, and an empty-activity PendingCoSign case if constructible (otherwise skip that one) each refuse and stamp nothing.
- `crates/wipe-server/tests/http_e2e.rs` -- extend the destroy-path test: after `POST /api/manifests`, `GET /api/jobs/:id` shows both ids equal to the manifest id, and they survive co-sign; add a test that a mixed batch returns 409 and leaves the valid Job unstamped and `GET /api/manifests` empty; add a test that re-escalating a linked Job is refused (assert non-2xx and that the error body names the destruction manifest, not a bare status) and the certificate's `signature` is unchanged; add a test that assembles the manifest immediately after the escalate call returns (no wait for the certificate), then waits for the certificate and asserts its `certificate` activities' `destruction` entry has `manifest_ref` null while `GET /api/jobs/:id` has it set.
- `CONTEXT.md`, `README.md`, `docs/ARCHITECTURE.md` -- correct the DestructionEvent, DestructionManifest (§5 and §14) and §10 status-table text, the README destroy path and known gaps, and ARCHITECTURE line 203: ids are stamped on assembly and never inside the signed certificate; re-escalating a Job on a manifest is refused. Keep README wrapping at ~80 columns.

**Acceptance Criteria:**
- Given a Job escalated to `pending_co_sign`, when `POST /api/manifests` succeeds with it, then `GET /api/jobs/:id` returns `manifest_id` equal to the manifest's `id` and its latest `destruction` activity's `manifest_ref` equal to the same id, and the Job's state and certificate are unchanged.
- Given that manifest is co-signed, when `GET /api/jobs/:id` is read, then the Job is `destroyed` and both ids are still the manifest id.
- Given any refused `POST /api/manifests`, when the Jobs are re-read, then none of them carries a `manifest_id`.
- `cargo test --workspace` passes.

## Spec Change Log

### 2026-11-08 — loop 1 (bad_spec)
- **Triggering findings:** (1) `escalate_to_destroy` checks `manifest_id` on a clone and mutates under a second lock, so a concurrent `link_to_manifest` can slip between them and the linked Job gains an unstamped DestructionEvent plus a re-signed certificate; (2) the certificate generator runs asynchronously and reads the live Job, so a manifest assembled right after escalation is signed into the certificate, contradicting the intent contract's "signed copy keeps `manifest_ref: null`"; plus stale `docs/ARCHITECTURE.md:203`, a vague re-escalation test assertion, a handler comment that overstated the preflight, missing runner refusal tests and a README line past the wrap width.
- **Amended:** Code Map entries for `runner.rs` `escalate_to_destroy` (one write guard), `handlers.rs` (comment wording), `app.rs` (no longer read-only: strip the link from `job_for_cert`), new `docs/ARCHITECTURE.md` entry; Execution tasks for runner, handler, new `app.rs` task, runner tests (unknown id, duplicate id), HTTP tests (error body names the manifest; new immediate-assembly test), docs (README, ARCHITECTURE).
- **Known-bad state avoided:** a Job on a manifest whose record changes after assembly, and certificate bytes that depend on how fast the client posted the manifest.
- **KEEP:** `link_to_manifest` signature, all-or-nothing validate-then-stamp under one `inner.write()`, latest-DestructionEvent-only stamping and no broadcast; `create_manifest` ordering (link, map refusal to 409, then insert); the four runner tests and their `destruction()`/`destruction_events()`/`mock_runner()` helpers; the `latest_destruction()` HTTP helper and the destroy-path, two-manifest, refused-batch and re-escalation HTTP assertions (signature unchanged, destruction count unchanged); CONTEXT.md wording for §5/§10/§14 apart from making "never inside the signed certificate" unconditional. Previous attempt for reference: `/tmp/story1-attempt1.diff`.

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 27 findings — high 0, medium 9, low 15, false 3, maybe-false 0
- findings:
  - `[medium]` `[bad_spec]` Blind: signed certificate can carry the manifest id if assembly precedes async cert signing — verified: `escalate_to_destroy` handler returns before `spawn_cert_generator` reads `runner.get` (app.rs:216); amendment: strip link from `job_for_cert` in app.rs + immediate-assembly HTTP test.
  - `[medium]` `[bad_spec]` Blind: re-escalation guard is check-then-act across two locks — verified runner.rs:184-205 (`get` clone, then `mutate_job`); amendment: one write guard.
  - `[low]` `[bad_spec]` Blind: `docs/ARCHITECTURE.md:203` still says `manifest_ref` is never stamped — verified; amendment: docs task includes ARCHITECTURE.md.
  - `[low]` `[bad_spec]` Blind: re-escalation HTTP test accepts any non-2xx — verified; amendment: assert the error body names the manifest (status stays story 3's).
  - `[low]` `[reject]` Blind: no WebSocket event tells clients about the link — ManifestsPage and JobDetail poll every 1.5–2 s; fix adds a new broadcast variant across ws.rs/ws.ts for a cosmetic delay.
  - `[low]` `[bad_spec]` Blind: missing runner tests for unknown/duplicate id, doc comment omits "listed twice", quadratic dup check — verified; amendment: tests + doc comment (quadratic over a hand-assembled manifest is negligible).
  - `[low]` `[bad_spec]` Blind: handler comment says the preflight answers every runner refusal — preflight scans manifests, runner checks `manifest_id`; amendment: comment wording.
  - `[low]` `[reject]` Blind: re-escalation HTTP test depends on the mock erasure still running — same pattern as the existing destroy-path test (http_e2e.rs:255-290, 0.3 s mock window, 25 ms polling); restructuring is not a direct correction.
  - `[low]` `[bad_spec]` Blind: README known-gaps line not re-wrapped — verified; amendment: docs task keeps ~80 columns.
  - `[medium]` `[bad_spec]` Edge: link can run between escalate's check and write — same root cause as row 2.
  - `[medium]` `[bad_spec]` Edge: manifest assembled before cert generator runs embeds `manifest_ref` — same root cause as row 1.
  - `[low]` `[reject]` Edge: refused re-escalation answers HTTP 500 — out of scope by intent: SPEC CAP-3 / story 3 maps escalate's runner errors to 409.
  - `[false]` `[reject]` Edge: a Job on a Rejected manifest becomes permanently unassemblable — no route sets `Rejected` (CONTEXT.md DestructionManifest), so the state is unreachable.
  - `[medium]` `[bad_spec]` Edge (claim): "escalate refuses a linked Job" does not hold under concurrency — same root cause as row 2.
  - `[medium]` `[bad_spec]` Edge (claim): "signed copy keeps `manifest_ref` empty" is timing-dependent — same root cause as row 1.
  - `[low]` `[bad_spec]` Edge (claim): handler comment misdescribes the preflight — same as row 7.
  - `[low]` `[defer]` Verification gap: `create_manifest`'s runner-refusal 409 path is unexercised over HTTP — pre-verified; reachable only by a race; deferred in frontmatter.
  - `[medium]` `[bad_spec]` Verification other: escalate check-then-act race — same root cause as row 2.
  - `[low]` `[reject]` Verification other: refused re-escalation is a 500 — story 3 scope, as row 12.
  - `[low]` `[reject]` Verification other: Job Detail still shows "Escalate to destroy" for a linked Job — shown only when the erasure failed (JobDetailPage.tsx:131), rare on a linked Job; the refusal already protects the record, and hiding it adds a UI guard.
  - `[medium]` `[bad_spec]` Verification other: cert "stays empty" claim is timing-dependent — same root cause as row 1.
  - `[low]` `[reject]` Intent: Manifests page filter now acts on stamped ids with no vitest — out of scope by intent: SPEC CAP-2 / story 2 owns the filter and its vitest.
  - `[false]` `[reject]` Intent: co-signature `manifest_ref` not asserted — http_e2e.rs:394 asserts `cosigs[0]["manifest_ref"] == manifest_id`.
  - `[low]` `[reject]` Intent: "no broadcast" proven only at runner level — the HTTP tests' unchanged-signature assertions observe its effect; another assertion adds nothing.
  - `[low]` `[defer]` Intent: link race described, not tested — same as row 17.
  - `[medium]` `[bad_spec]` Intent: escalation guard not atomic — same root cause as row 2.
  - `[false]` `[reject]` Intent: tests exceed CAP-1's single e2e assertion — more coverage is not a defect.

### 2026-11-08 — Review pass
- verdicts: 25 findings — high 0, medium 3, low 17, false 5, maybe-false 0
- findings:
  - `[medium]` `[defer]` Blind: aborting a linked Job strands its manifest — verified runner.rs:154 has no guard; pre-existing behaviour, deferred in frontmatter.
  - `[low]` `[patch]` Blind: create_manifest comment names only concurrent assembly as the cause of a runner refusal — a concurrent abort also reaches it; comment reworded.
  - `[low]` `[reject]` Blind: link and manifest insert are not atomic (ms window where the Job's manifest_id 404s) — harmless transient, never met in bench use; fixing needs cross-lock restructuring.
  - `[low]` `[patch]` Blind: `job_for_cert.manifest_id = None` has no effect — verified no wipe-cert code reads `manifest_id`; line deleted, comment narrowed to `manifest_ref`.
  - `[low]` `[patch]` Blind: escalate_to_destroy doc comment omits its refusals — doc sentence added (the HTTP 500 itself is story 3's).
  - `[low]` `[patch]` Blind: HTTP immediate-assembly test usually proves nothing yet README says it proves the strip — README row and test doc comment reworded to name the deterministic AppState test.
  - `[low]` `[reject]` Blind: two HTTP tests escalate while the mock erasure runs and could race on a loaded machine — carried: same pattern and verdict as pass-1 row 8 (existing destroy-path test uses it too).
  - `[low]` `[patch]` Blind: runner re-escalation test does not assert the stamp survives — assertions on manifest_id and manifest_ref added.
  - `[low]` `[reject]` Blind: handler scan and runner check are two sources for "already on a manifest" — the handler comment already states they agree only while nothing sets Rejected; no further harm today.
  - `[low]` `[patch]` Blind: awkward CONTEXT.md phrasing on the signed copy — reworded.
  - `[low]` `[reject]` Edge: re-escalation refusal answers HTTP 500 — carried: pass-1 row 12, story 3 scope.
  - `[medium]` `[defer]` Edge: abort is not gated by manifest_id — same root cause as row 1.
  - `[medium]` `[defer]` Edge: a lagging generator pass can overwrite a co-signed certificate — verified app.rs:290 inserts unconditionally; pre-existing race, deferred in frontmatter.
  - `[low]` `[patch]` Edge (claim): docs say the record cannot change after assembly, but abort still can — CONTEXT.md and ARCHITECTURE.md reworded to say a new DestructionEvent cannot replace it and abort is still allowed.
  - `[low]` `[defer]` Verification gap: handler ordering on a runner refusal is unexercised — carried: pass-1 row 17, already deferred.
  - `[low]` `[reject]` Verification other: refusal is HTTP 500 — carried: pass-1 row 19.
  - `[low]` `[reject]` Verification other: Job Detail still offers Escalate on a linked Job — carried: pass-1 row 20.
  - `[low]` `[patch]` Verification other: `manifest_id = None` strip has no effect — same as row 4.
  - `[false]` `[reject]` Intent: HTTP Job JSON surface — reported as matching; not a defect.
  - `[false]` `[reject]` Intent: signed certificate omits the link (reading R2) — the intent contract and SPEC constraint ("a signed certificate is immutable") require this; the co-signature block carries the link.
  - `[low]` `[defer]` Intent: refused-batch HTTP test is stopped by the preflight, not the runner — carried: pass-1 row 25.
  - `[false]` `[reject]` Intent: escalate route behaviour change — a recorded autonomous decision in the intent contract, not a divergence.
  - `[false]` `[reject]` Intent: latest-only stamping (R3a) — a recorded autonomous decision.
  - `[low]` `[reject]` Intent: UI filter goes live with no vitest — carried: pass-1 row 22, story 2 scope.
  - `[false]` `[reject]` Intent: link not durable across restart — Jobs and manifests are RAM-only by design today (CONTEXT.md DestructionManifest); not introduced here.

## Verification

**Commands:**
- `cargo test -p wipe-engine-mock -p wipe-server` -- expected: all pass, including the new tests.
- `cargo test --workspace` (after `docker compose up -d`) -- expected: all pass.
- `cargo clippy -p wipe-engine -p wipe-server --all-targets` -- expected: no new warnings.
- `rustfmt --edition 2021 <touched .rs files>` -- expected: only touched files change.

## Auto Run Result

Status: done

**Summary.** `POST /api/manifests` now stamps the manifest id on each member Job (`manifest_id`) and on that Job's latest DestructionEvent (`manifest_ref`). The new runner method `JobRunner::link_to_manifest` does this all-or-nothing under one write lock, with no state change and no broadcast. `create_manifest` calls it before storing the manifest and answers 409 if the runner refuses. Two integrity guards came out of review loop 1:
- `escalate_to_destroy` refuses a Job already on a manifest, with the check and the change under one lock.
- The certificate generator strips `manifest_ref` from the copy it signs, so the signed bytes never depend on how quickly the manifest was posted.

**Files changed**
- `crates/wipe-engine/src/runner.rs`: new `link_to_manifest`; `escalate_to_destroy` now does its checks and its change under one guard and refuses a linked Job.
- `crates/wipe-server/src/handlers.rs`: `create_manifest` links the Jobs before storing the manifest and maps a refusal to 409; the stale no-op loop is removed.
- `crates/wipe-server/src/app.rs`: the certificate generator clears DestructionEvent `manifest_ref` on the Job copy it signs.
- `crates/wipe-engine-mock/tests/end_to_end.rs`: six runner tests (stamping, mixed batch, already linked, re-escalation, unknown id, duplicate id).
- `crates/wipe-server/tests/http_e2e.rs`: destroy-path and two-manifest tests extended; four new tests (refused batch, re-escalation refused, immediate assembly over HTTP, and a deterministic strip test against `AppState`).
- `CONTEXT.md`, `README.md`, `docs/ARCHITECTURE.md`: as-built text updated (ids stamped at assembly, never inside the signed certificate, re-escalation of a linked Job refused).

**Review findings**
- Pass 1 (27 findings): 9 medium / 15 low / 3 false. One `bad_spec` loop fixed both races (the escalate check-then-act and the certificate timing), plus docs and test assertions.
- Pass 2 (25 findings): 3 medium / 17 low / 5 false.
  - Patches applied (7, all low): handler comment, a no-op strip line removed, the escalate doc comment, the README and test doc comment about what the HTTP timing test proves, stamp-preservation asserts, and two CONTEXT/ARCHITECTURE wordings.
- Deferred (3, in frontmatter):
  - The race-only 409 path in `create_manifest` has no HTTP test (low).
  - Aborting a linked Job strands its manifest (medium, pre-existing).
  - A lagging certificate-generator pass can overwrite a co-signed certificate (medium, pre-existing).
- Rejected (reasons in the triage log):
  - HTTP 500 on refused re-escalation: story 3's scope.
  - UI filter and Escalate button: story 2's scope, or rare.
  - No WebSocket event for the link: the UI polls.
  - A millisecond window between the link and the manifest insert.
  - The shared timing pattern of the mock-erasure tests.
  - Duplicated refusal rules: already documented in the handler comment.
  - All `false` findings.

**Follow-up review recommended: false.** Pass 2 patched 0 high and 0 medium (7 low); pass 1 routed everything through `bad_spec`, not patches.

**Verification**
- `cargo test --workspace` with compose up: exit 0, 706 passed, 0 failed.
- `cargo clippy -p wipe-engine -p wipe-server -p wipe-engine-mock --all-targets`: 9 warnings, the same as baseline.
- rustfmt `--check` is clean on the four touched files that were already formatted. `runner.rs` is a known-unformatted file, and its only rustfmt diffs are in old code.
- The implementer confirmed the `AppState` strip test fails when the `app.rs` fix is removed.

**Residual risks**
- Refused re-escalation still answers HTTP 500 until story 3.
- The Manifests page filter goes live now (story 2 adds its vitest).
- The two deferred medium races above.
- `AGENTS.md` still says 689 tests; the suite now has 706. It was already stale before this change, and agent-context edits are left to the owner.
