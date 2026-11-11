---
title: 'Organization credit pool and signed credit grants'
type: 'feature'
created: '2026-11-10'
status: 'done'
review_loop_iteration: 0
followup_review_recommended: false
context:
  - '{project-root}/AGENTS.md'
  - '{project-root}/docs/adr/0013-licensing-controls-every-erasure.md'
  - '{project-root}/docs/adr/0005-licensing-and-attestation.md'
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
warnings: ['oversized']
deferred: []
baseline_revision: 'a54e76cb5310a0e0dd96882cd04316928a2b1983'
---

<intent-contract>

## Intent

**Problem:** ADR-0013 §3 makes an entitlement's erasure quota one pool for the whole organization, handed to stations as vendor-signed credit grants, but today every station's licence carries the full order quota and nothing at the hub counts credits handed out or returned.

**Approach:** `wipe-license` gains a `CreditGrant` document (canonical, versioned body signed by the vendor root, verified offline against the same roots as licences) and a licence-file form that carries a licence plus one grant. The hub keeps an append-only grant/return ledger per entitlement with a quota, lets the owner put credits from the pool into the licence file at register and reissue, returns a released station's last reported balance, refuses annual offerings with a quota, and the portal shows the pool and each station's balance.

## Boundaries & Constraints

**Always:**
- **Grant document** (`wipe-license`, new `credit.rs`): `CreditGrant { body: CreditGrantBody, signature: LicenseSignature }`; body `{ grant_format_version: 1, grant_id, root_key_id, organization_id, instance_public_key_id, credits: u64, issued_at (RFC 3339) }`. Signed by `VendorRoot::issue_credit_grant(grant_id, organization_id, instance_key_id, credits, issued_at)` over `wipe_cert::canonical::canonical_bytes(body)` exactly as licences are; `CreditGrant::verify(roots: &[VerifyingKey]) -> LicenseResult<String>` applies the same checks and errors as `LicenseCertificate::verify` (algorithm, exact format version, re-canonicalized hash → `Tampered`, unknown root → `UntrustedRoot`); `check_grant_for(&grant, station_key_id, roots) -> Result<(), InstallProblem>` adds the instance-key compare. Share the verify core with the licence without changing licence behaviour. A fixture `tests/fixtures/credit-grant-v1.json` (seed `[7; 32]`, fixed time) pins the wire shape.
- **Licence file:** `LicenseFile { license: LicenseCertificate, credit_grant: Option<CreditGrant> }` (keys `license`, `credit_grant`). `parse_license_file(raw)` accepts the bare licence (grant `None`) or that object; `parse_license`/`load_license` accept both and return the licence, so today's station install path (`POST /api/license`, `--license`) takes a file with a grant and ignores the grant (stations use grants from story 4). The licence certificate type itself is unchanged, so erasure-certificate attestation chains never carry a grant.
- **Hub ledger** (migration `0022_credit_pool.sql`): `credit_grants (id uuid = grant_id, organization_id, entitlement_id, station_id, licence_id, instance_key_id, credits bigint > 0, issued_at, issued_by, document text)` and `credit_returns (id, organization_id, entitlement_id, station_id, credits bigint >= 0, reported_at NULL, returned_at, returned_by)`, both append-only (UPDATE/DELETE/TRUNCATE refused by trigger, as `audit_log`), FKs on `(entitlement_id, organization_id)`. `stations` gains nullable `credit_balance bigint >= 0` and `credit_balance_reported_at` (no route writes them yet: story 4's report fills them). `offerings` gains nullable `credit_target_reserve bigint` (1..1,000,000,000, only when `quota_erasures` is not null), frozen at publication in `offerings_lifecycle`; NULL reads as 50.
- **Pool arithmetic**, per entitlement with a quota: `purchased` = the order snapshot's `quota_erasures`; `granted` = Σ grants; `returned` = Σ returns; `available = purchased − granted + returned`. Every grant is decided under the entitlement row lock (`register` already takes `FOR UPDATE OF e`; `reissue` takes `s, e`; `release` must lock station then entitlement, the existing order), so racing grants can never take `available` below 0.
- **Choosing credits:** `RegisterRequest` and the reissue body gain optional `credits` (integer ≥ 0). On an entitlement with a quota, omitted means `min(target reserve, available)` for register and for a renewal reissue; an explicit value above `available` is 409 `MSG_NOT_ENOUGH_CREDITS` and writes nothing. On an unlimited entitlement any non-null `credits` is 400. Credits > 0 sign one grant bound to the station's instance key, stored and linked to the new licence, in the same transaction as the licence; 0 issues the licence alone.
- **Reissue** stays renewal-only (story 2), plus one new case: a station on a quota entitlement whose licence is up to date may be reissued when the body names `credits ≥ 1` (supersede reason `"credits added"`). An up-to-date station with no or zero credits stays 409 `MSG_LICENCE_UP_TO_DATE`.
- **Download:** `GET /api/licences/:id/download` serves the stored licence document unchanged when that licence has no grant, and the pretty JSON `LicenseFile` (stored licence + stored grant) when it has one; same headers and audit as today.
- **Release** of a station on a quota entitlement records a return of `min(credit_balance, credits granted to that station − credits already returned for it)`, 0 when nothing was reported, in the release transaction, and audits it.
- **Offering admin:** `OfferingRequest` gains `#[serde(default)] credit_target_reserve: Option<i64>` (top level, not in `grants`). With a quota: absent → 50, else 1..=1,000,000,000 (400 `credit_target_reserve must be between 1 and 1000000000`). Unlimited with a value → 400 `credit_target_reserve applies only to an offering with an erasure quota`. `interval: annual` with a counted quota → 400 `grants.quota_erasures must be unlimited for an annual offering: offerings with erasure credits are one-off until unused credits at renewal are decided`. Included in `same_terms`/`changes`.
- **Audit** (same transaction, ADR-0007 §6): `credits.granted {grant_id, entitlement_id, station_id, licence_id, credits, available_after}` subject `entitlement:<id>`; `credits.returned {entitlement_id, station_id, credits, reported_at}` subject `entitlement:<id>`; actor as the surrounding action (`user:<id>`).
- **Responses and mirrors** (same commit): `Entitlement` gains `credits: { purchased, granted, returned, available, target_reserve } | null` (null when unlimited); `Station` gains `credit_balance`, `credit_balance_reported_at` (nullable) and `credits_granted` (Σ for that station); register/reissue responses gain `credit_grant: { id, credits } | null`; admin `Offering` gains `credit_target_reserve`. Portal mirrors in `api/orders.ts`, `api/stations.ts`, `api/adminOfferings.ts`.

**Never:**
- No change to the signed `LicenseBody`/`Entitlements`/`Quota` (the licence still carries `Quota::Count` of the order quota; marking a licence as pool-backed is story 4), no station-side grant reserve, spending, top-up or balance-report route (story 4), no renewal work (stories 5–6).
- No grant for enrollment approval (`enrollments::approve` registers with `credits: 0`) or for the station-link renewal fetch (`reissue` with 0): their responses carry the bare licence only, and connected stations get credits by story 4's top-up. No grant in `stations::vendor_issue`.
- No editing of applied migrations, no retroactive rewrite of existing offerings, orders, entitlements or licences, no DB CHECK for annual+quota (existing rows may exist; the validator refuses new ones).
- No change to erasure-certificate schema, fixtures, or the v2 fixtures.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Grant round trip | grant issued by root R for key K | `verify([R])` ok; `check_grant_for(K)` ok | — |
| Grant tampered | credits edited in JSON | `Tampered` | — |
| Grant foreign | other root / other key | `UntrustedRoot` / `WrongInstanceKey` | — |
| Licence parsed as grant / vice versa | swap documents | parse fails | — |
| Bare licence file | today's file | `parse_license_file` → grant `None`; `parse_license` unchanged | — |
| Register default | pool 5000, target 50, no `credits` | 201; file is `LicenseFile` with grant 50 for the key; pool available 4950; `credits.granted` audited | — |
| Register small pool | trial pool 25, target 50 | grant 25; available 0 | — |
| Register explicit 0 | `credits: 0` | 201; bare licence file; no grant row | — |
| Register too many | `credits` > available | 409 `MSG_NOT_ENOUGH_CREDITS` | nothing written |
| Register empty pool | available 0, omitted | 201, no grant | — |
| Credits on unlimited | `credits: 5` | 400 | nothing written |
| Negative credits | `credits: -1` | 400 | nothing written |
| Race for last credits | available 60, two concurrent registers of 50 on a 2-seat entitlement | one 201, one 409; Σ granted 50 | — |
| Top-up reissue | up-to-date pool station, `credits: 20` | 200, new licence + grant 20, old superseded `"credits added"` | — |
| Reissue nothing to do | up-to-date, `{}` or `credits: 0` | 409 `MSG_LICENCE_UP_TO_DATE` | nothing written |
| Release with report | station granted 50, `credit_balance` 30 | return 30; available +30; `credits.returned` audited | — |
| Release over-report | granted 50, balance 80 | return 50 | — |
| Release unreported | balance NULL | return 0 recorded and audited | — |
| Download with grant | licence with grant | `{license, credit_grant}` pretty JSON | — |
| Download without grant | licence without grant | stored document byte-identical | — |
| Admin annual + quota | `annual`, quota 500 | 400 annual message | nothing written |
| Admin target reserve | quota 500, absent / 80 / 0 / with unlimited | 50 / 80 / 400 / 400 | — |
| Enrollment approval | pool entitlement | licence, no grant, pool unchanged | — |

## Autonomous decisions

- **How a grant travels in a licence file.** *About:* the file an owner downloads for an air-gapped bench. *Problem:* the file today is the bare signed licence, and the licence is also embedded in every erasure certificate. *Options:* (A) add a grant field to the licence type, which would copy grants into certificates; (B) a wrapper `{license, credit_grant}` that parsers also accept in bare form; (C) a separate grant file, a second thing to copy across. *Choice:* B: the licence and its stored bytes stay unchanged, certificates never carry grants, one file still installs, and every current parser accepts both forms in the same commit.
- **Domain separation of the two signed documents.** Grant and licence bodies are distinct structs with different required keys (`grant_format_version` vs `license_format_version`), so a signature over one can never verify as the other; a test pins it. No extra prefix needed.
- **Defaults for credits.** Register and renewal reissue default to `min(target reserve, available)` as CAP-3 says; enrollment approval and the station-link fetch grant nothing, because their responses carry only the licence and a grant nobody receives would strand credits until release (safe default: never move credits a station cannot get). An up-to-date reissue requires an explicit count, so pressing a button never silently moves credits.
- **Return cap.** A release returns at most what that station was granted and has not returned, so a wrong report can never grow the pool beyond what was bought (safe default; story 4 flags reports that do not add up).
- **Target reserve storage.** An admin-set, frozen offering column read through the order's offering (offerings are frozen once published), not a new order snapshot column; NULL means 50, so existing offerings and test helpers need no rewrite. The top-up threshold (default 10) is added with its first user, story 4.
- **Annual + quota refusal** is validator-only: existing rows are untouched and a NOT VALID CHECK would block retiring them.
- **Licence quota left as is.** The licence keeps `Quota::Count` of the order quota until story 4 teaches the station pools; changing the signed `Quota` now would break current parsers for no gain.
- **Wording.** `MSG_NOT_ENOUGH_CREDITS` = `not enough credits left in this entitlement's pool; choose fewer credits`, and `MSG_CREDITS_UNLIMITED` = `credits apply only to an entitlement with an erasure quota; leave credits empty` (house style: lowercase, remedy after a semicolon). Portal labels ("Credits", "Credit pool", "Last reported credits", "Not reported yet", "Add credits") are drafted functional UI text, not marketing copy.

</intent-contract>

## Code Map

- `crates/wipe-license/src/chain.rs:30` `LICENSE_FORMAT_VERSION`, `:77` `LicenseBody`, `:101` `LicenseCertificate`, `:107` `LicenseSignature` (reuse for the grant), `:117` `VendorRoot` (`issue_with_lapse_policy` `:165` shows the signing steps `:182-195`), `:201` `verify` (factor the hash/signature/root core into a private fn used by both) -- licence model to mirror.
- `crates/wipe-license/src/install.rs:22` `load_license`, `:36` `parse_license`, `:41` `save_license`, `:60` `InstallProblem`, `:86` `check_installable` -- add `LicenseFile`, `parse_license_file`; keep `save_license` writing the bare licence.
- `crates/wipe-license/src/lib.rs` -- module list and re-exports (`pub use credit::*`), `LicenseError`.
- `crates/wipe-license/tests/lapse_policy.rs` -- fixture-pin pattern (`include_str!`, hard-coded root pubkey hex) to copy into `tests/credit_grant.rs`; `tests/attestation.rs` behavioural pattern. Root in tests: `VendorRoot::from_signing_key(SigningKey::from_seed([7; 32]))`; `time` crate, not chrono.
- `crates/wipe-cert/src/canonical.rs:25` `canonical_bytes`; `sign.rs:53` `public_key_id` -- read-only.
- `crates/wipe-hub/migrations/0010_offerings.sql`, `0013_entitlements.sql`, `0014_stations.sql` (stations trigger `:63-85` freezes registration columns and released rows — new balance columns are not registration columns), `0015`, `0020_trial_offerings.sql:35-61` (latest `offerings_lifecycle` to `CREATE OR REPLACE` with `credit_target_reserve` added), `0002` (append-only trigger pattern) -- read-only; next file is `0022_credit_pool.sql`.
- `crates/wipe-hub/src/offering_admin.rs:40-55` constants, `:89` `OfferingRequest`, `:102` `GrantsRequest`, `:161` `validate` (annual+quota after the quota match `:203-212`), `Offering`, `same_terms`, `changes`, `Row`/select/insert/update SQL, unit tests `:650+`.
- `crates/wipe-hub/src/stations.rs:88-108` messages, `:132` `Licence`, `:174` `Station`, `:261` `LATEST_LICENCE`, `:273` `select_stations`, `:331` `RegisterRequest` + `:386` `validate`/`NewStation`, `:405` `StationError`, `:435` `LockedEntitlement` (+ `credit_target_reserve` via `JOIN offerings`), `:461` `Registered`, `:557` `sign_and_store` (only licence signer — add the grant beside it in a new `credits.rs` module called from it), `:647` `issue_licence`, `:704` `register`, `:849` `reissue(conn, root, org, who, id)` (add credits param), `:953` `release` (lock entitlement after station; write return), `:1026` `licence_document` (return licence + optional grant document).
- `crates/wipe-hub/src/station_routes.rs:58-62` routes, `:97` register, `:145` reissue (add optional `JsonBody` — the portal already sends `{}`; an empty body must still work), `:179`/`:197` download response.
- `crates/wipe-hub/src/enrollments.rs:550` `approve` → `stations::register` (pass `credits: Some(0)`); `src/station_link.rs:150,209` `fetch_licence` → `reissue` (pass 0) -- callers to update.
- `crates/wipe-hub/src/entitlements.rs:67-86` `Entitlement`, `:166` `list`, `:191` `get` -- add `credits` (one aggregate query, org-scoped).
- `crates/wipe-hub/src/audit.rs:28,75` `audit::record(&AuditEntry{actor, action, subject, detail})`; `auth::actor(user_id)`.
- `crates/wipe-hub/tests/support/hub.rs` (`spawn_hub_with_root`, `verified_owner`, `published_one_off_offering` quota 5000 / 2 site seats, `published_trial_offering` quota 25, `audited`, `assert_error`, `counts`), `tests/stations.rs` (`Fixture`, `buy`, `register`, `reissue`, `install` `:273` — extend to accept `LicenseFile`; quota asserts `:495,:1258,:1656` stay), concurrency patterns `tests/stations.rs:896,954`, `tests/orders.rs:1177`; `tests/isolation.rs:358` `rows_of` (add the two ledger tables), `tests/offering_admin.rs`, `tests/migrations.rs`.
- `apps/portal/src/api/stations.ts` (`Licence`, `Station`, guards, `registerBody`, `registerStation`, `reissueStation` posting `{}`), `api/orders.ts:81-100` (`Entitlement`, `Seats`), `api/adminOfferings.ts:56,101` (`Offering`, guards), `pages/StationsPage.tsx` (`RenewalAction :206`, `StationRow :259`, `RegisterForm :310`, helpers), `pages/SeatsPage.tsx` (`EntitlementTable`), `pages/OfferingForm.tsx` (quota `:51`, `:96-112`, `offeringBody`), with their `*.test.ts` (static render + fake fetch patterns).
- `CONTEXT.md` §9 (pool exists at the hub), §10.2 (new row), §14 glossary (**Credit pool**, **Credit grant**), `README.md` hub section, `docs/ROADMAP.md` if it tracks CAP-3.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-license/src/credit.rs`, `chain.rs`, `install.rs`, `lib.rs` -- grant type, issue, verify, `check_grant_for`, `LicenseFile`, `parse_license_file`, both-form `parse_license` -- the signed document CAP-3 asks for.
- `crates/wipe-license/tests/credit_grant.rs` + `tests/fixtures/credit-grant-v1.json` -- matrix rows 1–5 and the fixture pin.
- `crates/wipe-hub/migrations/0022_credit_pool.sql` -- ledger tables, append-only triggers, station balance columns, offering target reserve + lifecycle trigger.
- `crates/wipe-hub/src/credits.rs` (new) + `stations.rs`, `station_routes.rs`, `entitlements.rs`, `offering_admin.rs`, `enrollments.rs`, `station_link.rs`, `lib.rs` -- pool reads, grant issue + audit, return on release, request/response fields.
- `crates/wipe-hub/tests/credits.rs` (new) and updates to `stations.rs`, `offering_admin.rs`, `isolation.rs`, `migrations.rs`, `support/hub.rs` -- every hub matrix row over HTTP, audit rows, the race, append-only refusal, isolation.
- `apps/portal/src/api/*.ts`, `pages/{StationsPage,SeatsPage,OfferingForm}.tsx` and tests -- mirrors and guards; Seats shows each quota entitlement's pool (available of purchased, granted, returned); Stations shows each station's credits granted and last reported balance ("Not reported yet"); the register form has a Credits number field for quota entitlements, prefilled with `min(target_reserve, available)`; a quota station row has "Add credits" (count input → reissue with `credits`); offering form has a target-reserve field shown with a counted quota.
- `CONTEXT.md`, `README.md`, `docs/ROADMAP.md` -- what shipped and its limits (licence quota unchanged, stations ignore grants until story 4, balances unreported until story 4).

**Acceptance Criteria:**
- Given a verified owner with a one-off quota entitlement, when they register an air-gapped station on the portal Stations page with the default credits and download its licence file, then the file contains the licence and a credit grant for the station's key that `check_grant_for` accepts against the vendor root, and the Seats page shows the pool reduced by that amount.
- Given that downloaded file, when it is installed on a station through `POST /api/license`, then the install succeeds exactly as a bare licence would.
- Given `cargo test --workspace` and the portal suite, when run, then all pass with the new cases.

## Spec Change Log

## Review Triage Log

### 2026-11-10 — Review pass
- verdicts: 34 findings — high 0, medium 5, low 21, false 7, maybe-false 1
- findings:
  - `[medium]` `[patch]` Blind: renaming a quota offering published before 0022 (stored reserve NULL, read as 50) writes 50 over NULL, the lifecycle trigger raises and the request 500s — `update` now keeps a non-draft offering's stored reserve (`CASE WHEN status = 'draft'`); the rename is tested.
  - `[low]` `[reject]` Blind: until story 4, default grants leave the pool and releases return 0, so credits stay out of an append-only ledger — CAP-3 itself sets the default (target reserve) and the return (last reported balance); balance reports are CAP-4. Recorded under residual risks.
  - `[low]` `[patch]` Blind: the register form always sends the client's suggestion, so a stale seats cache gives 409 where the hub default would succeed — `registerInput` omits `credits` until the owner edits the field.
  - `[false]` `[reject]` Blind: the grant download re-parses the licence and drops fields the struct does not model — every stored licence was written by this hub from this struct (including `lapse_policy`), so nothing is dropped; the no-grant path still serves the stored bytes.
  - `[low]` `[patch]` Blind: older station builds cannot parse the wrapper, and `deny_unknown_fields` on `LicenseFile` breaks today's stations when a later story adds a key — nothing is deployed (ADR-0013 §5: nothing sold); `deny_unknown_fields` removed, and the test now asserts an unknown key is ignored and a bad grant still refused.
  - `[low]` `[reject]` Blind: reissue reads raw bytes and skips `JsonBody`'s 415 content-type check — every state-changing route still needs the CSRF token, so this is only defence in depth; the fix adds a guard for a case nobody meets in normal use.
  - `[low]` `[patch]` Blind: the reissue lock-order comment still says release locks only the station — comment rewritten.
  - `[low]` `[patch]` Blind: `CreditPool` was inserted under `Entitlement`'s doc comment in `api/orders.ts` — moved, with its own comment.
  - `[low]` `[patch]` Blind: while seats are loading or failed, a quota station shows "Unlimited" and loses "Add credits" — rows get `pool="unknown"` and show "…"; tested.
  - `[low]` `[patch]` Blind: the release confirmation implies granted credits come back — reworded: only credits the station last reported return; unreported credits do not.
  - `[low]` `[reject]` Blind: the grant body has no expiry or entitlement binding — CAP-3 lists the grant's fields (id, organization, instance key id, credits, issued time); adding fields means editing this spec. Story 4 owns how stations spend grants.
  - `[low]` `[reject]` Blind: a legacy published annual offering with a counted quota can no longer get copy edits — no such rows exist outside dev databases (nothing sold), and the fix adds a branch; it can still be retired.
  - `[low]` `[reject]` Blind: the renewal button moves the default credits without showing them — the spec sets omitted = default for renewal reissue; renewals with a quota exist only on legacy annual quota entitlements, which can no longer be created.
  - `[low]` `[patch]` Blind: "target reserve" is used as a domain term with no §14 glossary entry — entry added.
  - `[low]` `[reject]` Blind: ended or refunded entitlements still show credits as available on Seats — rare and harmless (the hub refuses grants from them); the fix adds a branch.
  - `[low]` `[reject]` Edge: renewal reissue `{}` moves credits silently — same as the Blind renewal row.
  - `[low]` `[reject]` Edge: a pre-0022 draft annual offering with a quota can be published without re-validation — only pre-existing dev drafts can reach this; the fix adds a guard.
  - `[false]` `[reject]` Edge: a released station's returned balance is re-granted while its old grant file still verifies offline — ADR-0013 §3 states this limit (stations spend grants offline; the hub flags reports that do not add up, story 4). Returning the last reported balance is what CAP-3 requires.
  - `[low]` `[patch]` Edge: seats loading treated as unlimited — same fix as the Blind row.
  - `[low]` `[patch]` Edge: stale explicit credits from the register form — same fix as the Blind row.
  - `[maybe-false]` `[reject]` Edge: deadlock safety between reissue (`FOR UPDATE OF s, e` in one statement) and release (station, then entitlement) depends on the planner's lock order — a cycle needs a concurrent reissue and release of the same station, and Postgres would abort one with a deadlock error rather than corrupt anything; settling it needs a stress test; if true it would be low. Stale comment patched (Blind row).
  - `[false]` `[reject]` Edge: a grant whose body `root_key_id` differs from `signature.public_key_id` still verifies — the body is covered by the vendor signature, so only the vendor root could produce such a grant.
  - `[false]` `[reject]` Edge: a stale higher reported balance returns credits already spent offline — the intent says release returns the last reported balance; the cap at outstanding credits stops the pool growing past what was bought.
  - `[medium]` `[patch]` VG: renaming a published NULL-reserve offering is untested — rename test added (with the Blind fix).
  - `[medium]` `[patch]` VG: nothing proves the station-link renewal fetch grants no credits on a pool — `a_renewal_fetch_on_a_credit_pool_grants_nothing` added.
  - `[medium]` `[patch]` VG: the portal's credits wiring into register and reissue is untested — `registerInput` extracted and tested for untouched, edited (20, 0) and unlimited.
  - `[low]` `[patch]` VG other: the register form always sends explicit credits — same fix as the Blind row.
  - `[low]` `[reject]` VG other: legacy annual quota offerings cannot be edited — same as the Blind row.
  - `[low]` `[patch]` VG other: the misplaced doc comment in `api/orders.ts` — same fix as the Blind row.
  - `[false]` `[reject]` Intent: the pool does not limit erasures at the station — CAP-4 builds the station reserve and the start gate's use of it; CAP-3's success list is hub, licence-file and portal work.
  - `[false]` `[reject]` Intent: a non-zero release return is reachable only through SQL — balance reporting is CAP-4 ("reporting its balance"); CAP-3 records the return from whatever was last reported.
  - `[medium]` `[patch]` Intent: AC1's portal default and the hub default were different code paths — the form now leaves an untouched count to the hub (same fix as the register-form rows).
  - `[low]` `[reject]` Intent: the station e2e test builds its own `LicenseFile` instead of using hub bytes — the hub test parses the downloaded text with the same `parse_license`; joining the two crates in one test adds little.
  - `[false]` `[reject]` Intent: only the target reserve is per offering, not the threshold — the threshold is CAP-4's ("the offering's threshold (default 10)"), recorded in Autonomous decisions.

### 2026-11-10 — Review pass (resumed run)
- verdicts: 33 findings — high 0, medium 2, low 19, false 11, maybe-false 1
- findings:
  - `[low]` `[reject]` Blind: default grants leave a trial pool empty and every release returns 0 until story 4 — carried: same claim as the first pass's row; CAP-3 sets the default and the return, balance reports are CAP-4.
  - `[low]` `[reject]` Blind: "Add credits" on a connected station signs a grant its station-link fetch never delivers — the grant is in the file the success notice links to, stations ignore grants until story 4, and story 4 builds the connected top-up; hiding the action adds a branch for a pre-story-4 state.
  - `[low]` `[patch]` Blind: the reissue comment "pressing a button never silently moves credits" contradicts the Renew button, which takes the default reserve — comment reworded to say an up-to-date licence is reissued only for a named count ≥ 1 and a renewal takes the default; the default itself is the intent's (carried reject).
  - `[low]` `[reject]` Blind: the grant has no expiry or licence/entitlement binding — carried: CAP-3 lists the grant's fields; story 4 owns spending.
  - `[false]` `[reject]` Blind: the grant download re-parses the licence and drops unmodelled fields — carried: every stored licence was written by this hub from this struct.
  - `[low]` `[reject]` Blind: legacy annual offerings with a quota can no longer be edited and can still be bought — carried: no such rows exist outside dev databases (nothing sold); the fix adds branches.
  - `[false]` `[reject]` Blind: the per-offering top-up threshold is missing — carried: the threshold is CAP-4's, recorded in Autonomous decisions.
  - `[false]` `[reject]` Blind: the target reserve is read from the live offering, not the order snapshot — the `offerings_lifecycle` trigger freezes it at publication, so the live value is the value bought; the read path is an Autonomous decision.
  - `[low]` `[reject]` Blind: the release confirmation does not say releases return 0 today — the text ("only the credits it last reported holding return; unreported credits do not") is accurate, and further wording is customer-visible copy; the limit is in CONTEXT.md and README.
  - `[low]` `[reject]` Blind: a malformed grant makes `parse_license` refuse an otherwise valid licence file — refusing a damaged or tampered file is the safe default; pinned deliberately in the first pass.
  - `[low]` `[reject]` Blind: no tests for release racing register/top-up or revoke — release takes station then entitlement locks, the same order as every other path, and `a_revoke_waits_for_a_reissue_holding_the_station_lock` covers the station lock; the station-link no-grant test exists (`a_renewal_fetch_on_a_credit_pool_grants_nothing`).
  - `[false]` `[reject]` Blind: ADR-0013 "inside their licence file" contradicts CONTEXT "never inside the licence" — CONTEXT §14 **Credit grant** already says the grant travels beside the licence in the licence file `{license, credit_grant}`, which is the ADR's reading.
  - `[maybe-false]` `[reject]` Edge: reissue (`FOR UPDATE OF s, e`) vs release (station, then entitlement) deadlock — carried: Postgres would abort one rather than corrupt anything; if true it would be low.
  - `[low]` `[reject]` Edge: a renewal reissue with `{}` grants the default reserve — carried: the intent's default for a renewal reissue.
  - `[low]` `[reject]` Edge: superseding a grant-bearing licence leaves its credits counted as granted — grants are a per-station cumulative ledger; release returns up to granted − returned for the station, so they are not lost beyond the documented pre-story-4 limit, and the superseded file stays downloadable.
  - `[low]` `[reject]` Edge: legacy annual-quota offering edits refused — carried (Blind row).
  - `[low]` `[reject]` Edge: after "Add credits" the success notice hides the input until reload — the same notice-then-reload pattern as `RenewedNotice`; the download link is the priority, and the fix adds state.
  - `[low]` `[reject]` Edge: a malformed station id with negative credits answers 400, not 404 — both are refusals that write nothing; reordering parsing for an invalid request nobody sends is not worth a change.
  - `[false]` `[reject]` Edge: `root_key_id` vs `signature.public_key_id` mismatch verifies — carried: only the vendor root could sign such a body.
  - `[low]` `[patch]` Edge (claim): the reissue comment's "never silently moves credits" — same root as the Blind comment row; same fix.
  - `[low]` `[reject]` Edge (claim): credits are stranded anyway by supersession — same as the supersession row above.
  - `[medium]` `[patch]` VG: the vendor-admin download (`GET /api/admin/licences/:id/download`) served the bare licence without the grant the customer download includes — both downloads now build the file through one `credits::licence_file`; `the_vendor_admin_downloads_and_inspects_the_same_licence_file` asserts the admin bytes equal the customer's and carry the 50-credit grant.
  - `[medium]` `[patch]` VG: admin inspect (`POST /api/admin/licences/inspect`) refused a downloaded `{license, credit_grant}` file — it now parses with `wipe_license::parse_license_file` and inspects the licence (the audited hash was already over the licence body); the same test pastes the customer file and gets `verified` and the record.
  - `[low]` `[patch]` VG other: release and renewal refreshed only the stations query, leaving the Seats pool and the credit limits stale — both now also invalidate `["seats"]`.
  - `[false]` `[reject]` Intent: stations are not bounded by the pool — the intent's Never section defers that to story 4, and the docs say so.
  - `[false]` `[reject]` Intent: the grant download is rebuilt, not byte-embedded — carried (Blind row).
  - `[false]` `[reject]` Intent: unlimited up-to-date `credits: 0` → 400 vs 409 — the intent says any non-null `credits` on an unlimited entitlement is 400; the precedence follows it.
  - `[low]` `[reject]` Intent: existing annual-quota offering edits now refused — carried (Blind row).
  - `[false]` `[reject]` Intent: release returns tested only through a SQL-set balance — carried: balance reporting is CAP-4.
  - `[false]` `[reject]` Intent: the race test cannot tell the lock from the post-insert guard — the guard's failure is a 500, which fails the test's one-201-one-409 assertion, so the test does tell them apart.
  - `[false]` `[reject]` Intent: grant downloads' headers and audit are untested — `licence_document` audits before the grant branch and the route sets headers independently of the body, so both paths share them; the no-grant path's tests cover them.
  - `[low]` `[reject]` Intent: portal coverage is static render plus pure functions — that is the portal's test convention; clicking flows need a browser harness the repo does not have.
  - `[low]` `[reject]` Intent: `--license` covered only through `load_license` — the CLI and bootstrap call that same function.

## Design Notes

Lock order stays station → entitlement everywhere (`reissue`'s existing comment at `stations.rs:856`); `register` locks only the entitlement. Compute `available` with one `SELECT` under that lock:

```sql
SELECT o.quota_erasures
     - COALESCE((SELECT sum(credits) FROM credit_grants WHERE entitlement_id = $1), 0)
     + COALESCE((SELECT sum(credits) FROM credit_returns WHERE entitlement_id = $1), 0)
```

Licence file with a grant:

```json
{ "license": { "body": { "license_format_version": 1, ... }, "signature": { ... } },
  "credit_grant": { "body": { "grant_format_version": 1, "grant_id": "…", "credits": 50, ... }, "signature": { ... } } }
```

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace --no-fail-fast` -- expected: all pass (never piped through grep).
- `cargo clippy -p wipe-license -p wipe-hub --all-targets -- -D warnings` -- expected: clean.
- `UPDATE_GOLDENS=1` is not needed: no route path, CLI help or `Feature` changes (check `cargo test -p wipe-server --test route_golden` still passes).
- `cd apps/portal && pnpm typecheck && pnpm test && pnpm build` (or `pnpm --filter portal …`) -- expected: pass.
- `rustfmt --edition 2021 <touched .rs files>` -- only touched files.

## Auto Run Result

Status: done

**Summary.** Each entitlement with an erasure quota is now one organization-wide credit pool at the hub (ADR-0013 §3). `wipe-license` gained a vendor-signed `CreditGrant` (verified offline against the same roots as licences) and a licence file `{license, credit_grant}` that every licence parser also accepts in bare form, so today's stations install it and ignore the grant. The hub keeps append-only grant and return ledgers (migration 0022). Register and reissue take an optional `credits` count (default `min(target reserve, available)`, 409 over the pool, 400 on unlimited entitlements). An up-to-date station can be topped up. Release returns the last reported balance, capped at what the station holds. Annual offerings with a quota are refused, and offerings have a per-offering target reserve. The portal shows the pool on Seats, each station's credits on Stations, a Credits field on register, "Add credits", and the target reserve on the offering form.

**Files changed.**
- `crates/wipe-license/src/credit.rs` (new): `CreditGrant`, `VendorRoot::issue_credit_grant`, `verify`, `check_grant_for`.
- `crates/wipe-license/src/{chain,install,lib}.rs`: shared verify core, `LicenseFile`, `parse_license_file`, both-form `parse_license`.
- `crates/wipe-license/tests/credit_grant.rs`, `tests/fixtures/credit-grant-v1.json` (new): grant matrix rows and wire-shape pin.
- `crates/wipe-hub/migrations/0022_credit_pool.sql` (new): ledgers, append-only triggers, station balance columns, offering target reserve and lifecycle trigger.
- `crates/wipe-hub/src/credits.rs` (new): pool reads, grant issue and audit, return on release, shared `licence_file` download builder.
- `crates/wipe-hub/src/{stations,station_routes,entitlements,offering_admin,enrollments,station_link,licence_admin,lib}.rs`: request and response fields, locking, downloads, admin download and inspect accept the licence file.
- `crates/wipe-hub/tests/{credits(new),stations,offering_admin,isolation,migrations,entitlements,enrollment_approval,station_licence}.rs`: every hub matrix row over HTTP.
- `crates/wipe-server/tests/http_e2e.rs`: a file carrying a grant installs through `POST /api/license`.
- `apps/portal/src/api/{stations,orders,adminOfferings}.ts`, `pages/{StationsPage,SeatsPage,OfferingForm}.tsx` and tests: mirrors, guards, pool and credit UI.
- `CONTEXT.md`, `README.md`, `docs/ROADMAP.md`: what shipped and its limits.

**Review findings (resumed pass, 33 findings).**
- Patched (2 medium, 2 low):
  - The vendor-admin download now serves the same licence file as the customer download (one `credits::licence_file`).
  - Admin inspect accepts the `{license, credit_grant}` file.
  - Release and renewal refresh the portal's Seats pool.
  - The misleading reissue comment is reworded.
  - New test: `the_vendor_admin_downloads_and_inspects_the_same_licence_file`.
  - `CONTEXT.md` describes the admin download and inspect accordingly.
- Deferred: none.
- Rejected: 18 low, 11 false, 1 maybe-false, each with its reason in the Review Triage Log above.

**Follow-up review recommendation:** false. Two medium entries were patched, but both are covered by the new HTTP test. A search found no other site that parses a stored or pasted licence document as a bare `LicenseCertificate` and could receive the new file (`licence_admin.rs:456` reads the hub's own stored bare licence). No unverified risk remains to name. Patched counts: high 0, medium 2, low 2.

**Verification.**
- `cargo test --workspace --no-fail-fast`: hit the 1-hour background limit on this machine.
  - Every binary it reached passed except `wipe-hub` `health::silent_smtp_and_database_time_out_within_bound`.
  - The rest was then run separately and passed: `wipe-hub` `stations`/`stripe`/`team`, `-p wipe-license`, `-p wipe-server` (including `route_golden`) and `wipe-hub --doc`. `credits` and `licence_admin` passed after the patch.
- The health test failure is pre-existing and environmental. It also fails on baseline `a54e76cb5310a0e0dd96882cd04316928a2b1983`, run in a temporary worktree (took 24 s against a 3.5 s bound). Here it took 12–44 s. The only hub start-up change in this story is `pub mod credits;`. The cause is the host: `syspolicyd` (macOS code-signing checks) was using ~60 % CPU and stalling every freshly built binary.
- `cargo clippy -p wipe-license -p wipe-hub --all-targets -- -D warnings`: clean.
- `pnpm --filter portal typecheck`, `test` (292 passed) and `build`: pass.
- `rustfmt --edition 2021` was run on the touched `.rs` files only.

**Residual risks.**
- Until story 4, stations ignore grants and report no balance. Default grants therefore leave the pool and every release returns 0, so a trial pool can be used up by registrations. This is documented in CONTEXT.md and README.
- Adding credits to a connected station puts the grant only in the downloadable file; the station-link fetch carries the bare licence.
- Reissue and release lock the same rows in possibly different planner orders. A concurrent reissue and release of one station could hit a Postgres deadlock abort (unverified; it would not corrupt anything).
- The portal pages were checked by typecheck, unit tests and a build, not in a real browser.

