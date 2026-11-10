---
title: 'Retire the lapse-policy setting and the never-blocks claims'
type: 'refactor'
created: '2026-11-10'
status: 'done'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '{project-root}/AGENTS.md'
  - '{project-root}/docs/adr/0013-licensing-controls-every-erasure.md'
warnings: ['oversized']
deferred:
  - summary: >-
      The station bench app still tells customers that licensing never stops erasure and shows a licence's lapse policy.
    evidence: |-
      apps/desktop/src/api/licence.ts lapsePolicyLine shows "When it expires, erasure continues, but certificates will be marked evaluation until a renewed licence is installed." (or the stop wording) in the licence banner, and GET /api/license still reports lapse_policy. CAP-2 assumes stations already ignore the field via licence-visibility story 5, which is ready-for-dev, not built; that story's plan removes lapsePolicyLine and the API field. Station code was outside this story's scope.
    location: >-
      apps/desktop/src/api/licence.ts:52
    severity: medium
baseline_revision: '8884742446c54d778276a60fbcf2fb570f4080ac'
---

<intent-contract>

## Intent

**Problem:** ADR-0013 §5 retires the customer lapse policy (every licence now behaves as `stop`), and ADR-0013 makes "Licensing never blocks erasure" false. Yet the hub still stores, audits and signs an organization's policy into licences, the portal's Stations page lets an owner switch it, and the marketing site, README and CONTEXT still promise that licensing never blocks erasure.

**Approach:** Remove the setting end to end at the hub and portal (routes, module, reissue-on-policy-change, response fields, UI) so new licences omit `lapse_policy`, while leaving the stored column, applied migration, audit history and `wipe-license` parsing untouched. Replace the site capability and its three page copies with the CAP-2 sentence, at a status that matches what has shipped, and bring README, CONTEXT, ARCHITECTURE and ROADMAP into line.

## Boundaries & Constraints

**Always:**
- The hub signs every licence (station register, reissue, vendor issue) with no `lapse_policy` key: `VendorRoot::issue` (default `Evaluation`, skipped on serialize), never `issue_with_lapse_policy`. `licence.issued` audit detail drops `lapse_policy`.
- `GET`/`POST /api/organization/lapse-policy` are gone: both answer the hub's ordinary JSON 404 for unknown `/api/*` paths. `crate::lapse_policy` and `MSG_OWNER_ONLY_LAPSE_POLICY` are deleted.
- Reissue is allowed only when `renewal_available` (licence behind the entitlement term); `lapse_policy_update_available` and the `"lapse policy changed"` supersede reason are removed. Otherwise 409 `MSG_LICENCE_UP_TO_DATE` as today.
- Hub response shapes drop `lapse_policy` (station `Licence`, vendor `AdminLicence`, admin organization detail) and `lapse_policy_update_available` (station). Portal TS mirrors and guards change in the same commit and no longer require these fields.
- Kept as is: `organizations.lapse_policy` column and migration `0016` (never edit an applied migration; no new migration), existing `org.lapse_policy_changed` audit rows, `wipe_license::LapsePolicy` and `issue_with_lapse_policy` (files carrying `stop` still parse, ADR-0013 §5), the portal's vendor-only `LicenseBody` mirror accepting an optional `lapse_policy` (parsed, not displayed).
- Site: capability `licence-never-blocks-erasure` is replaced by `licence-decides-at-start`, label `Licensing decides at the start, never mid-erasure`, status `planned` (the start gate, licence-visibility story 5, has not shipped at this commit). On product, pricing and enterprise-IT pages the item reads, inside `<Capability id="licence-decides-at-start">`: heading **Licensing decides at the start, never mid-erasure.** and body "A station checks its licence before each job starts. A job that started covered is always certified, however long it runs, and a station with a valid licence never needs a network connection to keep working." verbatim.
- The retired claims ("never blocks/stops … erasure", "still runs every erasure", "A licence problem never stops a refresh") appear on no built page; a site test pins this.
- Docs say what is true at this commit: the hub and portal no longer offer the setting; stations do not yet refuse starts (that is licence-visibility story 5), so docs must not claim either "never blocks" or "already refuses".

