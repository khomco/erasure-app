---
title: 'Record for destruction'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: 'de0380533df301066f912a31d2f86090089faa3a'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '{project-root}/docs/adr/0011-destruction-by-operator-attestation.md'
  - '{project-root}/.scratch/destruction-attestation/research.md'
warnings: [oversized]
deferred:
  - summary: >-
      A drive recorded for destruction before any erasure (every scanned drive, and a Queued Job recorded from the Job page) gets no certificate, so co-signing refuses it and it can never reach Destroyed; the Manifests page still offers it, and one such Job blocks co-signing its whole manifest.
    evidence: |-
      The certificate generator in crates/wipe-server/src/app.rs skips Jobs with no erasure activity, and cosign_manifest refuses a Job with no certificate (the c779361 guard). That was already true for Jobs escalated before erasure; this story makes it more common by adding the scan screen and the record button on Queued Jobs. Story 4 moves signing to Destroyed and certifies never-erased drives; story 3 reworks manifest assembly. Recorded as autonomous decision 4.
    location: >-
      crates/wipe-server/src/app.rs:218
    severity: medium
  - summary: >-
      The Job-page record dialog and the Record for destruction scan screen have no render test; acceptance criteria 1 and 2 are proven over HTTP and through pure helpers only.
    evidence: |-
      apps/desktop has no component-test toolchain (no @testing-library, jsdom or happy-dom), the same gap story 1 deferred. The gating logic (readyToRecord, previewKey, addScannedDrive, toRecordRequest) is unit-tested in api/recordForDestruction.test.ts, and the routes the screens call are covered by http_e2e tests.
    location: >-
      apps/desktop/src/pages/RecordForDestructionPage.tsx:1
    severity: low
---

<intent-contract>

## Intent

**Problem:** An operator can only send a drive toward destruction through "Escalate to destroy", which records no attestation: no equipment, location, signer title or statement, and no record of where the identifiers came from. It is offered only after an erasure failed, so a dead drive that never enumerates, or a drive meant for destruction from the start, cannot be recorded at all. The state is still called `PendingCoSign`, which says nothing about what has actually happened.

**Approach:** Replace the escalation with ADR-0011's first step, "recorded for destruction". The runner accepts a Destruction activity only when it carries a `DestructionAttestation`, and moves the Job to the renamed state `PendingDestruction`. Two routes build attestations through story 1's constructors: one records an existing Job, the other records a scanned group of drives, creating one Job per drive. A preview route renders the statement for the operator to read before confirming "I attest". The desktop UI gets a record dialog on the Job page and a new "Record for destruction" scan screen.

## Boundaries & Constraints

**Always:**
- `JobState::PendingCoSign` becomes `PendingDestruction`, serialized as `pending_destruction` and still accepting `pending_co_sign` on input. The UI calls the state "pending destruction" and never "destroyed".
- Attestations are built only through `DestructionAttestation::new` / `AttestationSigner::self_asserted`. Constructor errors return 422 with the error text. A request without `"attest": true` is 422 and records nothing.
- Identifier source is decided by the server, never claimed by the client. If the request carries `identifiers`, the source is `scanned`. Otherwise the source is `device`: the serial comes from the Job's latest erasure snapshot, or else from the enumerated device with the Job's `device_id`, and the asset tag from `JobSpec::asset_tag`. When no device serial can be read, the request is 422 with a message telling the operator to scan the identifiers. A device-sourced record never relies on an asset tag alone.
- Only `JobRunner` mutates Jobs. Every entry into `PendingDestruction` broadcasts `ActivityAdded` and `JobStateChanged`. The runner refuses a Destruction activity with no attestation (`InvalidState`).
- Recording a scanned group is all-or-nothing. Nothing is created if any drive fails validation, if two drives in the list share an asset tag or serial, or if any drive matches the asset tag or serial of a non-terminal Job. The last case is 409, and the message names the matching Job so the operator can record from it instead.
- Destruction never depends on licence state (ROADMAP §0.2).
- Rust and `types.ts` / `client.ts` change together. Docs that name the state or the route change in the same commit (AGENTS.md, CONTEXT.md §5/§14, README.md, docs/ARCHITECTURE.md).

