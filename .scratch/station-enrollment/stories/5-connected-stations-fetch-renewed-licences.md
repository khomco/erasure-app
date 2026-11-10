---
title: 'Connected stations fetch renewed licences'
type: 'feature'
created: '2026-11-10'
status: 'ready-for-dev'
review_loop_iteration: 0
followup_review_recommended: false
context:
  - '{project-root}/docs/adr/0008-station-enrollment-and-licence-fetch.md'
  - '{project-root}/docs/adr/0013-licensing-controls-every-erasure.md'
warnings: [oversized]
deferred: []
---

<intent-contract>

## Intent

**Problem:** A station registered with the hub (by enrollment or by file) only gets a renewed licence when someone downloads the file and installs it by hand. ADR-0008 §3 says a connected station collects its own renewals. Today the hub has no station-authenticated route and no challenge, and the station never contacts the hub after it enrolls.

**Approach:** The hub gains `GET /api/station/challenge` (single-use, 60 s) and `POST /api/station/licence`, signed over that challenge with the shared `wipe_license::station_request` scheme. It returns the station's latest licence, or 204 when the station's copy is current, and reissues automatically through `stations::reissue` when `renewal_available` is true, with the station as the audit actor. A station with a saved hub address runs a background check at start and every 24 h. The check verifies the licence against the station's own vendor roots, installs it through `AppState::install_license`, and reports the result as a status line on `GET /api/license`. The check never touches the job path.

## Boundaries & Constraints

**Always:**
- **Signing.** Requests use `station_request::sign`/`verify` exactly as pinned: the challenge, the hub origin (`stripe::hub_origin(public_base_url)`), the method, the path and the body hash.
- **Challenges.** Each challenge is 32 random bytes in unpadded base64url. It is consumed atomically on the first validly signed use (`DELETE … WHERE expires_at > now() RETURNING`) and expires after 60 s. Expired rows are purged in batches of `throttle::PURGE_BATCH`.
- **Throttling.** Challenge issuance is throttled per peer through `throttle.rs`, keyed `station:ip:<peer>` with IPv6 grouped by `/64`. The limit must comfortably allow 50 stations behind one NAT, each checking twice a day.
- **Header validation.** `station_request::verify` refuses a key id, challenge or signature that contains `\n`. This was deferred from story 3.
- **Station lookup.** The body is `{instance_public_key, licence_id: string|null}`. The hub refuses with 401 if the header key id is not `public_key_id(instance_public_key)`. It then looks up unreleased stations whose `instance_key_id` matches. If several match, the one that owns `licence_id` wins. If none or several are left, the hub refuses.
- **Responses.** All responses are JSON `{"error": …}` with `Cache-Control: no-store`/`Pragma: no-cache`, as `enrollment_routes` does. A 200 body is `{"licence": <the stored licence document as JSON>}`.
- **CSRF.** `POST /api/station/licence` is CSRF-exempt, the same as the enrollment posts.
- **Reissue.** An automatic reissue goes through `stations::reissue` in one transaction with the lookup. Concurrent fetches never issue two licences for one renewal. Audit actor `station:<station uuid>`. The existing `licence.issued`/`licence.superseded` rows carry `instance_key_id` in their detail.
- **Installing.** The station verifies what it fetched with `hub_client::check_licence` (against its own roots and key id) and installs it through `AppState::install_license`. When `settings_path` is set, it also saves the licence with `store::save_enrolled_licence`.
- **The check never blocks a job.** A failed check is logged with `tracing::warn` and recorded as the latest hub check. It never takes or holds a lock across an `.await`, and never touches `start_job` or the runner.
- **Paired changes.** The Rust `LicenseView` change and its TS mirror `LicenseInfo` ship in the same commit. `CONTEXT.md` and `docs/ROADMAP.md` are updated in that commit too.