**Never:**
- No station-side change: `crates/wipe-server`, `apps/desktop`, `crates/wipe-cli` and their lapse-policy code and tests belong to licence-visibility story 5.
- No change to `wipe-license`'s signed shapes or its fixtures; no edit to applied migrations; no deleting rows or audit history.
- No credit pool, grant, renewal or trial work (other stories). No weakening of any existing site guarded phrase other than removing the retired capability.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Register station | owner, current entitlement, org row `lapse_policy = 'stop'` | 201; licence file body has no `lapse_policy`; installs as `Evaluation`; response `licence` has no `lapse_policy`, `station` no `lapse_policy_update_available` | — |
| Vendor issue | vendor admin issues for an org with `stop` stored | file and `AdminLicence` without `lapse_policy`; audit `licence.issued` detail without it | — |
| Old route | `GET` or `POST /api/organization/lapse-policy` (any session or none) | JSON 404 like any unknown `/api/*` path | nothing written |
| Reissue, policy only | active licence signed with `stop`, term unchanged | 409 `MSG_LICENCE_UP_TO_DATE` | nothing written |
| Reissue, renewal | licence behind term | 201 as today, reason `renewed`, new file without `lapse_policy` | — |
| Admin org detail | vendor admin views an organization | no lapse-policy field or row | — |
| Portal /stations | any member | no lapse-policy section, panel or "lapse policy changed" text; renewal reissue unchanged | — |
| Site build | `pnpm --filter www build` | passes site check; three pages show the new item with a Planned badge; no retired claim anywhere | build fails if copy leaks unwrapped |

## Autonomous decisions

- **Response fields removed, not kept.** *About:* what the hub API and portal show about a licence's lapse policy. *Problem:* the setting is retired, but old files could carry `stop`. *Options:* (A) keep `lapse_policy` read-only in responses; (B) drop it everywhere customers or admins read it, keep the stored column and parsing. *Choice:* B: nothing has been sold (ADR-0013 §5), so no real file carries `stop`, and keeping a field for a retired setting invites confusion. Reversible.
- **Stored column kept.** Dropping `organizations.lapse_policy` deletes data and needs a migration for no gain; the SPEC says existing rows stay. The column is simply no longer read.
- **Capability status `planned`.** CAP-2 requires the status to match what has shipped; the start check is licence-visibility story 5 (not built), so the least-overclaiming status applies. A later story promotes it.
- **Docs for station behaviour.** README/ARCHITECTURE/CONTEXT lines that say licensing never stops an erasure are reworded to "today's stations do not refuse starts on licensing; ADR-0013 makes the licence decide at each start (licence-visibility story 5)", so they are true now and stop promising the retired rule. Story 5 rewrites them when it ships.

</intent-contract>

## Code Map

