---
title: 'Manifests page excludes Jobs already on a manifest'
type: 'bugfix'
created: '2026-11-08'
status: 'done'
baseline_revision: '55b031cb2ba92d9533945dc9eb21a21586b86b91'
review_loop_iteration: 0
followup_review_recommended: false
context: []
warnings: []
deferred:
  - summary: >-
      The Manifests page's wiring of the selection helpers (which lists it passes, and that Create manifest posts and counts only offered ids) is checked by code review only, not by a test.
    evidence: |-
      Only manifestAssembly.test.ts exercises the helpers; no test renders ManifestsPage. Reverting `job_ids: submitIds` to `Array.from(selected)`, or passing `[]` for the manifests, passes all 217 desktop tests. The desktop vitest runs in the node environment with no DOM harness (vite.config.ts comment: "A DOM harness can be added when component tests arrive"), and this story's contract keeps it that way. Add a ManifestsPage render test once a component harness (happy-dom plus a React renderer) exists.
    location: >-
      apps/desktop/src/pages/ManifestsPage.tsx:22-26,52-61,134-137
    severity: low
  - summary: >-
      AGENTS.md still says the desktop vitest suite has 200 tests; it now has 217.
    evidence: |-
      AGENTS.md:54 reads "vitest, 200 tests"; `pnpm test` reports 217 passed. AGENTS.md is owner-maintained agent context, so edits to it are deferred to the owner.
    location: >-
      AGENTS.md:54
    severity: low
---

<intent-contract>

## Intent

**Problem:** The Manifests page's "Pending Jobs" assembly list is an inline filter (`pending_co_sign && !manifest_id`) inside `ManifestsPage.tsx` with no test. Since story 1 the server stamps `manifest_id`, so the filter now works by accident of timing, but it ignores the manifests list. Because the Jobs and manifests queries poll separately, a Job can still be offered while it is already on a pending manifest. A checkbox ticked before another operator assembled that Job also stays in the submitted selection, and the server answers 409 (CAP-2, SPEC `destruction-cert-integrity`).

**Approach:** Move the selection rule into a pure, tested module. A Job is offered only when it is `pending_co_sign`, has no `manifest_id`, and is not listed on any manifest that is not `rejected`. The page renders that list and submits only selected ids that are still offered.

## Boundaries & Constraints

**Always:**
- The rule mirrors the server's two guards in `create_manifest`: the stored-manifest scan (state != Rejected) and the runner's `manifest_id` check. The UI filter is a convenience; the server stays the authority.
- Keep `apps/desktop/src/api/types.ts` unchanged; the fields already exist.
- Vitest stays in the node environment (pure function tests, no DOM harness).

**Never:**
- No Rust changes, no new routes, no runner-error status mapping (story 3).
- No porting of any other server rule to TypeScript beyond this selection filter.
- No visual redesign of the page or copy changes beyond what the filter needs.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Unassigned | Job `pending_co_sign`, `manifest_id` null, on no manifest | offered | — |
| Stamped | Job `pending_co_sign`, `manifest_id` set | not offered | — |
| Stale Jobs poll | Job `pending_co_sign`, `manifest_id` null, listed on a `pending` manifest | not offered | — |
| Rejected manifest | Job `pending_co_sign`, `manifest_id` null, listed only on a `rejected` manifest | offered | — |
| Other states | Job `queued`/`in_progress`/`erased`/`destroyed`, no manifest | not offered | — |
| Stale selection | id selected, then the Job stops being offered | not submitted; the button count excludes it | — |

## Autonomous decisions

- **Which manifests count as "live" in the page filter.** The page receives both the Jobs list and the manifests list, polled separately every 2 s, so either can be a poll behind. Options: (a) filter on `manifest_id` alone; (b) also exclude Jobs listed on any non-`rejected` manifest. Chosen: **(b)**. It matches the server's own two guards exactly, and it closes the window where the manifests list already shows a Job the Jobs list has not yet reported as stamped. Treating `rejected` as not live follows the server scan; no route sets `Rejected` today, so this changes nothing an operator can see.
- **Stale ticked checkboxes.** A Job ticked before another operator assembled it would still be posted and earn a 409 for the whole batch. Chosen: submit, and count on the button, only selected ids that are still offered. This is the least surprising behaviour and does not change what the server accepts.

</intent-contract>

## Code Map

