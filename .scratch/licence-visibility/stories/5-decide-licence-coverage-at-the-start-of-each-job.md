---
title: 'Decide licence coverage at the start of each Job'
type: 'feature'
created: '2026-11-09'
status: 'ready-for-dev'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/docs/adr/0013-licensing-controls-every-erasure.md', '{project-root}/docs/adr/0005-licensing-and-attestation.md', '{project-root}/docs/adr/0011-destruction-by-operator-attestation.md']
warnings: ['oversized']
deferred: []
---

<intent-contract>

## Intent

**Problem:** The licence is evaluated only when a certificate is signed. A station with no licence, or an expired, exhausted or foreign one, still erases on any backend and produces an `evaluation` certificate. A Job that starts covered and finishes after expiry is marked `evaluation`. `allowed_methods` is never enforced. ADR-0013 §1–§2 reverse this.

**Approach:** One start gate, `LicenseContext::decide_start`, runs when a Job starts (`POST /api/jobs/:id/start`) and when drives are recorded for destruction (from an existing Job or scanned). On a non-simulated backend it refuses with 409 a start the licence does not cover. Otherwise it records a decision on the Job (`Job.licence`): evaluation or licensed, the licence chain, and one reserved credit per drive. The certificate generator signs from that recorded decision, spends the reservation at signing, and returns it at `Aborted`. `GET /api/license` reports `new_jobs_refused`, and the bench banner shows it.

## Boundaries & Constraints

**Always:**
- The coverage rule lives in Rust only, in `crates/wipe-server/src/licensing.rs` (`LicenseContext`). TypeScript only displays the server's text.
- Refuse only at a start, only on a backend whose `DeviceBackend::is_simulated()` is false, and only for these reasons:
  - no licence;
  - expired;
  - not yet valid;
  - wrong machine;
  - clock rollback;
  - method outside `allowed_methods`;
  - on a `Count` licence, too few credits left (used + reserved + requested > allowed);
  - on a `Count` licence, the reservation not saved within `LEASE_SAVE_TIMEOUT`, or a lease load fault.
- Each refusal text names the reason and the fix. On refusal:
  - the response is 409 `{"error": text}`;
  - the Job stays `Queued` (or, for a scanned record, nothing is created);
  - no `JobStateChanged` or `ActivityAdded` is broadcast.
- Check and reserve under one write lock on the lease, so concurrent starts cannot both take the last credit. Reserve before the runner changes the Job's state, and save the reservation first on a `Count` licence. If the runner then rejects the start (for example the Job was aborted meanwhile), return the reservation.
- The certificate uses `Job.licence` only (`evaluation`, `attestation`). It never re-evaluates the licence at signing. A Job with no recorded decision is certified `evaluation` and logged at warn.
- A running Job is never interrupted. Installing a licence (`POST /api/license`) lifts a refusal at the next start, without a restart.
- `Job.licence` is `#[serde(default, skip_serializing_if = "Option::is_none")]` and is mirrored in `apps/desktop/src/api/types.ts` in the same commit. It is never copied into the signed `Certificate`.
- `wipe-engine` stays free of `wipe-license` and `wipe-server` types. The decision type lives in `wipe-common` and carries the chain as opaque `serde_json::Value`.

**Never:**
- Gating on an unreachable hub or licence server, offering any override, or reading `lapse_policy` (it still parses, ADR-0013 §5).
- Refusing on the simulated backend (`wipe-engine-mock`). Its decisions are licensed when covered and `evaluation` otherwise, never refused.
- Building the organization pool, credit grants, top-ups, trial offerings, or hub/portal/marketing-site lapse-policy copy (`.scratch/controlled-licensing`). Revocation (ROADMAP A2.3, D6).
- Adding a `Failed` Job state, persisting Jobs, or editing `crates/wipe-cert/tests/fixtures/v2-*.json`.
- Making any hardware-sanitization claim (ROADMAP §0.1).

## I/O & Edge-Case Matrix