**Never:**
- No change to when certificates are signed, or to the manifest/co-sign flow, apart from the state rename. Story 4 moves signing to `Destroyed`; story 3 reworks manifests.
- No unattested route into `PendingDestruction`. `POST /api/jobs/:id/escalate-to-destroy` is removed, not kept beside the new route.
- No edits to ADR text. ADRs are historical records and keep the name `PendingCoSign`.
- No SSO and no `verified` assurance.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| From Job, device read | Queued Job on attached `dev-nvme-0`, no `identifiers`, attest true | 200; Job `pending_destruction`; attestation `identifiers.source = device` with the device serial and the Job's asset tag | — |
| From Job, scanned | InProgress Job, `identifiers: {serial:"S1"}` | 200; `source = scanned`, serial `S1` | — |
| From Job, unreadable | Job's device detached, no erasure snapshot, no `identifiers` | nothing recorded | 422 naming the fix (scan identifiers) |
| Not attested | `attest` false or missing | nothing recorded | 422 |
| Blank field | blank equipment / location / full name / title | nothing recorded | 422 with the field name |
| Terminal / on manifest | Erased Job, or Job already on a manifest | nothing recorded | 409 |
| Unknown Job | random id | — | 404 |
| Scanned group | 3 drives with distinct serials, classification moderate | 200 `{job_ids:[3]}`; 3 new Jobs, intent `destroy`, `pending_destruction`, `source = scanned`, `device_id` `scanned:<serial or tag>` | — |
| Group duplicate | two drives with the same serial | nothing created | 422 |
| Group matches open Job | serial equals that of a non-terminal Job's device or recorded identifiers, or asset tag equals its `JobSpec::asset_tag` | nothing created | 409 naming the Job |
| Empty group | `drives: []` | — | 422 |
| Preview | technique, equipment, location, name, title | 200 `{statement}`, the same text a record with these inputs stores | 422 on blank fields |
| Old state name | `{"state":"pending_co_sign"}` | deserializes to `PendingDestruction`, re-serializes as `pending_destruction` | — |

## Autonomous decisions

1. **The unattested escalation is removed, not kept beside the new route.** *About:* how an operator sends a drive to destruction. *Problem:* if `escalate-to-destroy` stays, a Job can still reach pending destruction with no attestation, and story 4 would have to certify it anyway. *Options:* (a) keep both routes; (b) replace the old route with `record-for-destruction`, and have the runner refuse any Destruction activity without an attestation. *Choice:* (b). It is safest for the audit trail, and only this repo's desktop app and tests call the route.
2. **Scanned drives get new Jobs with a synthetic device id.** *About:* the scan screen for dead drives. *Problem:* `JobSpec::device_id` is required and sits inside signed v2 certificates, so it cannot be made optional. A dead drive has no enumerated device. *Options:* make the field optional (this breaks v2 bytes), or use `DeviceId("scanned:<serial, else asset tag>")` and skip the enumeration check for these Jobs. *Choice:* the synthetic id, created by a dedicated runner method.
3. **Duplicate protection refuses instead of merging.** When a scanned drive matches an open Job, the request is refused with that Job's id (409), rather than silently recording on the Job, so one asset never gets two open Jobs and nothing is recorded where the operator did not intend.
4. **Until story 4, a never-erased drive in pending destruction has no certificate.** The certificate trigger is unchanged (it skips Jobs with no erasure activity), so the existing co-sign guard (c779361) keeps these Jobs from reaching `Destroyed`. Story 4 certifies them at `Destroyed`. Doing that here would pre-empt story 4's schema change.
5. **The preview route exists because statement text is rendered only in Rust** (story 1's contract, and ADR-0002's single-source rule applied by analogy). The UI never builds the wording itself.
6. **The record button shows for Queued and InProgress Jobs**, whatever the latest erasure's state, which meets CAP-1's "any non-terminal state" for the states an operator acts from. The runner still accepts re-recording a pending Job that is not on a manifest, as escalation did (CONTEXT §5).
7. **A scanned group shares one statement and one time.** Each Job gets its own `DestructionAttestation` with its own identifiers. "Device(s)" in the research wording covers the group (ADR-0011 §1: "per drive or for a scanned group").
8. **Unknown `/api/*` paths on the station answer a JSON 404.** *About:* a client still calling the removed `escalate-to-destroy` route. *Problem:* `wipe-server` sent every unrouted path to the frontend or the "API only" landing page, so the removed route answered 200 and a stale client would believe it had recorded something; the acceptance criterion needs 404. *Options:* a catch-all only for the old path, or a `/api/*rest` catch-all for every unknown API path (what `wipe-hub` already does). *Choice:* the general catch-all; matched routes and their 405s are unchanged, and non-API paths still reach the frontend or landing page.
9. **Duplicate matching ignores ASCII case and surrounding whitespace**, both within a scanned group and against open Jobs, so a label typed in a different case is still caught (refusing is the safe direction).

