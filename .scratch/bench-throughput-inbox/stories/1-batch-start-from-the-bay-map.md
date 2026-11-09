---
title: 'Batch start from the bay map'
type: 'feature'
created: '2026-11-09'
status: 'done'
baseline_revision: 'd2c6282bcca2d81ca9090328b326b337785ddac0'
review_loop_iteration: 0
followup_review_recommended: true
context: []
warnings: [oversized]
deferred:
  - summary: >-
      A Queued Job (e.g. a batch "created, not started" drive) is drawn as wiping on the bay map.
    evidence: |-
      deriveSlotStatus maps queued and in_progress alike to wiping; pre-existing, the single wizard can leave the same Queued state when start fails.
    location: >-
      apps/desktop/src/bench/slotStatus.ts:39
    severity: low
  - summary: >-
      The single-drive wizard sends asset tag and ticket untrimmed, while the batch path trims blanks to null.
    evidence: |-
      EraseWizard sends `assetTag || null`; batch uses blankToNull. Spec keeps the single wizard unchanged, so aligning it is a later change.
    location: >-
      apps/desktop/src/pages/DevicesPage.tsx (EraseWizard)
    severity: low
  - summary: >-
      Bay-map trays have no keyboard activation (no tabIndex or key handler), so select mode cannot be driven from the keyboard.
    evidence: |-
      Pre-existing: Tray was mouse-only before this story; ROADMAP O3.4 covers the accessibility audit.
    location: >-
      apps/desktop/src/bench/BayMap.tsx (Tray)
    severity: low
  - summary: >-
      POST /api/jobs accepts a second Job for a device that already has an active Job, so a selection made just before another client starts that drive can create a duplicate.
    evidence: |-
      JobRunner::create_job only checks the device exists (crates/wipe-engine/src/runner.rs:111). Pre-existing for the single wizard too; the window is one 1.5 s jobs poll plus WS invalidation.
    location: >-
      crates/wipe-engine/src/runner.rs:111
    severity: medium
  - summary: >-
      Page-level wiring (select-mode click routing, prune effect, inFlight double-click ref, Cancel vs close) has no automated test.
    evidence: |-
      apps/desktop has no DOM interaction test library; the pure predicates are tested and the static render is tested, interaction needs a new dependency.
    location: >-
      apps/desktop/src/pages/DevicesPage.tsx
    severity: low
---

<intent-contract>

## Intent

**Problem:** An operator who loads a tray of eight drives must open the Sanitize wizard eight times, once per bay, re-entering the same classification, intent and ticket each time (ROADMAP O1.4).

**Approach:** Add a select mode to the bay map on the Devices page: the operator ticks idle bays, confirms one shared spec in a batch wizard, and the UI creates and starts one Job per drive through the existing `POST /api/jobs` and `POST /api/jobs/{id}/start` routes, then lists each drive's outcome. The selection and the batch run live in a pure TypeScript module covered by vitest.

## Boundaries & Constraints

