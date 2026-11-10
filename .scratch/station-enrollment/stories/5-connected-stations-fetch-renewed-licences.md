---
title: 'Connected stations fetch renewed licences'
type: 'feature'
created: '2026-11-10'
status: 'done'
baseline_revision: '8c11473bd245074c1b33e538213a68e98f38470e'
review_loop_iteration: 0
followup_review_recommended: false
context:
  - '{project-root}/docs/adr/0008-station-enrollment-and-licence-fetch.md'
  - '{project-root}/docs/adr/0013-licensing-controls-every-erasure.md'
warnings: [oversized]
deferred:
  - summary: >-
      The station's licence check has no test for a refused (e.g. 429) or unreadable challenge, or a malformed 200 body.
    evidence: |-
      The fake hub in crates/wipe-server/tests/licence_fetch.rs always answers the challenge with 200; a_refusal_names_the_hubs_reason scripts only the licence POST. A regression would change only the bench wording on a rare path.
    location: >-
      crates/wipe-server/src/hub_client.rs (HubClient::challenge)
    severity: low
  - summary: >-
      AGENTS.md test counts (946 Rust, 385 vitest) are stale.
    evidence: |-
      cargo test --workspace now runs 1093 tests and the desktop vitest 419; the fix edits an agent-context file, so it is deferred to the owner.
    location: >-
      AGENTS.md
    severity: low
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

Decisions taken while building (2026-11-10), each the safe default:

- **Migration number.** `0020` was taken by `controlled-licensing` story 1 (`0020_trial_offerings`), committed after this spec was planned, so the challenges table is `0021_station_challenges.sql`.
- **"Policy changed" row.** `controlled-licensing` story 2 retired the lapse-policy setting and made reissue renewals-only, so an owner can no longer reissue for a policy change. The row's behaviour (the station holds an older id than the latest active licence → 200 with that licence, no new reissue) is tested with an owner's renewal reissue instead (`an_owner_reissued_licence_is_delivered_without_another_reissue`).
- **"Still applies" wording.** The acceptance criterion says `GET /api/license` shows a failed check "that says the current licence still applies", while the desktop task says the page adds that sentence. To satisfy both without the bench showing it twice, the station's failure message itself ends "This station's current licence still applies." (`licence_fetch::STILL_APPLIES`), and the page shows the message as is, in amber.
- **`FetchOutcome::Unreachable(String)`.** Added beside the four planned variants so an unreachable or hanging hub gets its own bench wording ("Could not reach the platform at …").
- **Throttle figures.** `station:ip:<peer>`: 300 challenges per hour, final charges, the 300th locks the address for an hour, audited once as `station.challenge_locked` and summarised as `station.challenge_lock_summary` (the enrollment-start pattern). 50 stations restarting together behind one NAT use 50.
- **Challenge consumption.** Consumed by its own statement before the fetch's transaction, so a challenge is spent by its first validly signed use even if the fetch then fails.
- **Body.** `licence_id` is any string (a vendor-issued or test licence id need not be a UUID; it is compared as text). Unknown body fields are ignored, so a later station can add one without breaking this hub.
- **No vendor root on the hub.** A waiting renewal is then not reissued, and the fetch answers as if none were waiting (the latest active licence, or 204), rather than failing.
- **Where the loop runs.** `build_station` spawns it for every station that keeps configuration on disk (and runs inside Tokio); each round re-reads `station-settings.json`, so a station that enrolls from the bench app is checked from the next round. An in-memory station runs no loop. The start-up `hub:` line, when an address is saved, adds "checked for a renewed licence at start-up and every 24 hours" (the start-up line test pins only `hub: none`).

## Review Triage Log