**Never:**
- No new trust root from the hub.
- No portal credential on the station.
- No override.
- No gating, delaying or refusing a start because of the hub.
- No change to the file paths for registration and install.
- No audit row for issuing a challenge, or for a fetch that changes nothing.
- No carrying of a next-term licence: that waits for `controlled-licensing`.
- No revocation distribution (A2.3).
- No loading of `enrolled-licence.json` at start-up (story 4's deferred (b)).
- No faster retry than the 24 h schedule.
- No new station route, so no change to the route golden.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Renewal | Entitlement renewed; `renewal_available` true | 200 with the new licence. The old licence is superseded and audited with actor `station:<id>`. The station installs it, and `GET /api/license` shows the new `not_after`. | — |
| Policy changed | Owner reissued after a lapse-policy change; station holds the old id | 200 with the latest licence, no new reissue, and the station installs it | — |
| Current | `licence_id` equals the latest active id, nothing behind | 204, no audit row. The station status is ok ("up to date"). | — |
| Station has no licence | `licence_id` null, one active registration | 200 with the latest active licence | — |
| Released | No unreleased station for the key | 404 | Station status: "not registered with the platform" |
| Revoked latest | No active licence, latest revoked | 204 (nothing new) | — |
| Ambiguous key | Key active in 2 organizations, `licence_id` matches neither | 409 naming the problem | Station records the failure |
| Replay | Same challenge used twice | Second use 401 "challenge expired or already used" | — |
| Expired challenge | Used after 60 s | 401, same message | — |
| Bad signature / key mismatch / `\n` in header | — | 401; nothing consumed on a bad signature | — |
| Untrusted root or wrong key | Hub returns a licence the station cannot verify | Not installed | Status failure names the reason |
| Unreachable or hanging hub | Closed port, or a hub that never answers | Status failure after the client timeout. A job started meanwhile starts immediately. | `warn` log |
| No hub address | Settings absent or in-memory store | No check runs; `hub_check` absent | — |

## Autonomous decisions

- **How the hub knows a station's key.** The problem: the hub stores no public key, so it cannot check a station's signature. Options: (a) add a public-key column, filled in at enrollment approval; (b) the station sends its public key, and the hub checks it against the registered key id. **Chose (b).** It also covers stations registered from a file, and it is the same proof the enrollment start already uses.
- **A key registered in two organizations.** The problem: the database allows the same key to be active in two organizations, and the hub must not hand out the wrong organization's licence. Options: (a) pick one; (b) choose by the licence the station already holds, and otherwise refuse. **Chose (b), answering 409.** Never guessing which organization is the safe default.
- **What happens after a policy change.** The problem: the spec's tests include a "policy change" case, but its success criteria name only renewal as a trigger for automatic reissue. Options: (a) also reissue automatically when the lapse policy differs; (b) deliver a policy-changed licence only after an owner has reissued it. **Chose (b).** It means fewer licences issued without an owner's click, and ADR-0013 §5 retires the lapse policy anyway.
- **When the latest licence is revoked.** ADR-0008 §3 says "returns nothing new". **Chose 204**, rather than an error or the revoked document.
- **What the audit records.** Actor `station:<station uuid>`, with `instance_key_id` added to the detail. Issuing a challenge and a fetch that changes nothing are not audited, following the same reasoning as not auditing an expired enrollment (ADR-0008 §1).
- **Keeping the fetched licence on disk.** The fetched licence is saved to `enrolled-licence.json`, as `enroll` does. Loading that file at start-up stays deferred (story 4's deferred (b)). Residual risk: a station that restarts while the hub is unreachable falls back to its `--license` file.
- **Next-term licences.** The response is an object, `{licence}`, so `controlled-licensing` can add `next_licence` without breaking it. Nothing for next-term licences is built here.
- **Retrying after a failed check.** No faster retry: the next attempt is at the next 24 h check, or at the next restart. Renewals arrive 30 days early (ADR-0013 §4), so one missed day does not matter.
- **Status wording.** The bench status line follows story 4's operational wording style. It is status text, not marketing copy.

</intent-contract>

## Code Map

**Hub**

- `crates/wipe-license/src/station_request.rs` -- `sign` :71 and `verify` :91 (the caller checks the challenge), the header constants :35–39, and the pinned test `the_message_is_pinned` :134. Add the `\n` refusal in `verify`.
- `crates/wipe-license/src/chain.rs:132` -- `public_key_id()`: the key id is derived from the public key, so a station proves its key by sending it, as the enrollment start does.
- `crates/wipe-hub/migrations/` -- the next migration is `0020_station_challenges.sql`. No hub table holds challenges today. `stations` holds only `instance_key_id` (no public key); its registration columns are frozen by the `stations_lifecycle` trigger.
- `crates/wipe-hub/src/enrollment_routes.rs` -- the template to follow:
  - `routes()` :88 with the `no_store` layer;
  - `ApiError`;
  - `charge_start` :173 for the throttle and lock audit;
  - `instance_key_actor` :131 as the precedent for an actor string.
- `crates/wipe-hub/src/enrollments.rs` -- `purge_expired` :239 is the template for purging challenges. `approve`/collect :451–477 return `licences.document` parsed as JSON.
- `crates/wipe-hub/src/stations.rs` -- the main hub changes:
  - `reissue(conn, root, org, user_id: Uuid, id)` :912 and `IssueLicence.user_id` :690 → `SignAndStore.actor: &str` :585. Change them to take an actor string; the portal caller passes `auth::actor(user_id)`.
  - The `LATEST_LICENCE` :295, `LICENCE_BEHIND_TERM` :303 and `renewal_available` :330 SQL can be reused for the lookup.
  - The refusal constants live here, for example `MSG_LICENCE_UP_TO_DATE`.
- `crates/wipe-hub/src/station_routes.rs:1-41` -- its doc comment says every route needs a session. Put the station-signed routes in a new `station_auth_routes.rs` (or similar) and merge it in `lib.rs` next to :113–115.
- `crates/wipe-hub/src/csrf.rs` -- `is_exempt` :92 and the test `only_the_enrollment_posts_are_exempt` :246 (rename and extend it).
- `crates/wipe-hub/src/throttle.rs` -- `charge` :180, `PURGE_BATCH` :110, `ENROLL_*` constants :84–100; `auth::ip_key`.
- `crates/wipe-hub/src/stripe.rs:313` -- `hub_origin(public_base_url)` is the origin the hub verifies against.
- `crates/wipe-hub/tests/support/hub.rs` -- the hub test helpers:
  - `spawn_hub_with_root` :109;
  - `BASE` :26: sign with `BASE`, not the real port;
  - `audited` :304 and `actions` :312.
- `crates/wipe-hub/tests/stations.rs` -- `renew()` :177 and `extend_term()` :191 make a renewal available.
- `crates/wipe-hub/tests/enrollments.rs` -- `fixture()` :95 and `buy()` :59. Tests use `SigningKey::generate()`.

**Station**

- `crates/wipe-server/src/hub_client.rs`:
  - `HubOrigin` :71/:99;
  - `ProxySettings::from_env` :144;
  - `HubClient::new` :284, with 10 s connect and 30 s request timeouts, no redirects and explicit proxy handling;
  - `check_licence` :556;
  - `EnrollError` :170;
  - `Clock`/`SystemClock` :436/:443.
  - Add `challenge()` and `fetch_licence(key, held_id) -> FetchOutcome { Current, Licence(Value), NotRegistered, Refused(String) }`. They sign with `station_request::sign`.
- `crates/wipe-server/src/enrollment.rs` -- reading settings follows `saved_hub_origin` :263. `EnrollmentSlot` :200 (Mutex, AbortHandle, abort on Drop) is the model for the fetch slot.
- `crates/wipe-server/src/store.rs` (~:686–700) -- `StationSettings { hub_origin }`, `load_station_settings`, `save_enrolled_licence`.
- `crates/wipe-server/src/app.rs`:
  - `AppState` :22–90 (`license`, `vendor_roots`, `signing_key`, `settings_path`);
  - `install_license` :235, which is synchronous and in-memory and returns `LicenseInstallError` (Display);
  - `api_routes()` :522 is unchanged.
- `crates/wipe-server/src/licensing.rs` -- `LicenseContext` with its private `chain`, `view()` and `LicenseView` :428. Add a `license_id()` accessor and an optional `hub_check` field.
- `crates/wipe-server/src/handlers.rs:974` -- `license` handler, which fills in `hub_check` from the fetch slot.
- `crates/wipe-server/src/bootstrap.rs:126` -- `build_station` (runs inside tokio; settings :195–216, builder :250–262). Spawn the fetch loop here and nowhere else, so the CLI and Tauri hosts both get it.
- `crates/wipe-server/tests/hub_client.rs` -- the fake-hub test helpers:
  - `fake_hub` :118 (axum on `127.0.0.1:0`);
  - `closed_port` :129;
  - `licence(root, key_id)` :137;
  - `FakeClock` :168.
- `crates/wipe-server/tests/station_enrollment.rs:204` -- `station_with(...)` builds a served station with roots and a settings path.
- `apps/desktop/src/api/types.ts:344` -- `LicenseInfo`.
- `apps/desktop/src/pages/LicencePage.tsx:100` -- `LicenceStatus`.
- `CONTEXT.md:1025` -- the §10 rows for stories 1–4; add a row for story 5 after it.
- `docs/ROADMAP.md:357` -- A2.1.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-license/src/station_request.rs` -- Make `verify` refuse a key id, challenge or signature containing `\n` with a new `StationRequestError` variant, and add a unit test. This is the deferred story-3 finding.
- `crates/wipe-hub/migrations/0020_station_challenges.sql` -- Create `station_challenges (challenge text primary key, peer text, created_at, expires_at)` with an index on `expires_at`. Everything the challenge lifecycle needs.
- `crates/wipe-hub/src/station_link.rs` (new) -- Add the challenge functions:
  - `issue_challenge`, which purges expired rows first;
  - `consume_challenge`.

  Add `fetch_licence(conn, root, key_id, licence_id) -> Fetch { Licence(Value), Current, NotFound, Ambiguous }`. It locks the station row with `FOR UPDATE`, reissues with actor `station:<uuid>` when `renewal_available` is true, and otherwise compares the held id with the latest active id. The domain logic stays separate from the routes, following the `enrollments.rs` split.
- `crates/wipe-hub/src/stations.rs` -- Make `reissue`/`issue_licence` take an actor `&str` instead of a `user_id`, and add `instance_key_id` to the reissue audit details. Update the portal caller. Reissue then has one code path whatever the actor.
- `crates/wipe-hub/src/station_link_routes.rs` (new), `lib.rs`, `csrf.rs`, `throttle.rs` -- Add the routes, the `no_store` layer, the per-peer throttle constants and the CSRF exemption, and update the CSRF test. This exposes CAP-5 over HTTP.
- `crates/wipe-hub/tests/station_licence.rs` (new) -- Add compose-backed tests for every hub row of the I/O matrix, plus:
  - challenge throttle and purge;
  - concurrent fetches during a renewal produce one reissue;
  - audit actor and detail.

  This covers CAP-5's test list on the hub.
- `crates/wipe-server/src/hub_client.rs` -- Add `challenge` and `fetch_licence`, signed with `station_request::sign` over the origin and the path `/api/station/licence`, with operator-worded errors. This is the station transport.
- `crates/wipe-server/src/licence_fetch.rs` (new), `licensing.rs`, `app.rs`, `handlers.rs`, `bootstrap.rs` -- Add a `LicenceFetchSlot` holding the last `HubCheck { checked_at, ok, message }`, and `check_once(&AppState)`, which reads the settings, fetches, verifies, installs and saves. Add a loop run by `Clock`: check once at start, then sleep 24 h between checks. Spawn it from `build_station`; the slot aborts the loop on drop. Add `LicenseContext::license_id()`. Add `LicenseView.hub_check` as an `Option` with `#[serde(skip_serializing_if = "Option::is_none")]`, filled in by the handler. Together these give the 24 h check and the status line.
- `crates/wipe-server/tests/licence_fetch.rs` (new) -- Using the fake hub, test:
  - installed renewal;
  - current;
  - not registered;
  - untrusted root;
  - unreachable hub;
  - a hanging hub while a job starts and finishes on the mock backend;
  - no hub address;
  - the loop's sleeps under `FakeClock`.

  This covers the station rows of the I/O matrix.
- `apps/desktop/src/api/types.ts`, `apps/desktop/src/pages/LicencePage.tsx` (+ test) -- Add an optional `hub_check` and render one line: "Last check with the platform: <local time> — <message>". On a failure, add "Your current licence still applies." Add vitest cases for absent, ok and failed. This keeps the TS mirror in step and shows the status.
- `CONTEXT.md`, `docs/ROADMAP.md` -- Add a §10 row for story 5: routes, migration, actor, statuses and known limits (no start-up load of `enrolled-licence.json`, the Tauri shell has no vendor root, no next-term licence yet, the portal shows no audit trail). Mark A2.1 built. Docs move with the code.

**Acceptance Criteria:**
- Given a station enrolled through the hub whose entitlement has just renewed, when the station starts with its hub address saved, then within seconds `GET /api/license` reports the new `not_after` and a successful hub check. The hub's audit log has `licence.issued` and `licence.superseded` with actor `station:<station uuid>`, and no seat count changed.
- Given the hub is unreachable or never answers, when an operator starts a Job on the mock backend while a check is in flight, then the Job starts without waiting for the check, and `GET /api/license` later shows a failed hub check that says the current licence still applies.
- Given a station registered from a file (no enrollment row), when it is given a hub address and checks, then the hub authenticates it from the public key in the body and returns its licence.
- Given `cargo test --workspace` with compose up, and the desktop typecheck and tests, then all pass. `docs/generated/http-routes.txt` and the CLI help goldens are unchanged.

## Spec Change Log

## Review Triage Log

## Design Notes

**Checkpoint (SPEC CAP-5) — this needs the human.** The portal has **no audit view**: no portal page reads `audit_log`, and `StationsPage.tsx` shows only the latest licence's status. "Confirm the audit trail reads correctly in the portal" therefore cannot be checked in the portal. As planned, the trail is readable in SQL and in tests: `licence.issued` and `licence.superseded`, with actor `station:<uuid>`, detail `reason: "renewed"`, and `instance_key_id`. In the portal, the station's row shows the new expiry. Adding an audit view would widen the scope; it is not planned here.

**Why the public key goes in the body.** The hub keeps no public key for stations, and enrollment rows are purged 24 h after they expire. Because the key id is a hash of the public key, sending the key and checking `public_key_id(key) == key id` before verifying the signature proves the station holds the key. This works for file-registered stations and needs no column on the frozen `stations` table.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass, including `wipe-hub --test station_licence` and `wipe-server --test licence_fetch`.
- `cargo clippy --workspace --all-targets -- -D warnings` -- expected: clean.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: pass.
- `git diff --stat docs/generated` -- expected: no changes.

## Auto Run Result

Status: ready-for-dev
Blocking condition: none. Halted after planning, as instructed, so a human can review the spec and the SPEC CAP-5 checkpoint. Note that the portal has no audit view (see Design Notes), so the checkpoint can only be confirmed from the audit rows in SQL or in tests.
