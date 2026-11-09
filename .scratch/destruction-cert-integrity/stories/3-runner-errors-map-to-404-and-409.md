---
title: 'Runner errors map to 404 and 409'
type: 'bugfix'
created: '2026-11-08'
status: 'done'
baseline_revision: '2ec0f92e60b24976f4c22d2315c7ea086b162063'
review_loop_iteration: 0
followup_review_recommended: false
context: []
warnings: [oversized]
deferred:
  - summary: >-
      Supervisor co-sign is not atomic against concurrent requests: two simultaneous co-signs of one manifest both pass validation and the certificate gains two co-signatures; an abort landing mid co-sign leaves some Jobs Destroyed, some certificates co-signed and the manifest still Pending; mark_destroyed checks and transitions under two locks, so an abort between them is overwritten to Destroyed.
    evidence: |-
      cosign_manifest validates under a read lock, then loops co_sign + mark_destroyed per Job, and only marks the manifest Signed at the end (handlers.rs:440-500). mark_destroyed uses get() then mutate_job() (runner.rs:252-270). Pre-existing; this story only changed the status of a mark_destroyed refusal from 500 to 409, which now reads as a clean refusal although earlier Jobs were already changed. Conflicts with SPEC's success signal "no sequence of HTTP calls can produce ... a certificate co-signed twice" under concurrent calls. Fix: claim the manifest (Pending -> Signing) under manifests.write() before co-signing, and make mark_destroyed single-guard.
    location: >-
      crates/wipe-server/src/handlers.rs:440-500
    severity: medium
  - summary: >-
      An abort or escalation that lands while the erasure task is probing (after run_job's start-of-task check, before backend.issue) still issues the wipe command and appends an Erasure activity to an Aborted or PendingCoSign Job.
    evidence: |-
      run_erasure_event awaits enumerate/capabilities, appends the Erasure activity and calls backend.issue without re-checking the outer Job state (runner.rs:445-515); it only notices Aborted in its polling loop. Pre-existing (before this story the same window existed after run_job set InProgress). Fix: check state == InProgress under the write guard when appending the activity, and again before issue.
    location: >-
      crates/wipe-engine/src/runner.rs:445-515
    severity: medium
  - summary: >-
      AGENTS.md still gives the old Rust test count (689).
    evidence: |-
      AGENTS.md says `cargo test --workspace` runs 689 tests; the workspace now runs over 720. AGENTS.md is owner-maintained agent context.
    location: >-
      AGENTS.md
    severity: low
---

<intent-contract>

## Intent

**Problem:** An API client (the desktop app, or a script driving a bench) that calls start, abort or escalate-to-destroy on a Job, or co-signs a manifest, gets HTTP 500 Internal Server Error when the runner refuses: an unknown Job id and an invalid state transition (start a Job twice, escalate a terminal Job, re-escalate a Job on a manifest) all surface as 500 via `api_err`. The runner reports "not found" as `WipeError::InvalidState("job … not found")`, so the handler cannot tell the two apart. Separately, `start` checks `Queued` on a clone and the spawned task sets `InProgress` later, so two quick starts both answer 200 and run two erasures, and a Job escalated or aborted in that gap is overwritten back to `InProgress` (CAP-3, SPEC `destruction-cert-integrity`).

**Approach:** Give the runner a typed not-found error (`WipeError::JobNotFound(Uuid)`) and map runner errors in one handler helper: `JobNotFound` → 404, `InvalidState` → 409, anything else stays 500. Make `start` and `abort` check and transition under one write guard, as `escalate_to_destroy` already does.

## Boundaries & Constraints

**Always:**
- Only `JobRunner` mutates Jobs; the fix lives in the runner and the handler mapping, never in a handler-side state check that duplicates the runner's.
- Error bodies keep today's shape `{"error": "<message>"}` and today's message text (`job <id> not found`, `job <id> is in state …`, etc.).
- Keep c779361's and story 1's guarantees: co-sign validates the whole manifest before mutating and refuses a Job with no certificate; a Job on a manifest cannot be re-escalated; `create_manifest` keeps its own 404/409 mapping.
- Broadcasts are sent after the write guard is dropped, in the same order as today (`JobStateChanged` Queued→InProgress is now sent by `start`, before the task is spawned).

**Never:**
- No new routes, no TypeScript changes (`client.ts` already throws on any non-2xx and shows the body's message), no change to `wipe-cert` or the certificate.
- No change to `create_job`'s `DeviceNotFound` mapping or the device/topology/sim routes.
- No fix for the deferred story-1 items (aborting a linked PendingCoSign Job; the certificate generator overwrite race).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Unknown Job | random UUID to `POST /api/jobs/:id/start`, `/abort`, `/escalate-to-destroy` | 404, body names the Job | `JobNotFound` |
| Start twice | `start` on a Job, then `start` again immediately | first 200, second 409; one ErasureEvent only | `InvalidState` |
| Start after escalate | Queued Job escalated, then `start` | 409; Job stays `pending_co_sign` | `InvalidState` |
| Escalate terminal | Aborted Job, `escalate-to-destroy` | 409; no DestructionEvent appended | `InvalidState` |
| Re-escalate linked | Job on a manifest, `escalate-to-destroy` | 409, body names the manifest | `InvalidState` |
| Abort terminal | Erased or Destroyed Job, `abort` | 409; state unchanged | `InvalidState` |
| Abort twice | Aborted Job, `abort` again | 200 no-op | — |
| Co-sign unknown manifest | random UUID to `/api/manifests/:id/cosign` | 404 | existing |
| Co-sign twice | signed manifest co-signed again | 409; the certificate still has exactly one co-signature | existing |
| Malformed id | `POST /api/jobs/not-a-uuid/start` | 400 (Axum path rejection, unchanged) | — |

## Autonomous decisions

- **Aborting a Job that has already finished.** Today `POST /api/jobs/:id/abort` on an Erased or Destroyed Job answers 200 `{"ok": true}` and does nothing, so a script is told the Job was aborted when it was not. Options: (a) keep 200 for every terminal Job; (b) 409 for every terminal Job, including one already Aborted; (c) 409 for a Job that finished in any other way, 200 no-op for one already Aborted. Chosen: **(c)**. The reply is then always true (the Job is aborted, or the error says why it is not), and a retried abort after a lost reply still succeeds, which is the usual idempotent-retry expectation. The desktop app hides the Abort button on terminal and `pending_co_sign` Jobs, so operators see no change.
- **Fixing the double-start race here.** A second `start` before the spawned task runs is the "invalid transition" CAP-3 names, but today it answers 200 and runs a second erasure on the same drive. Chosen: move the Queued→InProgress transition into `start` under one write guard (the task no longer sets it), so the second call is a deterministic 409. It is the safe default for the evidence record (one erasure per start), and is one guard, the same pattern story 1 used for `escalate_to_destroy`.
- **Not-found as a typed error.** Matching on the message text "not found" would break silently when wording changes. Chosen: a new `WipeError::JobNotFound(Uuid)` variant with the same display text, used at every runner "job … not found" site.

</intent-contract>

## Code Map

- `crates/wipe-common/src/error.rs` -- add `#[error("job {0} not found")] JobNotFound(uuid::Uuid)`; check `uuid` is already a `wipe-common` dependency (it is used by `job.rs`). No exhaustive `match` on `WipeError` exists in the workspace (only `matches!`), but confirm by building.
- `crates/wipe-engine/src/runner.rs:127-150` -- `start`: today `get` clone + `Queued` check, then spawn. Change to: under one `self.inner.write()`, `get_mut` (→ `JobNotFound`), check `Queued` (→ `InvalidState`, keep the "is in state" text), set `InProgress` + `started_at`; drop the guard, send `JobStateChanged` Queued→InProgress, then spawn.
- `crates/wipe-engine/src/runner.rs:367-382` -- `run_job`: remove its own Queued→InProgress mutate and broadcast (now done by `start`); keep reading the Job for the spec. Update the doc comments on `start` and `run_job`.
- `crates/wipe-engine/src/runner.rs:154-174` -- `abort`: under one write guard: missing → `JobNotFound`; `Aborted` → `Ok(())` with no broadcast; other terminal → `InvalidState("job {id} already terminal (state …)")`; else set `Aborted` + `ended_at`. Broadcast after dropping the guard. Doc comment lists the refusals.
- `crates/wipe-engine/src/runner.rs:192,231,270,370` -- the other `"job {id} not found"` sites (`escalate_to_destroy`, `mark_destroyed`, `link_to_manifest`, `run_job`) → `JobNotFound(id)`. Leave `wait_for_*` helpers (test utilities) as they are.
- `crates/wipe-server/src/handlers.rs:505-509` -- next to `api_err`, add `runner_err(e: WipeError) -> ApiError` (`JobNotFound` → 404, `InvalidState` → 409, else 500). Use it in `start_job` (258), `abort_job` (266), `escalate_to_destroy` (305) and `cosign_manifest`'s `mark_destroyed` call (492). Leave `create_manifest`'s link mapping (417, always 409) and the `co_sign` `api_err` (wipe-cert errors stay 500).
- `crates/wipe-engine-mock/tests/end_to_end.rs:~400` -- story 1's `link_to_manifest_refuses_an_unknown_job_and_stamps_nothing` asserts `InvalidState`; change it to `JobNotFound`. Add runner tests here (`mock_runner()` helper exists).
- `crates/wipe-server/tests/http_e2e.rs` -- `spawn_server()` (13), `latest_destruction()` (433); `aborted_job_does_not_get_certificate` (159) shows create/start/abort; `re_escalating_a_job_on_a_manifest_is_refused` (754, asserts only non-2xx at ~847) to tighten to 409; `destroy_path_via_manifest_cosign_produces_two_signatures` (224) shows escalate + manifest + co-sign bodies.
- `apps/desktop/src/api/client.ts` -- `jsonFetch` throws on non-2xx; no change. Read-only.
- `CONTEXT.md:756-770` (§10.2 table, "REST: escalate-to-destroy, manifests, co-sign" row) and the JobState entry (~257-263) -- as-built note: unknown Job 404, refused transition 409; abort of a finished Job is refused unless already Aborted. `README.md:351,353` -- test-table rows and counts for `end_to_end.rs` and `http_e2e.rs`.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-common/src/error.rs` -- add `JobNotFound(Uuid)` -- lets the handler tell not-found from a refused transition without string matching.
- `crates/wipe-engine/src/runner.rs` -- `JobNotFound` at every runner not-found site; `start` and `abort` check and transition under one write guard as mapped above; `run_job` no longer performs Queued→InProgress -- the runner stays the only Job mutator and its refusals are race-free.
- `crates/wipe-server/src/handlers.rs` -- add `runner_err` and use it on start, abort, escalate and co-sign's `mark_destroyed` -- CAP-3.
- `crates/wipe-engine-mock/tests/end_to_end.rs` -- update the unknown-id assertion; add tests: start twice → second `InvalidState` and exactly one Erasure activity after the Job finishes; start of an escalated Job → `InvalidState`, state stays `PendingCoSign`; abort of an Erased Job → `InvalidState`, state unchanged; abort of an Aborted Job → `Ok`; start/abort/escalate of an unknown id → `JobNotFound`.
- `crates/wipe-server/tests/http_e2e.rs` -- one test per route asserting exact statuses: unknown id → 404 on start, abort and escalate (body `error` contains the id); start twice → 200 then 409; escalate an Aborted Job → 409; abort an Erased Job → 409 then `GET` still `erased`; abort an Aborted Job → 200; co-sign an unknown manifest → 404; co-sign a signed manifest again → 409 and the certificate's co-signature list still has length 1. Tighten `re_escalating_a_job_on_a_manifest_is_refused` to `assert_eq!(status, 409)`.
- `CONTEXT.md`, `README.md` -- the as-built notes and test counts above.

**Acceptance Criteria:**
- Given any route among start, abort, escalate-to-destroy, when called with a well-formed but unknown Job id, then the response is 404 with `{"error": …}` naming the id.
- Given a Job in a state the action cannot leave from, when start, abort, escalate-to-destroy or co-sign is called, then the response is 409 and the Job, its activities and its certificate are unchanged.
- Given a just-created Job, when `start` is posted twice back to back, then exactly one is 200, the other 409, and the finished Job has one Erasure activity.
- `cargo test --workspace` passes with compose up.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 26 findings — high 0, medium 8, low 12, false 6, maybe-false 0
- findings:
  - `[medium]` `[patch]` Blind: abort/escalate between `start` returning and the spawned task still runs the erasure — verified: `run_job` reads the Job and calls `run_erasure_event`, which appends the Erasure activity and calls `backend.issue` with no state check (runner.rs:386-515); patched: `run_job` returns early unless the Job is `InProgress`; `start` doc comment reworded; two no-await runner tests added.
  - `[low]` `[reject]` Blind: `start`'s Queued→InProgress broadcast can be overtaken by a concurrent abort's broadcast — real but needs two requests within microseconds; the UI refetches Job state by polling; sending under the guard contradicts the intent contract's "broadcasts after the guard is dropped" rule, the same as `escalate_to_destroy`.
  - `[false]` `[reject]` Blind: double-start fix has no concurrent test — `starting_a_job_twice_is_refused_and_runs_one_erasure` calls `start` twice with no await on a current-thread `#[tokio::test]`, so the old code (task not yet run) would have answered Ok twice; it reproduces the race deterministically.
  - `[low]` `[patch]` Blind: CONTEXT §10.2 says co-sign answers 404 for an unknown Job — verified: co-sign's preflight answers 409 for a member Job problem, 404 only for an unknown manifest; patched: row reworded.
  - `[medium]` `[defer]` Blind: co-sign failure in `mark_destroyed` now answers 409 after earlier Jobs were changed — verified: reachable only by a concurrent abort mid co-sign; partial application is pre-existing; deferred with the co-sign atomicity group.
  - `[low]` `[reject]` Blind: re-escalating an unlinked PendingCoSign Job is still 200 — intended: story 1 kept re-escalation allowed until a Job is on a manifest (CONTEXT.md records it); not a refused transition.
  - `[low]` `[patch]` Blind: abort docs omit `Quarantined` — verified `is_terminal` includes it; patched: doc comment and CONTEXT wording say "any terminal state other than Aborted".
  - `[low]` `[reject]` Blind: no unit test pins `runner_err`'s 500 fallback — a three-arm match read in one glance; HTTP tests pin the 404/409 arms; no everyday harm.
  - `[low]` `[reject]` Blind: `started_at` and broadcast `at` read the clock twice — microsecond skew, same pattern as every existing runner transition; no consumer compares them.
  - `[low]` `[defer]` Blind: AGENTS.md test count stale — agent-context file; deferred in frontmatter.
  - `[false]` `[reject]` Blind: story file has no recorded results — the triage log and Auto Run Result are written by this review and its finalize step.
  - `[medium]` `[patch]` Edge: erasure issued after abort/escalate in the post-start gap — same root cause as row 1.
  - `[low]` `[reject]` Edge: broadcast reordering between `start` and a concurrent abort — same as row 2.
  - `[medium]` `[defer]` Edge: two concurrent co-signs both pass validation and add two co-signatures — verified handlers.rs validation under a read lock, manifest marked Signed only at the end; pre-existing; deferred (co-sign atomicity).
  - `[medium]` `[defer]` Edge: `mark_destroyed` check-then-act lets an abort be overwritten to Destroyed — verified runner.rs `get` then `mutate_job`; pre-existing; deferred (co-sign atomicity).
  - `[medium]` `[defer]` Edge: 409 after partial co-sign — same as row 5.
  - `[medium]` `[patch]` Edge (claim): `start` doc comment says abort/escalate cannot interleave — same root cause as row 1; doc reworded.
  - `[low]` `[defer]` Edge (claim): "one co-signature" holds only for sequential calls — same as row 14.
  - `[low]` `[patch]` Edge (claim): CONTEXT says co-sign unknown Job is 404 — same as row 4.
  - `[low]` `[patch]` Verification gap: nothing tests abort/escalate right after `start` — pre-verified; patched with two no-await runner tests (state kept, no Erasure activity, one Queued→InProgress broadcast).
  - `[medium]` `[patch]` Verification other: task never checks `InProgress` before work — same as row 1.
  - `[low]` `[defer]` Verification other: escalate in the gap leaves a PendingCoSign Job whose erasure later completes and co-sign is refused — the post-start gap is patched (row 1); the wider probing window is deferred in frontmatter.
  - `[false]` `[reject]` Intent: abort-of-finished 409 and synchronous Queued→InProgress broadcast go beyond reading A — recorded autonomous decisions in the intent contract under CAP-3's "invalid transitions 409"; not a defect.
  - `[false]` `[reject]` Intent: escalate unknown-Job test passes on the baseline — true (the handler pre-looks the Job up), but it pins CAP-3's required 404; not a defect.
  - `[false]` `[reject]` Intent: co-sign tests pin behaviour that was already correct — CAP-3 asks for tests covering each route; pinning correct behaviour is the requirement.
  - `[false]` `[reject]` Intent: a manifest listing an unknown Job gets 409, not 404 — unreachable: Jobs are never removed from the runner and `create_manifest` 404s unknown ids before storing.

## Verification

**Commands:**
- `cargo test -p wipe-engine-mock -p wipe-server -p wipe-cert` -- expected: all pass, including new tests.
- `cargo test --workspace` (after `docker compose up -d`) -- expected: all pass.
- `cargo clippy -p wipe-common -p wipe-engine -p wipe-server --all-targets` -- expected: no new warnings versus baseline.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: diffs only in pre-existing unformatted code (`runner.rs` is known-unformatted; do not reformat it wholesale).

## Auto Run Result

Status: done

**Summary.** Start, abort and escalate-to-destroy now answer **404** for an unknown Job and **409** for a transition the Job's state does not allow, instead of 500. Co-sign keeps 404 for an unknown manifest and 409 for a manifest that is not pending (a second co-sign included); a runner refusal inside it is now 409 rather than 500. The runner reports a missing Job as a typed `WipeError::JobNotFound`, and one handler helper, `runner_err`, maps runner errors (anything else stays 500). Error bodies keep their shape and text. Two runner races were fixed along the way:
- `start` now checks `Queued` and moves the Job to `InProgress` under one lock, so a second `start` is a reliable 409 and only one erasure runs.
- The erasure task does nothing if the Job was aborted or escalated between `start` and the task's first step (found in review).

Abort of an `Erased`, `Destroyed` or `Quarantined` Job is now refused (409); aborting an already `Aborted` Job is a 200 no-op (recorded autonomous decision).

**Files changed**
- `crates/wipe-common/src/error.rs`: new `WipeError::JobNotFound(Uuid)`.
- `crates/wipe-engine/src/runner.rs`: `JobNotFound` at every runner not-found site; `start` and `abort` check and transition under one write guard; `run_job` no longer sets `InProgress` and returns early if the Job has left it.
- `crates/wipe-server/src/handlers.rs`: `runner_err` (404/409/500) used on start, abort, escalate and co-sign's `mark_destroyed`.
- `crates/wipe-engine-mock/tests/end_to_end.rs`: eight runner tests (start twice, start before return, start an escalated Job, abort Erased, abort Aborted, unknown ids, abort and escalate right after start); the story-1 unknown-id test now expects `JobNotFound`.
- `crates/wipe-server/tests/http_e2e.rs`: nine HTTP tests covering every scenario-table row; the re-escalation test now asserts exactly 409.
- `CONTEXT.md`, `README.md`: JobState and §10.2 REST notes; test inventory (713 Rust).

**Review findings** (26: 8 medium, 12 low, 6 false)
- Patched (3 entries: 1 medium, 2 low):
  - `run_job` no longer erases a Job that left `InProgress` before the task ran, with two new tests (rows 1, 12, 17, 20, 21).
  - The CONTEXT co-sign status wording (rows 4, 19).
  - Abort docs now name `Quarantined` (row 7).
- Deferred (frontmatter):
  - Co-sign is not atomic under concurrent requests (two co-signatures; partial application; `mark_destroyed` check-then-act). Medium, pre-existing.
  - An abort or escalation during probing still issues the wipe. Medium, pre-existing.
  - The AGENTS.md test count. Low.
- Rejected, with reasons:
  - Broadcast reordering between `start` and a concurrent abort: microsecond window, the UI polls, and the intent contract fixes the send-after-guard rule.
  - Re-escalating an unlinked PendingCoSign Job is still 200: allowed by story 1's decision.
  - No unit test for `runner_err`'s 500 fallback; the double clock read: negligible.
  - Six false findings: the double-start test does reproduce the race deterministically, the story file results are written here, and the four intent notes were recorded decisions or unreachable cases.

**Follow-up review recommended: false.** This pass patched 0 high, 1 medium and 2 low entries.

**Verification**
- `cargo test --workspace --exclude wipe-hub`: exit 0, 173 passed. With `wipe-hub` included (compose up), 722 tests passed. One `wipe-hub` test failed per run and the failing test differed between runs: `silent_smtp_and_database_time_out_within_bound`, then `a_bad_signature_is_400_and_stores_nothing`. The first fails identically at the baseline (stashed tree), and `wipe-hub` does not use the crates' changed code paths. These are environment timing flakes, not this change.
- The two new right-after-start tests fail with the `run_job` check removed (implementer-confirmed).
- `cargo clippy -p wipe-common -p wipe-engine -p wipe-server -p wipe-engine-mock --all-targets`: exit 0; only pre-existing warnings, all in untouched code.
- rustfmt `--check` is clean on the four already-formatted touched files. `runner.rs` keeps the same 6 diff hunks it had before.

**Residual risks**
- The two deferred medium races (co-sign concurrency; abort during probing).
- The `wipe-hub` timing tests are flaky on this machine.
- AGENTS.md test counts are stale (owner-maintained).
