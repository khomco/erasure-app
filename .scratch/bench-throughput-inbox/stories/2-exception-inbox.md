---
title: 'Exception inbox'
type: 'feature'
created: '2026-11-09'
status: 'done'
baseline_revision: '7349259af247c9caf8cd1eba19577d34136cca98'
review_loop_iteration: 1
followup_review_recommended: false
context: []
warnings: [oversized]
deferred:
  - summary: >-
      The Inbox nav item's count pill and "Inbox, N waiting" label are not covered by any rendered test.
    evidence: |-
      NavBar needs useRouterState and a query client; navInboxCount (the rule) is tested, the wiring is not. Closing it needs a presentational NavItem extraction.
    location: >-
      apps/desktop/src/components/Layout.tsx (NavBar)
    severity: low
  - summary: >-
      The bay map and Devices cards draw a Job whose verification failed as wiping forever.
    evidence: |-
      deriveSlotStatus checks only the erasure state; the runner marks the erasure Completed before verification and leaves the Job InProgress on VerificationFailed (crates/wipe-engine/src/runner.rs ~L760). The Inbox now lists it; the bay map does not.
    location: >-
      apps/desktop/src/bench/slotStatus.ts:39
    severity: medium
  - summary: >-
      The runner never records why an erasure failed, so every failed-erasure Inbox row reads "no reason recorded".
    evidence: |-
      BackendProgress::Failed's reason goes only into WipeError::Backend; no JobUpdateKind::Failed is ever emitted (grep finds none in crates/).
    location: >-
      crates/wipe-engine/src/runner.rs:711
    severity: low
  - summary: >-
      When the erasure task stops on an early runner error, the Job stays InProgress with no failed erasure, so neither the Inbox nor the bay map shows it needs a human.
    evidence: |-
      run_erasure_event returns via `?` from enumerate, device lookup, capabilities, method resolution (before the ErasureEvent is appended), capabilities re-read, unfreeze and issue (after it, leaving it probing/unfreezing/running), and from the verify call (completed erasure, no Verification). Verified in crates/wipe-engine/src/runner.rs ~L586-680 and ~L744. Fix is server-side: mark the erasure Failed with a reason on every early exit.
    location: >-
      crates/wipe-engine/src/runner.rs:586
    severity: medium
  - summary: >-
      A Job left Queued (created, start failed or never sent) waits on a human but is not in the Inbox.
    evidence: |-
      Story 1's batch start can leave "created, not started" Jobs Queued; listing Queued Jobs needs an age threshold so the normal create-then-start does not flash an entry, which widens this story's scope.
    location: >-
      apps/desktop/src/pages/inbox.ts (buildInbox)
    severity: medium
  - summary: >-
      No test renders a destruction run card to check it carries id runAnchorId(run.id).
    evidence: |-
      RunCard is private to ManifestsPage; the shared helper is tested, the card side is not. Needs a presentational extraction like the deferred NavItem.
    location: >-
      apps/desktop/src/pages/ManifestsPage.tsx (RunCard)
    severity: low
---

<intent-contract>

## Intent

**Problem:** Things waiting on an operator are scattered: a failed erasure shows only as an amber card or tray on the Devices page, drives recorded for destruction wait unseen on the Destruction runs page, and runs awaiting completion or concurrence sit on that same page. On a busy bench a missed failure means a drive leaves undocumented (ROADMAP O-R3, O2.1).

**Approach:** Add an **Inbox** page (`/inbox`) to the desktop UI that lists every item waiting on a human, grouped by kind, each linking to where it is resolved, and an "Inbox" nav item showing the count. The list is computed client-side from the existing `GET /api/jobs` and `GET /api/manifests` by a pure TypeScript module covered by vitest. No server change.

## Boundaries & Constraints

