---
title: 'Grouped Jobs list'
type: 'feature'
created: '2026-11-09'
status: 'done'
baseline_revision: '9a223dcb2f63eaaa6fb68d871c136ac69a037246'
review_loop_iteration: 0
followup_review_recommended: false
context: []
warnings: [oversized]
deferred:
  - summary: >-
      The Jobs page's "Pulse the list whenever a job event comes in" handler is a no-op, so section moves wait for the 1.5 s poll.
    evidence: |-
      `useEventStream(() => {})` only re-renders with the same cached data; nothing invalidates `["jobs"]` (apps/desktop/src/api/ws.ts:47-80). Pre-existing, unchanged by story 4.
    location: >-
      apps/desktop/src/pages/JobsPage.tsx (JobsPage)
    severity: low
---

<intent-contract>

## Intent

**Problem:** The Jobs page is one flat list in whatever order the server's hash map returns (`JobRunner::list` iterates a `HashMap`; the page just reverses it), so an operator cannot see at a glance how many drives are running, how many are waiting to start and which have finished (ROADMAP O1.5, in part). A failed `GET /api/jobs` also reads as "No jobs yet".

**Approach:** Group the Jobs page into three sections — **Active**, **Queued**, **Finished** — each headed with its count, computed by a pure TypeScript module from the existing `GET /api/jobs` (shared `["jobs"]` query key). No server change, no ETA.

## Boundaries & Constraints