### 2026-11-10 — Review pass
- verdicts: 37 findings — high 0, medium 1, low 29, false 4, maybe-false 3
- findings:
  - `medium` `patch` (Blind) Station installs any verifying licence with a different id, even one ending earlier than the one it runs — real: `check` compared ids only; ADR-0008 §3 says "a newer licence". Fixed: install only when none is installed or the fetched `not_after` is strictly later than the installed one's, re-read just before installing; test added.
  - `low` `patch` (Blind) Nothing serialises the check against an operator install made while it is in flight — same root cause as the row above (held licence read before the network calls); fixed by the same re-read-and-compare just before `install_license`.
  - `low` `patch` (Blind) `LicenceFetchSlot` never aborts the loop on drop: the task owns an `AppState` clone holding the slot's Arc — real; one station per process in production, and `EnrollmentSlot` already works this way. Fixed the docs (module, `app.rs`, CONTEXT) to say what happens.
  - `low` `reject` (Blind) The start-up `hub:` line promises checks even when `spawn` returns false or no vendor root is trusted — `spawn` is false only outside Tokio, and `build_station` runs inside it; a rootless station's checks do run and say why they fail. Fix would add branches for a line nobody acts on.
  - `low` `patch` (Blind) A hub with no vendor root silently answers "nothing new" while a renewal waits — real misconfiguration blind spot. Fixed: `tracing::warn!` naming the station in that branch.
  - `false` `reject` (Blind) Station says "Up to date" for a revoked latest licence — ADR-0008 §3: "If the latest licence is revoked, the fetch returns nothing new"; matrix row "Revoked latest → 204".
  - `low` `reject` (Blind) 300 challenge requests from inside a site's NAT lock its stations out for the day — needs an attacker on the customer's own network, delays only a renewal that arrives 30 days early, never touches erasure; the fix (sooner retry) is excluded by the intent ("No faster retry than the 24 h schedule").
  - `low` `reject` (Blind) No jitter on the 24 h schedule — stations start at different times, and 50 starting together use 50 of 300 an hour; intent fixes "at start and every 24 h".
  - `low` `patch` (Blind) "It is saved at <path>" implies the licence survives a restart, though nothing loads `enrolled-licence.json` at start-up — fixed: message now says to start with `--license <path>`, as story 4's does.
  - `low` `patch` (Blind) No hub test for a waiting renewal on a hub without a vendor root — same as the verification-gap row below; test added.
  - `maybe-false` `reject` (Blind) A station fetch racing an owner's reissue may deadlock — both paths lock the station row before the entitlement (the fetch locks `s`, then `reissue` locks `s, e` in one statement, as the owner path does); settling it needs a lock-order trace of every path that locks an entitlement first. If true it is only low: Postgres aborts one transaction, and the station tries again next round.
  - `low` `reject` (Blind) No test for a `reissue` refusal coming back through `/api/station/licence` — needs a term ending in the instant between two clocks; the generic `StationError` mapping is the existing route's.
  - `low` `patch` (Blind) Station tests miss the save-failure branch — same as the verification-gap row below; test added.
  - `low` `defer` (Blind) Station tests miss a 429 or unreadable challenge and a malformed 200 body — same as the verification-gap 429 row; deferred.
  - `low` `patch` (Blind) Station tests miss a held licence newer than the fetched one — added with the downgrade fix.
  - `low` `patch` (Blind) README's `licence_fetch.rs` test count is stale and `station_licence.rs` has no row — fixed.
  - `low` `defer` (Blind) AGENTS.md test counts (946 / 385) are stale — the fix edits an agent-context file.
  - `false` `reject` (Blind) ROADMAP strikes A2.1 while CONTEXT says partial — the spec task says "Mark A2.1 built", and the ROADMAP note lists the open items.
  - `low` `reject` (Blind) The throttle test comment says "twice a day" while the doc says one at start plus one a day — the comment quotes the intent's sizing ("each checking twice a day"); both fit within 300. Cosmetic.
  - `low` `reject` (Blind) `fetch_licence` takes `FOR UPDATE` on every routine check — the transaction holds it for a few statements twice a day; no measurable contention.
  - `low` `reject` (Blind) `licences.id::text = $1` cannot use the primary-key index — runs only when a key is in several organizations, already narrowed by `station_id = ANY($2)`.
  - `low` `patch` (Edge) Loop task holds a full `AppState`, so Drop never aborts it — same root cause as the Drop row above; docs fixed.
  - `low` `reject` (Edge) A `reissue` refusal after `renewal_available` was read returns 409/500 instead of the current licence — needs a refund or term end between two statements of one transaction; the station retries next round.
  - `maybe-false` `reject` (Edge) The fetch locks station then entitlement in separate statements, which could deadlock against an owner reissue — same analysis as the Blind deadlock row; if true, only low.
  - `low` `patch` (Edge) The hub returns the station row's latest licence even when the station holds a different, longer one — same root cause as the downgrade row; fixed station-side, and the hub's answer stays "the registration's latest".
  - `low` `patch` (Edge) The station installs a downgrade — same root cause as the downgrade row; fixed.
  - `low` `reject` (Edge) A transient DB error after the challenge is consumed leaves the station waiting 24 h — accepted by design (Spec Change Log); the intent excludes a faster retry.
  - `low` `patch` (Edge, claim) "the slot aborts the loop on drop" does not hold — same root cause as the Drop row; docs fixed.
  - `low` `reject` (Edge, claim) The failure sentence lives in the server message rather than being added by the UI — the acceptance criterion needs `GET /api/license` to say it; doing both would print it twice; recorded in the Spec Change Log.
  - `low` `patch` (Verification gap) No hub test for a waiting renewal without a vendor root — test added.
  - `low` `patch` (Verification gap) No station test for the "trusts no vendor root" guard — test added, asserting the fake hub saw no request.
  - `low` `patch` (Verification gap) No station test for the could-not-be-saved branch — test added.
  - `low` `defer` (Verification gap) No station test for a refused (429) challenge — wording on a rare path, as filed; deferred.
  - `low` `reject` (Intent alignment) No test runs the real station client against the real hub — checked by reading both halves: both serialise the origin with `Url::origin().ascii_serialization()`, the path strings are identical, the `{challenge}`/`{licence}` shapes match, and the hub tests assert the document's `license_id` equals `licences.id`. A cross-crate end-to-end test is more than a direct fix.
  - `false` `reject` (Intent alignment) The Tauri shell runs the loop and fails every round for want of a vendor root — a known limit the spec lists ("the Tauri shell has no vendor root").
  - `false` `reject` (Intent alignment) The `\n` header refusal is not exercised over HTTP — HTTP header values cannot carry a raw line break (hyper refuses them), so the unit test in `wipe-license` is the reachable surface.
  - `maybe-false` `reject` (Intent alignment) The hanging-hub test uses a 3 s timeout instead of the production 30 s — the production path is the same `check_once_with` with `REQUEST_TIMEOUT`; settling it needs a 30 s test, which buys nothing.

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

