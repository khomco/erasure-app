---
title: 'Station credit reserve and automatic top-up'
type: 'feature'
created: '2026-11-16'
status: 'done'
review_loop_iteration: 1
followup_review_recommended: true
context:
  - '{project-root}/AGENTS.md'
  - '{project-root}/docs/adr/0013-licensing-controls-every-erasure.md'
  - '{project-root}/docs/adr/0008-station-enrollment-and-licence-fetch.md'
  - '{project-root}/.scratch/controlled-licensing/stories/3-organization-credit-pool-and-signed-credit-grants.md'
  - '{project-root}/.scratch/licence-visibility/stories/5-decide-licence-coverage-at-the-start-of-each-job.md'
warnings: ['oversized']
deferred:
  - summary: >-
      The 409 for an ambiguous station key on POST /api/station/credits has no test.
    evidence: |-
      Verification-gap review: credit_topup.rs has no Ambiguous case; the shared lock_station lookup is covered through the licence fetch route, only this handler's mapping is not.
    location: >-
      crates/wipe-hub/src/station_link_routes.rs (credits handler)
    severity: low
baseline_revision: 'd1b0f2e5058c803f8d7e5e6a722c942444eebdf0'
---

<intent-contract>

## Intent

**Problem:** Story 3 put each organization's erasure quota in a pool at the hub and signs credit grants out of it, but stations ignore grants: a hub-issued licence still carries the whole order quota as a per-licence `Count`, so every bench can spend the full quota, and nothing reports a station's balance back (ADR-0013 §3).

**Approach:** The hub marks a pool-backed licence with a new signed quota kind `Quota::Pool`. The station keeps verified grants in its lease state as a **credit reserve**, and the start gate reserves, spends and returns pool credits from it. A station with a saved hub address tops the reserve up over a new signed route `POST /api/station/credits` when it falls below the offering's threshold. Each top-up reports the station's balance and credits spent, and the hub flags reports that do not add up.

## Boundaries & Constraints

