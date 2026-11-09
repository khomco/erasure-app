---
title: 'Licence banner and expiry warning'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: 'c778cfa8da9ae26e1a908307885ff330222e2151'
review_loop_iteration: 0
followup_review_recommended: true
context: ['{project-root}/docs/adr/0012-opt-in-hard-stop-on-licence-lapse.md']
warnings: ['oversized']
deferred:
  - summary: >-
      After a stop-policy licence lapses, the banner says only that certificates are marked evaluation, contradicting the pre-expiry warning that new jobs will be refused, and never says how to install a licence offline.
    evidence: |-
      licenceBanner's non-valid branch ignores lapse_policy. Accurate today (refusal is not built); ADR-0012 §6's after-expiry clause and SPEC CAP-5 belong to licence-visibility story 5, which must branch this heading on lapse_policy when it adds the refusal.
    location: >-
      apps/desktop/src/api/licence.ts
    severity: low
  - summary: >-
      A count-limited licence nearing its erasure quota gets no advance warning; the banner appears only once quota_exhausted.
    evidence: |-
      LeaseStatus valid carries remaining_erasures, unread by licenceBanner. CAP-3 and ADR-0012 §6 name expiry warnings only; a low-quota threshold is new product behaviour.
    location: >-
      apps/desktop/src/api/licence.ts
    severity: low
  - summary: >-
      No test checks that Layout mounts the banner on every bench page, so deleting the mount would leave the suite green.
    evidence: |-
      Layout needs the operator context, event stream and router; apps/desktop has no harness rendering it. Closing this needs component-test infrastructure beyond this story.
    location: >-
      apps/desktop/src/components/Layout.tsx
    severity: medium
---

<intent-contract>

## Intent

**Problem:** An operator at the bench sees a certificate's `evaluation` marking only after opening a finished certificate (story 2); nothing on the other pages says the station is unlicensed, lapsed or about to expire, so a whole shift can produce **Evaluation certificates** unnoticed (SPEC CAP-3, ROADMAP L1.4/L1.5/O3.5/O-R9, ADR-0012 §6).

**Approach:** A pure `licenceBanner(info, now?)` helper in a new `apps/desktop/src/api/licence.ts` turns `GET /api/license` (story 1) into either nothing or `{tone, heading, lines}`; a `LicenceBanner` component in `Layout` polls the route and renders it as a persistent, non-dismissible bar between the header and every page.

## Boundaries & Constraints

**Always:** The banner informs only: no button, link or modal that blocks, delays or confirms an erasure (ROADMAP §0.2). Any non-`valid` status shows the server's `operator_message` verbatim. A `valid` licence warns once `days_remaining` is at or below 30, with tiers at 30, 7 and 1 days held in exported named constants; above 30 days (or valid with no expiry concern) nothing renders. Under `lapse_policy: "stop"` the pre-expiry warning says new jobs will be refused at expiry; under `evaluation`/`null` it says erasure continues and certificates will be marked evaluation. Uses `role="status"`. Docs updated in the same commit: CONTEXT §10 licensing row and §11 licensing paragraph, README licensing paragraph and vitest table, AGENTS.md vitest count, ROADMAP L1.4/O3.5 struck and L1.5 annotated.

