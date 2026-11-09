---
title: 'Destruction runs, completion and concurrence policy'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '129ce2a045d71978c56b3f53ac2e8f3ad04d0df4'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '{project-root}/docs/adr/0011-destruction-by-operator-attestation.md'
  - '{project-root}/.scratch/destruction-attestation/research.md'
warnings: [oversized]
deferred:
  - summary: >-
      Aborting a Job that is already on a destruction run leaves the run unable to complete or concur (409 every time, no reject route); if the run awaits concurrence, its signed completion stays on the run beside an Aborted drive.
    evidence: |-
      JobRunner::abort accepts any non-terminal Job, including a PendingDestruction Job with a manifest_id; that was already true before this story (a co-sign was refused the same way and the manifest stayed Pending until restart). check_members now refuses completion and concurrence with 409 naming the Job (tested by http_e2e). The fix is a product choice between refusing abort for a Job on a live run and adding a way to reject or shrink a run.
    location: >-
      crates/wipe-engine/src/runner.rs:180
    severity: medium
  - summary: >-
      The documented recovery for a stuck run (restart the station) also discards every certificate held in station RAM.
    evidence: |-
      CONTEXT.md says a run that cannot complete holds its Jobs until the station restarts; certificates and runs live only in RAM by ADR-0003 design. Pre-existing limitation, carried from the co-sign era; resolved together with the deferred abort item above.
    location: >-
      CONTEXT.md:398
    severity: low
  - summary: >-
      Nothing tests that wipestation serve --concurrence-policy (or WIPESTATION_CONCURRENCE_POLICY) actually reaches the running station; deleting the with_concurrence_policy call fails no test.
    evidence: |-
      The CLI test only checks clap parsing; http_e2e builds AppState directly. cmd_serve is not exercised anywhere in the repo. The fix is to move cmd_serve's AppState building into a function a unit test can call, a refactor outside this story's test pattern.
    location: >-
      crates/wipe-cli/src/main.rs:515
    severity: medium
  - summary: >-
      The Destruction runs completion and concurrence forms have no render test; acceptance criteria are proven over HTTP and through pure helpers only.
    evidence: |-
      apps/desktop has no component-test toolchain (no @testing-library, jsdom or happy-dom), the same gap stories 1 and 2 deferred. The gating, preview keys, time conversion, drive labels and signer check are unit-tested in api/destructionRuns.test.ts.
    location: >-
      apps/desktop/src/pages/ManifestsPage.tsx:1
    severity: low
---

<intent-contract>

## Intent

**Problem:** A `DestructionManifest` still finishes through the pre-ADR-0011 supervisor co-sign: a mandatory second person, no record that destruction was completed or how it was verified, and no way for a site to choose whether a concurrence is needed. Story 1 built the `DestructionCompletion`, `Concurrence` and `ConcurrencePolicy` records; nothing produces them.

**Approach:** Turn manifests into destruction runs (CAP-2, CAP-3). An operator confirms a run's completion with the verification, an optional video reference and their attestation; with policy `none` every Job on the run becomes `Destroyed` at once. With `review` or `witness`, the run waits for a concurrence of exactly that kind, from a different signer, and only then do its Jobs become `Destroyed`. The policy is station configuration set at startup. Preview routes render each statement in Rust before the signer ticks "I attest". The co-sign route goes away, and the Manifests page becomes "Destruction runs".

## Boundaries & Constraints