"Real" means a backend with `is_simulated() == false`, which in tests is a wrapper around `MockBackend`.

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Licensed start | Real backend, valid `Count{5}` licence, 0 used | Start returns 200. `Job.licence.evaluation` is false and the chain is attached. `GET /api/license` shows `remaining_erasures` 4 while the Job runs. At signing the certificate is licensed, used becomes 1 and reserved becomes 0 | — |
| No licence | Real backend, unlicensed | 409, with text naming "no licence" and the fix (install a licence) | Job stays `Queued`, no broadcast |
| Expired / not yet valid / wrong machine / rollback | Real backend, the matching licence or clock state | 409 for each, with distinct reason text including the date or machine | Same as above |
| Credits exhausted | Real backend, `Count{1}` and one Job already started | Second start gets 409 "no credits left", with the fix (add credits or install a licence with credits) | Same as above |
| Method not allowed | Real backend, `AllowedMethods::Only{[BlockOverwrite]}`, device resolves NVMe sanitize | 409 naming the method class | Same as above |
| Method unresolvable | Real backend, `Only{..}`, device not enumerated | 409 saying the method could not be checked | Under `All`, the start proceeds unpinned and the run fails as today |
| Reservation not saved | Real backend, `Count` licence, lease save fails or load fault | 409 naming the lease file and the error | Reservation undone in RAM |
| Lease fault, unlimited | Real backend, valid `Unlimited` licence, lease save or load fault | Start allowed with a **licensed** decision (`evaluation: false`, chain attached); the certificate is licensed. Expiry, not-yet-valid, machine and method still refuse as usual; rollback is checked against the in-memory watermark | Banner reports `lease_state_unavailable` |
| Lease fault, mock | Simulated backend, valid `Unlimited` licence, lease fault | 200, licensed decision, as above | — |
| Simulated backend | Mock backend, unlicensed, expired or exhausted | 200. Decision is `evaluation` and the certificate is `evaluation` | — |
| Spans expiry | Real backend, licence `not_after` falls between start and finish | Certificate is licensed (`evaluation: false`, chain attached) | — |
| Abort | Licensed Job reserved 1 credit, then aborted (from `InProgress` or `PendingDestruction`) | Reservation returned, `remaining_erasures` back to the previous value | — |
| Scanned destruction | Real backend, `POST /api/destruction/records` with 3 drives, 2 credits left | 409 "needs 3 credits, 2 left" | Nothing created |
| Install lifts refusal | Real backend, unlicensed start refused, then `POST /api/license` with a valid licence | Next start of the same Job returns 200 | — |
| View | Real backend, expired | `GET /api/license`: `new_jobs_refused` is the refusal text and `simulated_backend` is false | On the mock backend `new_jobs_refused` is `null` |

## Autonomous decisions

1. **How the station knows it is simulated.**
   - *Problem:* Nothing tells the mock backend from a real one, and only the mock exists today.
   - *Options:* a `StationConfig` flag, or a trait method.
   - *Choice:* `DeviceBackend::is_simulated(&self) -> bool { false }`, overridden to `true` by `MockBackend`.
   - *Why:* A future real backend is gated unless it claims to be simulated, which is the safe default. It also keeps the fact on the backend, so the two hosts cannot diverge.
2. **When the erasure method is checked.**
   - *Problem:* The method is chosen inside the spawned run, after start returns.
   - *Choice:* The start resolves the method with read-only `enumerate`/`capabilities` + `select_method` (Destroy intent resolves to `Destroy`), checks it, and pins it on the run. The pin uses the existing `ErasureEventSpec.method: Some`, so the method checked is the method issued.
   - *Unresolvable method:* refused under `AllowedMethods::Only`, because coverage cannot be shown. Under `All` the run starts unpinned, as today.
   - Destruction records check `MethodClass::Destroy`.
3. **Where the decision lives.** On `Job` (ADR-0013 §1 says "records the decision on the Job"), as `wipe_common::JobLicence { evaluation, attestation: Option<Value>, license_id: Option<String>, credits_reserved: u32, decided_at }`. This is visible in `GET /api/jobs` and outside the signed payload.
4. **Credit accounting before the pool exists.**
   - `LeaseState` gains `credits_reserved: u64` (serde default 0, skipped when 0).
   - The quota is checked against used + reserved. Every licensed decision reserves one credit per drive.
   - Signing a certificate spends the reservation: reserved goes down and used goes up. This includes a never-erased destroyed drive, per ADR-0013 §3.
   - `Aborted` returns the reservation.
   - There is no `Failed` Job state: a failed erasure stays `InProgress` and keeps its credit until the operator aborts it.
   - Jobs are not persisted, so `LicenseContext::open` returns every stored reservation at restart.
   - A licence switch (`open`/`with_chain` with a different id) resets reservations along with the count. Settling a decision whose `license_id` is not the lease's changes nothing.