</intent-contract>

## Code Map

- `crates/wipe-common/src/job.rs:55-81` -- `JobState::PendingCoSign` plus doc comments (`:55-61`, `:235`, `:296`). Rename the variant to `PendingDestruction` with `#[serde(alias = "pending_co_sign")]` and add a round-trip test. JobState is internally tagged (`tag = "state"`).
- `crates/wipe-common/src/destruction.rs:120-170` -- `DestructionAttestation::new`. Extract the statement into `pub fn attestation_statement(technique, full_name, title, equipment_id, location) -> Result<String, DestructionRecordError>` (trims and validates as `new` does), have `new` call it, and add a test that the two agree.
- `crates/wipe-engine/src/runner.rs:203-248` -- `escalate_to_destroy`: rename to `record_for_destruction`, refuse `attestation: None`, target `PendingDestruction`. `:250-330` -- `mark_destroyed` and `link_to_manifest`: rename only. Add `pub async fn record_scanned_for_destruction(&self, drives: Vec<(JobSpec, DestructionEvent)>) -> WipeResult<Vec<Uuid>>`. It enumerates the backend best-effort for the serials of Jobs that have no erasure snapshot, takes one write guard, checks open-Job matches, inserts the Jobs already in `PendingDestruction`, then broadcasts per Job. `:1-16` module doc.
- `crates/wipe-server/src/handlers.rs:280-317` -- `EscalateRequest` / `escalate_to_destroy`: replace them with `RecordForDestructionRequest { method, equipment_id, location, operator, full_name, title, attest: bool, notes, identifiers: Option<{asset_tag, serial}> }` and `record_for_destruction`. Add `record_scanned` (`{classification, drives: [{asset_tag, serial}], …common fields}` → `{job_ids}`) and `preview_attestation_statement`. Map `DestructionRecordError` to 422. `runner_err` (`:493`) already maps 404/409. Rename the `PendingCoSign` checks and messages at `:354-470`.
- `crates/wipe-server/src/app.rs:196-245` -- cert generator: rename only; behaviour unchanged. `:327-333` router: replace the escalate route with `/api/jobs/:id/record-for-destruction`, and add `/api/destruction/records` and `/api/destruction/statement` (POST).
- `crates/wipe-server/tests/http_e2e.rs` -- 12 escalate call sites (`:286, 552, 625, 704, 802, 841, 947, 1196, 1245, 1269, 1342, 1405`) and the direct runner call `:1063`. Move them to a shared helper that posts an attested body. Rename the `pending_co_sign` assertions.
- `crates/wipe-engine-mock/tests/end_to_end.rs:255-270` -- the `destruction()` fixture gains an attestation. Rename the calls and states (`:293-609`).
- `apps/desktop/src/api/types.ts:106-116` -- `JobStateLabel`. `client.ts:52-56,121-125` -- replace `EscalateRequest` / `escalateToDestroy` with `recordForDestruction`, `recordScanned`, `previewStatement`.
- `apps/desktop/src/pages/JobDetailPage.tsx:68-74,131-158,170-300` -- `showCert`, the abort gate, the button gate (failed erasure → Queued/InProgress), and `EscalateButton` → `RecordForDestructionButton`. Reuse the technique picker and caution already there; move the picker into a shared component.
- `apps/desktop/src/operator/context.tsx` -- `useOperator()` gives the `OperatorRef`; prefill the full name from `display_name`.
- `apps/desktop/src/main.tsx:34-86`, `components/Layout.tsx:173` -- route `/destruction/record` and a nav item.
- `apps/desktop/src/bench/slotStatus.ts:22,51,117,151`, `pages/JobsPage.tsx:17`, `pages/DevicesPage.tsx:261,363,415,474,517`, `pages/ManifestsPage.tsx:32`, `pages/manifestAssembly.ts:10,28` (+ test) -- state rename.
- `CONTEXT.md:253-284,344-379,818-820,869,1182-1198`, `README.md:28-43,306,329,359`, `docs/ARCHITECTURE.md:36,127-185`, `AGENTS.md:88` -- rename, document the new routes and the scan screen, and fix test counts.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-common/src/job.rs`, `destruction.rs` -- rename with alias; statement function; tests for the alias round trip and for the statement matching `new`.
- `crates/wipe-engine/src/runner.rs` -- `record_for_destruction` (refuses events with no attestation) and `record_scanned_for_destruction` (all-or-nothing, 409-class `InvalidState` naming the matching Job); renames.
- `crates/wipe-engine-mock/tests/end_to_end.rs` -- attested fixture; new tests: a record with no attestation is refused; a scanned group creates N pending Jobs with intent `destroy` and broadcasts; a group matching an open Job's asset tag creates nothing.
- `crates/wipe-server/src/handlers.rs`, `app.rs` -- the three routes and the renames as in the Code Map.
- `crates/wipe-server/tests/http_e2e.rs` -- attested helper; renames; a new test for every I/O matrix row reachable over HTTP.
- `apps/desktop/src/api/types.ts`, `client.ts` -- mirror.
- `apps/desktop/src/api/recordForDestruction.ts` (new) + `.test.ts` -- pure helpers: `addScannedDrive(list, drive)` (trims; refuses empty and duplicates, with a reason), `canPreview(form)`, `toRecordRequest(form, operator, attested)`; tests cover each.
- `apps/desktop/src/components/TechniquePicker.tsx` (new) -- the technique picker, label and caution moved out of `JobDetailPage`.
- `apps/desktop/src/pages/JobDetailPage.tsx` -- record dialog: technique, equipment ID, location, full name, title, notes; identifiers "from the device" (default) or "scanned" (asset tag/serial inputs); "Review statement" calls the preview, shows the statement verbatim and an "I attest" checkbox, and "Record for destruction" is enabled only once that box is ticked. Dialog copy says the Job becomes pending destruction.
- `apps/desktop/src/pages/RecordForDestructionPage.tsx` (new), `main.tsx`, `Layout.tsx` -- scan screen: classification, technique, equipment, location, name, title; a scan row (asset tag, serial, Enter adds) building a drive list; review statement; "I attest"; submit records the group and lists the created Jobs with links. The drive list and the attest box clear after a successful submit; other fields stay.
- Renames across the desktop files in the Code Map.
- `AGENTS.md`, `CONTEXT.md`, `README.md`, `docs/ARCHITECTURE.md` -- as in the Code Map. CONTEXT §5 **Recorded for destruction**: replace "Today: schema only" with what is built; add **Pending destruction** (renaming the **PendingCoSign** entry, old serde name noted). §14 to match.

**Acceptance Criteria:**
- Given a Queued Job on an attached drive, when the operator records it from the Job page with technique Shred, equipment SHR-2, location Bay 3, name and title, reviews the statement and ticks "I attest", then `GET /api/jobs/:id` shows `pending_destruction` and a destruction activity whose attestation has `source: device`, the device serial, `identity_assurance.level: self_asserted`, and the statement shown in the dialog.
- Given three dead drives scanned on the Record for destruction screen, when the operator attests and submits, then three Jobs appear in `GET /api/jobs` in `pending_destruction`, each with `source: scanned` and its own identifiers.
- Given the removed escalation route, when a client posts to `/api/jobs/:id/escalate-to-destroy`, then it gets 404 and the Job is unchanged.
- Given an erased-path Job recorded for destruction after a failed erasure, when the manifest is co-signed, then the existing cert and co-sign e2e tests pass with only the route and state renamed (no signing change).

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 30 findings — high 0, medium 7, low 17, false 6, maybe-false 0
- findings:
  - `[medium]` `[patch]` Preview race: `StatementReview`'s `onSuccess` keyed the statement by the latest form, not the form sent, so an edit made while the preview was in flight let the operator attest to wording the record does not sign — fixed: the form is passed as the mutation variable and the key is computed from it.
  - `[low]` `[patch]` With "From the device" selected, the Job-page dialog showed no identifiers before "I attest" — fixed: the dialog shows the serial it will record (snapshot, else attached device) and the Job's asset tag, or says the drive cannot be read and to choose Scanned.
  - `[low]` `[reject]` The record button now shows during a running erasure — SPEC CAP-1 requires recording "from a Job (any non-terminal state)", and the runner already lets a recording override the in-flight erasure's outcome; story 4 moves signing to `Destroyed`, which removes the partial-snapshot certificate.
  - `[medium]` `[patch]` A barcode scanner's Enter after each scan split one drive into two entries (tag-only and serial-only), which no duplicate check catches — fixed: Enter in the asset-tag field moves focus to the serial field; only Enter in the serial field or Add adds the drive.
  - `[low]` `[reject]` Duplicate protection is one-way: `create_job` and a from-Job record with scanned identifiers do not check other open Jobs — reaching it needs a dead drive that later enumerates or a mistyped serial, and the fix adds new guards to two more paths.
  - `[low]` `[reject]` A terminal Job whose drive is detached and has no snapshot gets 422 "scan it" instead of 409; an enumerate failure reads as "cannot be read" — the UI shows no record button on terminal Jobs, so only a direct API call reaches it, and the fix adds a duplicate guard ahead of the runner's.
  - `[low]` `[reject]` Scanned Jobs broadcast `JobStateChanged { from: Queued }` although they were created pending — no consumer treats the stream as history (the desktop refetches on it), and a truthful alternative needs a new broadcast variant mirrored in ws.ts.
  - `[low]` `[reject]` Duplicate rules exist in TypeScript, the handler and the runner — all three refuse a repeated tag or serial independently, case-insensitively, so they agree today; the TS copy is input feedback, not authority.
  - `[low]` `[patch]` The scan screen preselected classification `moderate`, so an operator who forgot to set it recorded a wrong classification on every Job — fixed: classification starts unselected and the record button stays disabled until one is picked. (The `shred` technique default stays: the technique is named in the statement the operator reads and attests.)
  - `[low]` `[patch]` Missing tests: catch-all keeps 405s, a scanned group's statement equals the preview, and the UI gating — fixed: http_e2e tests for the 405 and the group statement; the gating is covered by the `readyToRecord` row below.
  - `[false]` `[reject]` The pre-filled full name weakens the signature — ADR-0011 §4 defines the self-asserted signature as "the typed full name and title from the station session", which is where the pre-fill comes from; the operator can still edit it.
  - `[medium]` `[defer]` Scanned Jobs get no certificate, so co-signing refuses them and they never reach `Destroyed` — existing c779361 behaviour for never-erased Jobs; story 4 certifies at `Destroyed` (autonomous decision 4). Deferred (see `deferred`).
  - `[medium]` `[defer]` A Queued Job recorded from the Job page gets no certificate and is stuck pending destruction — same root cause as the row above; deferred with it.
  - `[medium]` `[defer]` Manifest assembly offers certificate-less pending Jobs, and one blocks co-signing its whole manifest — same root cause; story 3 reworks manifests. Deferred with it.
  - `[medium]` `[patch]` Preview race (edge-case layer) — same defect as the first row; fixed by the same change.
  - `[low]` `[reject]` The button on an in-progress Job lets a certificate be signed from a still-running erasure snapshot — same as the running-erasure row above, same reasoning.
  - `[low]` `[reject]` Scanned identifiers on an existing Job are not checked against other open Jobs — same as the one-way duplicate row above, same reasoning.
  - `[false]` `[reject]` TOCTOU between the handler's identifier read and the runner call — `JobSpec::device_id` never changes, so the snapshot serial and the enumerated serial name the same drive; state changes in the gap are re-checked by the runner under its write lock.
  - `[low]` `[reject]` The full name stays blank if the operator signs in after the dialog mounts — the operator types it, which is the signature anyway; recording is disabled until sign-in.
  - `[low]` `[reject]` Bare `/api` and `/api/` miss the JSON 404 catch-all — no client calls them, and the fix adds two routes.
  - `[medium]` `[patch]` Acceptance 1's "statement shown in the dialog" can differ from the stored one (edge-case claim) — same defect as the first row; fixed by the same change.
  - `[low]` `[patch]` `readyToRecord`, the only guard against recording against a changed statement, had no test — fixed: moved into `api/recordForDestruction.ts` with tests for not attested, no review, each statement input changed, and the accepting case.
  - `[low]` `[patch]` A device-sourced record whose serial comes only from the erasure snapshot (drive detached after a failed wipe) was untested — fixed: http_e2e test on `dev-sata-0` asserting `source: device` with the snapshot serial.
  - `[low]` `[patch]` The open-Job match via an erasure-snapshot serial was untested — fixed: end_to_end test with a detached, failed drive; the group is refused naming the Job and nothing is created.
  - `[low]` `[reject]` 422-before-409 ordering (verification-gap layer) — same as the earlier ordering row, same reasoning.
  - `[false]` `[reject]` `attested` survives when a field is changed and changed back — the key matches only when the inputs, and so the rendered statement, are identical, so the tick still stands against the exact wording shown.
  - `[low]` `[defer]` Acceptance 1 and 2 are proven over HTTP, not by rendering the screens — apps/desktop has no component-test toolchain (pre-existing; story 1 deferred the same). Deferred (see `deferred`).
  - `[false]` `[reject]` Attestations now enter v2 signed bytes at pending destruction, pre-empting story 4 — `verify` re-canonicalizes the stored certificate and every cert e2e test passes; carrying the attestation inside the signature is what ADR-0011 §2 asks for, and certificates without it keep their bytes.
  - `[false]` `[reject]` Scanning refuses rather than recording onto a matching open Job — autonomous decision 3 chose this, and the 409 names the Job so the operator records from it.
  - `[false]` `[reject]` The Job page hides the button on pending Jobs, against "any non-terminal state" — a pending Job is already recorded for destruction, which is CAP-1's outcome; the runner still accepts re-recording one not on a manifest.

## Design Notes

Request body shared by the two record routes (the scanned route adds `classification` and `drives`, and drops `identifiers`):

```json
{ "method": "shred", "equipment_id": "SHR-2", "location": "Bay 3",
  "operator": {"id":"op-1","display_name":"Ana Ruiz","email":"ana@example.com"},
  "full_name": "Ana Ruiz", "title": "Bench lead", "attest": true,
  "notes": null, "identifiers": {"asset_tag": "A-17", "serial": null} }
