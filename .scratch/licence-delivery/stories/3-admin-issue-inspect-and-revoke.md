---
title: 'Admin issue, inspect and revoke'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '7c7229779f9fa047acd0b03a3ddc2333c6f9a36a'
review_loop_iteration: 0
followup_review_recommended: false
context:
  - '{project-root}/docs/adr/0005-licensing-and-attestation.md'
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
warnings: [oversized]
deferred: []
---

<intent-contract>

## Intent

**Problem:** Vendor staff can only issue a licence with the `wipestation license issue` CLI and a root seed on a laptop, with no audit trail, no way to look at a licence the hub signed, and no way to record that one is revoked (SPEC CAP-4; ROADMAP A1.2, A-R1, A-R7).

**Approach:** Add three vendor-admin actions to the hub's admin area. (1) **Issue**: a vendor admin signs a licence for any customer organization (a trial, an evaluation or a goodwill grant) with entitlements typed by hand, outside any entitlement and seat. (2) **Inspect**: open any licence the hub holds, or paste any licence file, and see its decoded body and whether it verifies against the hub's vendor root. (3) **Revoke**: mark a licence the hub holds `revoked` with a reason. Every action is audited. A revoked station licence shows as revoked on the customer's Stations page.

## Boundaries & Constraints

**Always:**
- Only a `VendorAdmin` (the existing extractor in `admin_routes.rs`, first extractor of every handler) reaches the new routes: no session 401, anyone else 403 `MSG_ADMIN_ONLY`, before any path, body or data is read.
- Sign only through `wipe_license::VendorRoot::issue` with the hub's configured root, and store the exact `serde_json::to_string_pretty` text, sharing one sign-and-store core with `stations::issue_licence` so the three issuing paths cannot drift. `stations.rs` stays the only code that writes `licences`; reads and inspection live in a new `licence_admin.rs`.
- A vendor-issued licence: `customer_id`/`customer_name` = the organization's id and current name, `not_before` = `issued_at` = now (microseconds), `not_after` = now + `days`, `allowed_methods: All`, `machine_binding: None`. Machine scope → `Scope::Machine { fingerprint: machine_fingerprint(station_id) }` (station id required); site scope → `Scope::Site { site_id }` with the entered site, or the organization id when blank. It belongs to no station and no entitlement, uses no seat, and is stored with `source = 'vendor'` and its `issue_reason`.
- Revoke: allowed from `active` or `superseded`; `revoked` is terminal. It locks the licence's station (if any) `FOR UPDATE` before the licence row, the same order as reissue and release, so it serializes with them. A station whose licence is revoked keeps its registration and seat; it can be released as before, but not reissued (409 `MSG_LICENCE_REVOKED`).
- Revocation is recorded in the hub only. The admin licence page states plainly that stations are not told and the file keeps verifying offline until its `not_after` (ADR-0005 consequences, ADR-0007 §5); nothing here can stop a station erasing (ROADMAP §0.2).
- Audit, actor `user:<admin id>`, in the request's transaction: `licence.issued` (subject `licence:<id>`, detail `{organization_id, instance_key_id, root_key_id, not_after, source: "vendor", scope, quota_erasures, features, reason}`), `licence.revoked` (`{organization_id, station_id, previous_status, reason}`), `licence.downloaded` (`{organization_id, by: "vendor"}`), `admin.organization_licences_viewed` (subject `organization:<id>`, `{licence_ids}`), `admin.licence_viewed`, `admin.licence_file_inspected` (subject `inspect:licence-file`, `{license_id, canonical_sha256_hex, verified, known}`). A lookup that finds nothing writes nothing.
- The vendor organization is never a target: its id is 404 `MSG_NO_ORGANIZATION` like an unknown one. A non-UUID licence id is 404 `MSG_NO_LICENCE`.
- The revocation reason is shown to vendor admins only; the customer sees "Revoked on <date>" without it.