**Always:**
- Completion and concurrence records are built only through `DestructionCompletion::new`, `Concurrence::new` and `AttestationSigner::self_asserted`. Their statements are rendered only in Rust (extract `completion_statement` / `concurrence_statement` in `destruction.rs` the way story 2 extracted `attestation_statement`, and have the constructors call them). Constructor errors are 422 with the error text. A request without `"attest": true` is 422 and changes nothing.
- The concurrence kind comes from the run, never from the client. A run records the policy's `required_kind()` at assembly (`required_concurrence`), so a reviewer is never recorded as a witness and a policy change never re-labels an assembled run.
- `ManifestState` becomes `pending` (assembled, awaiting completion) → `awaiting_concurrence` (only when one is required) → `completed`; `rejected` stays (still never set). `signed` is accepted on input as an alias of `completed`.
- Only `JobRunner` mutates Jobs. Moving a run's Jobs to `Destroyed` is all-or-nothing under one runner write lock, with a `JobStateChanged` broadcast per Job. Completion and concurrence validate everything (run state, every member Job `PendingDestruction`, every member Job has a certificate) before mutating the run or any Job; a refusal leaves both unchanged.
- No Job reaches `Destroyed` without a certificate (c779361): the guard moves from co-sign to whichever step moves Jobs to `Destroyed`. Assembly also refuses (409, naming them) Jobs with no certificate yet, so a run can never be stuck by one; the Destruction runs page does not offer Jobs with no erasure activity and says why.
- Destruction never depends on licence state (ROADMAP §0.2).
- Rust and `types.ts` / `client.ts` change together; `CONTEXT.md` §5/§10/§14, `README.md`, `docs/ARCHITECTURE.md` and `docs/ROADMAP.md` change in the same commit.

**Never:**
- No change to when or what certificates sign (story 4 moves signing to `Destroyed` and puts completion and concurrence inside the signed bytes). Completion and concurrence append no co-signature block. `wipe_cert::co_sign` and the `co_signatures` field stay so existing certificates still verify.
- No kept co-sign route: `POST /api/manifests/:id/cosign` is removed (the JSON 404 catch-all answers it).
- No editing the concurrence policy over the API or in the UI, and no persisting it in the topology store.
- No SSO, no `verified` assurance, no video storage, no remote concurrence.
- No edits to ADR text.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Complete, policy none | pending run of 2 erased-then-recorded Jobs; remnants and cycle checked, equipment SHR-2, criterion, name, title, attest | 200; run `completed` with `completion`; both Jobs `destroyed`; certificates unchanged, no co-signature | — |
| Complete, policy review | same, station started with `review` | 200; run `awaiting_concurrence`; Jobs still `pending_destruction` | — |
| Concur | awaiting run, a different operator, attest | 200; `concurrence.kind` = run's `required_concurrence`; statement says "reviewed the destruction records and verification evidence" (review) or "witnessed the destruction" (witness); run `completed`; Jobs `destroyed` | — |
| Same signer concurs | concurrence operator id equals the completion signer's, or same full name ignoring case/whitespace | nothing changes | 422 |
| Concur not needed / too early / twice | run with no required concurrence, or `pending`, or `completed` | nothing changes | 409 |
| Complete twice / complete non-pending | run `awaiting_concurrence` or `completed` | nothing changes | 409 |
| Unverified | `remnants_inspected` or `equipment_cycle_completed` false, blank criterion/equipment/name/title, `attest` false | nothing changes | 422 with the reason |
| Completion time | `completed_at` omitted → server now; later than now + 60 s, or before the run's `created_at` | stored time is the one in the statement | 422 for out of range |
| Member Job has no certificate | run assembled before this story's guard, or cert removed | nothing changes | 409 naming the Job |
| Assemble cert-less Job | `pending_destruction` Job never erased | nothing created | 409 naming the Job |
| Unknown run | random id on complete / concur / previews | — | 404 |
| Previews | completion fields (+ optional `completed_at`), or concurrence name/title | 200 `{statement, completed_at}` / `{statement}` equal to what the record stores | 422 on blank fields, 404 unknown run, 409 concurrence preview on a run needing none |
| Old route | `POST /api/manifests/:id/cosign` | — | 404 JSON |
| Policy read | `GET /api/destruction/policy` | `{"concurrence_policy":"none"}` by default | — |
| Old state name | `{"state":"signed"}` manifest | deserializes as `completed` | — |

## Autonomous decisions