**Always:**
- Inbox entries, in this group order (each group oldest first by `created_at`, ties by id):
  1. **Failed erasure** — a Job in `queued`/`in_progress` whose latest ErasureEvent is `failed` (the same rule as `deriveSlotStatus`'s `failed`; reuse it, do not re-implement), **or** whose latest ErasureEvent is `completed` and the latest Verification activity naming that erasure (`erasure_event_id`) has `report.all_passed === false` (verification failed; see Autonomous decisions). Links to `/jobs/$jobId` (Abort / Record for destruction live there).
  2. **Quarantined** — a Job in `quarantined`. Links to `/jobs/$jobId`.
  3. **Pending destruction, not on a run** — a `pending_destruction` Job not on any destruction run that itself appears in the Inbox (see Autonomous decisions). Links to `/manifests` (where runs are assembled).
  4. **Destruction run awaiting completion** — a run in `pending`; **awaiting concurrence** — a run in `awaiting_concurrence`. Shows `runStatusLabel` and the member drives (`runDriveLabels`). Links to `/manifests` with hash `run-<id>`; `RunCard` gets `id="run-<id>"` so the link lands on it.
- The nav count equals the number of Inbox entries (a run counts once); it is hidden when the count is 0 or either list has not loaded.
- Each Job entry names the drive by asset tag, else the latest erasure's device snapshot (vendor model · serial), else `spec.device_id`; an erasure-failed entry shows the reason from the latest erasure's last `events[]` entry with `event.kind === "failed"`, or "no reason recorded" when none; a verification-failed entry shows "Verification failed: N of M samples failed" from its report.
- Inbox data uses the shared `["jobs"]` and `["manifests"]` query keys (`api.jobs`, `api.manifests`) so it dedupes with the Jobs and Destruction runs pages; both polled (2 s) by the nav and the page.
- Terms from CONTEXT §14: Job, ErasureEvent, pending destruction (never "destroyed"/"awaiting approval"), destruction run, concurrence.

**Never:**
- No new server route, Rust change, or mutating action from the Inbox (it only links); no failure routing (D10 / O2.2), RBAC, ETA, dismiss/snooze state, or notification sound.
- Never list `erased`, `destroyed`, `aborted` Jobs, or `completed`/`rejected` runs.
- No hardware-sanitization claim in new copy (ROADMAP §0.1).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Nothing waiting | All Jobs erased/destroyed/aborted/running cleanly; runs completed | Page shows an empty state ("Nothing needs you"); nav shows no count | — |
| Failed erasure | `in_progress` Job, latest erasure `failed` | One "Failed erasure" entry → `/jobs/<id>` | — |
| Verification failed | `in_progress` Job, latest erasure `completed`, its Verification `all_passed: false` (2 of 8 samples failed) | One "Failed erasure" entry reading "Verification failed: 2 of 8 samples failed" → `/jobs/<id>` | — |
| Verification passed or pending | latest erasure `completed`, its Verification `all_passed: true`, or no Verification yet | Not listed | — |
| Earlier failure then success | Job whose older erasure failed but latest completed | Not listed | — |
| Recorded, unassembled | `pending_destruction`, `manifest_id` null, on no run | One "pending destruction" entry → `/manifests` | — |
| Recorded, on a pending run | 3 Jobs on one `pending` run | One run entry listing 3 drives; Jobs not listed separately; count 1 | — |
| Run awaiting concurrence | run `awaiting_concurrence` | One run entry, label "Awaiting review"/"Awaiting witness" | — |
| Orphan pending Job | `pending_destruction` with a `manifest_id` whose run is missing, completed or rejected | Listed as a pending-destruction entry (never hidden) | — |
| Mixed | 2 failed, 1 quarantined, 1 unassembled, 1 pending run | Groups in that order; count 5 | — |
| Load error | `/api/jobs` or `/api/manifests` fails | Page shows the error; nav count hidden | Error text shown verbatim |

</intent-contract>

## Code Map

- `apps/desktop/src/bench/slotStatus.ts` -- `deriveSlotStatus(job)` returns `{kind:"failed", erasure}` for queued/in_progress with latest erasure failed; reuse for group 1. `ATTENTION_KINDS` (DevicesPage summary strip) unchanged.
- `apps/desktop/src/api/ws.ts:151` -- `latestErasure(job)`; `ErasureEvent.device_snapshot`, `events: JobUpdate[]` whose `event` may be `{kind:"failed", reason}` (types.ts ~L205-226); `ErasureEventStateWire` itself carries no reason.
- `apps/desktop/src/api/destructionRuns.ts:22,64` -- `runStatusLabel(run)`, `runDriveLabels(jobIds, jobs)`: reuse for run entries.
- `apps/desktop/src/pages/manifestAssembly.ts` -- `assemblableJobs` pattern and test style for the new filter module; a run is live unless `rejected`.
- `apps/desktop/src/pages/ManifestsPage.tsx:231` -- `RunCard` root `<div>`: add `id={\`run-${run.id}\`}`. Queries use `["jobs"]`/`["manifests"]`, 2000 ms.
- `apps/desktop/src/components/Layout.tsx` -- `NavBar` `Item` (to/icon/label); add Inbox item with a count pill. Layout is the root route component, so its queries run on every page.
- `apps/desktop/src/main.tsx` -- route table; add `/inbox`.
- `apps/desktop/src/bench/BayMap.test.ts` -- static-render test style (`renderToStaticMarkup`, node env; no DOM library).
- `crates/wipe-engine/src/runner.rs:519` -- read-only evidence: a failed erasure leaves the Job `InProgress` awaiting abort or record-for-destruction; `Quarantined` is never entered today (CONTEXT §5 JobState).

## Tasks & Acceptance

**Execution:**
- `apps/desktop/src/api/destructionRuns.ts` -- add `runAnchorId(runId: string): string` (returns `run-<id>`), the single source for the run anchor used by `RunCard` and `inboxTarget`; test it in `destructionRuns.test.ts` or `inbox.test.ts` (run entry's href ends in `#${runAnchorId(id)}`).
- `apps/desktop/src/pages/inbox.ts` -- new pure module: `InboxEntry` union (`failed_erasure{job, erasure, reason}` — `reason` computed in `buildInbox` for both erasure- and verification-failed cases, `quarantined{job}`, `pending_destruction{job}`, `run{run, drives}`), `buildInbox(jobs, manifests): InboxEntry[]` in the group/sort order above, `inboxCount(jobs?, manifests?): number | null` (null until both loaded), `navInboxCount(jobs: InboxSource<Job>, manifests: InboxSource<DestructionManifest>)` where `InboxSource<T> = {data: readonly T[] | undefined; error: unknown}` (null while loading, while either errors even with cached data, and at 0), `jobDriveLabel(job)`, `inboxTarget(entry): {to: string; hash?: string}` and `inboxHref(entry)`.
- `apps/desktop/src/pages/inbox.test.ts` -- vitest for every I/O-matrix row, group ordering and oldest-first sort, run counted once, rejected/completed runs excluded, orphan pending Job listed, a pending Job covered only through its `manifest_id` (its id absent from the waiting run's `job_ids`) not listed, `inboxCount` null when either list is undefined, `navInboxCount` hidden at 0, while loading and on either error.
- `apps/desktop/src/pages/InboxPage.tsx` -- page: heading, grouped sections with counts, one row per entry with drive label, reason/status, and a link to its resolver; loading, error and empty states. Export a presentational `InboxView({jobs, manifests, RowLink?})` (given `InboxSource` states: error verbatim, loading, or the list) and `InboxList({entries, RowLink?})`. The in-app row link renders TanStack `<Link to={target.to} hash={target.hash}>` with `target = inboxTarget(entry)` — no per-kind route mapping of its own — so `inboxTarget` is the only place destinations are decided; a test-only `HrefRowLink` renders `<a href={inboxHref(entry)}>`.
- `apps/desktop/src/pages/InboxPage.test.ts` -- `renderToStaticMarkup` with `HrefRowLink` (TanStack `Link` throws outside a router): rows link to `/jobs/<id>`, `/manifests` and `/manifests#run-<id>`; group titles with counts in order; a verification-failed row shows its sample text; a run awaiting a witness shows "Awaiting witness"; the empty state; `InboxView` with a failing list shows the error verbatim and no rows.
- `apps/desktop/src/components/Layout.tsx` -- Inbox nav item (first after Devices) with an amber count pill from `navInboxCount`; `aria-label` "Inbox, N waiting". The shared queries hook (`useInboxQueries`, 2 s) lives in `InboxPage.tsx`.
- `apps/desktop/src/main.tsx` -- `/inbox` route.
- `apps/desktop/src/pages/ManifestsPage.tsx` -- `id={runAnchorId(run.id)}` on `RunCard`.
- `README.md` (pages table + frontend test row/count), `CONTEXT.md` §8 (new "Exception inbox" paragraph), §10.2 row, §14 glossary entry **Inbox**, `docs/ROADMAP.md` O2.1 struck "Shipped 2026-11-09 (bench-throughput-inbox story 2)", `AGENTS.md` frontend test count. -- docs ship with code.

**Acceptance Criteria:**
- Given a station with one failed erasure, one Job recorded for destruction and not assembled, and one destruction run pending completion, when the operator opens any page, then the nav shows "Inbox" with 3, and `/inbox` lists those three entries, each linking to `/jobs/<id>`, `/manifests` and `/manifests#run-<id>` respectively.
- Given the operator resolves an item (aborts the failed Job), when the next poll returns, then the entry disappears and the count drops.
- Given nothing waits on a human, when the operator opens `/inbox`, then an empty state shows and the nav shows no count.

## Spec Change Log

### 2026-11-09 — loop 1 (review pass 1)
- **Trigger:** intent-alignment finding — Jobs whose verification failed are outside the Inbox; verified at `runner.rs` ~L726-766 (erasure marked `Completed` before verification; a failed report returns `VerificationFailed` with the Job left `InProgress`). Also review findings that `RouterRowLink` mapped routes itself while tests pinned `inboxHref` (blind, verification-gap, intent-alignment), that the `RunCard` anchor and the Inbox hash were two literals (verification-gap), and that the `manifest_id`-only coverage path was untested (blind).
- **Amended:** intent-contract rule 1, reason rule and two matrix rows (Autonomous decision "A failed verification counts as a failed erasure"); tasks for `destructionRuns.ts` (`runAnchorId`), `inbox.ts` (entry `reason`, `navInboxCount`, `inboxTarget`), `inbox.test.ts`, `InboxPage.tsx` (`InboxView`; in-app link driven only by `inboxTarget`), `InboxPage.test.ts`, `Layout.tsx`, `ManifestsPage.tsx`; removed attempt-1 Design Notes that described the old link split.
- **Known-bad state avoided:** a drive with a failed verification never appearing anywhere as needing a human; app links and tested links drifting apart; the run anchor renamed on one side only.
- **KEEP (attempt 1 worked; diff at `/tmp/inbox-story2-attempt1.patch` if present):** group order and oldest-first `byAge` sort; run-counted-once dedup with orphan pending Jobs listed; `deriveSlotStatus`, `runStatusLabel`, `runDriveLabels`, `latestErasure` reuse; `INBOX_KINDS`, group titles and hints, icons, "Nothing needs you." empty state; `useInboxQueries` with `INBOX_POLL_MS = 2000` on `["jobs"]`/`["manifests"]`; `navInboxCount` + `InboxView` (pure, tested for the Load-error row); nav item after Devices with amber pill and aria-label; `/inbox` route; docs text in CONTEXT §8/§10.2/§14, README pages row and test row, ROADMAP O2.1 strike, AGENTS count (update counts and add the verification-failed case to the wording).

## Review Triage Log

### 2026-11-09 — Review pass
- verdicts: 29 findings — high 0, medium 4, low 16, false 9, maybe-false 0 (tally corrected in pass 2)
- findings:
  - `low` `reject` [blind] An orphan pending-destruction Job (run missing/completed/rejected) links to Destruction runs, where it cannot be assembled — not met in practice: runs and Jobs share station RAM, a run moves all its members to `Destroyed` together, nothing sets `rejected`; only a one-poll lag reaches it, and fixing needs a new branch and copy.
  - `false` `reject` [blind] Quarantined Jobs pin the nav count forever — nothing ever enters `Quarantined` (CONTEXT §5 JobState), so no badge can stick.
  - `low` `reject` [blind] A failed load hides the nav count, looking like "nothing waiting" — the header's live/offline pill already shows the station unreachable and the Inbox page shows the error; hiding is the spec's Load-error row.
  - `medium` `bad_spec` [blind] `RouterRowLink` maps routes itself; tests pin `inboxHref`, not the app's links — amended: in-app link takes `to` and `hash` only from `inboxTarget`.
  - `low` `reject` [blind] Layout imports a hook from a page module — no import cycle exists (InboxPage imports nothing from `components/`); moving it is churn with no named breakage.
  - `low` `reject` [blind] Nav polls Jobs and runs every 2 s on every page — localhost station API; the Devices page already polls the same list every 1.5 s; queries dedupe by key.
  - `low` `reject` [blind] Run link lands without a highlight, and scroll needs a warm cache — the nav keeps `["manifests"]` warm on every page, so the card is rendered at navigation; a highlight is cosmetic.
  - `low` `bad_spec` [blind] The `manifest_id`-only coverage path is untested — amended: test task added.
  - `false` `reject` [blind] `byAge` is inconsistent for unparseable `created_at` — the server always serializes RFC 3339 (`time::serde::rfc3339` on `Job.created_at`/`DestructionManifest.created_at`).
  - `low` `defer` [blind] Nav item (pill, aria-label) is never rendered in a test — same root as verification-gap row 3; deferred.
  - `low` `reject` [blind] Jobs poll can show a new `manifest_id` before the runs poll, flickering an orphan entry — assembly invalidates both queries together; at most one transient poll.
  - `low` `reject` [blind] `onWaitingRun` mixes run ids and Job ids — UUID v4 ids do not collide; readability only.
  - `false` `reject` [blind] Story file "not ready": empty triage log, failures sorted by Job creation not failure time — the triage log is written by this step; oldest-by-`created_at` is the specified order.
  - `low` `reject` [edge] Orphan pending Job routed to a page that never offers it — duplicate of blind row 1.
  - `low` `reject` [edge] Independent polls flicker an entry after assembly — duplicate of blind row 11.
  - `false` `reject` [edge] NaN `created_at` breaks the comparator — duplicate of blind row 9.
  - `false` `reject` [edge] A run with empty `job_ids` renders "0 drives:" — `create_manifest` refuses an empty list (`crates/wipe-server/src/handlers.rs:609`).
  - `false` `reject` [edge] `job.spec` undefined crashes `jobDriveLabel` — `spec` is a required field of every Job on the wire (`JobSpec` in `wipe-common` job.rs); the optional chain is merely redundant.
  - `low` `reject` [edge] Claim "linking to where it is resolved" fails for orphan pending Jobs — duplicate of blind row 1.
  - `medium` `bad_spec` [verification-gap] In-app row links (`RouterRowLink`) are never tested; the test swaps in `HrefRowLink` — grouped with blind row 4; amended so both render from `inboxTarget`.
  - `low` `bad_spec` [verification-gap] `RunCard` id and the Inbox hash are two literals, untested — amended: shared `runAnchorId` with a test.
  - `low` `defer` [verification-gap] Nav pill and "Inbox, N waiting" label untested — needs a presentational `NavItem` extraction; `navInboxCount` is tested; deferred as filed.
  - `false` `reject` [intent-alignment] Pending-destruction Jobs on a waiting run are not listed separately (count 1 for an 8-drive run) — the recorded autonomous decision, not a defect.
  - `false` `reject` [intent-alignment] Aborted Jobs with a failed latest erasure are excluded — aborting is the human resolution.
  - `low` `reject` [intent-alignment] Quarantined Jobs link to a page with no resolving action — recorded limitation (CONTEXT §10.2); unreachable today.
  - `medium` `bad_spec` [intent-alignment] Failed verifications are outside the Inbox — verified: the runner marks the erasure `Completed` before verifying and leaves the Job `InProgress` on `VerificationFailed` (`runner.rs` ~L726-766), so the drive is invisible; amended via Autonomous decision (list it as a failed erasure).
  - `medium` `bad_spec` [intent-alignment] Tests pin a test-only link path, not the app's router links — duplicate of verification-gap row 1.
  - `low` `defer` [intent-alignment] Nav item, route registration and query hook are untested — same root as verification-gap row 3.
  - `false` `reject` [intent-alignment] `pnpm build` and the manual check show no evidence of running — `pnpm build` ran clean in step 3; the manual check is optional when CLI checks exist and is recorded as residual risk.

### 2026-11-09 — Review pass (loop 1)
- verdicts: 33 findings — high 0, medium 6, low 20, false 7, maybe-false 0
- findings:
  - `low` `patch` [blind] Pass-1 triage tallies were wrong (4 medium, 16 low, 9 false, not 2/15/12) — corrected the tally line.
  - `low` `patch` [blind] `.scratch/STATUS.md` O row still lists O1.4/O2.1 as unbuilt — row updated.
  - `low` `reject` [blind] SPEC.md CAP-2 still says "PendingCoSign" and omits failed verifications — the epic SPEC is the owner's contract; the widening is a recorded autonomous decision in this story, and CONTEXT §14 maps the old name.
  - `low` `reject` [blind] `verificationReason` uses `sample_count` not `samples.length`; all-passed-false with zero failing samples reads "0 of N" — the only backend sets `sample_count = samples.len()` (`wipe-engine-mock` lib.rs:319, per verification-gap); a fallback adds a branch for an unreachable report.
  - `medium` `defer` [blind] Verify-call errors leave a Job invisible, the deferred severity "medium (unverified)" is malformed, and CONTEXT omits it — grouped with edge rows 1, 4, 5: verified pre-existing runner early exits; deferred item broadened and severity fixed; CONTEXT §10.2 clause added (patch).
  - `medium` `defer` [blind] Bay map draws a failed-verification Job as wiping while the Inbox lists it — carried: already deferred (medium); moving the rule into `deriveSlotStatus` changes Devices copy ("Erasure attempt failed") and story 1's surface.
  - `low` `reject` [blind] One failed poll swaps the cached list for the error — localhost station API; when it fails the header's offline pill shows the station is down; the spec's Load-error row asks for the error.
  - `low` `reject` [blind] Test fixtures use `as unknown as Job` and a `failed` reason path the runner never emits — the existing test style (manifestAssembly, batchSelection); the `failed` event path is the wire contract, and "no reason recorded" is unit-tested.
  - `low` `reject` [blind] Loading state, quarantined row and the pending-group title are not rendered in tests — trivial branches over tested data; the matrix rows are covered.
  - `low` `defer` [blind] `RouterRowLink`/hash scroll never rendered; manual check unrecorded — grouped with verification-gap row 1 (patch attempted, see that row); manual check recorded as residual risk.
  - `low` `reject` [blind] "Assemble these into a destruction run" is wrong for orphan pending Jobs — carried from pass 1 (orphans unreachable in practice).
  - `low` `patch` [blind] `jobDriveLabel` uses `job.spec?.` then `job.spec.` — redundant optional chain removed.
  - `low` `reject` [blind] Run group title vs pill wording, untruncated drive list, Inbox icon reused — cosmetic; the title names both states the pill distinguishes, and the list wraps.
  - `low` `patch` [blind] CONTEXT §14 Inbox entry wraps at ~100 columns — re-wrapped to ~72.
  - `medium` `defer` [edge] Runner early `?` exits leave a Job InProgress with no or a non-terminal erasure, invisible — verified at `runner.rs` ~L586-680; pre-existing server bug; deferred (same group as blind row 5).
  - `low` `reject` [edge] Verification "N of M" can disagree with the runner / read "0 of N" — duplicate of blind row 4.
  - `false` `reject` [edge] A Job whose `manifest_id` names a waiting run that omits it is hidden — `link_to_manifest` sets `manifest_id` only on the run's own `job_ids` (`runner.rs` ~L418-450), so the state cannot arise.
  - `medium` `defer` [edge] Claim "lists every item waiting on a human" fails for stalled runner exits — same group as blind row 5.
  - `low` `patch` [edge] Deferred list understates the invisible-Job cause (verify only) — deferred item broadened to every early exit.
  - `low` `patch` [verification-gap] In-app `RouterRowLink` never rendered; dropping `hash` would pass — patched: memory-router static render of the default link asserts all three hrefs, and fails with `hash` removed.
  - `low` `defer` [verification-gap] `RunCard` id never rendered in a test — deferred as filed (needs extraction).
  - `false` `reject` [intent-alignment] Pending-destruction Jobs folded into their run (reading A) — carried from pass 1: recorded autonomous decision.
  - `false` `reject` [intent-alignment] Count is actions, not drives (reading C) — same recorded decision; the run row lists its drives.
  - `false` `reject` [intent-alignment] Failed verifications listed beyond SPEC's letter — the recorded autonomous decision (safe default).
  - `medium` `defer` [intent-alignment] Verify-errored Jobs not listed — same group as blind row 5.
  - `medium` `defer` [intent-alignment] Queued Jobs never started are not listed — real (story 1 can leave them); needs an age threshold to avoid flashing during create-then-start, widening scope; deferred.
  - `low` `reject` [intent-alignment] Quarantined rows link where no action resolves them — carried: recorded limitation, unreachable today.
  - `low` `reject` [intent-alignment] Orphan pending rows link where they are not offered — carried from pass 1.
  - `low` `defer` [intent-alignment] Nav count never rendered — carried: already deferred.
  - `false` `reject` [intent-alignment] `/inbox` route registration untested — `pnpm build` and typecheck compile the route tree; a missing route is a build-visible omission, not silent.
  - `low` `patch` [intent-alignment] Real router links and hash never rendered — duplicate of verification-gap row 1.
  - `false` `reject` [intent-alignment] Run-card scroll untested — scrolling is TanStack's documented `hashScrollIntoView` default; the anchor contract is the shared `runAnchorId`.
  - `false` `reject` [intent-alignment] App never run; entry disappearance only tested at `buildInbox` level — the Inbox is a pure function of the polled lists; the manual check is optional and listed as residual risk.

## Design Notes

The inbox is derived, not stored: it is exactly what the server lists right now, so resolving an item anywhere removes it. Grouping a run's drives under the run keeps the count equal to "actions a human must take": one completion covers every drive on it.

## Autonomous decisions

- **Pending-destruction Jobs already on a run.** SPEC lists "PendingCoSign Jobs" (the pre-ADR-0011 name for pending destruction) and "pending manifests" separately; listing both would count each drive on a pending run twice (eight drives + the run = nine "things"). Options: (a) list every pending-destruction Job and every run; (b) list a pending-destruction Job only when it is not on a run that is itself in the Inbox. Chosen: (b) — the count matches actions a human must take, and nothing is hidden: a Job on a missing, completed or rejected run still shows on its own.
- **Quarantined Jobs.** Nothing enters `Quarantined` today, and it is terminal. Listed anyway as the SPEC requires, linking to the Job; no "resolve" action exists, which CONTEXT §10.2 records as a limitation.
- **A failed verification counts as a failed erasure.** When an erasure completes but its sampled verification fails, the runner leaves the Job `InProgress` with a *completed* ErasureEvent (`crates/wipe-engine/src/runner.rs` ~L760, `VerificationFailed`), so the "latest erasure failed" rule never lists it and the drive waits on a human invisibly — the exact loss this story exists to prevent. Options: (a) list only erasure-state failures, as SPEC words it; (b) also list a Job whose latest completed erasure has a failed Verification. Chosen: (b) — the safe default, most protective of the audit trail. It is grouped as a failed erasure (same resolution: abort or record for destruction on the Job page). The bay map's `deriveSlotStatus` is left unchanged (still draws it as wiping); that pre-existing mismatch is deferred.
- **Where a failed erasure is resolved.** The Job page (Abort, Record for destruction) — the runner leaves such a Job `InProgress` for exactly those actions.

## Verification

**Commands:**
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: clean typecheck; all vitest pass including `inbox.test.ts` and `InboxPage.test.ts`.
- `cd apps/desktop && pnpm build` -- expected: success.

**Manual checks (if no CLI):**
- `cargo run -p wipe-cli -- serve --static-dir apps/desktop/dist` (mock backend) after `pnpm build`; produce a failed erasure if the mock allows, record a drive for destruction, assemble a run; check `/inbox` entries, links and nav count.

## Auto Run Result

Status: done

**Summary.** A new **Inbox** page (`/inbox`) in the desktop UI lists every item waiting on a human, in four groups, each oldest first: failed erasures (a `Queued`/`InProgress` Job whose latest ErasureEvent failed, or whose latest erasure completed but its sampled verification failed, with "Verification failed: N of M samples failed"), quarantined Jobs, Jobs pending destruction on no waiting destruction run, and destruction runs pending completion or awaiting concurrence (counted once, listing their drives). Each row links to where it is resolved: the Job page, the Destruction runs page, or that run's card (`/manifests#run-<id>`). An "Inbox" nav item after Devices shows the count, hidden at zero, while loading and on a failed load. Everything is derived in the browser from `GET /api/jobs` and `GET /api/manifests` (polled every 2 s on shared query keys); no server or Rust change, and no action is taken from the Inbox.

**Files changed.**
- `apps/desktop/src/pages/inbox.ts` — new pure module: `buildInbox`, `inboxCount`, `navInboxCount`, `inboxTarget`/`inboxHref`, drive labels and reasons.
- `apps/desktop/src/pages/inbox.test.ts` — vitest over every I/O-matrix row, ordering, dedup, targets, nav-count rules.
- `apps/desktop/src/pages/InboxPage.tsx` — the page, `InboxView`/`InboxList`, the router link driven only by `inboxTarget`, `useInboxQueries`.
- `apps/desktop/src/pages/InboxPage.test.ts` — static renders: rows, groups, empty and error states, and the real router links under a memory router.
- `apps/desktop/src/api/destructionRuns.ts` — `runAnchorId`, the one spelling of the run anchor.
- `apps/desktop/src/pages/ManifestsPage.tsx` — run cards carry `id={runAnchorId(run.id)}`.
- `apps/desktop/src/components/Layout.tsx`, `apps/desktop/src/main.tsx` — nav item with count pill; `/inbox` route.
- `CONTEXT.md` (§8, §10.2 row with limitations, §14 **Inbox**), `README.md`, `docs/ROADMAP.md` (O2.1 struck), `AGENTS.md` (351 tests), `.scratch/STATUS.md` (O row).

**Review.** Two passes (one bad_spec loop).
- Pass 1, 29 findings: 5 entries looped to the spec (failed verifications invisible — autonomous decision to list them; router links mapped separately from the tested target; run anchor as two literals; `manifest_id`-only path untested), 2 deferred, the rest rejected or false with reasons in the triage log.
- Pass 2, 33 findings: 6 patches applied (pass-1 tally corrected, STATUS row, redundant optional chain, glossary wrap, CONTEXT clause and broadened deferral for stalled runner exits, memory-router test of the real links), 6 deferred (runner early exits leave Jobs invisible; Queued Jobs never started; bay map draws failed verification as wiping; nav pill and run card id not rendered in tests), the rest rejected with reasons.

**Follow-up review recommended:** false — this pass patched no high and no medium entry (patched by verdict: high 0, medium 0, low 6).

**Verification.** `pnpm typecheck` clean; `pnpm test` 351/351 passed (15 files); `pnpm build` succeeded. The router-link test was shown to fail with `hash` removed.

**Residual risks.**
- Nobody has opened the Inbox in a browser against a running station.
- Two medium gaps are deferred and sit outside this story's client-only scope. A Job whose erasure task stopped on an early runner error, and a Job left `Queued` and never started, are still not listed.
- The runner records no failure reason, so erasure-failed rows read "no reason recorded".

