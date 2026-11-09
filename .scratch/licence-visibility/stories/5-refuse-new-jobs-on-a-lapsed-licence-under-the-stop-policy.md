---
title: 'Refuse new jobs on a lapsed licence under the stop policy'
type: 'feature'
created: '2026-11-09'
status: 'ready-for-dev'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/docs/adr/0012-opt-in-hard-stop-on-licence-lapse.md', '{project-root}/docs/adr/0005-licensing-and-attestation.md']
warnings: ['oversized']
deferred:
  - summary: >-
      A station does not refuse new Jobs for a revoked stop licence: stations receive no revocation or supersession data, so only expiry and clock rollback trigger the refusal.
    evidence: |-
      Autonomous decision 3. ADR-0012 §5 limits revocation to what the station knows about; no revocation input exists on stations (licence-delivery story 4's deferred item; ADR candidates 0008/0010). Recorded as a CONTEXT §11 limitation.
    location: >-
      crates/wipe-server/src/licensing.rs (job_start_refusal)
    severity: medium
  - summary: >-
      Recording scanned drives for destruction still creates Jobs under a lapsed stop licence, so their destruction certificates are marked evaluation.
    evidence: |-
      Autonomous decision 4. ADR-0012's Consequences gate only the start_job path ("nothing else is gated"); §4's "destruction recording follows the same rule" is read as not interrupting destruction in progress. Gating new destruction records widens the §0.2 exception and needs its own decision. Recorded as a CONTEXT §11 limitation.
    location: >-
      crates/wipe-engine/src/runner.rs (record_scanned_for_destruction)
    severity: medium
---

<intent-contract>

## Intent

**Problem:** The hub signs a customer's `stop` **Lapse policy** into its licences (licence-delivery story 4), but no station code reads it: a station whose `stop` licence has expired keeps starting Jobs and producing `evaluation` certificates, which is exactly the outcome the customer opted out of (ADR-0012, SPEC CAP-5). The bench banner then also says "certificates are marked evaluation", which under `stop` would be wrong.

**Approach:** Give `JobRunner::start` a start gate, a trait defined in `wipe-engine` and installed by `AppState` over the shared licence cell. The gate refuses when the installed licence's policy is `stop` and the licence has lapsed (expired, or clock rollback detected). `start_job` answers 409 with the reason and how to install a licence offline; the Job stays `Queued`. `GET /api/license` gains `new_jobs_refused` (the same text, or `null`) so the banner says new Jobs are refused instead of re-deriving the rule in TypeScript.

## Boundaries & Constraints

**Always:** One rule, in Rust, in `LicenseContext` (`job_start_refusal(now) -> Option<String>`), used by both the gate and `view`. It is a pure read: no watermark fold, no lease save, no disk I/O. The gate reads the licence cell on every start, so a licence installed with `POST /api/license` lifts or imposes the refusal without a restart. The refusal is checked after the existing not-found and not-`Queued` checks (their 404/409 and wording are unchanged) and before the Job leaves `Queued`; a refused start emits no broadcast and changes no Job. A Job already `InProgress` is never touched: it runs to completion and its certificate follows `signing_decision` at signing time. The refusal text names the expiry time (or the rolled-back clock), says the licence's lapse policy is `stop`, says running Jobs are unaffected, and says how to install a licence offline: POST the licence file to `/api/license`, or restart `wipestation serve --license <file>`. Rust `LicenseView` and TS `LicenseInfo` change in the same commit. Docs in the same commit (see tasks).

**Never:** No refusal for `evaluation` or absent policy, an unlicensed station, `QuotaExhausted`, `WrongMachine`, `NotYetValid` or `LeaseStateUnavailable` alone (ADR-0012 §5 names only expiry, revocation and rollback). Nothing about a hub, network or licence server enters the check. No override, flag or config switch (ADR-0012 §7). No gate on enumeration, Job creation, abort, recording for destruction (device or scanned), manifests and destruction runs, certificate viewing or verification, or licence install. `wipe-engine` must not depend on `wipe-license` or `wipe-server`. No TypeScript re-evaluation of licence status. No hardware-sanitization claim.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Expired under stop | `stop` licence, `not_after` in the past; Job `Queued` | `POST /api/jobs/:id/start` → 409 `{"error": …}` naming the expiry time, `stop`, offline install; Job still `Queued`; `GET /api/license` `new_jobs_refused` = same text | Job untouched |
| Clock rolled back under stop | `stop` licence in date, lease watermark ahead of now beyond skew | 409, text says the clock moved back before the last recorded time | Job untouched |
| Valid under stop | `stop` licence in date, no network, no hub | start 200; `new_jobs_refused` `null` | none |
| Lease file fault, valid stop licence | context with a save or load fault | start 200 (status `lease_state_unavailable` does not refuse) | none |
| Running Job when the licence lapses | slow mock; Job started under a valid `stop` licence; then an authentic expired `stop` licence POSTed to `/api/license` | the running Job reaches `Erased`; its certificate has `evaluation: true`; a second Job's start → 409 | none |
| Install lifts the refusal | refused as above; POST a valid licence | 200; `new_jobs_refused` `null`; the same `Queued` Job now starts and is certified licensed | none |
| Evaluation or no field | expired licence with policy `evaluation` (serialized without the field) | start 200; certificate `evaluation: true` (unchanged behaviour) | none |
| Unlicensed | no licence | start 200 | none |
| Not refused elsewhere | expired `stop` licence | `GET /api/devices`, `GET /api/jobs/:id/certificate` of an earlier Job, offline `verify-cert` of that certificate, `POST /api/license`, `POST /api/jobs/:id/record-for-destruction` all behave as before | none |
| Unknown or non-queued Job | expired `stop` licence; unknown id / already-started Job | existing 404 / 409 "invalid state" text, not the licence refusal | unchanged |

## Autonomous decisions

1. **Where the check lives.** *What:* an operator starts a Job from the bench (`POST /api/jobs/:id/start`). *Problem:* ADR-0012 names "the `JobRunner` path behind `start_job`", but `wipe-engine` may depend only on `wipe-common`, so the runner cannot read a licence. *Options:* (a) check in the HTTP handler only; (b) a `StartGate` trait in `wipe-engine`, held by `JobRunner` in a cell shared by all its clones, implemented in `wipe-server` over the licence cell and installed by `AppState`. *Choice:* (b). *Why:* it follows the ADR's wording and the one-way dependency rule, and any future caller of `start` (bench inbox, CLI) goes through the same gate rather than bypassing it.
2. **What "lapsed" means at the station.** *Problem:* `evaluate` reports one verdict, in an order where `WrongMachine` precedes `Expired` and a lease fault overrides both, so reading its first verdict could hide an expiry. *Choice:* refuse when the licence is present, its policy is `stop`, and either the clock rollback check against the stored watermark fires or `now > not_after`, computed directly. Every other status never refuses. *Why:* ADR-0012 §5 lists exactly expiry, known revocation and rollback; anything wider gates erasure without a decision (ROADMAP §0.2).
3. **Revocation.** *Problem:* the SPEC lists "revoked", but stations receive no revocation or supersession data today (licence-delivery's deferred item; ADR candidates 0008/0010). *Choice:* no revocation input exists to check; the refusal covers expiry and rollback, and revocation joins when stations learn of it. Recorded as a CONTEXT limitation and a deferred item; no revocation test is possible. *Why:* inventing a station-side revocation list is new scope; ADR-0012 limits it to "revocation the station knows about", which is none.
4. **Destruction and other Job paths are not gated.** *Problem:* ADR-0012 §4 says destruction "follows the same rule", while its Consequences say "nothing else is gated". Recording scanned drives for destruction creates new Jobs whose destruction certificates would be `evaluation` under a lapsed `stop` licence. *Choice:* gate only `start`, as the Consequences and SPEC say; record the destruction gap as a deferred item and CONTEXT limitation. *Why:* gating destruction would widen the ADR's single exception to §0.2 and needs its own decision.
5. **HTTP shape.** *Choice:* new `WipeError::StartRefused(String)` (displayed verbatim), mapped to 409 Conflict with the existing `{"error"}` body; the Job stays `Queued` and can be started after a licence is installed. *Why:* 409 is the station's existing answer for "the station's state forbids this" (start of a non-queued Job, install without a vendor root); 403 would suggest authentication, which these routes do not have (D2).
6. **Banner after lapse under `stop`.** *Problem:* the current banner says certificates are marked evaluation, which is false for new Jobs under `stop`, and ADR-0012 §6 requires the station to show why it refuses and how to install. *Choice:* `LicenseView.new_jobs_refused` carries the refusal text; when set, the banner (danger tone) heads "New jobs are refused — licence lapsed under the stop policy" and shows that text. *Why:* keeps the rule server-side, like `operator_message`; the content is prescribed by ADR-0012 §6.

</intent-contract>

## Code Map

- `crates/wipe-engine/src/runner.rs:69-91` -- `JobRunner { backend, inner, events }`, `new`; add the shared gate cell here. `:140-179` `start`: check-then-transition under one write guard; insert the gate between a read-guard existence/`Queued` check and the write-guard re-check. Engine depends only on `wipe-common` (`crates/wipe-engine/Cargo.toml`).
- `crates/wipe-common/src/error.rs:4-29` -- `WipeError`; add `StartRefused(String)` (`#[error("{0}")]`). Existing unused-looking `License(String)` reads "license check failed: …"; do not reuse.
- `crates/wipe-server/src/handlers.rs:257-263` -- `start_job`; `:1013-1020` `runner_err` (add `StartRefused` → 409).
- `crates/wipe-server/src/licensing.rs:339-400` -- `status`, `view` (pure, `LicenseView` built at `:361-385`), `with_fault`; `:428-450` `LicenseView` struct. `LeaseState::rollback_detected(now)` and `Entitlements::not_after` give the two lapse checks; `body.lapse_policy` (`wipe_license::LapsePolicy`, `crates/wipe-license/src/chain.rs:39-96`).
- `crates/wipe-license/src/lease.rs:236-384` -- `LeaseStatus` (no revoked variant), `evaluate` (order: rollback, machine, not_before, not_after, quota).
- `crates/wipe-server/src/app.rs:62,85-125` -- `license` shared cell, `with_static_dir` builds state then `spawn_cert_generator`; install the gate there over `self.license.clone()`. `:181-230` `install_license` swaps the cell (lifts the refusal). `:490` start route.
- `crates/wipe-server/tests/http_e2e.rs:2266-2320` -- `spawn_server_with_license`, `license_chain(not_after, policy)`, `licensed_ctx`, `get_license`; the view key-set test near `:2340` must gain `new_jobs_refused`. Story 4's install tests show how to build a signed file for `POST /api/license` against a configured root (`with_vendor_roots`). `MockTiming` (`crates/wipe-engine-mock/src/lib.rs:22`) has slower timings for the running-Job case.
- `crates/wipe-engine-mock/tests/end_to_end.rs:530-640` -- runner `start` error tests; home for gate unit tests.
- `apps/desktop/src/api/types.ts:326-381` -- `LapsePolicy`, `LicenseInfo`; `apps/desktop/src/api/licence.ts:66-89` `licenceBanner`; fixtures in `licence.test.ts`, `components/LicenceBanner.test.ts`. `pages/DevicesPage.tsx:576-596,718` creates then starts a Job and already shows `create.error`.
- Docs: `README.md:376` (route table: `/api/license` fields, add start 409), `README.md:478-479` (limitation "no station code reads it" — remove), `CONTEXT.md:919` (§10 licensing row), `CONTEXT.md:1380-1385` ("not yet enforced"), §11 limitations near `:1003-1030`, `docs/ROADMAP.md:519-523` (L-R5), `.scratch/STATUS.md` L row.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-common/src/error.rs` -- add `WipeError::StartRefused(String)` -- distinct HTTP mapping and verbatim text.
- `crates/wipe-engine/src/runner.rs` -- `pub trait StartGate: Send + Sync { fn check_start(&self) -> Result<(), String>; }`; gate cell `Arc<RwLock<Option<Arc<dyn StartGate>>>>` shared by clones; `set_start_gate(&self, gate)`; `start` consults it as in Boundaries; doc comment states the ADR-0012 rule and that nothing else is gated -- the runner path ADR-0012 names.
- `crates/wipe-engine-mock/tests/end_to_end.rs` -- gate refusing: `StartRefused` with the gate's text, Job still `Queued`, no `JobStateChanged`; gate set on one clone applies to another; unknown/non-queued ids keep their errors under a refusing gate; clearing the gate lets the same Job start.
- `crates/wipe-server/src/licensing.rs` -- `job_start_refusal(&self, now) -> Option<String>` (decision 2, pure); `LicenseView.new_jobs_refused: Option<String>` filled from it; unit tests: expired-stop, rollback-stop, valid-stop, expired-evaluation, unlicensed, quota-exhausted/wrong-machine-but-in-date stop, save-fault valid-stop.
- `crates/wipe-server/src/app.rs` -- `LicenceStartGate` over the shared licence cell, installed in `with_static_dir` -- every host (CLI, Tauri, tests) gets it.
- `crates/wipe-server/src/handlers.rs` -- `runner_err`: `StartRefused` → 409.
- `crates/wipe-server/tests/http_e2e.rs` -- one test per I/O matrix row; update the view key-set test.
- `apps/desktop/src/api/types.ts` -- `LicenseInfo.new_jobs_refused: string | null`.
- `apps/desktop/src/api/licence.ts` + `licence.test.ts`, `components/LicenceBanner.test.ts` -- refusal banner first when `new_jobs_refused` is set (decision 6); fixtures gain the field; vitest for the new branch and that a `null` field leaves existing banners unchanged.
- `README.md`, `CONTEXT.md`, `docs/ROADMAP.md`, `.scratch/STATUS.md` -- enforced on stations; start 409; new view field; limitations: no revocation input (decision 3), destruction not gated (decision 4); ROADMAP L-R5 note shipped.

**Acceptance Criteria:**
- Given a station holding a lapsed `stop` licence, when the operator creates and starts a Job from the Devices page, then the dialog shows the refusal text and the Licence banner says new jobs are refused, on every page.
- Given the refusal, when a valid licence is POSTed to `/api/license`, then without a restart the banner clears and the same Job starts.
- Given `cargo test --workspace` (compose up) and `cd apps/desktop && pnpm typecheck && pnpm test`, when run, then all pass.

## Spec Change Log

## Review Triage Log

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass, including the new gate, licensing and HTTP tests.
- `cargo clippy -p wipe-engine -p wipe-server -p wipe-common --all-targets` -- expected: no new warnings in touched files.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: pass.
- `cargo tree -p wipe-engine --edges normal | grep -E "wipe-(license|server|cert)"` -- expected: no output.

## Auto Run Result

Status: ready-for-dev
Blocking condition: none — halted after planning as requested, so the spec can be reviewed before implementation (spec_checkpoint).