1. **The concurrence policy is set when the station starts, not edited in the app.** *About:* how a site chooses whether destruction runs need a review or a witness. *Problem:* the station's only persisted configuration is the bay topology (ADR-0003), and the API has no sign-in or roles, so a policy editable from the bench could be switched off by the very operator it is meant to check. *Options:* (a) a startup flag `wipestation serve --concurrence-policy none|review|witness` (env `WIPESTATION_CONCURRENCE_POLICY`, default `none`) with a read-only `GET /api/destruction/policy`; (b) an API/UI setting persisted beside the topology. *Choice:* (a). It is the safer default for the audit trail, matches ADR-0003's note that settings editing should wait for roles, and the Tauri shell keeps `none`. Persisted, role-gated editing is left as a CONTEXT.md limitation.
2. **A run remembers the concurrence it needs from assembly.** The required kind is copied onto the run (`required_concurrence`) when it is assembled, so restarting with another policy never changes what an already-assembled run claims or lets one skip its concurrence.
3. **The concurring signer must be someone other than the person who confirmed completion** (different operator id and different full name). A concurrence by the same person adds nothing a reviewer or witness is supposed to add; refusing is the protective choice.
4. **Concurrence comes after completion.** A review reviews the verification evidence, which exists only once completion is recorded; a witness concurs with "the destruction" as completed. So a concurrence on a `pending` run is 409.
5. **The completion time is attested, within limits.** The statement names the completion time, so the preview returns the time it used and the completion request sends it back; the server refuses a time in the future (more than 60 s of clock skew) or before the run was assembled. The operator signs the time they read.
6. **Until story 4, completion does not touch certificates and adds no co-signature.** ADR-0011 §2 removes after-the-fact co-signatures; story 4 puts completion and concurrence inside the signed bytes. Keeping the supervisor co-sign meanwhile would record a role this flow no longer has.
7. **Never-erased Jobs cannot be assembled until story 4.** They have no certificate (story 2 decision 4), so a run containing one could never complete. Assembly refuses them with 409 naming them, and the page leaves them out with a note, so no run is stuck. Story 4 lifts this when it certifies at `Destroyed`.
8. **The manifest's `supervisor` and `signed_at` fields are removed.** Manifests live only in station RAM and are not inside certificates, so nothing signed depends on them; `completion.completed_at` and `concurrence` replace them. `DestructionEvent::supervisor` stays, because it is inside v2 signed bytes.

</intent-contract>

## Code Map