- `apps/desktop/src/pages/ManifestsPage.tsx:21-27` -- inline `pendingJobs` filter (`useMemo`); replace with the new helper, passing `manifests.data ?? []`.
- `apps/desktop/src/pages/ManifestsPage.tsx:46-63,140-146` -- `PendingJobsCard`: `selected` is a `Set<string>` posted as `Array.from(selected)` and counted as `selected.size` on the button. Make both use selected ids intersected with the offered `jobs` prop.
- `apps/desktop/src/api/types.ts:272-295` -- `Job.manifest_id: string | null`, `Job.state.state` (`JobStateWire`, `"pending_co_sign"` at :104), `DestructionManifest.job_ids`, `ManifestStateLabel` = `pending|signed|rejected`. Read-only.
- `crates/wipe-server/src/handlers.rs:375-417` -- the server guards the UI mirrors (manifest scan excluding `Rejected`, PendingCoSign check, runner `link_to_manifest`). Read-only.
- `apps/desktop/vite.config.ts` -- vitest `include: ["src/**/*.test.ts"]`, node environment; `@` alias to `src`.
- `apps/desktop/src/bench/identify.test.ts` -- style reference for pure-module tests with small fixture builders (`as Device` casts are fine).
- `README.md:359` and `AGENTS.md:54` -- the desktop vitest count (200) and the test-table description; README row must gain the new module and count. AGENTS.md's count is owner-maintained agent context: leave it, and note it in the result.
- `CONTEXT.md:349-365` (§5 DestructionManifest "Today") -- add one sentence on what the Manifests page offers for assembly.

## Tasks & Acceptance

**Execution:**
- `apps/desktop/src/pages/manifestAssembly.ts` -- new pure module: `assemblableJobs(jobs: Job[], manifests: DestructionManifest[]): Job[]` (order preserved) and `offeredSelection(selected: ReadonlySet<string>, offered: Job[]): string[]` (selected ids still offered, in offered order); doc comment names the server guards it mirrors -- testable selection logic.
- `apps/desktop/src/pages/manifestAssembly.test.ts` -- vitest cases for every row of the I/O matrix, plus a Job listed on a `signed` manifest (not offered) and empty inputs -- CAP-2 success signal.
- `apps/desktop/src/pages/ManifestsPage.tsx` -- use `assemblableJobs` for the list, `offeredSelection` for the posted ids and the button count/disabled state -- the page offers only unassigned Jobs.
- `README.md` -- update the `apps/desktop` test-table row (count and a short `manifestAssembly` entry) -- docs track code.
- `CONTEXT.md` -- in the DestructionManifest "Today" text, state that the Manifests page offers only PendingCoSign Jobs with no `manifest_id` that are on no pending or signed manifest -- docs track code.

**Acceptance Criteria:**
- Given a Job already on a pending manifest (stamped, or only listed on the manifest), when the Manifests page computes its assembly list, then that Job is absent; a vitest proves it.
- Given a selected Job that is no longer offered, when the operator presses Create manifest, then its id is not in the request and the button count excludes it.
- `cd apps/desktop && pnpm typecheck && pnpm test` passes.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 17 findings — high 0, medium 0, low 13, false 4, maybe-false 0
- findings:
  - `[low]` `[patch]` Blind: the Pending Jobs card's loading gate checked only the Jobs query, so on the manifests query's first load the filter ran against `[]` (manifest_id check only) — verified ManifestsPage.tsx passed `loading={jobs.isLoading}`; patched to `jobs.isLoading || manifests.isLoading`. A failed first manifests fetch still falls back to the `manifest_id` check; the server answers 409, which is acceptable.
  - `[false]` `[reject]` Blind: stale ticks are hidden but never cleared, so a Job could return already ticked after a failed-then-recovered manifests fetch — TanStack Query keeps the previous `data` when a refetch fails, a stamped `manifest_id` is never cleared, and nothing sets `Rejected`, so a dropped Job cannot be offered again.
  - `[low]` `[reject]` Blind: no notice when a ticked Job disappears because someone else assembled it — the button count drops visibly and the server stays the authority; a notice would be new customer-visible copy and UI state for a rare two-operator race.
  - `[low]` `[reject]` Blind: the `rejected` branch and its tests describe an unreachable state (a Job on a rejected manifest still carries `manifest_id`) — the branch mirrors the server scan exactly as documented, and the `manifest_id` check keeps such a Job off the list anyway; no operator or developer harm named.
  - `[low]` `[defer]` Blind: AC 2 is proven only on the helper, not on the page's request and button — the same gap as the verification-gap finding; deferred in frontmatter (needs a DOM harness).
  - `[low]` `[defer]` Blind: AGENTS.md still says 200 frontend tests — agent-context file; deferred in frontmatter.
  - `[low]` `[patch]` Blind: `assemblableJobs` doc comment said it mirrors "the two guards" and attributed the PendingCoSign check to the runner only — patched: the comment now lists the refusals an operator can hit from this page and names the server refusals it does not mirror.
  - `[low]` `[patch]` Blind: redundant `expect(submitted.length).toBe(1)` after `toEqual(["a"])` — line deleted.
  - `[low]` `[patch]` Blind: rewrapped CONTEXT.md line ran past the paragraph's wrap width — the changed lines reflowed to ~72 columns.
  - `[low]` `[patch]` Edge: manifests query loading or failed leaves the filter on `manifest_id` alone — same root cause as row 1; same patch.
  - `[low]` `[reject]` Edge: right after a successful create, the just-assembled Jobs stay offered until the invalidated queries refetch — the selection is cleared, the window is one refetch (milliseconds), the old inline filter had it too, and a repost earns a 409 with nothing stamped; optimistic cache writes add state for no real exposure.
  - `[low]` `[patch]` Edge (claim): doc comment implies the client applies every server guard — same root cause as row 7; same patch.
  - `[low]` `[defer]` Verification gap: no test exercises the page's wiring of the helpers — pre-verified; filed disposition defer (no component test harness exists); deferred in frontmatter.
  - `[low]` `[defer]` Intent: the page-level surface of CAP-2 and AC 2 is shown only by reading the code — same root cause as row 13.
  - `[false]` `[reject]` Intent: ticks trimmed, not cleared — same claim as row 2; unreachable today.
  - `[false]` `[reject]` Intent: diff goes beyond reading A (stories.yaml) — the extra manifest-list check and selection rule are recorded autonomous decisions that follow CAP-2's "live manifest" wording; not a defect.
  - `[false]` `[reject]` Intent: `signed` treated as live (reading B′) — the auditor notes no visible difference (those Jobs are `destroyed`), and it matches the server scan.