**Always:**
- Bay↔device comes only from the server-resolved topology (`ResolvedBayTopology.occupancy`, via the `BayCellData` that `BayMap`'s lookup already produces); never re-derive matching in TS (ADR-0002).
- Use only the existing create and start routes; no new server route, no Rust change. Each drive gets its own Job, attributed to the signed-in session operator; the batch button is disabled with no operator, as the single wizard is.
- Only bays whose slot status is `idle` (drive present, no Job) can be selected (see Autonomous decisions).
- One drive's failure never stops the others: every target is attempted, in bay-map order, and each gets its own result.
- The selection is pruned whenever devices, jobs or topology refetch: an entry whose bay no longer holds the same device, or whose status is no longer `idle`, is dropped before it can be started.
- Asset tag is per drive (optional input per row); classification, intent and ticket/RMA are shared.
- Outside select mode, the bay map and single-drive wizard behave exactly as today.

**Never:**
- No ETA, queue view, failure routing, RBAC, persisted "Batch" entity (CONTEXT §5: a Batch is an ad-hoc UX selection, not a WorkOrder), or card-grid multi-select.
- No navigation to a Job page after a batch start; the results list replaces it.
- No hardware-sanitization claim in any new copy (ROADMAP §0.1).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Happy path | 8 idle bays selected, operator signed in, confirm | 8 Jobs created and started; results list 8 × "started" with links to `/jobs/$jobId`; map shows them wiping | No error expected |
| Create fails | `createJob` rejects for one drive (e.g. device pulled → 404 DeviceNotFound) | That row reads "not created" with the error message; the other 7 still run | Message from the rejected Error shown verbatim |
| Start fails | `createJob` resolves, `startJob` rejects | Row reads "created, not started" with job link and error; operator can open the queued Job | Not retried automatically |
| Stale selection | A selected bay's drive is pulled or another Job starts on it before confirm | It drops out of the selection; the count and wizard rows update | Nothing is sent for it |
| Non-idle click in select mode | Click a wiping/erased/failed/empty/blanked bay | Selection unchanged | — |
| Empty selection | 0 bays selected | "Start" control disabled | — |
| Double confirm | Operator clicks Begin twice | Only one run; button disabled while running | — |

</intent-contract>

## Code Map

- `apps/desktop/src/pages/DevicesPage.tsx` -- `DevicesPage` owns bay-map state (`focusedBayId`, `onBaySelect` ~L132 opens `EraseWizard` for idle bays only); `deviceToLatestJob` + `deriveSlotStatus` give status. `EraseWizard` (~L560) is the single-drive flow: `api.createJob` then `api.startJob`, shared spec controls (classification, intent, asset tag, ticket), operator chip, `create.isError` display. Reuse its spec controls by extracting them rather than duplicating.
- `apps/desktop/src/bench/BayMap.tsx` -- `BayMap` props `onSelect`, `selectedBayId`; `lookup` builds `BayCellData {bay, device, job, status}` from server occupancy; `Tray` draws `selected` as `strokeWidth 2`; blanked (`bay.disabled`) bays are not clickable. Iteration order = `topology.enclosures[].banks[].bays[]` (the bay-map order). `BenchSetupPage.tsx:333` also renders `BayMap` with `selectedBayId` — keep that prop working.
- `apps/desktop/src/bench/slotStatus.ts` -- `SlotStatus`, `deriveSlotStatus`; `idle` = device present, no Job.
- `apps/desktop/src/api/client.ts` -- `api.createJob(CreateJobRequest)` → `{job_id}`, `api.startJob(id)`; errors are `Error("<status> <text>: <body>")`.
- `crates/wipe-engine/src/runner.rs:111,140` -- read-only evidence: `create_job` refuses an unknown device; `start` refuses a non-`Queued` Job. No server change needed.
- `apps/desktop/src/bench/identify.test.ts` -- test style to follow (pure functions, hand-built `Device` fixtures, `@/` imports, node env — no DOM test library is installed).

## Tasks & Acceptance

**Execution:**
- `apps/desktop/src/bench/batchSelection.ts` -- new pure module: selection type (bay id → device id); `isSelectable(cell)`; `toggleBay(sel, cell)`; `selectAllIdle(sel, cells)`; `pruneSelection(sel, cells)`; `canStart({targets, operator, running})` (false with no targets, no operator, or a run in flight — the page's Begin/Start buttons use it); `batchTargets(sel, topology/cells)` in bay-map order; `runBatch(targets, sharedSpec, perDriveAssetTags, client)` where `client` is `{createJob, startJob}` (injected so tests need no fetch), returning one result per target: `started{jobId}` | `not_started{jobId, error}` | `not_created{error}`. -- keeps the logic testable and out of the page.
- `apps/desktop/src/bench/batchSelection.test.ts` -- vitest for every I/O-matrix row the module owns: toggle/selectable rules per status and blanked/empty bays, select-all-idle, pruning (device swapped, device gone, status no longer idle), bay-map ordering, `canStart` for empty selection, no operator and a run in flight (double confirm), runBatch happy path, create failure continues, start failure yields `not_started`, specs carry the shared fields, operator and each drive's own asset tag (empty → null).
- `apps/desktop/src/bench/BayMap.tsx` -- add optional `selectedBayIds?: ReadonlySet<string>` rendered as selected (alongside `selectedBayId`), visibly distinct from a merely focused bay. -- the map must show what will be started.
- `apps/desktop/src/pages/DevicesPage.tsx` -- add select-mode toggle on the bay-map view, a selection bar (count, "Select all idle", "Clear", "Start N…"), prune on data change, and a `BatchWizard` modal: rows per target (bay label, model, serial, asset-tag input), shared classification/intent/ticket controls extracted from `EraseWizard`, operator chip, Begin disabled while running or with no operator, then a per-drive results list with Job links; closing clears selection and leaves select mode.
- `README.md` (Devices row ~L423 / bay-map section), `CONTEXT.md` §8 "Wizard flow (current)" and a §10.2 table row, `docs/ROADMAP.md` O1.4 struck "Shipped 2026-11-09 (bench-throughput-inbox story 1)". -- AGENTS.md: docs ship with code.

**Acceptance Criteria:**
- Given the Devices page in bay-map view with several idle drives, when the operator enters select mode and clicks three idle bays, then those three trays are drawn selected and the bar reads 3 selected.
- Given a selection of N idle bays and a signed-in operator, when the operator confirms one shared spec in the batch wizard, then N Jobs exist in `/api/jobs`, each with that classification, intent, ticket and operator and its own asset tag, and each is started.
- Given a batch where one drive's create or start fails, when the run finishes, then the results list shows that drive's failure message and every other drive's started Job.
- Given select mode is off, when the operator clicks an idle bay, then the single-drive wizard opens as before.

## Spec Change Log

## Review Triage Log

### 2026-11-09 — Review pass
- verdicts: 31 findings — high 0, medium 5, low 22, false 4, maybe-false 0
- findings:
  - `low` `reject` [blind] Batch wizard omits the per-drive capability preview — informational only; JobRunner unfreezes frozen drives itself (runner.rs:643-650) and any failure is reported per drive; a fix means N capability queries. Recorded as a limitation in CONTEXT §10.2.
  - `low` `defer` [blind] A created-but-not-started Job draws as wiping — pre-existing deriveSlotStatus behaviour (queued → wiping); deferred.
  - `medium` `patch` [blind] Cancel before Begin discards the whole selection — patched: before a run, closing only closes the modal; after results, closing clears and leaves select mode.
  - `low` `reject` [blind] Leaving the page mid-run loses results — a run of a tray is well under a second of local HTTP; the Jobs still get created and appear on the Jobs page; a route blocker adds surface. Recorded as a limitation in CONTEXT §10.2.
  - `low` `reject` [blind] No per-drive progress during the run — sub-second sequential run; cosmetic.
  - `false` `reject` [blind] begin() never resets inFlight and lacks a catch — runBatch catches every per-drive error and cannot reject; after the run, results replace the Begin button, so the stale ref is never consulted.
  - `low` `defer` [blind] Single and batch wizards trim fields differently — spec keeps the single wizard unchanged; deferred.
  - `low` `patch` [blind] Unticked selectable bays carry no aria-pressed — patched: with selectedBayIds given, clickable trays carry aria-pressed true/false.
  - `low` `reject` [blind] Non-selectable bays keep role=button in select mode — clicking is a harmless no-op the spec calls for; removing the affordance needs selectability threaded into BayMap.
  - `low` `defer` [blind] Trays have no keyboard activation — pre-existing; deferred (ROADMAP O3.4).
  - `medium` `patch` [blind] makeBayLookup and page wiring untested — patched for makeBayLookup (topology → cells → selectAllIdle test) and checked rendering (BayMap static render test); page interaction deferred (see verification-gap row 3).
  - `low` `patch` [blind] CONTEXT §10.2 row lists no limitations — patched: capability preview, Queued-draws-as-wiping and unstored results added.
  - `low` `reject` [blind] ROADMAP doesn't say whether the O1 exit criterion is met — the diff does not claim it; O1.4 only is struck.
  - `low` `reject` [edge] Navigating away mid-run loses results — duplicate of blind row 4; same reason.
  - `low` `reject` [edge] Lost createJob response leaves an orphan Queued Job reported as not created — localhost API; the Job still appears on the Jobs page; same as the existing single wizard.
  - `low` `reject` [edge] Lost startJob response reports a running Job as not started — same reason; the row links the Job, whose page shows its true state.
  - `medium` `defer` [edge] Another client can start a Job on a selected drive before the refetch, creating a duplicate — server accepts duplicate Jobs per device (pre-existing, single wizard too); deferred.
  - `false` `reject` [edge] runBatch or invalidateQueries rejection wedges Begin — runBatch cannot reject; TanStack v5 invalidateQueries does not throw by default (throwOnError false).
  - `low` `patch` [edge] aria-pressed missing on unticked bays — duplicate of blind row 8; same patch.
  - `low` `reject` [edge] Clicks on non-selectable bays silently do nothing — duplicate of blind row 9.
  - `false` `reject` [edge] resolved becoming falsy hides Done selecting — select mode can only be entered once bays.data exists, and React Query keeps data on later errors, so resolved never returns to null.
  - `medium` `patch` [verification-gap] makeBayLookup decides the device id and is untested — patched: new test builds cells from a ResolvedBayTopology via makeBayLookup + deriveSlotStatus.
  - `low` `patch` [verification-gap] Ticked rendering (AC 1) has no assertion — patched: BayMap renderToStaticMarkup test for aria-pressed and ", selected".
  - `low` `defer` [verification-gap] Double-confirm ref, prune effect and select-mode routing untested at page level — no DOM interaction library; deferred as the reviewer filed.
  - `low` `patch` [verification-gap other] aria-pressed only when ticked — duplicate of blind row 8.
  - `low` `reject` [verification-gap other] Non-idle bays keep pointer affordance — duplicate of blind row 9.
  - `low` `patch` [intent-alignment] Tests exercise only the module, not the operator surface — covered by the two new tests above; interaction surface deferred.
  - `low` `reject` [intent-alignment] Not literally "one gesture" — CAP-1 success is "one confirm creates and starts N jobs", which this meets.
  - `low` `reject` [intent-alignment] Reading B (non-idle drives) declined — recorded autonomous decision, not a defect.
  - `medium` `patch` [intent-alignment] Cancel also discards the selection — duplicate of blind row 3; same patch.
  - `false` `reject` [intent-alignment] Manual check unevidenced — the implementer ran a live batch against a mock station (3 started, 1 not created, fields verified in /api/jobs); browser click-through is noted as residual risk.

## Design Notes

Runs are sequential, not parallel: eight drives take well under a second of HTTP, per-drive results stay in bay order, and the server sees no request burst. A `not_started` Job is left `Queued` (the single wizard already leaves the same state when start fails); the row links to it.

## Autonomous decisions

- **Which bays a batch may include.** On the bay map an operator can already "Start new" on an erased, destroyed, quarantined or aborted drive from its card. In a one-gesture batch, sweeping such drives in would start a fresh Job that hides a quarantine or a destruction record behind a new "wiping" state. Options: (a) idle only; (b) idle plus every status the card offers "Start new" for. Chosen: (a) — the safe default; re-wiping a drive with history stays a deliberate per-drive action.
- **Asset tag in a shared spec.** An asset tag identifies one drive; sharing it would stamp the same tag on N certificates. Chosen: per-drive optional asset tag in the wizard rows; ticket/RMA is shared. Story 3 (scanner fields) can later make these rows scanner-friendly.
- **Partial failure.** Options: stop at first failure, or attempt all. Chosen: attempt all and report each — matches CAP-1 "partial failures are listed per drive".

## Verification

**Commands:**
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: clean typecheck; all vitest pass including the new `batchSelection.test.ts`.

**Manual checks (if no CLI):**
- `cd apps/desktop && pnpm build`, then `cargo run -p wipe-cli -- serve` (mock backend), open the Devices page: select idle bays, Start, see per-drive results and bays turn wiping.

## Auto Run Result

Status: done

**Summary.** Select mode on the Devices bay map ticks idle bays (drive present, no Job); "Start N…" opens a batch wizard with a shared classification, intent and ticket/RMA, a per-drive optional asset tag and the operator chip; Begin creates and starts one Job per drive through the existing `POST /api/jobs` and `/start` routes, sequentially in bay-map order, attempting every drive, then lists each drive's outcome (started / created, not started / not created) with Job links. Ticked bays drop out when a refetch shows them pulled, swapped or no longer idle. No Rust or route change.

**Files changed.**
- `apps/desktop/src/bench/batchSelection.ts` — new pure selection and batch-run module.
- `apps/desktop/src/bench/batchSelection.test.ts` — 19 tests over the module, including a cell set built through `makeBayLookup`.
- `apps/desktop/src/bench/BayMap.test.ts` — static render test for ticked bays and `aria-pressed`.
- `apps/desktop/src/bench/BayMap.tsx` — exported `makeBayLookup`; `selectedBayIds` drawn with a sky-blue outline and check.
- `apps/desktop/src/pages/DevicesPage.tsx` — select mode, selection bar, `BatchWizard`, shared spec controls extracted from `EraseWizard`.
- `CONTEXT.md` (§8, §10.2 row with limitations), `README.md`, `docs/ROADMAP.md` (O1.4 struck), `AGENTS.md` (frontend test count 312).

**Review.** 31 findings: 9 rows patched (5 distinct fixes: Cancel keeps the selection before a run; `aria-pressed` true/false in select mode; `makeBayLookup` test; BayMap render test; CONTEXT limitations), 5 deferred (Queued draws as wiping; untrimmed single-wizard fields; no keyboard on trays; server accepts duplicate Jobs per device; page interaction untested), 13 rejected low with reasons in the triage log, 4 false.

**Follow-up review recommended:** true — two medium-verdict entries were patched (Cancel discarding the selection; the untested bay lookup that decides which device a batch sends). Patched by verdict: high 0, medium 2, low 3. Named unverified risk: the Cancel-vs-close behaviour in `BatchWizard` is covered by no automated test.

**Verification.** `pnpm typecheck` clean; `pnpm test` 312/312 passed (13 files); `pnpm build` succeeded. The implementer ran a live batch against a mock station (4 targets, one bogus: 3 started, 1 not created; shared fields, operator and the one asset tag confirmed in `/api/jobs`).

**Residual risks.** Nobody has clicked through the new screens in a browser. Interaction-level behaviour (double-click guard ref, prune effect, Cancel vs close) has no automated test because the desktop app has no DOM interaction library.
