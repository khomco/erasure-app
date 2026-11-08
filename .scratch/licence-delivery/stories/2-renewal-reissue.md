---
title: 'Renewal reissue'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '7ac96727773be7ee6b40b2e3cdaf78c136fe1abd'
review_loop_iteration: 0
followup_review_recommended: false
context:
  - '{project-root}/docs/adr/0005-licensing-and-attestation.md'
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
warnings: [oversized]
deferred:
  - summary: >-
      Whether renewing an annual entitlement restores a count quota (per term) or keeps one count for the life of the subscription is undecided.
    evidence: |-
      The hub copies quota_erasures unchanged into every licence, renewed ones included, because a licence mirrors its entitlement. Stations count erasures locally (LeaseState, created fresh at startup in crates/wipe-cli/src/main.rs:752) and not per licence id, so what a renewal does to the count is station-side behaviour, not this story's. It is a pricing and product choice for the owner.
    location: >-
      crates/wipe-hub/src/stations.rs (licence_entitlements)
    severity: medium
---

<intent-contract>

## Intent

**Problem:** When an annual entitlement renews (offerings-and-payments story 4), its `term_end` moves out by 12 months, but every station's licence still names the old `not_after`. When that date passes, the stations start marking certificates `evaluation` even though the customer has paid. Today the only fix is to release the station and register it again (SPEC CAP-3).

**Approach:** The hub marks a station whose active licence ends before its entitlement's current `term_end` as having a renewed licence available. An owner presses "Issue renewed licence" on the portal's Stations page. The hub then signs a fresh licence for the same station (same key, scope, fingerprint or site, quota and features, `not_after` = new `term_end`), marks the old one `superseded` (reason `renewed`), and offers the new file for download. The station keeps its seat and its registration.

## Boundaries & Constraints

**Always:**
- Sign the reissued licence the same way `stations::register` does: factor the shared code into one helper so the two paths cannot drift. That covers the `Entitlements` build, `VendorRoot::issue`, the `to_string_pretty` document, microsecond `now`, the licence insert and the `licence.issued` audit row. Machine scope uses `machine_fingerprint(<stored station_id>)` and site scope uses the stored `site`, or the organization id when none was stored.
- Lock the station and its entitlement `FOR UPDATE` in one statement, scoped to the session's organization. Supersede the old active licence, insert the new one and write the audit rows in that transaction. Concurrent reissues of one station must yield exactly one new licence.
- `renewal_available` on every `Station` = not released, entitlement current (not refunded, `term_end` > now), and the latest licence `active` with `not_after` < the entitlement's `term_end`. It is computed in SQL in `select_stations`, never stored. A reissue is allowed exactly when it is true.
- Audit (actor `user:<id>`): `licence.superseded {station_id, reason: "renewed", superseded_by: <new id>}` and `licence.issued` with story 1's detail plus `reissue_of: <old id>`.
- The old licence file keeps verifying offline until its own `not_after`. The page says so, and also says that the station uses the renewed licence only after it is installed.
- ROADMAP §0.2: nothing here can stop a station erasing. No vendor root → 503 `MSG_ISSUANCE_UNAVAILABLE` and nothing is written.

**Never:** no reissue inside the payment-event or renewal transaction; no station-side code; no push or online fetch of licences (D6); no admin pages, revocation or lapse policy (stories 3–4); never edit an applied migration (none is needed).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Renewal shown | machine station registered, then Fake "renew" on its entitlement | `GET /api/stations`: `renewal_available: true`; before the renewal `false` | — |
| Reissue | owner, `POST /api/stations/:id/reissue` | 201 `{station, licence}`; new licence `active`, `not_after` = new `term_end`, same key/fingerprint/features/quota; installs and evaluates `Valid`; old licence `superseded` and still downloadable byte-identical; station `renewal_available: false`; seat count unchanged; 2 audit rows | — |
| Site station | site entitlement station reissued (state forced by extending `term_end` in SQL) | same site id (entered or org id) | — |
| Up to date | licence already ends at `term_end` | 409 `MSG_LICENCE_UP_TO_DATE` | nothing written |
| Released | released station | 409 `MSG_ALREADY_RELEASED` | nothing written |
| Not current | entitlement refunded or ended | 409 `MSG_ENTITLEMENT_NOT_CURRENT` | nothing written |
| Unknown / foreign / non-UUID | | 404 `MSG_NO_STATION` | nothing written |
| Member | role member | 403 `MSG_OWNER_ONLY_STATIONS` (wording now covers renewing) | nothing written |
| No vendor root | | 503 `MSG_ISSUANCE_UNAVAILABLE` | nothing written |
| Concurrent | 8 parallel reissues of one station | exactly one 201, rest 409 up-to-date; one active licence | — |