- `crates/wipe-common/src/destruction.rs:218-330` -- `DestructionCompletion::new`, `Concurrence::new`. Extract `pub fn completion_statement(completed_at, equipment_id, criterion, video_ref: Option<&str>) -> Result<String, _>` and `pub fn concurrence_statement(kind, manifest_id, full_name, title) -> Result<String, _>` (trim/validate as the constructors do); constructors call them; add agreement tests. `ConcurrencePolicy::required_kind` (`:281`) is the policy → kind map.
- `crates/wipe-common/src/job.rs:292-340` -- `DestructionManifest`, `ManifestState`, `new`. Add `required_concurrence: Option<ConcurrenceKind>` (`#[serde(default)]`) as a `new` parameter; remove `supervisor`/`signed_at`; states as in the contract (`#[serde(alias = "signed")]` on `Completed`). Add pure methods `complete(&mut self, DestructionCompletion) -> Result<(), ManifestTransitionError>` (pending only; moves to `AwaitingConcurrence` or `Completed`) and `concur(&mut self, Concurrence)` (awaiting only; kind must equal `required_concurrence`; same-signer refused); `fn destroys_jobs(&self) -> bool`-style helper optional. Unit-test every transition. Existing tests at `:396-450` need the new parameter.
- `crates/wipe-engine/src/runner.rs:364-386` -- `mark_destroyed`: replace with `pub fn mark_destroyed(&self, ids: &[Uuid]) -> WipeResult<()>` validating all under one write lock (exists, `PendingDestruction`, not listed twice), then transitioning and broadcasting each. Update the module doc and `end_to_end.rs` callers.
- `crates/wipe-server/src/handlers.rs:570-750` -- `create_manifest`: pass `state.concurrence_policy.required_kind()`; refuse Jobs with no entry in `state.certs` (409 naming them). Replace `CosignManifestRequest`/`cosign_manifest` with `complete_manifest`, `concur_manifest`, `preview_completion_statement`, `preview_concurrence_statement`, and `destruction_policy`. Reuse story 2's attestation request fields (`operator`, `full_name`, `title`, `attest`) and its 422 mapping of `DestructionRecordError`. Hold `state.manifests.write()` across validate → runner `mark_destroyed` → store so two concurrent completions cannot both pass; the cert check reads `state.certs`.
- `crates/wipe-server/src/app.rs:28,66-175,345-353` -- add `concurrence_policy: ConcurrencePolicy` to `AppState` (default `None`) and `with_concurrence_policy`; routes `POST /api/manifests/:id/completion`, `/api/manifests/:id/completion/statement`, `/api/manifests/:id/concurrence`, `/api/manifests/:id/concurrence/statement`, `GET /api/destruction/policy`; drop the cosign route. The cert generator (`:196-290`) changes only its doc comment (no co-sign).
- `crates/wipe-cli/src/main.rs:35-90,492` -- `--concurrence-policy` (`value_parser` over `none|review|witness`, env `WIPESTATION_CONCURRENCE_POLICY`, default `none`) → `with_concurrence_policy`. Tauri (`apps/desktop/src-tauri/src/lib.rs:143`) unchanged (default).
- `crates/wipe-server/tests/http_e2e.rs` -- `spawn_server` (`:13`) gains a policy variant. Rewrite the co-sign tests (`:224` two-signatures, `:525` no-certificate guard, `:1335`, `:1349`) into completion/concurrence tests; keep the manifest-linking tests (`:598`, `:746`) with renamed comments.
- `crates/wipe-engine-mock/tests/end_to_end.rs` -- `mark_destroyed` callers; add batch tests (all-or-nothing on one bad id; broadcasts per Job).
- `apps/desktop/src/api/types.ts:323-365` -- `ManifestStateLabel`, `DestructionManifest` (`required_concurrence`, no `supervisor`/`signed_at`), `ConcurrenceKind`. `client.ts:170-185` -- replace `cosignManifest` with `completeRun`, `concurRun`, `previewCompletion`, `previewConcurrence`, `destructionPolicy`.
- `apps/desktop/src/components/AttestationFields.tsx` -- story 2's review/"I attest" block with the preview-race fix (statement keyed by the inputs sent). Reuse its pattern for both new forms.
- `apps/desktop/src/pages/ManifestsPage.tsx` -- retitle "Destruction runs"; show the station policy; per run: state badge ("Pending completion", "Awaiting review/witness", "Completed"), the completion form (remnants inspected, equipment cycle completed, equipment ID prefilled when all member attestations name the same one, criterion, video reference, full name, title, review statement, I attest), the concurrence form when awaiting, and the recorded statements once done. `pages/manifestAssembly.ts` (+ test) -- exclude Jobs with no erasure activity and expose them separately for the note; treat `completed`/`awaiting_concurrence` like the old `signed`.
- `apps/desktop/src/api/destructionRuns.ts` (new) + test -- pure helpers: `runStatusLabel`, `commonEquipmentId(jobs)`, `readyToComplete(form, preview)`, `readyToConcur(form, preview)`, `toCompletionRequest`, `toConcurrenceRequest`.
- `apps/desktop/src/components/Layout.tsx` -- nav label "Destruction runs" (route path `/manifests` unchanged).
- Docs: `CONTEXT.md:273-290,365-395,425-446,853-857,1235-1255` (DestructionManifest, Destruction completed, Concurrence: replace "schema only" with what is built; the §10 rows); `README.md:43-44,62,323-347,367-390`; `docs/ARCHITECTURE.md:33,130,206-218`; `docs/ROADMAP.md` (any co-sign mention).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-common/src/destruction.rs`, `job.rs` -- statement functions; manifest fields, states, alias, `complete`/`concur` transitions; unit tests for every matrix row they own.
- `crates/wipe-engine/src/runner.rs` -- batch `mark_destroyed`.
- `crates/wipe-server/src/app.rs`, `handlers.rs` -- policy on `AppState`, the five routes, assembly guards, co-sign removal.
- `crates/wipe-cli/src/main.rs` -- the flag, with a parse test.
- `crates/wipe-server/tests/http_e2e.rs`, `crates/wipe-engine-mock/tests/end_to_end.rs` -- a test for every I/O matrix row reachable over HTTP or the runner.
- `apps/desktop/src/api/{types,client,destructionRuns}.ts` (+ test), `pages/{ManifestsPage.tsx,manifestAssembly.ts}` (+ test), `components/Layout.tsx` -- mirror, helpers, page.
- Docs as in the Code Map, test counts included.

**Acceptance Criteria:**
- Given a station started with the default policy and a run of three recorded, erased drives, when the operator confirms completion on the Destruction runs page with remnants inspected, cycle completed, criterion "particles ≤ 2 mm", reviews the statement and ticks "I attest", then `GET /api/manifests/:id` shows `completed` with that statement and each Job in `GET /api/jobs` is `destroyed` with its certificate still verifying offline.
- Given a station started with `--concurrence-policy witness`, when completion is confirmed, then the run shows "Awaiting witness" and no Job is `destroyed`; when a second operator concurs, then the stored concurrence is `witness` with "witnessed the destruction", and the Jobs are `destroyed`.
- Given any refused completion or concurrence, when the run and its Jobs are re-read, then nothing changed.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 31 findings — high 0, medium 5, low 12, false 14, maybe-false 0
- findings:
  - `[medium]` `[defer]` Aborting a run member leaves the run stuck, and an awaiting run keeps a signed completion beside an Aborted drive — abort on a linked PendingDestruction Job was already allowed at baseline (co-sign was stuck the same way); fixing it is a product choice (refuse abort vs. reject/shrink a run). Deferred (see `deferred`).
  - `[low]` `[defer]` The documented "restart" recovery for a stuck run also loses RAM-held certificates — pre-existing limitation of ADR-0003's evidence-in-RAM design; deferred with the abort item.
  - `[medium]` `[patch]` The completion form had no completion-time input, so a run shredded at 09:00 and confirmed at 14:00 was attested as 14:00 — fixed: a local date-time input (default now) is sent as `completed_at` in the preview and is part of the preview key.
  - `[medium]` `[patch]` The statements say "the device(s) listed above" but the forms listed no drives — fixed: both forms show "Devices in this run" (asset tag and serial from each Job's attestation, else short Job id) above the statement.
  - `[low]` `[reject]` The concurrence statement says "manifest {uuid}" while the UI says "destruction run" — the wording is research.md's adopted statement (story 1); changing it is a customer-visible copy decision, and the full id is shown in the run's details.
  - `[low]` `[patch]` The completion signer could fill in and attest the concurrence form before getting a 422 — fixed: `concurrenceSignerProblem` applies the server's same-signer rule and the form blocks with a reason; unit-tested.
  - `[low]` `[patch]` Previews ignored the run's state (concurrence statement rendered for a pending run) — fixed: completion preview 409 unless `pending`, concurrence preview 409 unless `awaiting_concurrence`; http_e2e test.
  - `[low]` `[patch]` A non-UTC `completed_at` was stored with its offset while the statement prints UTC — fixed: normalized to UTC in completion and preview; http_e2e test with `+02:00`.
  - `[false]` `[reject]` The page duplicates `AttestationFields` instead of reusing it — the Code Map says to reuse its *pattern* (statement keyed by the inputs sent), which `CompletionCard`/`ConcurrenceCard` do; `AttestationFields` is specific to the recorded-for-destruction fields.
  - `[low]` `[patch]` Docs implied the startup-only policy enforces a real second person, while signer separation is self-asserted — fixed: one sentence in CONTEXT §5/§10 and README says the check compares self-asserted operator and typed name until sign-in and roles exist.
  - `[low]` `[patch]` Missing tests (member no longer pending at completion; race; concurrence-preview 409; component tests) — fixed: http_e2e test aborting one member makes completion 409 and changes nothing; the concurrence-preview 409 already existed; the race is prevented by holding the manifests write lock across validate and commit; component tests deferred with the toolchain gap.
  - `[low]` `[patch]` The startup line printed the policy as `None`/`Review`/`Witness` — fixed: prints the serde name.
  - `[low]` `[reject]` A concurrence submitted right after a completion dated up to 60 s ahead can carry an earlier timestamp — needs the clock-skew window to be used deliberately; any fix either records a time that did not happen or adds a new refusal.
  - `[false]` `[reject]` `DestructionManifest::complete` lacks the future-time bound — the only caller (the handler) checks it via `check_completion_time` first; no other caller exists.
  - `[low]` `[patch]` The no-certificate 409 claimed "recorded before any erasure" for an erased Job whose certificate was still being signed — fixed: the message covers both cases and says to retry.
  - `[low]` `[patch]` Completion preview on non-pending runs (edge-case layer) — same defect as the preview-state row; fixed by the same change.
  - `[low]` `[patch]` Concurrence preview on non-awaiting runs (edge-case layer) — same defect as the preview-state row; fixed by the same change.
  - `[low]` `[reject]` The full-name prefill stays blank if the operator signs in after the card mounts — the operator types the name, which is the signature; confirming is disabled until sign-in (same reasoning as story 2).
  - `[false]` `[reject]` `commonEquipmentId` crashes on a Job without `activities` — `Job::activities` is a required `Vec` always serialized, and `types.ts` declares it required.
  - `[false]` `[reject]` `runStatusLabel` shows "Awaiting review" for an awaiting run with no required concurrence — unreachable: `complete` sets `AwaitingConcurrence` only when `required_concurrence` is set.
  - `[low]` `[reject]` `wipe_cert::co_sign` now has no production caller — developers will not meet it in everyday use, and story 4 reshapes certificate signing and decides its fate; the spec keeps it so pre-ADR-0011 certificates stay explainable.
  - `[medium]` `[defer]` The `--concurrence-policy` flag/env reaching the running station is untested (verification-gap, filed `defer`) — needs `cmd_serve`'s state building made testable. Deferred (see `deferred`).
  - `[medium]` `[patch]` The default policy `none` the Tauri shell relies on was unpinned (verification-gap) — fixed: http_e2e test on plain `spawn_server()` asserts `{"concurrence_policy":"none"}` and `required_concurrence: null` on an assembled run.
  - `[false]` `[reject]` The SPEC's dead-drive journey cannot complete in this story (intent-alignment) — certifying never-erased drives is CAP-4, story 4; this story's decision 7 keeps them off runs so none is stuck, and story 2 deferred the same.
  - `[low]` `[defer]` Acceptance criteria name the page but tests run over HTTP and pure helpers (intent-alignment) — same root cause as the component-test gap; deferred with it.
  - `[low]` `[reject]` The Tauri shell cannot set a concurrence policy (intent-alignment) — decision 1 keeps it on `none`; only the mock backend exists (ROADMAP §0.1), so no site runs the desktop shell in production, and adding a policy path there is new surface.
  - `[false]` `[reject]` "A reviewer is never recorded as a witness" is enforced structurally, not at the client boundary — that is the intended design (the request carries no kind); the review-policy test proves the stored kind and wording.
  - `[low]` `[patch]` "Second operator" relies on typed identity (intent-alignment) — same root cause as the docs row; fixed by the same sentence.
  - `[false]` `[reject]` Offline verification is checked in-process, not via the `verify-cert` CLI — the CLI calls the same `wipe_cert::verify`; `verify-cert` checks belong to CAP-4 (story 4).
  - `[false]` `[reject]` Co-sign removal is applied at the route level — ADR-0011 §2 removes after-the-fact co-signatures; the route was their only producer, and existing co-signed certificates still verify.
  - `[false]` `[reject]` Concurrence is accepted only after completion — ADR-0011 sets no order; decision 4 records why (a review needs the verification evidence; a witness concurs that destruction happened as stated).

## Design Notes

Completion request (the concurrence request is the last line's fields only):

```json
{ "completed_at": "2026-11-08T15:04:05Z", "remnants_inspected": true,
  "equipment_cycle_completed": true, "equipment_id": "SHR-2",
  "criterion": "particles ≤ 2 mm", "video_ref": "CAM-3 14:02",
  "operator": {"id":"op-1","display_name":"Ana Ruiz","email":null},
  "full_name": "Ana Ruiz", "title": "Bench lead", "attest": true }