**Always:**
- **Signed quota** (`wipe-license/src/entitlement.rs`): `Quota::Pool` (wire `{"kind":"pool"}`): "credits come from credit grants held by the station for the licence's organization (`customer_id`)". The hub issues it for every licence on an entitlement with `quota_erasures` (`stations.rs` `issue_licence` ~:517; register, reissue, enrollment approval, station-link renewal). `vendor_issue` keeps `Count` (air-gapped trial file: credits inside the licence, ADR-0013 §2). `Count` and `Unlimited` behave exactly as today. Mirror in portal `api/adminLicences.ts` (`Quota`, guard, `quotaText` → "Credit pool").
- **Reserve in lease state** (`LeaseState`, all `#[serde(default, skip_serializing_if…)]` so existing lease files load): `credit_grants: Vec<HeldGrant { grant_id, organization_id, credits, spent }>` (kept forever, oldest first; never pruned, so a grant id seen before is always recognised), `pool_credits_reserved: u64` (reset to 0 at `open`, like `credits_reserved`; **not** reset by a licence switch), `last_topup_grant_id: Option<String>`, `credit_terms: Option<CreditTerms { threshold, target }>` (from the hub's last top-up answer). For organization `o`: `held(o) = Σ credits`, `spent(o) = Σ spent`, `balance(o) = held − spent`, `available(o) = balance(o) − pool_credits_reserved` (saturating).
- **Adding a grant** (`LicenseContext::add_grant`): only after `check_grant_for(grant, station key id, vendor roots)` passes. A grant id already held → `AlreadyHeld`, nothing changes. Otherwise appended and saved (`persist_checked`; a failed save is reported in the result but the grant stays in RAM and is saved by the next save). Grants arrive from: a licence file with `credit_grant` through `POST /api/license` (`AppState::install_license`) and `--license` (bootstrap, a new `wipe_license::load_license_file`); and a top-up answer. A file whose grant fails the check is refused whole, nothing installed: `POST /api/license` → 422 like other `NotInstallable` (reason names the grant: "credit grant was issued to instance key …" / "credit grant did not verify: …"); `--license` → start-up error, as a bad licence is.
- **Start gate** (`decide_start`, on a `Pool` licence): counts as a counting licence (lease load fault or reservation not saved within `LEASE_SAVE_TIMEOUT` refuses, as `Count`). `available(customer_id) < drives` → `StartRefusal::NoPoolCredits { needed, left }`, message names the reason and the fix: "This station has no credits left. Connect it to the platform so it can top up, or install a licence file carrying credits, to start new jobs." (needed ≤ 1, left 0) / "This start needs {n} credits, and this station holds {left}. Connect it to the platform so it can top up, or install a licence file carrying credits, to start it." Otherwise `pool_credits_reserved += drives` and the decision carries `JobLicence.credit_pool = Some(customer_id)` (new `#[serde(default, skip_serializing_if = "Option::is_none")]` field in `wipe-common`, TS mirror `credit_pool?: string`), `credits_reserved = drives`.
- **Settle a pool decision** (whatever licence is installed now): `release` → `pool_credits_reserved −= n`; `spend` → same, plus `n` added to `spent` of that organization's grants oldest first (saturating). Then save, best effort. `Count` decisions settle exactly as today.
- **Status** (`evaluate`): `Pool` → `Valid { remaining_erasures: Some(available(o)) }`, or `QuotaExhausted { used: spent + reserved, allowed: held }` when `available(o) == 0`. `view`'s `new_jobs_refused` predicts `NoPoolCredits`.
- **Top-up (station)**, new `crates/wipe-server/src/credit_topup.rs`: runs only when the station has a saved hub address (`licence_fetch::saved_hub`), vendor roots, and an installed `Pool` licence, and either `available(o) < threshold` (`credit_terms.threshold`) or `credit_terms` is `None` (the hub has never answered: ask once to learn the offering's terms). The measure is `available`, credits held and neither spent nor reserved by running Jobs, the same number the station reports and the hub grants on. Request `POST /api/station/credits` signed with `station_request::sign` over a fresh challenge, body `{instance_public_key, licence_id, balance: available(o), credits_spent: spent(o), last_grant_id}` (`credits_spent` is cumulative, see decisions). Request and answer are one pair of serde types in a new `wipe_license::station_credits` module (`TopUpRequest`, `TopUpAnswer { credit_grant: Option<CreditGrant>, threshold, target_reserve, available }`, `STATION_CREDITS_PATH`), used by both the hub route and `HubClient::top_up`, so the wire contract is one definition. A returned grant is checked (`check_grant_for`) and must name the installed licence's organization (`organization_id == customer_id`); a grant that fails either is not added, `last_topup_grant_id` is not changed, and the outcome is a failure naming why. A grant added, or one already held (a resend), records its id as `last_topup_grant_id`; `credit_terms` are saved from every answer. Triggered: by the licence-fetch loop after each check (start-up and every 24 h); and by a non-blocking `nudge` after every successful pool start decision, every pool spend, every `NoPoolCredits` refusal, and every successful `AppState::install_license` (runtime install, enrollment and licence fetch all install through it); at most one top-up in flight, and after an attempt that left `available < threshold` (failure, empty pool) nudges are ignored for `TOPUP_RETRY_AFTER` = 15 min (the 24 h loop still runs). Outcome kept as a `HubCheck` in a `CreditTopUpSlot` on `AppState` and shown on `GET /api/license` as `credit_top_up` (omitted when none), mirrored in `types.ts` `LicenseInfo` and shown on `LicencePage` beside `hub_check`. Never holds a lock across `.await`, never touches `start_job`, never refuses a start.
- **Top-up (hub)**, `POST /api/station/credits` in `station_link_routes.rs` (CSRF-exempt, `no_store`, same header/body/signature/challenge checks and messages, in the same order, as `/api/station/licence`); domain in `station_link.rs` (`top_up`), sharing the station lookup with `fetch_licence` (same `NotFound` 404 / `Ambiguous` 409). Body: `balance` and `credits_spent` integers ≥ 0, `last_grant_id` string|null, else 400. In one transaction, station row locked then its entitlement `FOR UPDATE` (station → entitlement order):
  0. `licence_id` null, or not the id of one of this station row's licences (`licences.station_id`) → 409 `MSG_FOREIGN_LICENCE` = `this station holds a licence from another registration; install the licence issued for its current registration, then try again`, nothing written (a report for one organization is never judged, or granted, against another).
  1. Entitlement without a quota → 409 `MSG_NO_CREDIT_POOL` = `this station's entitlement has no credit pool; its licence does not count credits`.
  2. **Add-up check:** `granted_to_key` = Σ `credit_grants.credits` for this organization and `instance_key_id` (every station row, any source). `adds_up = balance + credits_spent ≤ granted_to_key AND credits_spent ≥ stations.credit_spent_reported (NULL as 0)`. When false, set `stations.credit_report_mismatch_at = now()` (sticky).
  3. **Redelivery:** `L` = the station's latest top-up grant (`licence_id IS NULL`, by `issued_at`, `id`). If `L` exists, `last_grant_id ≠ L.id`, `last_grant_id` equals the id of the top-up grant issued just before `L` (or is null and `L` is the first), and `credits_spent ≥ stored credit_spent_reported`: answer with `L`'s stored document again; no new grant.
  4. Otherwise the hub grants when `balance < threshold`: `credits = min(target − balance, available)`, only when the entitlement is current (`ENTITLEMENT_CURRENT`; else treat `available` as 0) and `credits > 0`, through `credits::grant` with `licence_id: None` and actor `station:<id>` (audited `credits.granted`, `licence_id` null).
  5. Record the report: `stations.credit_balance = balance + credits of the grant in this answer (new or resent; 0 when none)`, so the stored balance is what the station holds once the answer lands (a release returns it), `credit_balance_reported_at = now()`, `credit_spent_reported = credits_spent`; audit `credits.reported {station_id, balance, credits_spent, spent_since_last, adds_up}` subject `entitlement:<id>`, actor `station:<id>`.
  Response 200 `{credit_grant: <grant JSON>|null, threshold, target_reserve, available}` (`available` after any grant).
- **Offering threshold:** `offerings.credit_topup_threshold bigint` (only with a counted quota, `1 ≤ threshold ≤ target`; NULL reads as `min(10, target)`; frozen at publication — `CREATE OR REPLACE offerings_lifecycle` with it added). `OfferingRequest.credit_topup_threshold: Option<i64>`: absent → `min(10, target)` stored; out of range → 400 `credit_topup_threshold must be between 1 and the target reserve`; with an unlimited offering → 400 `credit_topup_threshold applies only to an offering with an erasure quota`. In `same_terms`/`changes`, admin `Offering` response, portal `api/adminOfferings.ts` and `OfferingForm.tsx` (field shown with a counted quota, beside the target reserve).
- **Migration** `0023_credit_topup.sql`: `credit_grants.licence_id DROP NOT NULL` (a top-up grant has no licence; the unique index still allows one grant per licence); `stations` gains `credit_spent_reported bigint CHECK (>= 0)` and `credit_report_mismatch_at timestamptz` (not registration columns); the offering column and lifecycle function. Hub `Station` gains `credit_report_mismatch_at` (nullable), portal mirror + guard, and the Stations page shows "Reported credits did not add up on {date}" on that row.
- Paired changes in one commit: Rust serde shapes and TS mirrors; `CONTEXT.md` (§9, §10.2 row, §14 **Credit reserve**, **Top-up**, **Top-up threshold**; limits: a single start can use at most the target reserve, so larger destruction groups are split; a released station key registered again in the same organization still holds its old grants until revocation exists, A2.3), `README.md` (hub route, station top-up), `docs/ROADMAP.md` (A2.2: a balance report is sent with each top-up, which happens only below the threshold; regular reporting stays open).

**Never:**
- Refuse or delay a start because of the hub; an unreachable hub or a failed top-up only changes the `credit_top_up` status line while credits remain. No override. The simulated backend is never refused (its uncovered starts stay `evaluation`).
- Trust a grant the station's own vendor roots do not verify; the hub cannot introduce a root.
- Change `Count`/`Unlimited` behaviour, the erasure-certificate schema, `v2-*.json` fixtures, the signed `CreditGrantBody`, or any applied migration.
- Stop granting to a flagged station, renewal work (stories 5–6), revocation (A2.3), unused credits at renewal (D17), a hardware counter (D12).
- Add a station route (`docs/generated/http-routes.txt` stays unchanged), or change CLI help.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Grant in licence file | Real backend, `Pool` licence file with 25-credit grant via `POST /api/license` | 200; `GET /api/license` `remaining_erasures` 25; 25 starts succeed, the 26th is 409 `NoPoolCredits` | — |
| Duplicate grant | Same file installed again, or same file at restart via `--license` | Reserve unchanged (grant id already held) | — |
| Foreign grant | Grant for another key, or signed by an untrusted root | `POST /api/license` 422 naming the grant problem; nothing installed | `--license`: start-up error |
| Spend / abort | Pool start with 3 drives, then certify / abort | reserved +3 → spent +3 (oldest grant first) / reserved −3 | — |
| Licence switch mid-run | Pool Job running, renewal licence installed, Job certified | Spent from the reserve under the new licence; reserve not reset | — |
| Top-up at threshold | Reserve 9, threshold 10, target 50, pool 1000 | Hub grants 41; station holds 50; report recorded and audited; `credit_top_up.ok` | — |
| Above threshold | Reserve 30 | No request | — |
| Small pool | Reserve 0, pool 25 | Grant 25, `available` 0 | — |
| Empty pool | Pool 0 | 200 `credit_grant: null`, `available: 0`; station status says the pool is empty; next start with 0 credits refused with the connect/install text | — |
| Unreachable hub | Closed port, 5 credits held | Starts succeed until 0; `credit_top_up.ok` false | `warn` log |
| Restart | Station with held grants restarts on a file lease store | Same balance; reservations returned | — |
| Lost answer | Hub granted L, answer lost; station retries naming L's predecessor | Hub resends L, no new grant | — |
| Reset state | Station reports `credits_spent` lower than last report | No redelivery; flagged `credit_report_mismatch_at`; normal grant rules | — |
| Over-report | `balance + credits_spent` > granted to key | Flagged; audit `adds_up: false` | — |
| Race | Two top-ups for different stations racing for the pool's last 40 credits | Σ granted ≤ 40, no overdraft | — |
| Foreign registration | Station holds a licence of another registration (or none) and calls top-up | 409 `MSG_FOREIGN_LICENCE` | nothing written |
| Grant for another organization | Hub answers with a grant whose organization is not the licence's | Not added; `last_topup_grant_id` unchanged; `credit_top_up.ok` false | — |
| Reserved credits | 12 held, threshold 10, a 12-drive Job running | `available` 0 < 10: top-up runs, reports `balance` 0, hub grants 50 | — |
| Stored balance | Report `balance` 9, hub grants 41 | `stations.credit_balance` 50 | — |
| Unlimited | Station on unlimited entitlement calls top-up | 409 `MSG_NO_CREDIT_POOL` | nothing written |
| Unregistered / bad signature / reused challenge | — | 404 / 401 / 401 as `/api/station/licence` | nothing written |
| Offering admin | quota 500, threshold absent / 20 / 0 / 60 (target 50) / with unlimited | 10 / 20 / 400 / 400 / 400 | nothing written |

## Autonomous decisions

- **How a station knows a licence draws from the pool.** *About:* a bench installing a licence the hub issued for a quota entitlement. *Problem:* today that licence says `Count {order quota}`, so each bench could spend the whole order. *Options:* (A) a new signed quota kind `Pool`; (B) a separate flag beside `Count`; (C) infer from a grant being present. *Choice:* A. The signature covers it, the meaning is explicit, and a licence never pretends to carry a count it does not hold. Older station builds cannot parse it, but nothing has shipped (ADR-0013 §5). Vendor-issued files keep `Count`, as ADR-0013 §2 puts the air-gapped trial's credits inside the file.
- **Grants are tied to the organization, not the licence.** A renewal or reissue changes the licence id, and resetting the reserve then would destroy paid credits. Grants count for the installed licence only when `organization_id == customer_id` (both are the organization id at the hub), and pool reservations are settled by organization, whatever licence is installed.
- **Reporting spent credits cumulatively.** *Problem:* CAP-4 says "credits spent since the last top-up", but a top-up answer can be lost, and resending a per-interval number counts it twice. *Choice:* the station reports its cumulative `credits_spent` for the organization. The hub stores it, derives `spent_since_last` for the audit, and the comparison makes the report retry-safe (an idempotent counter, recognised practice).
- **"Does not add up"** means the station claims more than it was ever given (`balance + spent > granted to its key in this organization`) or its spent count went backwards (reset or restored lease state). Flag only, sticky, audited and shown in the portal. Granting continues: ADR-0013 says "flags", and stopping would stop benches over a state wipe.
- **Lost top-up answer.** The hub resends the latest top-up grant only to a station that names the grant before it and whose spent count has not gone backwards. A wiped or restored lease never gets credits back this way, so wiping state still "loses credits, never creates them" (ADR-0013 §3). Restoring an old copy can still re-spend a grant (the ADR's stated limit).
- **Reported balance** *(changed at review, 2026-11-16)* is `available`: credits held, not spent and not reserved by running Jobs. A bench whose running Jobs have reserved most of its reserve tops up at once, instead of refusing starts while it is connected. The add-up check still holds for an honest station, since `balance + credits_spent ≤ Σ grant credits`. Reserved credits are about to be spent, so a release does not return them.
- **Threshold storage and default.** An admin-set, frozen offering column like the target reserve (story 3). NULL reads as `min(10, target)`, so the threshold never exceeds the target. The station learns both from each top-up answer and uses 10 until then.
- **Retry pacing.** After a top-up that leaves the reserve below threshold, nudges wait 15 minutes. That spares the hub's per-network challenge throttle (300 an hour) and an empty pool. The 24-hour check still runs. A start is never refused because of a top-up.
- **Bad grant in a licence file.** The whole file is refused, matching story 3's parse rule: a damaged or foreign grant in a file means the file is not the one issued for this station. This is the safe default.
- **Stored balance after a grant** *(review, 2026-11-16)*: the hub stores the reported balance plus the credits in its answer. Otherwise the portal and a release would see the balance from before the grant, and a release would strand the grant's credits.
- **A report from another registration** *(review, 2026-11-16)*: a station still holding a licence of an earlier registration (another organization, or a released row) is refused with 409, and the station never adds a grant for an organization other than its licence's. Without this, a report about organization A would be judged and topped up against organization B every 15 minutes, draining B's pool. Refusing is the safe default, and the fix (install the current licence) is named.
- **More top-up triggers** *(review, 2026-11-16)*: a refused start for lack of credits, and every licence install, also nudge a top-up. A station that has just enrolled, or that ran dry during an outage, then tops up within minutes instead of at the next 24-hour check. A station that has never had a top-up answer asks once to learn its offering's threshold. The 15-minute quiet period still bounds the requests.
- **One wire definition** *(review, 2026-11-16)*: station crates may not depend on `wipe-hub`, and the only device backend is simulated, so no test can run a real-backend station against a real hub. The request and answer types live once in `wipe-license`, which both depend on, so a drift in field names fails to compile. Each side is then tested over HTTP against the other side's stand-in.
- **Limits recorded, not built** *(review, 2026-11-16)*: a start needing more credits than the target reserve can never be covered on a pool licence, so larger destruction groups must be split. Raising grants to the drives a start needs would change the top-up protocol, so this is deferred to `CONTEXT.md` as a limit (the refusal already names the credits needed and held). A released key registered again keeps its old grants until revocation (A2.3).
- **Wording.** The refusal and status texts are functional operator text in the house style of licence-visibility story 5, not marketing copy. Portal label: "Reported credits did not add up on {date}".

</intent-contract>

## Code Map

**wipe-license**
- `src/entitlement.rs:13` `Quota` (+`Pool`; `permits`/`remaining` arms return `true`/`None`, `evaluate` handles `Pool` itself).
- `src/lease.rs:31` `LeaseState` (+ fields, helpers `pool_balance(org)` etc.), `:323` `evaluate` (`Pool` branch), `LeaseStatus` unchanged.
- `src/credit.rs` `CreditGrant`, `check_grant_for` (reuse). `src/install.rs` `LicenseFile`, `parse_license_file`; add `load_license_file(path)`. `src/lib.rs` re-exports.
- Tests: `tests/attestation.rs`, `tests/credit_grant.rs` patterns; root `VendorRoot::from_signing_key(SigningKey::from_seed([7; 32]))`.

**wipe-common**
- `src/job.rs:231` `JobLicence` (+`credit_pool`), `evaluation()` ctor.

**wipe-server**
- `src/licensing.rs`: `adopt_licence` :115 (leave pool fields), `open` :143 (reset `pool_credits_reserved`), `decide_start` :421, `counts_credits` :530 (+Pool), `credit_refusal` :586, `settle` :496, `StartRefusal` :617 (+`NoPoolCredits`), `predicted_refusal` :383, `LicenseView` :738 (+`credit_top_up`). Add `add_grant`, `pool_reserve()` accessor (org, available, balance, spent, terms, last grant id), `set_credit_terms`.
- `src/app.rs`: `AppState` fields :22–95 (+`credit_topup: Arc<CreditTopUpSlot>`), `install_license` :242 (parse file, check grant, add), `decide` :393 (nudge after a pool decision), cert generator spend :600 (nudge after spend); `LicenseInstallError`.
- `src/handlers.rs:977` `license` (fill `credit_top_up`), `:990` `install_license` status mapping.
- `src/bootstrap.rs:700` `build_license_context` (load file, check grant, add after `open`); `:269` licence-fetch spawn.
- `src/licence_fetch.rs:248` `run_loop` (call top-up after `check_once`), `saved_hub` :99, `HubCheck` :55 (reuse).
- `src/hub_client.rs:444` `challenge`, `:476` `fetch_licence` (template for `top_up`), `FetchOutcome` :286.
- Tests: `tests/licence_fetch.rs` and `tests/hub_client.rs` (`fake_hub`, `closed_port`, `licence(root, key)`), `tests/lease_store.rs` (file store restart), `tests/http_e2e.rs` (`spawn_server_with_license`, real-backend wrapper from licence-visibility story 5).

**wipe-hub**
- `migrations/0022_credit_pool.sql` (read-only; lifecycle fn to copy), new `0023_credit_topup.sql`.
- `src/stations.rs:517` quota → `Pool`; `:1216` `vendor_issue` stays `Count`; `Station` :174 + select :273 (+`credit_report_mismatch_at`); `ENTITLEMENT_CURRENT`.
- `src/credits.rs` `NewGrant.licence_id` → `Option<Uuid>`, `grant`, `available`, `DEFAULT_TARGET_RESERVE`; add `DEFAULT_TOPUP_THRESHOLD = 10`.
- `src/station_link.rs:149` `fetch_licence` (extract the locked lookup), add `top_up`. `src/station_link_routes.rs` (route, request type, handler; doc comment). `src/csrf.rs:94` `is_exempt` + its test.
- `src/offering_admin.rs:118,162,237,298,315,343` (threshold beside target reserve).
- Tests: `tests/support/hub.rs` (`spawn_hub_with_root`, `BASE`, `published_one_off_offering`, `published_trial_offering`, `audited`), `tests/station_licence.rs` (signed-request helpers to reuse), `tests/credits.rs`, `tests/stations.rs` quota asserts (`Count` → `Pool`), `tests/offering_admin.rs`, `tests/migrations.rs`, `tests/isolation.rs`.

**Frontends**
- `apps/desktop/src/api/types.ts` `LicenseInfo` :358 (+`credit_top_up?`), `JobLicence` :488 (+`credit_pool?`); `pages/LicencePage.tsx:100` `LicenceStatus`.
- `apps/portal/src/api/{adminLicences,adminOfferings,stations}.ts`, `pages/{StationsPage,OfferingForm}.tsx` and tests.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-license/src/{entitlement,lease,install,lib}.rs` + tests -- `Quota::Pool`, reserve fields and arithmetic, `evaluate` for `Pool`, `load_license_file` -- the station's offline reserve.
- `crates/wipe-common/src/job.rs` -- `JobLicence.credit_pool`.
- `crates/wipe-server/src/{licensing,app,handlers,bootstrap,licence_fetch,hub_client}.rs`, new `credit_topup.rs`, `lib.rs` -- grant intake, pool start/settle, top-up client, loop and nudge, status.
- `crates/wipe-server/tests/credit_reserve.rs` (new) -- station matrix rows over HTTP with a fake hub (grant file, duplicate, foreign, threshold top-up, empty pool, unreachable hub, restart with a file lease store, licence switch).
- `crates/wipe-hub/migrations/0023_credit_topup.sql`, `src/{stations,credits,station_link,station_link_routes,csrf,offering_admin}.rs` -- `Pool` licences, threshold, top-up route, report, flag, redelivery.
- `crates/wipe-hub/tests/credit_topup.rs` (new) + updates to `stations.rs`, `credits.rs`, `offering_admin.rs`, `migrations.rs`, `isolation.rs`, `csrf` test -- hub matrix rows, audit rows, race.
- `apps/desktop/src/api/types.ts`, `pages/LicencePage.tsx` (+ tests) -- mirrors, top-up status line.
- `apps/portal/src/api/{adminLicences,adminOfferings,stations}.ts`, `pages/{StationsPage,OfferingForm}.tsx` (+ tests) -- `pool` quota, threshold field, mismatch flag.
- `CONTEXT.md`, `README.md`, `docs/ROADMAP.md`, `AGENTS.md` test counts -- what shipped and its limits.

**Acceptance Criteria:**
- Given a hub with a trial offering (25 credits) and a station registered to it, when the station sends its first top-up (`POST /api/station/credits`, balance 0, terms unknown), then the hub grants 25, answers `available: 0`, and the portal's Stations response shows the reported balance 25 (hub test).
- Given a real-backend station with a saved hub address and no credits, whose hub (a stand-in speaking the shared `wipe_license::station_credits` types) answers with a 25-credit grant, when the station starts (the licence-fetch loop's first round, no direct `top_up_once` call), then it holds 25 credits, 25 starts succeed, and the 26th is refused with the connect/install text (station test).
- Given a connected station whose hub is unreachable, when it holds credits, then starts succeed until the reserve is empty, and `GET /api/license` reports the failed top-up without refusing anything else.
- Given `cargo test --workspace`, the desktop and portal suites, when run, then all pass with the new cases.

## Spec Change Log

### 2026-11-16 — Review loop 1 (bad_spec)
- **Triggering findings:** the station measured its reserve including credits reserved by running Jobs, so a connected bench refused starts and never topped up (Blind: threshold vs drives; Edge: reservations). The hub stored the pre-grant balance, so the portal and a release saw a stale-low balance (Blind). A station holding another registration's licence was judged and topped up against the wrong organization every 15 minutes (Edge, 2 rows). A just-enrolled or emptied station waited up to 24 h for a top-up (Blind). Offering thresholds above 10 were ignored until the first answer (Edge). Station and hub wire shapes were defined twice and never checked against each other, and AC 1 had no test (Intent).
- **Amended:** Station top-up rule (measure `available`; ask once when terms are unknown; shared `wipe_license::station_credits` types; organization guard on grants; record the id of a resent held grant; nudges on `NoPoolCredits` refusals and on every licence install). Hub rule (step 0 `MSG_FOREIGN_LICENCE`; step 5 stores balance plus the delivered grant). Docs rule (limits, ROADMAP wording). Matrix (4 rows). Autonomous decisions (6 entries). Acceptance criteria 1 split into a hub test and a station test.
- **Known-bad state avoided:** benches refused while connected; pools drained across organizations; releases stranding just-granted credits; drifting wire shapes.
- **KEEP:** the loop-1 implementation is saved at `/tmp/controlled-licensing-4-loop1.patch` (apply from the repo root with `git apply`). It passed `cargo test --workspace` (1195), clippy (apart from the existing `large_enum_variant` on `JobBroadcast`), both goldens, desktop (425) and portal (297) suites. Re-derive by applying it first, then changing only what the amendments require. Keep: `Quota::Pool`, the `LeaseState` reserve and its helpers, `LicenseContext::add_grant`/`pool_reserve`/`record_top_up`, the pool settle path, `credit_topup.rs` structure (slot, claim, nudge, quiet window), the hub `lock_station` extraction, migration 0023, the offering threshold, the portal and desktop changes, and every existing test.
- **Also apply these review patches** (patch-routed in the same pass): ROADMAP A2.2 wording (reported only with a top-up); `OfferingForm` threshold field starts blank (hub default applies) instead of 10; `credits.rs` module doc (top-up grants have no licence) and a `CONTEXT.md` §14 entry for **Top-up threshold**; the portal `stations.ts` comment on `credit_report_mismatch_at` says the latest mismatch, not the first. New tests: a station test where the licence-fetch loop (or `build_station` with a saved hub) tops up with no direct `top_up_once`; a hub test with an offering threshold 20 / target 80 (answer echoes them; balance 15 gets 65; 25 gets nothing); a station test where the hub answers threshold 30 and a later top-up at 20 is sent, at 30 not; a hub test where a non-current entitlement grants nothing and answers `available: 0`; a station test where a spend (not a start) nudges a top-up; and a station test that a failed top-up is not retried by an immediate further start.

## Review Triage Log

### 2026-11-16 — Review pass
- verdicts: 30 findings — high 0, medium 11, low 18, false 1, maybe-false 0
- findings:
  - `[medium]` `[bad_spec]` Blind: no top-up trigger after a refused start or a licence install, so an enrolled or emptied station waits up to 24 h — amendment: nudges on `NoPoolCredits` refusals and every successful `install_license`.
  - `[medium]` `[bad_spec]` Blind: the threshold compares the balance, not what a start needs, and a start above the target reserve can never be covered — amendment: measure `available` (with Edge reservations row); a start above the target reserve is a recorded `CONTEXT.md` limit (protocol change deferred).
  - `[medium]` `[bad_spec]` Blind: the hub stores the pre-grant balance, so the portal is stale-low and a release strands just-granted credits — amendment: step 5 stores balance plus the answer's grant.
  - `[low]` `[bad_spec]` Blind: a released key registered again in the same organization keeps spending old grants whose balance was returned — amendment: recorded as a `CONTEXT.md` limit until revocation (A2.3, an epic non-goal).
  - `[low]` `[patch]` Blind: ROADMAP A2.2 overclaims the balance report — wording carried into the re-derivation (reported only with a top-up).
  - `[low]` `[reject]` Blind: the top-up status reads "no top-up needed" on a hub without a vendor root, and "pool empty" on a lapsed entitlement — a rootless hub is development-only, and on a lapsed entitlement the licence's own expiry refusal applies; fixing it needs a reason field in the answer.
  - `[low]` `[reject]` Blind: a grant the station cannot verify (root rotation) is resent forever — the alternative, recording it as delivered, would make the hub grant again every 15 minutes and drain the pool; today one grant is stranded, and root rotation is not built.
  - `[low]` `[reject]` Blind: `QuotaExhausted 0 of 0` wording for a station with no grants, an ever-growing `allowed`, and one global reservation counter — the banner shows the refusal text naming the fix; the global counter only ever over-reserves (never over-commits) after a rare organization switch; a new status variant adds a mirrored type.
  - `[low]` `[patch]` Blind: `OfferingForm` prefills a threshold of 10, so a target reserve below 10 gets a 400 — carried into the re-derivation: the field starts blank.
  - `[low]` `[reject]` Blind: the mismatch flag never clears and a reverted lease after a failed best-effort save trips it — a reverted lease can re-spend, so flagging it is correct; the flag blocks nothing (sticky by decision).
  - `[low]` `[patch]` Blind: stale `credits.rs` grant doc and no glossary entry for **top-up threshold** (thin licence tests are covered by `licensing.rs` unit tests) — doc fixes carried into the re-derivation.
  - `[low]` `[patch]` Blind: hub tests missing for a non-current entitlement, ambiguous key, rootless hub, exact threshold, top-up racing a registration — non-current added (VG row); ambiguous deferred (VG row); exact threshold is `a_station_at_or_above_the_threshold_gets_nothing…`; rootless hub and the cross-path race follow the same lock order as every path (rejected parts).
  - `[medium]` `[bad_spec]` Edge: a hub grant for another organization than the licence's is added but never counted, so the station re-requests every 15 minutes and drains that pool — amendment: hub step 0 `MSG_FOREIGN_LICENCE` and the station's organization guard.
  - `[low]` `[bad_spec]` Edge: a report for the licence's organization is judged against the hub row's organization and flagged unearned — same root and amendment as the row above.
  - `[low]` `[reject]` Edge: the global `pool_credits_reserved` counter crosses organizations — same as the Blind wording row: conservative and rare.
  - `[medium]` `[bad_spec]` Edge: reservations keep the balance at or above the threshold while `available` is 0, so starts are refused while connected — grouped with the Blind threshold row; amendment: measure `available`.
  - `[low]` `[bad_spec]` Edge: an offering threshold above 10 is ignored until the first answer — amendment: ask once while terms are unknown.
  - `[low]` `[reject]` Edge: a lapsed entitlement is reported as an empty pool — same as the Blind status row.
  - `[low]` `[reject]` Edge: a rootless hub reports "no top-up needed" — same as the Blind status row.
  - `[false]` `[reject]` Edge: a nudge task dropped before its first poll leaves `in_flight` set — that only happens when the station's runtime shuts down, after which no top-up can run anyway; each `AppState` lives on one runtime.
  - `[low]` `[reject]` Edge: older station builds cannot parse `Quota::Pool` — nothing has shipped (ADR-0013 §5); recorded decision.
  - `[medium]` `[patch]` VG: the licence-fetch loop's top-up is untested — loop-driven station test carried into the re-derivation.
  - `[medium]` `[patch]` VG: the hub never uses an offering's own threshold or target in a test — hub test (20/80) carried into the re-derivation.
  - `[medium]` `[patch]` VG: the station never shows it uses the hub's threshold — station test (threshold 30) carried into the re-derivation.
  - `[medium]` `[patch]` VG: a lapsed entitlement's no-grant rule is untested — hub test carried into the re-derivation.
  - `[medium]` `[patch]` VG: the spend nudge and the quiet window are untested — spend-nudge test and a no-immediate-retry test carried into the re-derivation.
  - `[low]` `[defer]` VG: the 409 for an ambiguous key on the top-up route is untested — deferred in frontmatter (the lookup is covered through the licence fetch).
  - `[low]` `[patch]` VG other: the portal comment says the first mismatch, the hub stores the latest — comment fix carried into the re-derivation.
  - `[medium]` `[bad_spec]` Intent: no test runs station and hub together; AC 1 and the wire contract are untested — amendment: shared `wipe_license::station_credits` types and AC 1 split into a hub test and a loop-driven station test (a joint test is impossible: station crates may not depend on `wipe-hub`, and the only backend is simulated).
  - `[low]` `[reject]` Intent: the mismatch flag is tested only from fabricated requests — the hub's flag is the hub's surface, tested over signed HTTP; an honest station cannot produce a false report.

### 2026-11-16 — Review pass (after loop 1)
- verdicts: 27 findings — high 0, medium 6, low 20, false 1, maybe-false 0
- findings:
  - `[medium]` `[patch]` Blind: a release returns the stored balance from the last top-up, including credits spent since — inherent to CAP-3's "last reported balance" until regular reporting (A2.2) exists; recorded as a limit in `CONTEXT.md` and README.
  - `[medium]` `[patch]` Blind: with RAM-only lease state (e.g. a PXE bench with a read-only data directory and a provisioned hub address) held credits are lost at every restart and a fresh grant is taken — consistent with ADR-0013 §3 ("wiping loses credits"); the RAM-only warning in `store.rs`, README and `CONTEXT.md` now say so.
  - `[medium]` `[patch]` Blind: the hub keeps granting to a station whose latest licence is revoked — `top_up` now records the report and grants or resends nothing when the latest licence is not active; test `a_station_whose_licence_was_revoked_gets_no_credits`.
  - `[low]` `[patch]` Blind: a resend leaves no audit trail and the stored balance cannot be rebuilt from the audit — `credits.reported` gains `grant_id`, `credits_delivered`, `resent`; asserted.
  - `[low]` `[patch]` Blind: an out-of-range count consumes the challenge before the 400 — now refused when the body is parsed; test reuses the challenge.
  - `[low]` `[reject]` Blind: the refusal for a start above the target reserve suggests connecting — rare; the text names credits needed and held, and the limit is in `CONTEXT.md`; a separate message adds a branch.
  - `[low]` `[patch]` Blind: `CONTEXT.md` start-check row and README rows describe only `Count` licences — updated for `NoPoolCredits`, pool lease faults and the credit reserve.
  - `[low]` `[reject]` Blind: the story's Design Notes and the target-reserve glossary wording disagree with the review amendment — fixing it means editing this build's spec (Design Notes); the glossary wording stays accurate for `available`.
  - `[low]` `[patch]` Blind: README hub test count off — recounted with `--list` (663: 181 unit, 482 integration); README and AGENTS.md corrected.
  - `[false]` `[reject]` Blind: `LicencePage.test.ts` pins an unreal message — the page renders the station's text verbatim, so any string tests the display.
  - `[low]` `[reject]` Blind: the portal threshold field has no upper bound — the hub's 400 names the rule; a local check adds validation for a rare admin edit.
  - `[low]` `[reject]` Edge: global `pool_credits_reserved` across organizations — carried: conservative and rare.
  - `[low]` `[reject]` Edge: editing a counted draft with threshold 10 and lowering the reserve below 10 is a 400 — rare admin edit; the hub's message names the fix.
  - `[low]` `[patch]` Edge: a reserve wiped after its first-ever top-up, with nothing reported spent, is resent that grant, contradicting "never gets credits back" — the module doc and `CONTEXT.md` now state this narrow case.
  - `[low]` `[reject]` Edge: a release does not return credits reserved by running Jobs — recorded decision ("Reported balance"): they are about to be spent.
  - `[low]` `[reject]` Edge: a simulated-backend station that runs dry gets an `evaluation` decision and no nudge — the simulated backend erases nothing; the 24-hour loop still tops it up.
  - `[low]` `[reject]` Edge: older station builds cannot parse `Quota::Pool` — carried: nothing has shipped (ADR-0013 §5).
  - `[low]` `[reject]` Edge: already-installed `Count` licences keep the full quota until a newer licence — nothing has shipped, so no such benches exist.
  - `[medium]` `[patch]` VG: the `NoPoolCredits` refusal nudge is untested — `a_start_refused_for_lack_of_credits_nudges_a_top_up`.
  - `[medium]` `[patch]` VG: the quiet period after a successful but still-low top-up is untested — `a_top_up_that_leaves_the_station_low_quiets_the_next_nudges`.
  - `[medium]` `[patch]` VG: the stored balance after a resend is unasserted — asserted twice in `a_lost_answer_is_resent_and_nothing_new_is_granted`.
  - `[low]` `[reject]` Intent: station and hub behaviour are never run together — impossible under the crate dependency rule with only a simulated backend; recorded decision, shared wire types in place.
  - `[low]` `[reject]` Intent: no hub-produced `Pool` file is installed through the station's `POST /api/license` — same dependency rule; both sides use the same `wipe-license` types and parser.
  - `[low]` `[reject]` Intent: "spent since the last top-up" exists only at the hub — recorded decision (cumulative, retry-safe).
  - `[low]` `[reject]` Intent: the restart test drives the licence context directly — `build_station`, `--license` and the file lease store are the real start-up path; starts over HTTP are covered elsewhere.
  - `[low]` `[reject]` Intent: the portal flag renders from a mocked shape — guard tests pin the mirror; the hub sets it over HTTP.
  - `[low]` `[reject]` Intent: an admin-set threshold is not traced to a station — each half is tested (`an_offering_s_own_threshold_and_target_reserve_apply`, `the_hub_s_threshold_decides_when_a_top_up_is_sent`).

## Design Notes

Station reserve arithmetic for organization `o` (only grants with `organization_id == o`):

```
held = Σ credits   spent = Σ spent   balance = held − spent
available = balance − pool_credits_reserved     # what a start may reserve
report: { balance, credits_spent: spent }       # balance + spent == held
```

Hub top-up order inside one transaction: lock station → lock entitlement → add-up check → redelivery → grant (if `balance < threshold`) → record report → audit. The add-up check and the redelivery both compare against the stored `credit_spent_reported` read **before** step 5 overwrites it.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace --no-fail-fast` -- expected: all pass (never piped through grep).
- `cargo clippy --workspace --all-targets -- -D warnings` -- expected: clean.
- `cargo test -p wipe-server --test route_golden` and `cargo test -p wipe-cli --test help_golden` -- expected: pass without `UPDATE_GOLDENS`.
- `cd apps/desktop && pnpm typecheck && pnpm test`; `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: pass.
- `rustfmt --edition 2021 <touched .rs files>` -- only touched files.

## Auto Run Result

Status: done

**Summary.** Each station now holds its organization's erasure credits as a local **credit reserve** of vendor-signed credit grants (ADR-0013 §3). The hub issues every licence on a quota entitlement as a new signed quota kind, `Quota::Pool`. Vendor-issued files keep `Count`. A pool start reserves one credit per drive from the reserve. A certificate spends it, from the oldest grant first. An abort returns it, whatever licence is installed by then, and a licence switch keeps the reserve. A start with too few credits is refused (409) with the connect-or-install text. Grants arrive in a licence file (`POST /api/license`, `--license`; a bad grant refuses the whole file) or by top-up. A station with a saved hub address tops up over the signed route `POST /api/station/credits` when its available credits fall below the offering's threshold, or when it has never had an answer. Top-ups are triggered by the 24-hour licence loop and nudged by starts, spends, credit refusals and licence installs. They are bounded by a 15-minute quiet period and never refuse a start. The hub works in this order:
- refuses a station holding another registration's licence;
- checks the report adds up and flags it sticky if not;
- resends a grant whose answer was lost;
- grants up to the target reserve while the entitlement is current and the licence not revoked;
- stores and audits the report.

Request and answer are one serde definition in `wipe_license::station_credits`. Offerings gain a frozen `credit_topup_threshold` (default `min(10, target)`). The bench shows the top-up status, and the portal shows the threshold field and the mismatch flag.

**Files changed.**
- `crates/wipe-license/src/{entitlement,lease,install,station_credits(new),lib}.rs`: `Quota::Pool`, the reserve in `LeaseState`, `load_license_file`, shared top-up wire types.
- `crates/wipe-common/src/job.rs`: `JobLicence.credit_pool`.
- `crates/wipe-server/src/{licensing,app,handlers,bootstrap,licence_fetch,hub_client,store,credit_topup(new),lib}.rs`: grant intake, pool start and settle, the top-up client, loop and nudges, status, RAM-only warning.
- `crates/wipe-hub/migrations/0023_credit_topup.sql` (new): nullable grant licence, station report columns, offering threshold and lifecycle.
- `crates/wipe-hub/src/{station_link,station_link_routes,stations,credits,csrf,offering_admin}.rs`: the top-up route, `Pool` licences, the threshold.
- Tests:
  - new: `crates/wipe-hub/tests/credit_topup.rs`, `crates/wipe-server/tests/credit_reserve.rs`, `crates/wipe-license/tests/credit_reserve.rs`;
  - updated: hub `stations`, `credits`, `offering_admin`, `migrations`, `isolation`.
- `apps/desktop/src/api/types.ts`, `pages/LicencePage.tsx` and tests: mirrors and the top-up line.
- `apps/portal/src/api/{adminLicences,adminOfferings,stations}.ts`, `pages/{StationsPage,OfferingForm}.tsx` and tests: the `pool` quota, the threshold field, the mismatch flag.
- `CONTEXT.md`, `README.md`, `docs/ROADMAP.md`, `AGENTS.md`: what shipped, its limits, test counts.

**Review findings.**
- **Pass 1** (30 findings): routed `bad_spec`, looped once with the spec amended.
  - The reserve is measured by available credits.
  - The hub stores the balance after its grant.
  - A licence from another registration is refused (409).
  - More top-up triggers.
  - One shared wire definition.
  - Recorded limits.
  - Its patch rows were carried into the re-derivation. One low item was deferred: the ambiguous-key 409 on the top-up route has no test.
- **Pass 2** (27 findings): 9 patched.
  - 3 medium: the revoked-licence rule, and the release and RAM-only limits documented.
  - 3 medium tests: the refusal nudge, the quiet period after a low success, the stored balance after a resend.
  - 3 low: audit fields, out-of-range counts before the challenge is spent, docs and counts. One more low doc fix: the redelivery doc.
  - Nothing new deferred. 16 low and 1 false rejected, each with its reason in the triage log.

**Follow-up review recommendation:** true. Pass 2 patched three medium entries. The named unverified risk is the hub's new revoked-licence rule: it was patched after review and is covered by one test only. It interacts with the lost-answer resend, a revoked station's report and the add-up flag in ways no reviewer has examined. Patched counts in pass 2: high 0, medium 6 (3 code or doc, 3 tests), low 4.

**Verification.**
- `cargo test --workspace --no-fail-fast`: 1208 passed, 1 failed. The failure is `wipe-hub` `health::silent_smtp_and_database_time_out_within_bound` (4.8 s against a 3.5 s bound). It is environmental: this tree's same test passed twice in 2.2 s when built into a fresh target directory, and the hub's start-up and health paths are untouched.
- `cargo clippy --workspace --all-targets -- -D warnings -A clippy::large_enum_variant`: clean. The allowed lint, on `JobBroadcast`, fails identically on the baseline, checked in a temporary worktree.
- `route_golden` and `help_golden` pass without `UPDATE_GOLDENS`.
- Desktop: typecheck and 425 tests pass. Portal: typecheck, 297 tests and build pass.
- `rustfmt --check` on the touched files reports only lines that were already unformatted (`ws.rs`, `job.rs:16`, both unchanged by this story).
- Every I/O matrix row, including the 4 added at review, has a passing test.

**Residual risks.**
- No test runs a real station against the real hub. The crate dependency rule forbids it and the only backend is simulated. Both sides share one wire definition and are each tested over HTTP.
- A release returns the balance as of the last top-up, so credits spent since then are returned too, until regular balance reporting (A2.2).
- RAM-only lease state loses held credits at every restart.
- Restoring an old lease copy can re-spend a grant (ADR-0013 §3); the hub flags it but cannot prevent it.
- A start needing more credits than the target reserve can never be covered on a pool licence; larger destruction groups must be split.
- The portal and bench pages were checked by typecheck and unit tests, not in a browser.