## Autonomous decisions

- **When the renewed licence is signed.** When a paid renewal extends an entitlement, its stations need new licence files. Options: (a) sign them automatically inside the payment webhook's transaction; (b) mark them "renewal available" and sign on an owner's request. Chosen (b). The payment path stays free of signing, so a missing or broken vendor root can never make a renewal payment fail to apply. Nothing is superseded unless someone asks for it. The same mechanism serves story 4's reissue on a lapse-policy change. The cost is one click per station.
- **Who may reissue.** Chosen owners only, matching register and release (least privilege). Members see that a renewal is available and that an owner issues it. Every role can still download.
- **Reissue only when renewal is available.** A reissue at any other time would only churn licences, so it is refused with 409. Story 4 widens the condition for a changed lapse policy.
- **Quota.** The reissued licence carries the entitlement's quota unchanged, because the licence mirrors the entitlement exactly (story 1). Stations count usage locally, not per licence file at the hub.

</intent-contract>

## Code Map

- `crates/wipe-hub/src/stations.rs` -- the only writer of `stations`/`licences`. `select_stations` (l.183) has a LATERAL latest-licence join; add `renewal_available`. `LockedEntitlement` (l.340), `licence_entitlements` (l.363; takes `&NewStation`, refactor to take station_id/site), `register` (l.421; signing and insert block l.477-586 to extract), `release` (l.598, supersede pattern), messages (l.54-70), `RFC3339_UTC_MICROS` (l.353).
- `crates/wipe-hub/src/station_routes.rs` -- add `POST /api/stations/:id/reissue` (owner check, vendor-root 503 like `register`, non-UUID → 404 like `release`); update the module doc.
- `crates/wipe-hub/migrations/0014_stations.sql` -- read-only: `licences_active_station_idx` (one active licence per station) means supersede before inserting; the trigger allows only active→superseded.
- `crates/wipe-hub/src/entitlements.rs:304` `renew` -- read-only: extends `term_end`; its doc and CONTEXT say no licence is reissued there, which stays true.
- `crates/wipe-hub/tests/stations.rs` -- `Fixture` (`machine` = annual, renewable; `site` = one-off), `register_machine`, `download`, `install`, `term_end`, `counts`, `futures_join_all`. The exact key-set pin at l.236 gains `renewal_available`.
- `crates/wipe-hub/tests/entitlements.rs:121` -- Fake renewal: owner `POST /api/payments/fake/entitlements/{id}/renew`.
- `crates/wipe-hub/tests/isolation.rs:147-151,478,520-530` -- route table and per-route expectations; add the reissue route (owner 404, member 403 on a foreign id).
- `apps/portal/src/api/stations.ts` (+ `stations.test.ts`) -- `Station`, `isStation`, `releaseStation` pattern.
- `apps/portal/src/pages/StationsPage.tsx` (+ test) -- `stationState`, `StationRow`, `SupersessionNote`.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/src/stations.rs` -- extract the shared issue helper. Add `renewal_available` to `Station`/`StationRow`/`select_stations`, `MSG_LICENCE_UP_TO_DATE` and `reissue(conn, root, org, user, station_id) -> Registered`. Broaden `MSG_OWNER_ONLY_STATIONS` to "register, renew or release". Update the module doc.
- `crates/wipe-hub/src/station_routes.rs` -- add the route and its docs.
- `crates/wipe-hub/tests/stations.rs` -- one test per I/O row through HTTP, plus the concurrency test. Update the key-set pin.
- `crates/wipe-hub/tests/isolation.rs` -- add the route.
- `apps/portal/src/api/stations.ts` (+ test) -- `renewal_available` in the type and guard; `reissueStation(id)`.
- `apps/portal/src/pages/StationsPage.tsx` (+ test) -- state "Renewal available" when the flag is set. Owners get an "Issue renewed licence" button, followed by a download link. Members see "an owner can issue it". The note covers renewal: the old file works until its expiry, and the station needs the renewed file installed.
- `CONTEXT.md` §10 (new row; update story 4's and story 1's rows that say renewal reissue is pending), `docs/ROADMAP.md`, `README.md` if it lists hub routes, `AGENTS.md` test counts -- record what shipped.

**Acceptance Criteria:**
- Given a station licensed to term end T and a Fake renewal moving the term to T+12 months, when the owner reissues and downloads, then the new file installs with the vendor root and `evaluate` at a time after T (before T+12 months) is `Valid`, while the old file evaluates `Expired` at that time.
- Given the Stations page with a station whose renewal is available, when an owner views it, then a renewed licence can be issued and downloaded from that row, and the page states the old file keeps working until its own expiry.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 22 findings — high 0, medium 3, low 15, false 4, maybe-false 0
- findings:
  - `[low]` `[reject]` (blind) Renew button ignores whether the hub has a vendor root — that only happens if the root is removed after stations were registered, and the 503 still shows its own clear message ("licence issuance is unavailable…"). Fixing it would add a prop and a branch.
  - `[low]` `[patch]` (blind) A refused reissue leaves a stale button — `RenewalAction` now invalidates `["stations"]` in `onSettled`.
  - `[low]` `[patch]` (blind) The renewed-licence notice outlives a release, and duplicates the row's download link — the notice now shows only while the station is not released. The duplicate link is cosmetic and was left: the notice's link carries the install instruction.
  - `[low]` `[patch]` (blind) CONTEXT §14 does not describe supersession by renewal or Renewal available — the "Registered station" entry now describes both.
  - `[medium]` `[defer]` (blind) Whether a renewal restores a count quota is a pricing decision — the hub can only mirror the entitlement's quota; how a station counts usage across licences is station-side and was not caused by this story. Recorded in frontmatter `deferred`.
  - `[false]` `[reject]` (blind) A reissue after an organization rename carries a different `customer_name` while the docs promise "the same" — the docs list key, scope, quota and features, not the name; the licence names the organization as it is now, the same rule as at registration.
  - `[low]` `[patch]` (blind) Nothing tells owners a renewal is waiting — a notification would be new scope; the limitation is now in the CONTEXT §10 row's limitations cell.
  - `[low]` `[reject]` (blind) No hub test for a renewal that arrives after the old licence expired — `renewal_available` and `reissue` compare `not_after` with `term_end`, never with now, and `current` is already tested. The path is identical.
  - `[low]` `[reject]` (blind) `licence.issued` is audited before `licence.superseded` — both rows are in one transaction and point at each other (`reissue_of` / `superseded_by`); reordering them would mean moving id generation out of the shared helper, for a cosmetic gain.
  - `[false]` `[reject]` (blind) The second reissue query is not scoped to the organization, and an UPDATE matching no row is a 500 — the station id has just been looked up in the session's organization and locked `FOR UPDATE`. Release and other reissues take the same lock, so the active licence found cannot change before the UPDATE.
  - `[low]` `[reject]` (blind) The renewed notice does not repeat the install command — the page's install section, on the same page, holds the command; repeating it would add a second copy to maintain.
  - `[low]` `[patch]` (blind) Members read "Renewal available" twice in a row — the member text is now "An owner can issue the renewed licence."
  - `[low]` `[patch]` (edge) If the hub clock steps back, a superseded licence can rank as latest — `LATEST_LICENCE` now orders an active licence first.
  - `[low]` `[patch]` (edge) Stale button after a 409 — same root cause and fix as the blind row above.
  - `[low]` `[patch]` (edge) Renewed notice stays after a release — same root cause and fix as the blind row above.
  - `[low]` `[patch]` (edge) The `reissue` doc claims every refusal comes before any write — the doc now names the one refusal found while signing, which relies on the transaction rolling back.
  - `[medium]` `[patch]` (verification-gap) No test renders a station row, so the reissue wiring and the owner/member gating are unchecked — `StationRow` is exported and a row-level test covers owner and member.
  - `[medium]` `[patch]` (intent) The row wiring and the click path are not exercised at the portal surface — same root cause and fix as the verification-gap row.
  - `[low]` `[reject]` (intent) No test renews an entitlement that has several stations — the flag is computed per station in one SQL expression, and the site test already lists two stations.
  - `[false]` `[reject]` (intent) The on-request reading is recorded only in the story, not in DECISIONS.md or an ADR — AGENTS.md lists the story's intent contract as a place to record autonomous decisions.
  - `[low]` `[patch]` (intent) The old file is never shown to keep working before its expiry after a reissue — the main reissue test now evaluates the old file at now and expects `Valid`.
  - `[false]` `[reject]` (intent) The Stripe renewal path is not exercised for the flag — the flag reads the state (`not_after < term_end`), not the trigger, and Stripe renewals extend `term_end` through the same `entitlements::renew` that the Fake test drives.

## Design Notes

Lock with `SELECT … FROM stations s JOIN entitlements e … JOIN orders o … WHERE s.organization_id = $1 AND s.id = $2 FOR UPDATE OF s, e`. Register locks only `e` and release locks only `s`, so no lock cycle arises. Check in this order: unknown → 404, released → 409, not current → 409, latest licence `not_after` ≥ `term_end` → 409 up-to-date. Then `UPDATE licences SET status='superseded' … RETURNING id` before the insert (unique active index). Compare `not_after` with `term_end` as timestamps in SQL, not as strings.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including new reissue tests
- `cargo test --workspace` -- expected: pass
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean
- `cd apps/portal && pnpm typecheck && pnpm test && pnpm build` -- expected: pass

## Auto Run Result

Status: done

**Summary.**
- After an annual entitlement renews, every station still on it shows "Renewal available" on the portal's Stations page (`renewal_available` in `GET /api/stations`, computed in SQL).
- An owner presses "Issue renewed licence". The hub then signs a fresh licence for the same key, scope, quota and features, valid to the new term end, and supersedes the old one (reason `renewed`).
- The station keeps its seat and its registration.
- The page offers the new file for download. It says the station uses it only once it is installed, and that the old file keeps working until its own expiry.
- Nothing is signed in the payment path.
- Register and reissue now sign through one helper.

**Files changed**
- `crates/wipe-hub/src/stations.rs` — the shared `issue_licence` helper, `renewal_available`, `reissue`, `MSG_LICENCE_UP_TO_DATE`, and the broadened owner-only message.
- `crates/wipe-hub/src/station_routes.rs` — `POST /api/stations/:id/reissue`.
- `crates/wipe-hub/tests/stations.rs` — 7 reissue tests covering every I/O row through HTTP, including concurrency. `tests/isolation.rs` covers the new route.
- `apps/portal/src/api/stations.ts` (+ test) — `renewal_available` and `reissueStation`.
- `apps/portal/src/pages/StationsPage.tsx` (+ test) — renewal state, `RenewalAction`, `RenewedNotice`, the updated note, and the exported `StationRow`.
- `CONTEXT.md`, `docs/ROADMAP.md`, `README.md`, `AGENTS.md` — what shipped, the §14 glossary entry, the limitations, and the test count (589).

**Review.** 22 findings: high 0, medium 3, low 15, false 4.
- **Patched (11 rows in 8 fixes; entry verdicts: 1 medium, 7 low):**
  - the stale button after a refusal;
  - the notice shown after a release;
  - the member wording;
  - the latest-licence ordering under clock steps;
  - the `reissue` doc;
  - the row-level portal test;
  - the old file shown `Valid` before its expiry;
  - the CONTEXT §14 entry and the no-notification limitation.
- **Deferred (1):** whether a renewal restores a count quota is a pricing and station-side question.
- **Rejected (10):** 6 low and 4 false. The reasons are in the Review Triage Log.

**Follow-up review recommended: false.** One medium entry was patched (the row-level test) and no high; this is a first pass.

**Verification.**
- `cargo test --workspace` passes all 589 tests, including all 23 station tests.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` is clean.
- Portal typecheck passes, all 158 tests pass, and `pnpm build` passes.
- After the review patches, I applied them myself: the implementer subagent could only be resumed detached, so I stopped it before it made any edit. I then re-ran every command above.

**Residual risks**
- Nobody is notified that a renewal is waiting. Until an owner reissues, each bench marks certificates `evaluation` after the old `not_after`.
- Each station is reissued separately, and the renewed file must be installed by hand (no push or fetch, D6).
- The quota semantics across a renewal are undecided (deferred).
- The portal page was checked by typecheck, unit tests and a build, not in a real browser.
- The vendor root is still file-held (ADR-0007).