- `crates/wipe-hub/src/lapse_policy.rs` (whole module), `src/lib.rs:29` -- delete.
- `crates/wipe-hub/src/station_routes.rs:6-33` (module doc), `:52-55` imports, `:67-68` route, `:85-94` `From<LapsePolicyError>`, `:244-273` handlers -- remove route and handlers.
- `crates/wipe-hub/src/stations.rs` -- module doc `:12`, `:42-52`; `:87` `LapsePolicy` import; `:111-112` `MSG_OWNER_ONLY_LAPSE_POLICY` and the register-owner message mentioning the policy (`:111`, reword without it); `:157-183` `Licence.lapse_policy` and `licence_lapse_policy()`; `:219-282` `Station.lapse_policy_update_available` and row mapping; `:305-339` `licence_policy_differs()` and its SQL columns; `:601-672` `issue_licence`/`sign_and_store` (drop org `FOR SHARE` policy read, call `issue`, drop audit key); `:895-990` `reissue` (renewal only; drop the org join/lock that existed only for the policy compare).
- `crates/wipe-hub/src/licence_admin.rs:252-323` -- drop `AdminLicence.lapse_policy`, row field and SQL column.
- `crates/wipe-hub/src/admin.rs:182,347-394` -- drop `lapse_policy` from organization detail and its SELECT.
- `crates/wipe-hub/tests/stations.rs` (`:22-26` imports, `:220-250` policy helpers, key lists `:323`, `:341`, fixtures `:457`, `:1223`, section `:1415-1761`), `tests/licence_admin.rs` (`:388`, `:646`, `:1157-1240`), `tests/isolation.rs:172-173,562` -- remove policy tests; replace with: register/reissue/vendor-issue files omit `lapse_policy` even when the org row holds `stop` (set directly in SQL), old routes 404, policy-only reissue is 409. `tests/migrations.rs:836` stays (schema unchanged).
- `apps/portal/src/api/stations.ts` -- drop `LAPSE_POLICIES`/`LapsePolicy` exports only if no remaining user; drop `LAPSE_POLICY_LABELS`, `Licence.lapse_policy`, `Station.lapse_policy_update_available`, `isPolicyBody`, `fetchLapsePolicy`, `setLapsePolicy`; header comment.
- `apps/portal/src/api/adminLicences.ts:14-198` -- `AdminLicence.lapse_policy` removed from type and guard; `LicenseBody.lapse_policy?` stays optional (move the allowed-values list here if `stations.ts` loses it).
- `apps/portal/src/api/admin.ts:21,62,122` -- drop org detail `lapse_policy`.
- `apps/portal/src/pages/StationsPage.tsx:7-17,79-80,100-103,139,199-381` -- remove `LapsePolicySection`, `LapsePolicyPanel`, `LapsePolicyText`, policy-only reissue wording; reissue shown only for `renewal_available`.
- `apps/portal/src/pages/AdminLicencePage.tsx:20,110,153`, `AdminOrganizationPage.tsx:18,88` -- drop the lapse-policy rows.
- Portal tests: `api/stations.test.ts`, `api/admin.test.ts`, `api/adminLicences.test.ts`, `api/enrollments.test.ts`, `pages/StationsPage.test.ts`, `pages/AdminLicencePage.test.ts`, `pages/ActivatePage.test.ts` -- update fixtures; assert guards accept bodies without the fields and the Stations page renders no lapse-policy text.
- `sites/www/src/data/capabilities.ts:277-290` -- replace the capability; guarded phrases matching its label/summary (e.g. `/\bdecides at the start\b/i`, `/\bnever mid-erasure\b/i`).
- `sites/www/src/pages/product.astro:159-164`, `pricing.astro:53-58`, `segments/enterprise-it.astro:55-60` -- new copy.
- `sites/www/test/` (`pages.test.ts`, capabilities tests) -- retired claims absent from every built page; new item present with Planned badge on the three pages.
- Docs: `CONTEXT.md` §10.2 row at `:1020` (mark retired, column kept, routes removed), §10.2 new shipped row, glossary `:1485`, `:1620-1630` (**Lapse policy** → retired by ADR-0013 §5), `:1114`, `:1476`; `README.md` hub route list/`:65`, `:520`, `:529`; `docs/ARCHITECTURE.md:357`; `docs/ROADMAP.md:343`.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/src/{lapse_policy.rs,lib.rs,station_routes.rs,stations.rs,licence_admin.rs,admin.rs}` -- remove the setting and fields per Code Map; licences signed without the policy.
- `crates/wipe-hub/tests/{stations,licence_admin,isolation}.rs` -- delete policy tests, add the matrix rows over HTTP.
- `apps/portal/src/api/*.ts`, `apps/portal/src/pages/*.tsx` and tests -- remove types, guards, calls and UI; update fixtures.
- `sites/www/src/data/capabilities.ts`, the three pages, `sites/www/test/*` -- new capability, copy and tests.
- `CONTEXT.md`, `README.md`, `docs/ARCHITECTURE.md`, `docs/ROADMAP.md` -- document what changed in the same commit.

**Acceptance Criteria:**
- Given an organization whose stored `lapse_policy` is `stop`, when its owner registers a station through the hub, then the downloaded licence file has no `lapse_policy` key and verifies with `wipe_license` as `Evaluation`.
- Given the portal is built, when any member opens `/stations`, then no lapse-policy setting, text or reissue reason is rendered, and renewal reissue still works.
- Given the site is built, when the product, pricing and enterprise-IT pages render, then each shows "Licensing decides at the start, never mid-erasure." with the CAP-2 body and a Planned badge, and no page contains a retired never-blocks claim.
- Given `cargo test --workspace`, portal and site suites, when run, then all pass.

## Spec Change Log

## Review Triage Log

### 2026-11-10 — Review pass
- verdicts: 24 findings — high 0, medium 6, low 13, false 5, maybe-false 0
- findings:
  - `[medium]` `[patch]` Blind: the build-time site check no longer guards the retired never-blocks claim (its guarded phrases left with the capability) — grouped with the next two rows and VG2/Intent I5: the vitest guard now uses the old pattern (with `gate`, a `GAP(30)` gap), adds the "marking changes, to evaluation" phrasings, and a new test requires every `.astro` page to be in the checked `pages` list.
  - `[medium]` `[patch]` Blind: `RETIRED_LICENSING_CLAIMS` is narrower than the pattern it replaced (no `gate`, `[^.]` gap, evaluation-marking half unpinned) — same patch as above.
  - `[low]` `[reject]` Blind: `CERTIFICATION` guarded by a non-certification capability — the body's "certified" is the SPEC's verbatim copy and the whole element carries its own Planned badge; changing it means editing owner copy.
  - `[low]` `[patch]` Blind: CONTEXT glossary says every licence "now behaves as `stop`" though stations do not enforce yet — reworded to "will behave as `stop` once the start check ships (licence-visibility story 5)". The ADR-0005 decision note reads as a decision, not shipped behaviour.
  - `[low]` `[patch]` Blind: ROADMAP L1.5 still framed as ADR-0012's drop-to-evaluation and `stop` refusal — retitled to the ADR-0013 framing. T0.5 is a struck historical decision row and stays.
  - `[low]` `[patch]` Blind: portal guard tests only prove fixtures lack the retired keys — added positive cases that `isLicence`, `isStation`, `isAdminLicence` and the organization detail accept bodies still carrying them.
  - `[false]` `[reject]` Blind: no way to replace an active `stop`-signed licence before renewal — under ADR-0013 every licence behaves as `stop`, so a `stop`-signed file behaves exactly like any other; nothing has been sold (ADR-0013 §5).
  - `[low]` `[reject]` Blind: vendor admins can no longer see a file's lapse policy — the recorded autonomous decision removes it; reverting means editing the spec, and no sold file carries `stop`.
  - `[low]` `[reject]` Blind: the air-gap guarantee sits under a Planned badge — it under-claims (safe direction), and the sentence is the SPEC's verbatim copy for the owner to approve.
  - `[low]` `[reject]` Blind: the new `<h3>` ends with a full stop — the SPEC gives the heading verbatim with the period; the owner reviews this copy at the checkpoint.
  - `[low]` `[reject]` Blind: story file drift (verification grep names stations.ts; empty logs) — logs are filled by this pass; the grep note's fix edits the spec.
  - `[low]` `[reject]` Blind: `/stations` and admin pages not seen in a browser — the change is deletion-only UI covered by render tests; recorded as a residual risk.
  - `[false]` `[reject]` Edge: reissue cannot replace a `stop`-signed licence before renewal — same refutation as the Blind row above.
  - `[low]` `[reject]` Edge: a portal tab loaded before deploy fails its guard against the new hub until reload — pre-launch, a reload fixes it; forcing reloads adds machinery for a one-off deploy moment.
  - `[low]` `[reject]` Edge: the admin licence record no longer marks `stop`-signed licences — same as the Blind vendor-admin row (spec decision).
  - `[false]` `[reject]` Edge: owners cannot tell which stations carry `stop`-signed files — no file is signed with `stop` now and every licence behaves as `stop` under ADR-0013, so there is nothing to distinguish.
  - `[medium]` `[patch]` VG: the policy-only reissue test sets org and file both to `stop`, so the removed `policy_differs` rule would also give 409 — the test now leaves the org at `evaluation` against the `stop`-signed file and asserts the stored value.
  - `[medium]` `[patch]` VG: retired claims are no longer caught by the build-time check and the vitest guard covers only the hand-listed pages — grouped with the Blind site-check rows; page-list completeness test added.
  - `[medium]` `[defer]` Intent: the station bench app still says erasure continues past expiry and shows the lapse policy — station code is licence-visibility story 5's (CAP-2 itself assumes story 5 handles stations); deferred with evidence.
  - `[low]` `[patch]` Intent: CONTEXT renewal-reissue row still says reissue carries a changed lapse policy — now says reissue is for renewals only since ADR-0013 §5.
  - `[false]` `[reject]` Intent: CONTEXT lease-file line says a lease failure never stops an erasure — that is accurate as-built station behaviour today, about lease I/O, not a licensing promise.
  - `[low]` `[reject]` Intent: portal tests render pieces, not the whole routed page — the removed section is gone from the source and the row/notice pieces are rendered; a router harness adds setup for a deletion.
  - `[medium]` `[patch]` Intent: the site test's retired-claim pattern differs from the old guarded phrase — same patch as the Blind site-check rows.
  - `[false]` `[reject]` Intent: owner approval of the finished story is not satisfied by the diff — that is a human checkpoint outside any diff; flagged in the Auto Run Result.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass (do not pipe through grep).
- `cargo test --workspace` -- expected: all pass.
- `cargo clippy -p wipe-hub --all-targets -- -D warnings` -- expected: clean.
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: pass.
- `pnpm --filter www typecheck && pnpm --filter www test && pnpm --filter www build` -- expected: pass, site check 0 violations.
- `git grep -n -i "lapse" -- crates/wipe-hub/src apps/portal/src sites/www/src` -- expected: only the kept items (stations.ts/adminLicences.ts optional body parse, comments citing ADR-0013).
- `rustfmt --edition 2021 <touched .rs files>` -- only touched files.

## Auto Run Result

Status: done

**Summary.** The organization lapse-policy setting is gone from the hub and the portal (ADR-0013 §5). `GET`/`POST /api/organization/lapse-policy` now return the ordinary JSON 404. Every licence the hub signs (station register, renewal reissue, vendor issue) omits `lapse_policy`, and so does the `licence.issued` audit detail. Reissue is for renewals only. Hub responses and portal mirrors drop `lapse_policy` and `lapse_policy_update_available`. The stored column, migration 0016, audit history and `wipe_license::LapsePolicy` parsing all stay. On the marketing site, the "Licensing never blocks erasure" capability becomes `licence-decides-at-start` (status `planned`). The product, pricing and enterprise-IT pages carry the CAP-2 heading and body verbatim. README, CONTEXT, ARCHITECTURE and ROADMAP now say the setting is retired and that stations do not refuse starts until licence-visibility story 5.

**Files changed.**
- `crates/wipe-hub/src/lapse_policy.rs` (deleted), `lib.rs`, `station_routes.rs`: the module, routes and handlers are removed.
- `crates/wipe-hub/src/stations.rs`: signing through `VendorRoot::issue`, no policy in responses or audit, renewal-only reissue.
- `crates/wipe-hub/src/licence_admin.rs`, `admin.rs`: `lapse_policy` removed from the admin licence and the organization detail.
- `crates/wipe-hub/tests/{stations,licence_admin,isolation}.rs`: the policy tests are replaced by matrix tests, with `stop` stored directly in SQL.
- `apps/portal/src/api/{stations,adminLicences,admin}.ts` and the pages `StationsPage`, `AdminLicencePage`, `AdminOrganizationPage`, plus tests: types, guards, the setting UI and the policy-only reissue are removed.
- `sites/www/src/data/capabilities.ts`, `pages/{product,pricing,segments/enterprise-it}.astro`, `test/pages.test.ts`: the new capability and copy; a retired-claim test over every page.
- `CONTEXT.md`, `README.md`, `docs/ARCHITECTURE.md`, `docs/ROADMAP.md`: documentation.

**Review.**
- 24 findings: medium 6, low 13, false 5.
- 6 entries patched: 2 medium, 4 low.
  - Medium: the retired-claim site test now uses the old pattern and covers every page; the policy-only reissue test can now catch the removed rule.
  - Low: the CONTEXT glossary tense, the CONTEXT reissue row, ROADMAP L1.5, and portal guard tolerance tests.
- 1 deferred (medium): the station bench banner still says erasure continues past expiry. This belongs to licence-visibility story 5.
- 12 rejected: 7 low and 5 false, each with its reason in the Review Triage Log.

**Follow-up review: recommended.** Two medium patches landed after the review and were not re-reviewed:
- the strengthened retired-claim site test, including the new page-list completeness test;
- the reworked reissue regression test.

Both pass, but no reviewer has checked that they catch what they are meant to catch.

**Verification.**
- `cargo test --workspace --no-fail-fast` (before the review patches): 1060 passed, 1 failed. The failure is `wipe-hub` `health::silent_smtp_and_database_time_out_within_bound`, which took about 27 s against a 3.5 s limit. This change touches no health, config, mail or database code, and story 1 recorded the same test failing on its untouched baseline.
- After the patches:
  - `cargo test -p wipe-hub --test stations`: 29 passed.
  - `cargo clippy -p wipe-hub --all-targets -D warnings`: clean.
  - `rustfmt --check` on the touched files: clean.
  - Portal: typecheck and 274 tests pass.
  - Site: typecheck, 291 tests and the build pass; the site check reports 13 pages and 0 violations.
- Matrix audit: every row is covered by a hub HTTP test, a portal render test or a site test that ran and passed.

**Residual risks.**
- **Owner approval needed:** the CAP-2 site wording and the `planned` status are agent-placed and await the owner's approval of the finished story (CAP-2 checkpoint).
- The station bench app still shows the lapse policy and says erasure continues past expiry (deferred to licence-visibility story 5).
- `MSG_LICENCE_UP_TO_DATE` is shortened to drop the lapse-policy clause: the customer-visible error text changed.
- The portal changes were not checked in a real browser.
- The `wipe-hub` health timing test fails on this machine whether or not this change is applied.