**Never:** No Rust change (route and wire shape are story 1's). No licence install (story 4), no job refusal (story 5). No change to `operator_message` wording. No dismiss control. No hardware-sanitization claim. No TS port of licence evaluation: `status` comes from the server.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Unlicensed | `status.status="unlicensed"` | warning-tone banner, heading "Evaluation mode — no licence installed", line = `operator_message` | none |
| Lapsed (expired, not_yet_valid, quota_exhausted, wrong_machine, clock_rollback, lease_state_unavailable) | each status | danger-tone banner, heading "Licensed signing unavailable — certificates are marked evaluation", line = `operator_message` | none |
| Valid, far | `valid`, `days_remaining: 31` | `null`, nothing rendered | none |
| Valid, 30-day tier | `days_remaining: 30` (and 8) | warning tone; heading names expiry date from `not_after` and days left; evaluation-policy line | none |
| Valid, 7-day tier | `days_remaining: 7` (and 2) | warning tone, heading says "in 7 days" | none |
| Valid, 1-day tier | `days_remaining: 1` and `0` | danger tone; `0` reads "within a day", `1` "in 1 day"; every printed date labelled UTC | none |
| Stop policy, warning | `valid`, 7 days, `lapse_policy: "stop"` | line says new jobs will be refused when the licence expires, renew before then | none |
| Valid, `not_after` null | defensive | heading uses days left only | no throw |
| Route error | fetch rejects after retries | warning banner "Licence status unknown" telling the operator the station could not report its licence | no throw |
| Not yet loaded | no data, no error | nothing rendered | none |

## Autonomous decisions

1. **"Warn at 30, 7 and 1 days" — on those days only, or from then on?** *What:* the bench banner before a licence expires. *Problem:* a banner shown only on exactly day 30, 7 and 1 disappears on day 29 and 6, so a station could run a week without a warning. *Options:* (a) exact days only; (b) thresholds: warn from 30 days on, escalating tone at 7 and 1. *Choice:* (b). *Why:* the safe default — more warning, never less — and how expiry notices conventionally work; the constants still name 30, 7 and 1.
2. **What if the station cannot report its licence?** *Options:* (a) show nothing, as the portal's development banner does; (b) show "Licence status unknown". *Choice:* (b). *Why:* protective: the portal banner flags a non-production hub, where silence is harmless; here silence would read as "licensed". The header's live/offline pill still covers a dead server.
3. **The `stop` warning describes refusal that story 5 has not built yet.** *Problem:* until story 5 ships, a `stop` station that lapses keeps erasing with evaluation certificates, so "new jobs will be refused" overstates today's behaviour. *Choice:* ship CAP-3's wording as the SPEC and ADR-0012 §6 state it. *Why:* the error runs in the safe direction (it urges an earlier renewal), story 5 is queued in this epic, and after expiry the banner shows the server's `operator_message`, which stays accurate. Recorded as a residual risk.
4. **Wording.** Headings paraphrase `operator_message`, the CONTEXT §14 **Evaluation certificate** and **Lapse policy** entries and ADR-0012 §6; no new product claim, so not a copy decision for the owner (as story 2's decision 3).

5. **Expiry and lapse wording after review.** *Problem:* `days_remaining` rounds down, so "today"/"tomorrow" beside the UTC date of `not_after` named the wrong day, a bare date read as local time, and "Licence not valid" was false for a broken lease file. *Choice:* "within a day"/"in 1 day", dates labelled UTC (as `operator_message` prints them), and the heading "Licensed signing unavailable" for all six non-valid statuses. *Why:* accurate wording is the safe default; no product claim changes.

</intent-contract>

## Code Map

- `apps/desktop/src/api/types.ts:326-353` -- `LapsePolicy`, `LeaseStatus` (8 variants, tag `status`), `LicenseInfo` (story 1). Read-only.
- `apps/desktop/src/api/client.ts:239` -- `api.license()` exists; reuse.
- `crates/wipe-license/src/lease.rs:281-325` -- `operator_message` wording per status; `:382` `days_remaining = (not_after - now).whole_days()`, so `0` means under 24 h left.
- `crates/wipe-server/src/licensing.rs` -- `LicenseContext::view` is a pure read (story 1), so polling never writes the lease file.
- `apps/desktop/src/components/Layout.tsx:23-73` -- header then `<main>`; insert `<LicenceBanner />` between them. Existing polls use `refetchInterval: 4000`.
- `apps/portal/src/DevelopmentBanner.tsx` + `.test.ts` -- precedent: persistent `role="status"` bar, pure line helper, tested with `renderToStaticMarkup` and a pre-seeded `QueryClient` (works in the desktop's node vitest env; `react-dom` is a dependency).
- `apps/desktop/src/api/certificate.ts` -- house style for pure view helpers with doc comments.
- `apps/desktop/src/index.css:30-44` -- amber/rose/indigo palette used by pills.
- `CONTEXT.md:918` (§10 row: "no UI shows it yet", "no station-wide licence banner yet"), `CONTEXT.md:1003-1016` (§11 "Not built: a licence UI"), `README.md:489-494`, `README.md:408` (desktop vitest row, 269), `AGENTS.md` (269 tests), `docs/ROADMAP.md:470` (O3.5), `:578-579` (L1.4, L1.5).

## Tasks & Acceptance

**Execution:**
- `apps/desktop/src/api/licence.ts` -- export `EXPIRY_NOTICE_DAYS = 30`, `EXPIRY_URGENT_DAYS = 7`, `EXPIRY_FINAL_DAYS = 1`, `type LicenceBannerView = {tone: "warning"|"danger"; heading: string; lines: string[]}`, `licenceBanner(info: LicenseInfo): LicenceBannerView | null`, `LICENCE_UNKNOWN_BANNER` -- pure, testable view logic.
- `apps/desktop/src/api/licence.test.ts` -- one test per I/O matrix row (each lapsed status individually), boundary days 31/30/8/7/2/1/0.
- `apps/desktop/src/components/LicenceBanner.tsx` -- `useQuery({queryKey: ["license"], queryFn: api.license, refetchInterval: 60_000, refetchOnWindowFocus: true})`; render nothing while loading, `LICENCE_UNKNOWN_BANNER` on error, else the view as a full-width bar (amber or rose), heading bold, one `<p>` per line, no button.
- `apps/desktop/src/components/LicenceBanner.test.ts` -- `renderToStaticMarkup` with seeded query data: unlicensed renders `role="status"` and the message with no `<button`; valid 31 days renders `""`.
- `apps/desktop/src/components/Layout.tsx` -- mount `<LicenceBanner />` between `</header>` and `<main>`.
- `CONTEXT.md`, `README.md`, `AGENTS.md`, `docs/ROADMAP.md` -- banner shipped; counts; strike L1.4 and O3.5 with a dated note; L1.5 note "expiry warnings shipped; install is licence-visibility story 4".

**Acceptance Criteria:**
- Given an unlicensed station, when the operator opens any page of the bench UI, then a banner above the page content says the station is in evaluation mode, before any job is started.
- Given the rendered banner in any state, when inspected, then it contains no button or link and nothing blocks starting a job.
- Given `cd apps/desktop && pnpm typecheck && pnpm test`, when run, then all pass including the new tests.

## Spec Change Log

- 2026-11-08 (implementation): `licenceBanner(info)` takes no `now` (the Approach's `now?`): `days_remaining` is computed by the server, so a client clock would only re-introduce a TS evaluation the Never rule forbids. The expiry date is the `YYYY-MM-DD` prefix of `not_after` (UTC), matching how `operator_message` prints dates. The component shows `LICENCE_UNKNOWN_BANNER` whenever the query is in error (after react-query's default retries), even if an earlier answer is cached: a stale "nothing to say" must not hide a station that stopped reporting. The unknown banner says only that certificates *may* be marked evaluation, making no claim about erasure, since story 5 may refuse jobs under `stop`.

- 2026-11-08 (review): review found the matrix's "today"/"tomorrow" and "Licence not valid" wording inaccurate (Blind/Edge rows). Amended the I/O matrix rows and added autonomous decision 5; the code was patched in place, not re-derived, since the only change is these strings. Known-bad state avoided: a banner naming expiry a day early or blaming the licence for a broken lease file. KEEP: thresholds 30/7/1, tones, stop line, unknown-on-error rule.

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 25 findings — high 0, medium 6, low 15, false 4, maybe-false 0
- findings:
  - `[medium]` `[patch]` Blind: "today"/"tomorrow" disagree with the printed date because days_remaining rounds down — headings now say "within a day"/"in 1 day" and dates are labelled UTC.
  - `[low]` `[defer]` Blind: a stop licence's post-expiry banner contradicts the pre-expiry refusal warning — accurate until story 5 builds refusal; story 5 owns ADR-0012 §6's after-expiry wording; deferred.
  - `[low]` `[reject]` Blind: the stop warning promises unbuilt enforcement and is a copy decision — the wording is mandated by the approved SPEC CAP-3 and ADR-0012 §6, recorded as autonomous decision 3; the fix would edit this spec.
  - `[low]` `[defer]` Blind: no warning as an erasure quota nears exhaustion — not in CAP-3 or ADR-0012 §6; new behaviour; deferred.
  - `[low]` `[patch]` Blind: ROADMAP L1.5 note names install as the remaining work — reworded to the stop refusal (story 5). Striking L1.4/O3.5 kept: the bench status surface shipped; lease counts were already deferred in story 1.
  - `[medium]` `[patch]` Blind: the cached-data error path is untested — added a test seeding valid data then an error, asserting "Licence status unknown". The transient-blip cost is autonomous decision 2's accepted trade-off.
  - `[low]` `[reject]` Blind: ACs thin; status in-review with empty triage log — the fix edits this spec; the matrix covers the behaviours and the log is written by this pass.
  - `[low]` `[patch]` Blind: docs do not say the always-unlicensed desktop shell always shows the banner — CONTEXT §10 row says so.
  - `[low]` `[reject]` Blind: danger states should use role="alert" — the intent contract fixes role="status"; the banner is persistent information, not an interrupting alert; the fix edits this spec.
  - `[low]` `[reject]` Blind: CONTEXT row names story numbers — CONTEXT already cites story numbers throughout (Lapse policy glossary names story 5), and story 5 must update this row under AGENTS.md's docs rule.
  - `[low]` `[patch]` Blind: relative import between `@/` imports in Layout — moved after them.
  - `[low]` `[patch]` Blind: refetchOnWindowFocus overrides the app default without explanation — comment added.
  - `[false]` `[reject]` Blind: `key={line}` could collide — every view has exactly one line.
  - `[medium]` `[patch]` Edge: "expires today (2026-12-08)" a day early — same root and patch as the Blind today/tomorrow row.
  - `[low]` `[patch]` Edge: UTC date read as local date — dates labelled UTC, matching the server's operator_message dates.
  - `[low]` `[defer]` Edge: no low-quota warning — same as the Blind quota row; deferred.
  - `[medium]` `[patch]` Verification gap: unknown banner untested with cached data — same group as the Blind cached-data row; test added.
  - `[medium]` `[defer]` Verification gap: nothing checks Layout mounts the banner — needs a Layout render harness; deferred.
  - `[medium]` `[defer]` Intent: Layout wiring, polling and the Tauri path are untested at the live surface — same root as the Layout-mount row; deferred.
  - `[false]` `[reject]` Intent: the sign-in modal may cover the banner before the first job — no job can be started before sign-in; once signed in the banner is visible above every page.
  - `[low]` `[defer]` Intent: stop after-expiry clause (R3b) unmet — same as the Blind post-expiry row; story 5.
  - `[low]` `[reject]` Intent: pre-expiry stop warning untruthful today — same as the Blind enforcement row; SPEC-mandated.
  - `[false]` `[reject]` Intent: expiry warnings omit the operator message (R4b) — CAP-3 ties "with the operator message" to non-Valid statuses.
  - `[false]` `[reject]` Intent: thresholds instead of exact days — recorded autonomous decision 1, not a defect.
  - `[low]` `[patch]` Intent: "Licence not valid" heading is wrong for lease_state_unavailable — heading changed to one true for all six statuses.

## Design Notes

```ts
if (s.status !== "valid") return { tone: s.status === "unlicensed" ? "warning" : "danger",
  heading: ..., lines: [info.operator_message] };
if (s.days_remaining > EXPIRY_NOTICE_DAYS) return null;
const tone = s.days_remaining <= EXPIRY_FINAL_DAYS ? "danger" : "warning";
```

`EXPIRY_URGENT_DAYS` changes the heading's urgency ("Licence expires in 7 days — renew now"), not only the tone, so all three constants have an observable effect.

## Verification

**Commands:**
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: clean; new tests pass.
- `cd apps/desktop && pnpm build` -- expected: builds.

## Auto Run Result

Status: done

**Summary:** Every bench page now carries a persistent, information-only licence banner between the header and the page (`LicenceBanner` in `Layout`, polling `GET /api/license` each minute and on window focus). Unlicensed: amber "Evaluation mode — no licence installed" with the server's message. Any other non-valid status: red "Licensed signing unavailable — certificates are marked evaluation" with the server's message. Valid: nothing above 30 days; amber warning from 30 days naming the UTC expiry date, "renew now" from 7, red from 1 ("in 1 day", "within a day"); the line under it says new jobs will be refused at expiry under the `stop` lapse policy, else that erasure continues with evaluation certificates. Route failure: "Licence status unknown", even over a cached answer. No button, link or dismiss. No Rust change.

**Files changed:**
- `apps/desktop/src/api/licence.ts` -- `licenceBanner`, the 30/7/1 constants, `LICENCE_UNKNOWN_BANNER`.
- `apps/desktop/src/api/licence.test.ts` -- 16 tests over every matrix row and boundary.
- `apps/desktop/src/components/LicenceBanner.tsx` -- the polled bar.
- `apps/desktop/src/components/LicenceBanner.test.ts` -- 6 render tests (`renderToStaticMarkup`), incl. error over cached data.
- `apps/desktop/src/components/Layout.tsx` -- mounts the banner.
- `CONTEXT.md`, `README.md`, `AGENTS.md`, `docs/ROADMAP.md` -- §10 row and §11 paragraph, licensing paragraph and test table (291), L1.4/O3.5 struck, L1.5 annotated.

**Review:** 25 findings (high 0, medium 6, low 15, false 4). Patched entries: two medium (today/tomorrow vs UTC date, 2 rows; untested error-over-cache rule, 2 rows) and six low (UTC label, lapsed heading for a broken lease file, ROADMAP L1.5 note, desktop-shell doc clause, import order, focus-refetch comment). Deferred: stop post-expiry wording (story 5), low-quota warning, Layout mount untested (no harness). Rejected: the SPEC-mandated `stop` refusal wording (2 rows), thin ACs (spec edit), `role="alert"` (contract fixes `role="status"`), story numbers in CONTEXT (existing convention), and four false findings (key collision, sign-in modal, operator message on valid warnings, thresholds). The patch changed two strings the intent matrix pinned; recorded as autonomous decision 5 and the matrix amended, patched in place rather than re-derived. Patched entries by verdict: high 0, medium 2, low 6. Follow-up review: recommended — two medium entries patched; the unverified risk is the banner's date/day wording and error-over-cache rule, never seen in a running bench UI.

**Verification:** `pnpm typecheck` clean; `pnpm test` 291 passed; `pnpm build` succeeds. Not opened in a browser or the Tauri shell.

**Residual risks:** The `stop` warning promises refusal at expiry that story 5 has not built (decision 3). A transient route failure shows "Licence status unknown" until the next good poll. Nothing tests that `Layout` mounts the banner. The desktop shell, always unlicensed, always shows the amber bar.