Status: done
Blocking condition: none.

**Summary.** Connected stations now collect their own renewed licences (ADR-0008 §3). The hub serves `GET /api/station/challenge`: a single-use challenge that lasts 60 s, throttled per address. It also serves `POST /api/station/licence`, signed with the instance key over that challenge and exempt from the CSRF check. That route returns the station's latest licence, or 204 when there is nothing new; 404 means the station is not registered and 409 means its key is ambiguous. When a renewal is waiting, the hub reissues it through `stations::reissue` with actor `station:<uuid>`. A station with a saved hub address checks at start and every 24 h. It verifies the licence against its own vendor roots and installs it only when it ends later than the licence it runs. It then saves it to `enrolled-licence.json` and reports `hub_check` on `GET /api/license`, which the Licence page shows. The check never touches the job path.

**Files changed**
- `crates/wipe-hub/migrations/0021_station_challenges.sql`: the challenges table (0020 was taken by `controlled-licensing`).
- `crates/wipe-hub/src/station_link.rs`, `station_link_routes.rs`: challenge issue, purge and consume, the station lookup and fetch, and the routes.
- `crates/wipe-hub/src/stations.rs`, `station_routes.rs`: `reissue` takes an actor string, and `instance_key_id` is added to the supersede audit detail.
- `crates/wipe-hub/src/csrf.rs`, `throttle.rs`, `auth.rs`, `lib.rs`, `enrollment_routes.rs`: the CSRF exemption, the `station:ip:` throttle and its lock summary, route wiring, and a shared `no_store`.
- `crates/wipe-license/src/station_request.rs`: `verify` refuses `\n` in any header (the deferred story-3 finding).
- `crates/wipe-server/src/hub_client.rs`: `challenge` and `fetch_licence`.
- `crates/wipe-server/src/licence_fetch.rs`, `licensing.rs`, `app.rs`, `handlers.rs`, `bootstrap.rs`: the check, the 24 h loop, `hub_check`, and spawning from `build_station`.
- `apps/desktop/src/api/types.ts`, `pages/LicencePage.tsx`, and the page's test: the `HubCheck` mirror and the status line.
- Tests: `crates/wipe-hub/tests/station_licence.rs` (14), `crates/wipe-server/tests/licence_fetch.rs` (14), and the isolation route list.
- Docs: `CONTEXT.md` (§10 row), `docs/ROADMAP.md` (A2.1), `README.md`.