```

A device-sourced record keeps the Job's asset tag beside the device serial. `source: device` describes the serial, the identifier that was actually read from the drive. That is why a device-sourced record with no device serial is refused rather than falling back to the asset tag.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass.
- `cargo clippy --workspace --all-targets` -- expected: no new warnings in touched files.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: pass.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: no new diffs on the changed lines.
- `grep -rn "PendingCoSign\|pending_co_sign" crates apps/desktop/src AGENTS.md CONTEXT.md README.md docs/ARCHITECTURE.md` -- expected: only the serde alias, its test, and lines that name the old name on purpose.

## Auto Run Result

Status: done

**Summary.** Records drives for destruction under ADR-0011's first step. `PendingCoSign` is renamed `PendingDestruction` (serialized `pending_destruction`, old name still accepted on input). The runner accepts a destruction record only with the operator's attestation. `POST /api/jobs/:id/record-for-destruction` records an existing Job: the server decides whether the identifiers were scanned or read from the device. `POST /api/destruction/records` records a scanned group, one new pending Job per drive, all-or-nothing, refusing a drive that matches an open Job. `POST /api/destruction/statement` renders the statement the operator reads before "I attest". The unattested `escalate-to-destroy` route is gone, and unknown `/api/*` paths now answer a JSON 404. The desktop has a record dialog on the Job page and a new Record for destruction scan screen.

**Files changed**
- `crates/wipe-common/src/job.rs`, `destruction.rs` — state rename with alias; `attestation_statement`; tests.
- `crates/wipe-engine/src/runner.rs` — `record_for_destruction`, `record_scanned_for_destruction`, renames.
- `crates/wipe-server/src/handlers.rs`, `app.rs` — the three routes, the JSON 404 catch-all, renames.
- `crates/wipe-server/tests/http_e2e.rs`, `crates/wipe-engine-mock/tests/end_to_end.rs` — attested fixtures and new tests for every I/O matrix row.
- `apps/desktop/src/api/{types,client,recordForDestruction}.ts` (+ test) — mirror and pure helpers.
- `apps/desktop/src/components/{TechniquePicker,AttestationFields}.tsx`, `pages/{JobDetailPage,RecordForDestructionPage}.tsx`, `main.tsx`, `Layout.tsx` — dialog, scan screen, route and nav.
- `apps/desktop/src/{bench/slotStatus.ts,pages/{JobsPage,DevicesPage,ManifestsPage}.tsx,pages/manifestAssembly.ts}` (+ test) — state rename.
- `AGENTS.md`, `CONTEXT.md`, `README.md`, `docs/ARCHITECTURE.md`, `docs/ROADMAP.md` — docs and test counts.

**Review.** 30 findings: high 0, medium 7, low 17, false 6.
- **Patched** (by entry): medium 2 (the preview race, reported three times; scanner Enter splitting a drive); low 5 (device identifiers shown before attesting; classification starts unselected; `readyToRecord` tests; snapshot-sourced record test; snapshot-serial match test, plus the 405 and group-statement tests).
- **Deferred**: never-erased drives get no certificate and cannot reach `Destroyed` (medium, stories 3–4; three findings); no render test for the two screens (low, no toolchain).
- **Rejected**: every reason is in the triage log.
- **Follow-up review recommended: yes.** Two medium entries were patched. The specific unverified risk is the preview-race fix in `AttestationFields.tsx`: the statement shown and the one signed must match, and only the pure `readyToRecord` gate is tested, not the component.

**Verification**
- `cargo test --workspace` with compose up: 765 passed, 0 failed after the patches. The pre-patch run also failed `wipe-hub` `silent_smtp_and_database_time_out_within_bound` (3.5 s wall-clock limit), which is flaky on the base commit too and passed on the final run.
- `cargo clippy --workspace --all-targets`: no warnings in touched lines.
- rustfmt: touched files clean or no new diffs (`runner.rs`, `job.rs` were already unformatted at base).
- Desktop: typecheck passes, 235 vitest tests pass.
- The grep for `PendingCoSign`/`pending_co_sign` finds only the serde alias, its test, and deliberate mentions.
- I did not open the dialog or scan screen in a running app.

**Residual risks**
- Never-erased drives stay pending destruction without a certificate until story 4.
- Duplicate detection sees only Jobs in this station's memory, so not other stations or a restart; and only scanned groups are checked against open Jobs.
- Attestations now sit inside the signed bytes of v2 certificates issued at pending destruction; story 4's v3 schema should account for these.
- Scanned Jobs broadcast `from: queued` although they were created pending.