## Verification

**Commands:**
- `cd apps/desktop && pnpm typecheck` -- expected: exit 0.
- `cd apps/desktop && pnpm test` -- expected: all pass, count above 200 including the new file.
- `cd apps/desktop && pnpm build` -- expected: exit 0.

## Auto Run Result

Status: done

**Summary.** The Manifests page now offers a Job for assembly only when it is `pending_co_sign`, has no `manifest_id`, and is listed on no manifest that is not `rejected`. This mirrors the server's checks in `create_manifest`. The rule lives in a pure, tested module, `manifestAssembly.ts`. The Create manifest button counts and posts only ticked Jobs that are still offered, so a Job another operator assembled first no longer turns the whole batch into a 409. The card waits for both the Jobs and the manifests lists before showing anything.

**Files changed**
- `apps/desktop/src/pages/manifestAssembly.ts`: new; `assemblableJobs` and `offeredSelection`.
- `apps/desktop/src/pages/manifestAssembly.test.ts`: new; 17 vitest cases covering every I/O matrix row, plus signed and mixed rejected/live manifests, list order and empty inputs.
- `apps/desktop/src/pages/ManifestsPage.tsx`: uses the helpers for the list, the posted ids, the button count and its disabled state; the loading gate covers both queries.
- `README.md`: desktop test row 200 → 217 with a `manifestAssembly` entry; TypeScript total 706 → 723.
- `CONTEXT.md` §5 DestructionManifest: one sentence on what the page offers, with the touched lines reflowed.

**Review findings** (17: 13 low, 4 false)
- Patches applied (6 rows, 4 root causes, all low): the loading gate now waits for the manifests query; the doc comment lists exactly the refusals it mirrors; a redundant assertion was removed; CONTEXT.md was reflowed.
- Deferred (frontmatter):
  - The page wiring is checked by review only, because there is no DOM test harness.
  - AGENTS.md still says 200 desktop tests.
- Rejected:
  - Stale ticks reappearing: false; nothing can put a dropped Job back on the list.
  - No notice when a ticked Job vanishes: new copy and UI for a rare race; the count visibly drops.
  - The `rejected` branch covers an unreachable state: it mirrors the server scan as documented.
  - The millisecond window after create, before the refetch: pre-existing, and a repost gets a harmless 409.
  - Three false intent-reading notes.

**Follow-up review recommended: false.** Patched: 0 high, 0 medium, 4 low entries.

**Verification**
- `pnpm typecheck` passed (exit 0).
- `pnpm test`: 217 passed across 5 files.
- `pnpm build` passed (exit 0).
- All three were re-run after the patches.
- No Rust changed, so `cargo test` was not run.

**Residual risks**
- A failed *first* manifests fetch still leaves the filter on `manifest_id` alone. The server's 409 stays the backstop.
- The page wiring has no automated test (deferred).