**Never:** No revocation list, push or online check (ROADMAP A2.3, ADR candidates 0008/0010); no station-side code; no change to `wipe-license`; no edit of an applied migration (add `0015`); no seat or entitlement change from any admin action; no lapse policy (story 4); no hardware-sanitization claim in copy.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Issue (machine) | admin, customer org, valid key, `scope:"machine"`, `station_id`, `quota_erasures` null or n, `features`, `days` 30, `reason` | 201 `{licence}`; downloaded file passes `check_installable` + `evaluate(.., machine_fingerprint(station_id), now)` `Valid`; customer id/name = org; `not_after` = now + 30 d; no station row, seat counts unchanged; audited | — |
| Issue (site) | `scope:"site"`, site blank / given | `Scope::Site` with org id / site | — |
| Bad issue body | bad key, unknown scope or feature, quota outside 1..=`QUOTA_MAX_ERASURES`, `days` outside 1..=1095, blank or >500-char or control-char reason, machine without station id, unknown field | 400 naming the field | nothing written |
| Issue target | unknown, non-UUID or vendor org | 404 `MSG_NO_ORGANIZATION` | nothing written |
| No vendor root | hub started without seed | issue 503 `MSG_ISSUANCE_UNAVAILABLE`; view/inspect still work with `verification.status: "no_root"` | nothing written |
| List | `GET /api/admin/organizations/:id/licences` | `{licences}` every licence of the org, station and vendor, newest first, each with `source`, status, station label/station id when any | audited |
| View | `GET /api/admin/licences/:id` | `{licence, body, verification}`: `body` = the stored file's `LicenseBody`, `verification` `{status: "verified"\|"failed"\|"no_root", root_key_id, detail}` | unknown → 404, nothing audited |
| Inspect file | `POST /api/admin/licences/inspect {document}` | `{body, verification, licence}` (`licence` = the hub's record with that `license_id` or null) | not JSON / not a licence / > 64 KiB → 400 `MSG_NOT_A_LICENCE`, nothing audited |
| Tampered file | pasted file with an edited field | `verification.status: "failed"`, detail names tampering | — |
| Admin download | `GET /api/admin/licences/:id/download` | stored bytes, same headers as the customer route | audited |
| Revoke | `POST /api/admin/licences/:id/revoke {reason}` on active or superseded | 200 `{licence}` status `revoked`, `revoked_at`, `revocation_reason`; audited | already revoked → 409 `MSG_ALREADY_REVOKED`; bad reason → 400 |
| Customer after revoke | owner/member | `GET /api/stations` shows licence `status:"revoked"`, `revoked_at`, no reason; `renewal_available:false`; reissue 409 `MSG_LICENCE_REVOKED`; customer download 409 `MSG_LICENCE_REVOKED`; release still 200 | — |
| Not an admin | none / owner / member | 401 / 403 `MSG_ADMIN_ONLY` | nothing read or written |

## Autonomous decisions

- **What a vendor-issued licence draws from.** SPEC CAP-4's examples (evaluation, trial) have no purchase behind them. Options: (a) require an entitlement and a free seat, (b) issue outside entitlements and seats. Chosen (b): it is what the SPEC describes and what the CLI already does; the admin page states it uses no seat, and every issue carries a mandatory reason in the audit log so each grant is attributable (ROADMAP A-R7).
- **Licence length cap.** A typo in a hand-typed expiry could grant a decades-long licence that can never be withdrawn offline. Chosen: a `days` count 1–1095 (three years), mirroring the CLI's `--days` (default 365), rather than a free date.
- **What revocation does to a station.** Options: (a) also release the station and free its seat, (b) leave the registration and seat alone. Chosen (b): revocation is a vendor statement about one file, not a seat action; the owner still decides to release. Reissue is refused, since a renewal would otherwise undo the revocation.
- **Customer download of a revoked licence** is refused (409), so the hub never hands out a file it has revoked; superseded ones stay downloadable as before.
- **The revocation reason is vendor-internal.** It may hold support or fraud notes; customers see only that and when it was revoked.
- **Inspecting pasted files** is included, because "decode and verify" a licence a customer sends to support is the CLI `license inspect` flow ROADMAP A-R1 asks to move into the console. Verification uses only the hub's own root.
- **Vendor-issued licences are not listed on the customer's Stations page** (they belong to no station); the vendor admin downloads the file and hands it over. Recorded as a limitation in CONTEXT §10.

</intent-contract>

## Code Map

- `crates/wipe-hub/src/stations.rs` -- only writer of `stations`/`licences`. `issue_licence` (l.~470-575): split into a sign-and-store core taking ready-made `Entitlements` + source/reason, used by register, reissue and the new `vendor_issue`. `reissue` (l.~700): add the revoked check before `LICENCE_BEHIND_TERM`. `licence_document` (l.~880): refuse revoked. `Licence` struct gains `revoked_at` (customer shape; no reason). `LATEST_LICENCE`, `LICENCE_BEHIND_TERM` already exclude non-active. `is_instance_key_id`, `optional_text`, `TEXT_MAX_CHARS`, `MSG_ISSUANCE_UNAVAILABLE`, `MSG_NO_LICENCE`, `RFC3339_UTC_MICROS`, `StationError` to reuse.
- `crates/wipe-hub/migrations/0014_stations.sql` -- read-only: `licences` columns, `licences_superseded_at_check`, the inline status CHECK (auto-named `licences_status_check`), `licences_lifecycle()` trigger to replace via `CREATE OR REPLACE FUNCTION` in 0015.
- `crates/wipe-hub/src/admin_routes.rs` -- `VendorAdmin` extractor, route list, handler pattern (one transaction, data layer audits), `MSG_NO_ORGANIZATION`, `MSG_ADMIN_ONLY`; module doc lists routes.
- `crates/wipe-hub/src/admin.rs` -- `audit_read`, `VendorAdmin`, vendor-org exclusion (`NOT o.is_vendor`), `RFC3339_UTC`.
- `crates/wipe-hub/src/offering_admin.rs:54` `FEATURES`, `QUOTA_MAX_ERASURES`, feature parse via `wipe_license::Feature` serde -- reuse for issue validation.
- `crates/wipe-hub/src/station_routes.rs` -- `From<StationError> for ApiError`, download response headers (reuse for the admin download), vendor-root 503 pattern.
- `crates/wipe-license/src/chain.rs` -- `LicenseCertificate::verify(&[root.verifying_key()])` → `Tampered`/`UntrustedRoot`/`Verification`; `LicenseBody` serializes as the `body` shape. Read-only.
- `crates/wipe-hub/tests/stations.rs` -- `Fixture`, `register_machine`, `download`, `install`, key-set pin of `Licence` (gains `revoked_at`). `tests/admin.rs` -- how a vendor admin is created and signed in. `tests/isolation.rs:147-167` -- `ROUTES` table: classify every new route `Admin`. `tests/migrations.rs` -- trigger-refusal test pattern.
- `apps/portal/src/api/admin.ts`, `adminOfferings.ts` (+ tests) -- admin client and guard patterns, `retryAdminQuery`. `apps/portal/src/pages/AdminOrganizationPage.tsx`, `AdminPage.tsx`, `main.tsx:231-302` -- admin page/route patterns. `apps/portal/src/api/stations.ts` `LICENCE_STATUSES`, `pages/StationsPage.tsx:62` `stationState`, l.256 download link.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0015_licence_admin.sql` -- `licences`: `station_id`/`entitlement_id` nullable; `source text NOT NULL DEFAULT 'station'` in (`station`,`vendor`), CHECK station ⇔ both set, vendor ⇔ both null; `issue_reason` (vendor ⇔ non-blank ≤500); status adds `revoked` with `revoked_at` and `revocation_reason` set exactly when revoked; superseded_at check relaxed to `active` ⇒ null, `superseded` ⇒ set; replace `licences_lifecycle()` so signed content, `source` and `issue_reason` never change and only active→superseded, active→revoked, superseded→revoked are allowed -- DB enforces the lifecycle.
- `crates/wipe-hub/src/stations.rs` -- shared sign-and-store core; `vendor_issue`, `revoke`; revoked handling in `reissue` and `licence_document`; `MSG_LICENCE_REVOKED`, `MSG_ALREADY_REVOKED`; `revoked_at` on `Licence`; module doc.
- `crates/wipe-hub/src/licence_admin.rs` -- `IssueRequest` validation (deny unknown fields), `AdminLicence` shape (incl. organization, source, station label/id, issue/revocation reason), `list_for_organization`, `detail`, `inspect_document` (64 KiB cap, `MSG_NOT_A_LICENCE`), verification summary; each read audited via `audit_read`.
- `crates/wipe-hub/src/admin_routes.rs`, `src/lib.rs` -- `GET|POST /api/admin/organizations/:id/licences`, `GET /api/admin/licences/:id`, `GET /api/admin/licences/:id/download`, `POST /api/admin/licences/inspect`, `POST /api/admin/licences/:id/revoke`; module doc.
- `crates/wipe-hub/tests/licence_admin.rs` -- one test per I/O row through HTTP, incl. installing a vendor-issued file, tampered paste, no-root hub, revoke during concurrent reissue yields no 500, customer-side effects. `tests/isolation.rs` -- new routes. `tests/migrations.rs` -- 0015 trigger and CHECK refusals. `tests/stations.rs` -- updated key-set pin.
- `apps/portal/src/api/adminLicences.ts` (+ test) -- typed client and guards. `apps/portal/src/api/stations.ts` (+ test) -- `revoked` status, `revoked_at`.
- `apps/portal/src/pages/AdminOrganizationPage.tsx`, new `AdminLicencePage.tsx` (`/admin/licences/$id`: body, verification, status, download, revoke form with confirm and the hub-only note), new `AdminLicenceInspectPage.tsx` (`/admin/licences/inspect`), `AdminPage.tsx` link, `main.tsx` routes; org page gains a licence list and an issue form stating it uses no seat (+ tests for pure helpers and rows).
- `apps/portal/src/pages/StationsPage.tsx` (+ test) -- `stationState` "Revoked on <date>"; no download link for a revoked licence.
- `CONTEXT.md` §10 row and §14 **Licence** entry (statuses, vendor-issued), `docs/ROADMAP.md` A1.2/A1.3, `README.md` hub routes if listed, `AGENTS.md` test counts -- record what shipped.

**Acceptance Criteria:**
- Given a hub with a vendor root and a customer organization, when a vendor admin issues a 30-day machine licence for station id `bench-1` and downloads it, then the file installs with the vendor root, `evaluate` with `machine_fingerprint("bench-1")` is `Valid`, the certificate names the organization, and the organization's seat counts are unchanged.
- Given a station licence revoked by a vendor admin, when the owner opens the Stations page, then the station reads "Revoked on <date>" with no download link and no reason, and the admin licence page states the file keeps verifying offline until its expiry.

## Spec Change Log

Implementation choices where the spec was silent (safe defaults, recorded here):
- The customer download (`GET /api/licences/:id/download`) serves station licences only; a vendor-issued licence is 404 there, consistent with it not being listed on the Stations page (it reaches the customer through the vendor).
- `POST /api/admin/licences/inspect`: any body that is not `{document: "<text>"}` (missing field, unknown field, not an object) is also 400 `MSG_NOT_A_LICENCE`, unaudited.
- `admin.licence_viewed` detail is `{organization_id}`.
- `verification.root_key_id` is the hub's configured root key id (`null` with `no_root`); `detail` is the `LicenseError` text on `failed`, else `null`.
- Issue and revocation reasons refuse every control character, newlines included (one line of text).
- The admin download returns a revoked licence's file (vendor staff may need it for support); only the customer download refuses it.

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 24 findings — high 0, medium 1, low 21, false 2, maybe-false 0
- findings:
  - `[low]` `[patch]` (blind) An owner can undo a revocation by releasing the station and registering the same key again — confirmed. The owner has paid for the seat, and revocation is a hub record by design (intent contract), so refusing re-registration would deny a paid seat. What was wrong was the admin page implying revocation ends the customer's use. Fix: the admin `RevocationNote` now says the owner can release and re-register, and that refunding or cancelling the entitlement is the lever on seats; the same limitation is in the CONTEXT §10 row.
  - `[low]` `[reject]` (blind) Inspect matches a pasted file to the hub record by `license_id` only, without comparing the bytes — with a vendor root configured, any edited file already reads `failed`, and the root cannot be forged. The confusing case needs a hub with no root, which cannot issue licences at all. Fixing it adds a new response field.
  - `[low]` `[patch]` (blind) The inspect audit row stores the hash the pasted file claims, not one the hub computed — fixed: `inspect_document` computes the canonical SHA-256 of the pasted body; the test asserts a tampered file's logged hash differs from its claimed one.
  - `[low]` `[patch]` (blind) A pasted `license_id` of any length goes into the append-only audit log — fixed: logged only when it parses as a UUID, else null; the test updated.
  - `[low]` `[patch]` (blind) The revocation note promises customer-side effects for vendor-issued licences, which customers never see — fixed: `RevocationNote` takes the licence's `source` and words each case; a test covers the vendor wording.
  - `[low]` `[patch]` (blind) The revoke-versus-reissue race test does not force either order — same root cause as the verification-gap row; fixed there.
  - `[low]` `[reject]` (blind) Untested paths: revoke racing release, two admins revoking at once, revoking a superseded licence while a newer one is active, release-and-re-register — the code shows each is handled: the second revoke waits on the licence-row lock and reads `revoked` (409), release and revoke share the station lock, and `LATEST_LICENCE` ranks the active licence first. Adding tests for paths already confirmed in the code is not worth the extra suite time.
  - `[low]` `[patch]` (blind) `licences_vendor_idx` is never used by a query, and `licences_organization_idx` already serves the organization list — fixed: dropped from 0015, which is not yet applied anywhere.
  - `[low]` `[reject]` (blind) The admin area cannot open a licence by its id — this is a new feature (an extra field and navigation). Licences are reached from the organization page or by pasting the file.
  - `[low]` `[patch]` (blind) After a 409 (another admin revoked first), the revoke form stays open on a stale "Active" record — fixed: `RevokeForm` `onError` closes the confirmation and invalidates the licence query.
  - `[low]` `[reject]` (blind) Issuing has no confirming press — the form needs a mandatory reason, the result is audited and can be revoked in the hub. A second press adds state for a rare mistake.
  - `[false]` `[reject]` (blind) Revoke checks the reason before the id, giving 400 where the matrix says 404 — the matrix does not rank two errors in one request. Validating the body before the lookup is the existing admin-route convention (`update_offering`). An unknown id with a valid reason is 404, as tested.
  - `[low]` `[patch]` (edge) Release then re-register bypasses revocation — same root cause and fix as the first blind row.
  - `[medium]` `[patch]` (edge) The quota field is `type=number`: text the browser cannot parse (such as `5,000`) reads as empty, which sends `null` and signs an unlimited licence — fixed: the field is now `type="text"`, so unparseable text reaches `wholeNumber`, is sent as -1 and the hub refuses it with a 400.
  - `[low]` `[reject]` (edge) Inspect shows the pasted file next to the genuine record when only the `license_id` matches — same as the second blind row.
  - `[low]` `[patch]` (edge) Revoke 409 leaves the page stale — same root cause and fix as the blind row above.
  - `[low]` `[patch]` (edge) Editing the pasted text leaves the previous result on screen — fixed: the textarea's `onChange` calls `inspect.reset()`.
  - `[false]` `[reject]` (edge) Unknown licence id with a bad reason is 400, contradicting the module doc's 404 — same as the blind row; the doc's 404 covers the id, and the reason is validated first by convention.
  - `[low]` `[patch]` (verification-gap) Nothing deterministically proves revoke waits on the station lock — fixed: `a_revoke_waits_for_a_reissue_holding_the_station_lock` holds the station row in a test transaction, waits until the revoke is queued on a lock (`pg_stat_activity`), supersedes the licence as reissue does, commits, and asserts 200 with `previous_status: "superseded"` and `superseded_at` kept. Without the station lock, the revoke never waits and the test fails.
  - `[low]` `[reject]` (intent) Vendor-issued licences are not on the customer's Stations page (reading I-c excluded) — this is a recorded autonomous decision in the intent contract, and a CONTEXT §10 limitation.
  - `[low]` `[reject]` (intent) A licence the hub never stored (CLI-signed) can be inspected but not revoked (reading R-d excluded) — ADR-0007 §5 says revocation is recorded in the hub, and the hub can only hold that record for a licence it stores. There is no row to revoke.
  - `[low]` `[reject]` (intent) Customer-side effects of revocation (reissue and download refused, seat kept) go beyond CAP-4's wording — these are recorded autonomous decisions, taken as the safe default.
  - `[low]` `[reject]` (intent) The interactive admin forms and the new routes are not exercised at page level — the portal's convention is to test helpers and static components (story 1 and 2 triage). Typecheck, 179 tests and a production build pass.
  - `[low]` `[reject]` (intent) AC2 is verified in pieces (`StationRow`/`stationState`, `RevocationNote` text) plus the HTTP data path — same root cause as the row above.

## Design Notes

Revoke, in one transaction: `SELECT station_id, status FROM licences WHERE id = $1` (404 if none); if `station_id` is set, `SELECT 1 FROM stations WHERE id = $2 FOR UPDATE`; then `UPDATE licences SET status='revoked', revoked_at=now(), revocation_reason=$3 WHERE id=$1 AND status IN ('active','superseded') RETURNING …` — no row → 409 `MSG_ALREADY_REVOKED`. Reissue already holds the station lock, so after it waits it re-reads the latest licence in its own statement and finds the revoked one → 409, never a 500.

Verification summary: `Ok(id)` → `verified`; `Err(e)` → `failed` with `e.to_string()` (so tampering and untrusted roots read as such); no root configured → `no_root`.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including `licence_admin`, isolation and migration tests
- `cargo test --workspace` -- expected: pass
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean
- `cd apps/portal && pnpm typecheck && pnpm test && pnpm build` -- expected: pass

## Auto Run Result

Status: done

**Summary.**
- Vendor admins can now sign a licence for any customer organization from the admin organization page: a trial, an evaluation or a goodwill grant. It uses no seat and no entitlement and needs a reason; the entitlements are typed in (scope, quota, features, 1–1095 days).
- They can open any licence the hub holds, or paste any licence file, and see the decoded body and whether it verifies against the hub's vendor root.
- They can mark a licence `revoked` with a reason. Revocation is recorded in the hub only, and the page says the file keeps verifying offline until it expires.
- Every action, reads included, is audited.
- On the customer's Stations page, a station whose licence was revoked reads "Revoked on <date>", without the reason and with no download. It cannot be reissued, but it can still be released.

**Files changed**
- `crates/wipe-hub/migrations/0015_licence_admin.sql` — licence `source` (station or vendor), issue reason, `revoked` status, and the replaced lifecycle trigger.
- `crates/wipe-hub/src/stations.rs` — one sign-and-store core shared by register, reissue and the new `vendor_issue`; `revoke`; revoked handling in reissue and the customer download.
- `crates/wipe-hub/src/licence_admin.rs` (new) — issue/revoke validation and the audited admin reads: list, view, inspect, download.
- `crates/wipe-hub/src/admin_routes.rs`, `station_routes.rs`, `lib.rs` — the six admin routes and a shared download response.
- `crates/wipe-hub/tests/licence_admin.rs` (new, 13 tests), `tests/isolation.rs`, `tests/migrations.rs`, `tests/stations.rs` — HTTP tests for every I/O row, the race and lock tests, route classification, and the 0015 refusals.
- `apps/portal/src/api/adminLicences.ts` (new, + test), `api/stations.ts` (+ test) — typed clients.
- `apps/portal/src/pages/AdminLicencePage.tsx`, `AdminLicenceInspectPage.tsx` (new), `AdminOrganizationPage.tsx`, `AdminPage.tsx`, `StationsPage.tsx` (+ tests), `main.tsx` — the admin pages and the customer's revoked state.
- `CONTEXT.md`, `docs/ROADMAP.md`, `README.md`, `AGENTS.md` — what shipped, the limitations, and the test count (607).

**Review.** 24 findings: high 0, medium 1, low 21, false 2.

- **Patched (12 rows in 9 fixes; entry verdicts: 1 medium, 8 low):**
  - the quota field could sign an unlimited licence (medium);
  - the revocation note now covers re-registration and vendor-issued licences;
  - the inspect audit row now holds the hub-computed hash and only a UUID `license_id`;
  - the unused index is dropped;
  - the revoke form recovers from a 409;
  - the inspect result resets when the text changes;
  - a deterministic test proves revoke waits on the station lock.
- **Deferred:** none.
- **Rejected (12):** 10 low and 2 false. The reasons are in the Review Triage Log.

The implementer subagent was not re-engaged for the patches, because in story 2 it could only be resumed detached, which this workflow forbids. I applied them myself and re-ran every verification command.

**Follow-up review recommended: false.** This is a first pass, and one medium entry was patched, with no high.

**Verification.**
- `cargo test --workspace --no-fail-fast`: 606 passed, 1 failed. The failure is `wipe-hub` `auth::concurrent_guesses_get_exactly_five_password_checks` (Retry-After 901 where at most 900 is expected). It fails the same way on the baseline commit `7c72297`, which I checked in a throwaway worktree, and it is in code this story does not touch. All 13 `licence_admin` tests, the isolation, migration and station tests pass.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` is clean.
- Portal: `pnpm typecheck` passes, all 179 tests pass, and `pnpm build` passes.

**Residual risks**
- Revocation is never distributed to stations (A2.3, ADR candidates 0008/0010). An owner can also release a station and register its key again after a revocation; this is documented.
- Vendor-issued licences reach the customer only when the vendor sends the file.
- The vendor root is still file-held (A1.5).
- The admin pages were checked by typecheck, unit tests and a build, not in a real browser.
- The pre-existing auth lockout test failure above is unrelated, but it leaves the workspace suite red until someone fixes it.