**Review findings.** There were 37 findings: medium 1, low 29, false 4, maybe-false 3.
- **Patched.** One medium entry: the station installed any licence that verified, even one ending earlier than its own. It now installs only a newer one, re-reading its own licence just before installing. The low entries patched:
  - the docs no longer claim the loop is aborted on drop;
  - the hub warns when a renewal waits and it has no vendor root;
  - the saved-licence message names `--license <path>`;
  - README test rows;
  - three new tests: a hub without a vendor root, a station that trusts no root, and an unwritable settings directory.
- **Deferred (see frontmatter).** No test for a refused challenge; stale AGENTS.md test counts.
- **Rejected,** each with its reason in the Review Triage Log:
  - the start-up line wording;
  - the revoked licence answering "Up to date" (ADR);
  - NAT lockout and jitter (the intent fixes the 24 h schedule);
  - deadlock speculation (same lock order as the owner path);
  - `reissue` refusal tests;
  - the A2.1 strike (spec task);
  - the throttle comment, `FOR UPDATE` cost and the `::text` index;
  - consuming a challenge before a transient error;
  - the placement of the failure sentence;
  - no end-to-end test across the two halves (the agreements were checked by reading);
  - the Tauri host having no root (a known limit);
  - `\n` over HTTP;
  - the 3 s test timeout.

**Follow-up review recommendation.** `false`. One medium entry was patched, and no high. The patch is a single guard covered by its own test.

**Verification**
- `cargo test --workspace --no-fail-fast` with compose up: 1092 passed, 1 failed. The failure is `wipe-hub --test health silent_smtp_and_database_time_out_within_bound`, which is environmental and pre-existing. The baseline commit 8c11473 fails it the same way when built into the repo's `target/` (21 s against a 3.5 s bound). It passes from the patched tree built in a fresh target directory.
- `cargo clippy --workspace --all-targets -- -D warnings` fails only on the pre-existing `clippy::large_enum_variant` in `crates/wipe-engine/src/runner.rs`, which is untouched. With that one lint allowed, the workspace is clean.
- `pnpm typecheck && pnpm test` in `apps/desktop`: 419 passed.
- `docs/generated` is unchanged, so the route golden and the CLI help goldens are the same.
- Matrix audit: every I/O row has a passing test on the hub side, the station side, or both. The "Policy changed" row is tested as an owner's renewal reissue, because the lapse policy was retired.

**Residual risks**
- Nothing loads `enrolled-licence.json` at start-up. A restart while the hub is unreachable falls back to the `--license` file; the next check fetches the licence again.
- The Tauri shell trusts no vendor root, so its check always fails with that reason.
- The loop task lives until the runtime shuts down.
- The portal shows no audit trail. The SPEC CAP-5 checkpoint can only be confirmed in SQL or the tests (rows `licence.issued` and `licence.superseded`, actor `station:<uuid>`).
- No next-term licence and no revocation distribution yet.