**Always:**
- Grouping by outer JobState only (`job.state.state`):
  - **Active** — `in_progress` and `pending_destruction` (non-terminal, already past the queue; see Autonomous decisions).
  - **Queued** — `queued`.
  - **Finished** — the terminal states `erased`, `destroyed`, `quarantined`, `aborted` (CONTEXT §5 JobState; the same four the page's `isOuterTerminal` already names).
- Section order Active, Queued, Finished. Active and Queued oldest first by `created_at` (the order they will be worked); Finished newest first by `ended_at`, else `created_at`. Ties by id. Unparseable timestamps fall back to id order, never throw.
- Every section always shows its heading with its count ("Active · 3"); an empty section shows one muted "None" line. The whole-page "No jobs yet. Start one from the Devices page." stays for an empty list.
- Rows keep today's content and link (`/jobs/$jobId`): state icon, drive (snapshot vendor model or device id), serial · capacity · classification/intent, progress bar with percent and stage for non-terminal Jobs, state pill. A Job with a failed latest erasure stays in Active (its bar is already red); the Inbox owns calling it out.
- Load error: the error message shown verbatim instead of the list (never "No jobs yet"); loading: "Loading jobs…".
- Query stays `["jobs"]`, `api.jobs`, 1500 ms poll, `useEventStream` pulse kept.
- Terms from CONTEXT §14: Job, ErasureEvent, pending destruction (never "destroyed" for that state).

**Never:**
- No ETA, throughput figure or time-remaining text (SPEC non-goal); `progress.eta_seconds` is never rendered.
- No Rust, route, request-shape or `api/types.ts` change; no new mutating action on the Jobs page.
- No collapsing/filter/pagination state, no RBAC, no failure routing.
- No hardware-sanitization claim in any new copy (ROADMAP §0.1).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Mixed bench | 2 `in_progress`, 1 `pending_destruction`, 3 `queued`, 1 each `erased`/`destroyed`/`quarantined`/`aborted` | Active · 3, Queued · 3, Finished · 4, in that order | — |
| Ordering | Two queued Jobs created 10:00 and 10:05; two erased Jobs ended 11:00 and 11:30 | Queued: 10:00 then 10:05; Finished: 11:30 then 11:00 | — |
| Finished, no `ended_at` | terminal Job with `ended_at: null` | Sorted by its `created_at` among Finished | — |
| Tie / bad timestamp | equal or unparseable timestamps | Ordered by id, no exception | — |
| Failed erasure | `in_progress` Job, latest erasure `failed` | In Active | — |
| Empty group | Only finished Jobs | Active · 0 and Queued · 0 each show "None"; Finished lists them | — |
| No Jobs | `[]` | "No jobs yet…" empty state, no sections | — |
| Load error | `api.jobs` rejects | Error text verbatim; no sections, no "No jobs yet" | Error shown verbatim |

</intent-contract>

## Code Map

- `apps/desktop/src/pages/JobsPage.tsx` -- whole page (105 lines): `StateIcon`, `JobsPage` (query, `.slice().reverse()`, empty state; no error state), `JobRow` (TanStack `Link to="/jobs/$jobId"`, progress bar, `isOuterTerminal`). Restructure; keep `StateIcon`/row content.
- `crates/wipe-engine/src/runner.rs:77,101` -- read-only evidence: `jobs: HashMap<Uuid, Job>`, `list()` returns `values()`, so server order is arbitrary; sort client-side.
- `apps/desktop/src/api/types.ts:106,423` -- `JobStateLabel` (7 states), `Job { created_at, started_at, ended_at }`.
- `apps/desktop/src/pages/inbox.ts` -- `byAge` comparator (created_at, NaN-safe, ties by id) and the `InboxSource<T> = {data, error}` type: reuse `InboxSource` for the view's input; write a sibling comparator rather than exporting/altering Inbox internals unless trivially shareable.
- `apps/desktop/src/pages/InboxPage.tsx` / `InboxPage.test.ts` -- precedent for a presentational `View`/`List` export with an injectable `RowLink` and a test-only `HrefRowLink` (`<a href>`), rendered with `renderToStaticMarkup` in the node env (TanStack `Link` throws outside a router).
- `apps/desktop/src/api/ws.ts:151` -- `latestErasure(job)`, used by the row.

## Tasks & Acceptance

**Execution:**
- `apps/desktop/src/pages/jobGroups.ts` -- new pure module: `JobGroupKind = "active" | "queued" | "finished"`, `JOB_GROUP_KINDS` (that order), `jobGroupOf(state: JobStateLabel): JobGroupKind` (exhaustive switch), `groupJobs(jobs: readonly Job[]): {kind, jobs: Job[]}[]` (all three groups, always, sorted per the rules above) -- the grouping rule in one tested place.
- `apps/desktop/src/pages/jobGroups.test.ts` -- vitest over every I/O-matrix row the module owns: each of the 7 states to its group, group order, Active/Queued oldest first, Finished newest first by `ended_at` with `created_at` fallback, ties and unparseable timestamps by id, empty groups present with no jobs, input not mutated.
- `apps/desktop/src/pages/JobsPage.tsx` -- `JobsPage` keeps the query and stream; exports presentational `JobsView({jobs: InboxSource<Job>, RowLink?})` (error set → error verbatim; else `data` undefined → loading; else empty state or sections) and `JobsList({jobs, RowLink?})` rendering one `<section>` per group with an `<h3>` "Active · N" etc. and "None" when empty; default `RowLink` is TanStack `Link to="/jobs/$jobId"`, plus exported test-only `HrefJobLink` rendering `<a href="/jobs/<id>">`. Never render `eta_seconds`.
- `apps/desktop/src/pages/JobsPage.test.ts` -- `renderToStaticMarkup` with `HrefJobLink`: headings in order with counts, rows under the right heading linking to `/jobs/<id>`, "None" for an empty group, the "No jobs yet" state, a failing source shows its error verbatim and neither sections nor "No jobs yet", and no "ETA"/"remaining" text with a progress carrying `eta_seconds`.
- `README.md` (Jobs row in Frontend surfaces; `apps/desktop` test count and `jobGroups`/`JobsPage` description), `CONTEXT.md` §8 (short "Grouped Jobs list" paragraph) and §10.2 row (limitations: no ETA/throughput, client-side, no paging), `docs/ROADMAP.md` O1.5 annotated "grouped Jobs list shipped 2026-11-09 (bench-throughput-inbox story 4); throughput and ETA not built" (not struck: only part ships), `AGENTS.md` frontend test count -- docs ship with code.

**Acceptance Criteria:**
- Given a station with two wiping Jobs, one queued Job and three erased Jobs, when the operator opens `/jobs`, then the page shows "Active · 2", "Queued · 1", "Finished · 3" in that order, each Job under its section and linking to its Job page, and no ETA anywhere.
- Given a queued Job is started, when the next poll returns, then it moves from Queued to Active and both counts update.
- Given `cd apps/desktop && pnpm typecheck && pnpm test && pnpm build`, then all pass with the two new test files included.

## Spec Change Log

## Review Triage Log

### 2026-11-09 — Review pass
- verdicts: 25 findings — high 0, medium 2, low 17, false 6, maybe-false 0
- findings:
  - `low` `patch` [blind] Comparator not transitive when parseable and unparseable timestamps mix — fixed: unparseable timestamps sort after every parseable one in each section, by id among themselves.
  - `low` `patch` [blind] Unparseable-timestamp test filters job "a" out; README overstates "unparseable by id" — same root as row 1; fixed: exact mixed-order assertions for Queued and Finished; README/CONTEXT wording updated.
  - `low` `reject` [blind] Active mixes pending destruction with wiping Jobs, inflating "running" — recorded autonomous decision (safe default: never show an uncertified drive as finished); each row's pill names its state; changing it means editing this spec.
  - `false` `reject` [blind] One failed 1.5 s poll replaces the list with an error — the app's QueryClient keeps react-query's default 3 retries with backoff (`apps/desktop/src/main.tsx:19`), so `error` is set only after ~7 s of consecutive failures, not one dropped request.
  - `low` `patch` [blind] `.scratch/STATUS.md` O row omits stories 3–4 and lists O3.3 as unbuilt — fixed: row records O1.4, O2.1, O3.3 and O1.5 (in part) shipped, O3.3 dropped from not-built.
  - `false` `reject` [blind] Story records not updated, `pnpm build` not evidenced — the triage log and result are written by this review step by design; `pnpm build` ran and succeeded in step 3 verification.
  - `low` `patch` [blind] README Jobs row omits Active's oldest-first order — fixed in README.
  - `low` `patch` [blind] "moves a started Job from Queued to Active on the next poll" test is two static renders — fixed: renamed to say what it checks.
  - `low` `defer` [blind] `useEventStream(() => {})` "pulse" comment claims a refresh that never happens — pre-existing code and comment, unchanged by this story; deferred.
  - `low` `reject` [blind] No page-level test renders a pending-destruction Job — its grouping is tested in `jobGroups.test.ts` ("puts pending_destruction under active") and the row markup is unchanged from before; not met in everyday use.
  - `low` `reject` [blind] Test fixtures cast `as unknown as Job` — the repo-wide test precedent (`inbox.test.ts`, `InboxPage.test.ts`); a shared typed builder is a refactor beyond this story.
  - `low` `reject` [blind] Jobs page imports `InboxSource` from the Inbox module — a two-field `{data, error}` shape; moving it is churn across a shipped module with no named divergence.
  - `low` `reject` [blind] Sections lack `aria-labelledby`; "·" read literally — each section starts with an `<h3>`, so heading navigation works; adding ids/labels adds surface for a case not met at the bench today.
  - `false` `reject` [edge] Unknown `state.state` crashes `groups[...]` — the server serializes the seven `JobState` labels only (`crates/wipe-common/src/job.rs`; `pending_co_sign` is input-only) and `api/types.ts` mirrors it by rule; the switch is exhaustive.
  - `low` `patch` [edge] Non-transitive comparator with mixed NaN — same root as row 1, same fix.
  - `false` `reject` [edge] `ended_at: ""` defeats `??` — `ended_at` is `Option<OffsetDateTime>` (`crates/wipe-common/src/job.rs:243`), serialized as null or RFC 3339, never an empty string.
  - `low` `patch` [edge] Claim "unparseable timestamps order by id" not met — same root as row 1, same fix.
  - `medium` `patch` [verification-gap] V1: the real in-app row link (`RouterJobLink`) is never rendered by a test — fixed: `JobsList` rendered under a TanStack memory router, hrefs asserted in section order (Inbox precedent).
  - `low` `patch` [verification-gap] Non-transitive comparator — same root as row 1, same fix.
  - `low` `reject` [intent-alignment] Pending destruction under Active is one of several readings — the recorded autonomous decision, not a defect.
  - `low` `reject` [intent-alignment] A Job whose erasure failed counts in Active — by design (spec Always: grouping by outer JobState; the Inbox calls failures out, and the row's bar is red).
  - `false` `reject` [intent-alignment] Error over cached data hides the queue on any failed refetch — same refutation as row 4 (three retries before `error`).
  - `medium` `patch` [intent-alignment] `RouterJobLink` and page wrapper untested — same root as V1, same fix; the query wiring itself is unchanged from before this story.
  - `low` `patch` [intent-alignment] "Next poll" acceptance tested by two static renders — same root as row 8, same fix.
  - `false` `reject` [intent-alignment] No evidence `pnpm build` ran — it ran and succeeded in step 3; the manual browser check was not run (residual risk).

## Design Notes

Grouping is by outer JobState, not by `deriveSlotStatus`: the Jobs page answers "where is each Job in its life", while failures waiting on a human are the Inbox's job (story 2). Finished sorts newest first because the operator scanning it wants the latest results; Active and Queued sort oldest first because that is the order the bench works them.

## Autonomous decisions

- **Where pending destruction goes.** A Job recorded for destruction (`pending_destruction`) is not terminal, has no certificate yet, and cannot be started. Options: (a) Finished; (b) Queued; (c) Active. Chosen: (c) — the safe default: putting it under Finished would suggest the drive's disposition is complete when no destruction certificate exists (CONTEXT §5 says the UI never implies it is destroyed), and it is not waiting to be started, so it is not Queued.
- **Load error.** Today a failed `GET /api/jobs` renders "No jobs yet". Chosen: show the error verbatim — an empty-looking queue on a fault hides drives, the loss this spec exists to prevent.

## Verification

**Commands:**
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: clean typecheck; all vitest pass including `jobGroups.test.ts` and `JobsPage.test.ts`.
- `cd apps/desktop && pnpm build` -- expected: success.

**Manual checks (if no CLI):**
- `cd apps/desktop && pnpm build`, then `cargo run -p wipe-cli -- serve --static-dir apps/desktop/dist` (mock backend); start a few Jobs, leave one queued, open `/jobs` and see three headed sections with counts.

## Auto Run Result

Status: done

**Summary.** The desktop Jobs page (`/jobs`) now shows every Job in three sections, Active, Queued and Finished, each headed with its count ("Active · 3"). An empty section reads "None". Active holds Jobs in progress or pending destruction, Queued holds Jobs waiting to start, and both sort oldest first. Finished holds erased, destroyed, quarantined and aborted Jobs, newest first by when they ended. A Job with an unreadable timestamp sorts last in its section, by id. Before this, the server returned Jobs in arbitrary hash-map order. A failed load now shows the error instead of "No jobs yet". The page shows no ETA. Nothing changed in Rust, the routes or the types.

**Files changed.**
- `apps/desktop/src/pages/jobGroups.ts` — new pure module: `jobGroupOf`, `groupJobs`, `JOB_GROUP_KINDS`, with a transitive sort.
- `apps/desktop/src/pages/jobGroups.test.ts` — 15 tests: each of the seven states in its group, group order, both sort orders, the `ended_at` fallback, ties and mixed unparseable timestamps, empty groups, input not mutated.
- `apps/desktop/src/pages/JobsPage.tsx` — `JobsView`/`JobsList` presentational exports with an injectable row link (`HrefJobLink` for tests); error, loading and empty states.
- `apps/desktop/src/pages/JobsPage.test.ts` — 8 static-render tests, including the real router link.
- `CONTEXT.md` (§8 paragraph, §10.2 row), `README.md` (Jobs row, test table), `docs/ROADMAP.md` (O1.5 annotated, not struck), `AGENTS.md` (385 tests), `.scratch/STATUS.md` (O row).

**Review.** 25 findings (high 0, medium 2, low 17, false 6).
- **Patched, 5 entries:**
  - The real in-app row link was untested (medium). A router-rendered test now covers it.
  - The sort comparator was not transitive when readable and unreadable timestamps mixed (low). Unreadable timestamps now sort last, by id, with exact-order tests.
  - README wording for the Active sort order (low).
  - A test name that described a poll the test does not run (low).
  - The `.scratch/STATUS.md` O row was stale (low).
- **Deferred, 1:** the Jobs page's event-stream "pulse" does nothing. This predates the story.
- **Rejected, with reasons in the triage log:**
  - Low: pending destruction under Active (a recorded decision, two rows); a failed erasure counted in Active (by design); no page test for pending destruction; `as unknown as Job` fixtures (the repo's precedent); `InboxSource` imported from the Inbox module; `aria-labelledby` on the sections.
  - False: one failed poll hides the list (react-query retries three times first, twice); an unknown state crashes the page; an empty-string `ended_at`; the story records were not updated and the build was never run (twice).

**Follow-up review recommended:** false. Patched entries by verdict: high 0, medium 1, low 4.

**Verification.** In `apps/desktop`: `pnpm typecheck` was clean, `pnpm test` passed 385 of 385 (19 files), and `pnpm build` succeeded, both before and after the patches.

**Residual risks.**
- Nobody has opened `/jobs` in a browser or the Tauri shell; the manual check was not run.
- A station with many finished Jobs renders all of them, with no paging.
- The `JobsPage` query wiring (unchanged) has no rendered test.