```

Story 4 will replace the "every member Job has a certificate" guard and the assembly refusal when it signs at `Destroyed`; keep both in one helper so the change is local.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass.
- `cargo clippy --workspace --all-targets` -- expected: no new warnings in touched files.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: pass.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: no new diffs on changed lines.
- `grep -rn "cosign\|co-sign\|cosignManifest" crates/wipe-server/src apps/desktop/src README.md docs/ARCHITECTURE.md` -- expected: only certificate co-signature display/verification and deliberate history.

## Auto Run Result

Status: done

**Summary.** Manifests are now destruction runs (ADR-0011 §1 second step, §3). An operator confirms a run's completion — remnants inspected, equipment cycle completed, equipment ID, criterion, optional video reference, completion time — reads the Rust-rendered statement and ticks "I attest". With the default policy `none` every Job on the run becomes `Destroyed` at once; with `review` or `witness` (set by `wipestation serve --concurrence-policy`, read-only at `GET /api/destruction/policy`) the run waits for a concurrence of that kind from a different signer. Each run copies the required kind at assembly. Moving Jobs to `Destroyed` is all-or-nothing in the runner. The supervisor co-sign route is gone; certificates are untouched until story 4. Never-erased Jobs cannot join a run yet (no certificate) and the page says why.

**Files changed**
- `crates/wipe-common/src/destruction.rs` — `completion_statement`, `concurrence_statement`, agreement tests.
- `crates/wipe-common/src/job.rs` — run states (`signed` alias), `required_concurrence`, `complete`/`concur`/`check_completion_time`, transition tests.
- `crates/wipe-engine/src/runner.rs` — batch, all-or-nothing `mark_destroyed`.
- `crates/wipe-server/src/app.rs`, `handlers.rs` — policy on `AppState`, completion/concurrence routes and previews, policy route, certificate guard helper, co-sign removal.
- `crates/wipe-cli/src/main.rs` — `--concurrence-policy` flag with parse test.
- `crates/wipe-server/tests/http_e2e.rs`, `crates/wipe-engine-mock/tests/end_to_end.rs` — tests for every I/O matrix row; co-sign tests replaced.
- `apps/desktop/src/api/{types,client,destructionRuns}.ts` (+ test), `pages/{ManifestsPage.tsx,manifestAssembly.ts}` (+ test), `components/Layout.tsx`, `pages/JobDetailPage.tsx` — mirror, helpers, the Destruction runs page.
- `AGENTS.md`, `CONTEXT.md`, `README.md`, `docs/ARCHITECTURE.md`, `docs/ROADMAP.md` — docs and test counts.

**Review.** 31 findings: high 0, medium 5, low 12, false 14.
- **Patched** (by entry): medium 3 (completion-time input; drive list above the statements; default policy pinned by a test); low 7 (concurrence form blocks the completion signer; previews check run state; UTC completion time; self-asserted signer-separation docs; aborted-member test; serde-name startup log; clearer no-certificate message).
- **Deferred**: aborting a run member leaves the run stuck (medium, pre-existing); restart recovery loses RAM certificates (low, pre-existing); CLI policy wiring untested (medium); no render tests for the page (low, no toolchain).
- **Rejected**: every reason is in the triage log.
- **Follow-up review recommended: yes.** Three medium entries were patched. The unverified risk is the page's new completion-time input: `localInputToRfc3339` converts local time to UTC and feeds both the preview key and the attested time, and only the pure helper is tested, not the rendered form.

**Verification**
- `cargo test --workspace --no-fail-fast` with compose up: 791 passed, 0 failed after the patches. Before the patches, `wipe-hub` `silent_smtp_and_database_time_out_within_bound` (3.5 s wall-clock limit) failed twice and then passed twice on rerun; this story changes no `wipe-hub` code, and stories 1–2 recorded it as flaky on the base commit.
- `cargo clippy --workspace --all-targets`: no warnings in touched files.
- rustfmt: edited files clean on changed lines (`job.rs`, `runner.rs` were already unformatted at baseline).
- Desktop: typecheck passes; 251 vitest tests pass.
- I did not open the Destruction runs page in a running app.

**Residual risks**
- Signer separation and identity are self-asserted until sign-in and roles exist.
- Certificates still say nothing about completion or concurrence until story 4 signs at `Destroyed`; never-erased drives stay off runs until then, and story 4 must replace the `uncertified` guard.
- A run with an aborted member cannot complete and has no reject route.