5. **Lease fault on an `Unlimited` licence.** *(Changed at owner review, 2026-11-11.)* The start is not refused, because the §0.2 invariant lists no such reason. The decision is **licensed** when the licence's own checks pass (expiry, not yet valid, machine, method, and rollback against the in-memory watermark): a lease fault alone never makes a start uncovered. ADR-0013's baseline is that a paid licence never yields an `evaluation` certificate, and on an `Unlimited` licence the lease file only guards the rollback watermark. On a `Count` licence a lease fault still refuses (Boundaries).
6. **A started Job recorded for destruction keeps its start decision.** No second check and no second credit: the start's answer is final.
7. **`GET /api/license` shape.**
   - Drops `lapse_policy`, which is ignored now.
   - Adds `new_jobs_refused: string | null`, the status-based refusal text, or null on the simulated backend or when covered. Method refusals depend on the drive and are not predicted.
   - Adds `simulated_backend: bool`.
   - `LeaseStatus::operator_message` becomes consequence-neutral: it states the status and drops "Erasure still works" and "marked evaluation". The banner adds the consequence.
8. **Revocation** is listed in ADR-0013 §1, but no revocation channel exists (A2.3, D6). It is not a refusal reason here, and `CONTEXT.md` §10 records the limitation.
9. **Marketing-site and hub lapse-policy copy** is owned by `controlled-licensing` CAP-2, which shipped on 2026-11-10 (8c11473) with owner approval. This story does not touch it.

</intent-contract>

## Code Map

**Licensing core**
- `crates/wipe-server/src/licensing.rs`:
  - `LicenseContext` (L56) and `persist` (L258), which returns nothing. Add a variant that returns whether it saved.
  - `status` (L339) shows the evaluate-then-fold-watermark order to copy.
  - `view` (L361) is a pure read.
  - `signing_decision` (L403) and `SigningDecision` (L452) are replaced by `decide_start` / `spend` / `release`.
  - `record_erasure` (L313) is replaced by `spend`. `record_evaluation_erasure` (L327) is kept.
  - `open` (L131) and `with_chain` (L195) hold the licence-switch reset; extend it to reservations.
  - The module doc (L10-24) states the old rule.
- `crates/wipe-license/src/lease.rs`:
  - `LeaseState` (L31) gains `credits_reserved`.
  - `evaluate` (L332): quota checks against `erasures_used + credits_reserved`.
  - `LeaseStatus::operator_message` (L281): rewrite wording.
  - The doc at L276 says "still erases".
  - Pinned strings to update: `crates/wipe-license/tests/attestation.rs`, `apps/desktop/src/api/licence.test.ts`, `LicenceBanner.test.ts`.
- `crates/wipe-license/src/entitlement.rs`: `AllowedMethods::permits(&Method)` and `MethodClass::of`/`Destroy` (reuse); `Quota::remaining`.

**Engine and Job**
- `crates/wipe-engine/src/backend.rs:43`: `DeviceBackend`. Add `is_simulated` with a default.
- `crates/wipe-engine-mock/src/lib.rs:137`: override it to `true`.
- `crates/wipe-engine/src/runner.rs`:
  - `start` (L140): add `start_with(id, method: Option<Method>, licence: Option<JobLicence>)`, setting both under the existing write lock. Keep `start(id)` for engine and cert tests.
  - `run_job` (L522, spec built at L533 with `method: None`) gains a `method: Option<Method>` parameter. `start_with` passes the pin into the task it spawns (L160), so nothing is stored on the Job and nothing reaches the cert.
  - `record_for_destruction` (L215) and `record_scanned_for_destruction` (L267) gain an optional decision, set only when `job.licence` is `None`.
- `crates/wipe-common/src/job.rs:226`: `Job` gains `licence`. No literal `Job {}` exists in tests. `JobSpec` literals are unaffected.
- `crates/wipe-common/src/method.rs:197`: `select_method`.

**Server**
- `crates/wipe-server/src/app.rs`:
  - The `AppState.license` comment (L51) says "Never gates erasure".
  - Add `AppState::start_job`, `record_for_destruction` and `record_scanned` orchestration (resolve method, `spawn_blocking(decide_start)`, runner call, release on runner error).
  - `spawn_cert_generator` (L273-423): use `job.licence` instead of `signing_decision` (L336-350), spend after signing (replacing L398-413), and handle `to: Aborted`, which calls `release`.
  - `install_license` (L194) is unchanged.
- `crates/wipe-server/src/handlers.rs`:
  - `start_job` (L257) calls `state.runner.start` directly; route it through AppState.
  - `record_for_destruction` (L409, runner call L464) and `record_scanned` (L474, call L531): validate the request first, then gate.
  - The `runner_err`/`ApiError` precedent is at L1004-1025.
- `crates/wipe-cli/src/main.rs:81`: `--license` help says "still erases and still signs". Regenerate the help goldens.

**Tests**
- `crates/wipe-server/tests/http_e2e.rs`: `spawn_server_with_license` (L2268), `license_chain` (L2284, `Count`/`Only` variants needed), `erase_and_certify` (L2632), `http_wait_for_certificate` (L1294). `license_route_reports_a_stop_lapse_policy` (L2412) goes. `signing_decision` users at L2490-2546 and L2874.
- Other `signing_decision` callers to migrate to `decide_start` + `spend`: `crates/wipe-server/tests/lease_store.rs` (many, plus `runner.start` at L563, which must use `AppState::start_job` to get a licensed cert), `tests/topology_store.rs:330-362`, `tests/bootstrap.rs:165`, `src/bootstrap.rs:965-974`, and `src/licensing.rs` unit tests L596-620.

**Frontend**
- `apps/desktop/src/api/types.ts`: `LicenseInfo` (L345); remove `lapse_policy` (L351) but keep the `LicenseBody.lapse_policy` chain mirror (L381). `Job` (L423) gains `licence?`.
- `apps/desktop/src/api/licence.ts`: `licenceBanner` (L66), `lapsePolicyLine` (L52-56) and `LICENCE_UNKNOWN_BANNER` (L23).
- Start errors already reach the UI as `409 Conflict: <text>`: `client.ts:34`, `DevicesPage.tsx:677` and `batchSelection.ts:208` (`not_started`). No change.

**Docs**
- `CONTEXT.md`:
  - L937-944: the interim "until story 5" note.
  - L993: §10 licensing row ("`allowed_methods` unenforced").
  - L1017 and L1091-1094: lapse policy and the `stop` warning.
  - L1263: §12.
  - §5 "Lease state" glossary near L164-181.
- `README.md`: L380 (route row), L411 (`lapse_policy.rs` row is hub-side, so keep it), L491, L505-508, L516.
- `docs/ROADMAP.md`: L289, L591 (strike story-5 note), L629.
- `AGENTS.md`: test counts (946 / 385).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-engine/src/backend.rs`, `crates/wipe-engine-mock/src/lib.rs` -- add `is_simulated` (default false) and the mock override -- decision 1.
- `crates/wipe-common/src/job.rs` (+ `lib.rs` re-export) -- add `JobLicence` and `Job.licence` with serde default/skip, plus a doc stating it is not signed -- decision 3.
- `crates/wipe-engine/src/runner.rs` -- add `start_with` (sets `licence` under the start lock, passes the pinned method to `run_job(id, method)`), and let both destruction recorders take an optional decision, kept if one is already set -- one place mutates Job state.
- `crates/wipe-license/src/lease.rs` -- add `credits_reserved`, quota against used + reserved, and neutral `operator_message` wording; update the doc comments -- decisions 4 and 7.
- `crates/wipe-server/src/licensing.rs`:
  - Add `StartRequest { method: Option<Method>, drives: u32, simulated: bool }`, `StartRefusal` (reason enum + `message()` naming the fix) and `decide_start(now) -> Result<JobLicence, StartRefusal>`. It evaluates against the stored watermark, folds `now`, checks the method and credits, reserves, and saves with a checked result.
  - Add `spend(&JobLicence)` and `release(&JobLicence)`.
  - `open`/`with_chain` reset reservations.
  - `view(now, simulated)` adds `new_jobs_refused` and `simulated_backend` and drops `lapse_policy`.
  - Remove `signing_decision`, `SigningDecision` and `record_erasure`, and rewrite the module doc.
  - Add unit tests for each refusal reason, the reserve/spend/release arithmetic, the restart and licence-switch resets, and the concurrent last-credit case.
- `crates/wipe-server/src/app.rs`, `handlers.rs`, `lib.rs` -- AppState start/record orchestration with release on runner error; 409 refusals; the certificate generator signs from `job.licence`, spends after signing, keeps the evaluation count for evaluation decisions with an erasure, and releases on `Aborted`; `view` callers pass `runner.backend().is_simulated()`.
- `crates/wipe-server/tests/http_e2e.rs` -- add a `RealBackend(Arc<MockBackend>)` wrapper (delegates and reports `is_simulated() == false`) and a builder for it. Add one HTTP test per I/O matrix row. Migrate the `signing_decision` users and remove the lapse-policy route test.
- `crates/wipe-server/tests/lease_store.rs`, `topology_store.rs`, `bootstrap.rs`, `crates/wipe-server/src/bootstrap.rs` -- migrate to `decide_start`/`spend` and the gated start.
- `crates/wipe-cli/src/main.rs` -- update the `--license` help text, then run `UPDATE_GOLDENS=1 cargo test -p wipe-cli --test help_golden`.
- `apps/desktop/src/api/types.ts`, `licence.ts`, `licence.test.ts`, `components/LicenceBanner.test.ts`:
  - Mirror the types.
  - When `new_jobs_refused` is set, show a danger banner "New jobs are refused" with that text.
  - In expiry warnings, the line says new jobs will be refused from the expiry date until a renewed licence is installed; when `simulated_backend` is true, it says jobs still run with `evaluation` certificates.
  - Remove `lapsePolicyLine`.
  - The unknown banner says new jobs may be refused or certificates marked evaluation.
- `CONTEXT.md`, `README.md`, `docs/ROADMAP.md`, `AGENTS.md`, `.scratch/STATUS.md`:
  - Describe the start gate as built and the reservation accounting.
  - Record the revocation and `Failed`-state limitations.
  - Strike the ROADMAP story-5 notes.
  - Update test counts.

**Acceptance Criteria:**
- Given a station on a real backend with no licence, when the operator starts a Job or records a scanned drive, then the HTTP response is 409 naming the reason and fix, `GET /api/jobs/:id` shows `queued`, and no WebSocket `job_state_changed` arrives for it.
- Given that refused station, when a valid licence is installed via `POST /api/license`, then starting the same Job returns 200 without a restart, and its certificate carries the attestation chain.
- Given a licensed Job started before `not_after` and signed after it, when `GET /api/jobs/:id/certificate` is read, then `certificate.evaluation` is false and `attestation` is present.
- Given a `wipestation serve` station (mock backend) with no licence, when a Job runs, then it is never refused, its certificate is `evaluation`, and `GET /api/license` shows `new_jobs_refused: null` and `simulated_backend: true`.
- Given a refused start on a real backend, when `GET /api/license` is read, then `new_jobs_refused` holds the same reason text, and the desktop banner model (`licenceBanner`) returns a danger banner showing it.
- Given a `Count{2}` licence with one Job aborted and one certified, when `GET /api/license` is read, then `remaining_erasures` is 1.

## Spec Change Log

- 2026-11-11, owner review: decision 5 changed. A lease fault on a valid `Unlimited` licence now yields a licensed decision, not `evaluation` (ADR-0013: a paid licence never produces an `evaluation` certificate). The matrix row and design note follow; a mock-backend row was added. Decision 9 updated: `controlled-licensing` story 2 has shipped.

## Review Triage Log

## Design Notes

The order inside `decide_start`, under the lease write lock, then save:

```rust
let status = evaluate(ent, &lease, &fp, now);          // before folding now in
lease.observe(now);
// A LeaseStateUnavailable verdict is not itself uncovered: judge the licence's own
// checks (expiry, not-yet-valid, machine, rollback vs the in-memory watermark).
// On a Count licence the fault refuses separately (Boundaries).
let covered = licence_checks_pass(&status) && method_ok && credits_ok(req.drives);
if !covered && !req.simulated { return Err(refusal) }  // nothing reserved
if covered { lease.credits_reserved += req.drives }    // then save; Count + save error => undo, refuse
```

The refusal text pattern is "<what is wrong>. <how to fix it>." For example: "This station's licence expired on 2026-12-31. Install the renewed licence to start new jobs."

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass, no output piped through grep.
- `cargo clippy -p wipe-server -p wipe-license -p wipe-engine -p wipe-common --all-targets` -- expected: no new warnings in touched files.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.
- `UPDATE_GOLDENS=1 cargo test -p wipe-cli --test help_golden`, then `cargo test -p wipe-server --test route_golden` -- expected: the help goldens regenerated and committed; the route golden unchanged.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: clean and all pass.

## Auto Run Result

Status: ready-for-dev
Blocking condition: none. Halted after planning, as the invocation directed, so the owner can review the plan before implementation (`spec_checkpoint: true`).

Planning made nine decisions without halting; they are listed under "Autonomous decisions" in the intent contract. The ones to read first:
- **Decision 1:** a backend counts as simulated only if it says so (`is_simulated`, default false).
- **Decision 4:** credits are reserved against the licence's own `Count` quota. Reservations are returned at restart, and a failed erasure keeps its credit until it is aborted.
- **Decision 5:** a lease fault on an unlimited licence is not a refusal reason, and the start is licensed (changed at owner review, 2026-11-11).
- **Decision 9:** marketing-site and hub lapse-policy copy is left to `controlled-licensing`.
