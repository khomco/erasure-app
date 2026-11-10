---
title: 'Hub enrollment start and poll (device authorization)'
type: 'feature'
created: '2026-11-09'
status: 'done'
baseline_revision: '5e31eb3cb60fef9c42a3056b24dea8b8e34d61c0'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '{project-root}/docs/adr/0008-station-enrollment-and-licence-fetch.md'
  - '{project-root}/.scratch/station-enrollment/SPEC.md'
warnings: [oversized]
deferred: []
---

<intent-contract>

## Intent

**Problem:** A station can only be registered by an owner typing its key id into the portal and copying a licence file to the bench. ADR-0008 §1 adds enrollment by device authorization (RFC 8628, the "enter this code on your phone" flow); its hub half does not exist.

**Approach:** Add an `enrollments` table and two no-session hub routes: `POST /api/enrollments` (a station, proving it holds its instance key, asks to enroll and gets a user code and a secret device code) and `POST /api/enrollments/poll` (the station polls with its device code until the enrollment is approved, denied or expired, and collects its licence once approved). Approval itself is story 2; here tests approve by writing the row directly.

## Boundaries & Constraints

**Always:**
- The start body is `{instance_key_id, public_key_b64, station_id, label?, signature_b64}`; 400 (`{"error": msg}`) when the key id does not equal `wipe_cert::VerifyingKey::public_key_id()` of the key, the key or signature does not decode or verify, `station_id` is blank, or a text is over `stations::TEXT_MAX_CHARS`. Text is trimmed like `stations::optional_text`.
- Start answers 200 `{device_code, user_code, verification_uri, verification_uri_complete, expires_in: 600, interval: 5}`; `verification_uri` = `{public_base_url}/activate`, `_complete` adds `?code=XXXX-XXXX`. A key has at most one `pending` enrollment: a new start deletes its pending row in the same transaction.
- User code: 8 chars from `BCDFGHJKLMNPQRSTVWXZ` drawn with the OS RNG, stored undashed, shown `XXXX-XXXX`; unique among pending rows (retry on collision). A pub `normalize_user_code` accepts any case, with or without the dash (story 2 uses it). Device code: 32 OS-RNG bytes, base64url unpadded, stored only as `accounts::hash_token` (SHA-256).
- Poll body `{device_code, signature_b64}`, signature by the enrollment's stored key. Errors are 400 `{"error": code}` (RFC 6749 §5.2 shape): `authorization_pending`; `slow_down` (poll sooner than `interval` s after the previous poll's arrival: interval += 5 and persisted, body also carries `interval`); `expired_token` (past expiry, or already collected); `access_denied` (denied); `invalid_grant` (unknown/malformed device code, or bad signature). Approved and unexpired → 200 `{station, licence}`, status → `collected`, in one transaction.
- `station` is the `stations::Station` shape `GET /api/stations` gives for that row; `licence` is the stored signed licence file parsed as JSON (what `wipe_license` loads), so a station can verify it against its vendor root.
- Starts charge `enroll:ip:<peer>` (`auth::ip_key`, IPv6 by /64) via `throttle.rs` after validation, final like leads: 20 per hour, then locked 1 h → 429 with `Retry-After`. Lock audited once as `enrollment.start_locked`; summaries as `enrollment.start_lock_summary` (map the prefix in `auth::lock_summary_action`). No peer, no charge.
- Each start deletes up to `throttle::PURGE_BATCH` enrollments whose `expires_at` is over 24 h past.
- `enrollment.requested` audited in the start transaction: actor `instance-key:<key id>`, subject `enrollment:<uuid>`, detail `{instance_key_id, station_id, label, replaced, peer}`. Nothing audited for expiry, polls or collection.

**Never:**
- No portal approve/deny route, page or code lookup (story 2); no station-side client, CLI or request signer for post-enrollment calls (story 3).
- Never store or log a raw device code; never `sqlx::query!`; never edit an applied migration.
- Nothing here touches the station crates or gates an erasure.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected | Error |
|---|---|---|---|
| Start ok | valid signed body | 200 codes; 1 pending row; audit row | — |
| Key id mismatch / bad sig / blank station_id | | 400, no row, no audit | message |
| Restart same key | pending exists | old row gone; old device code polls `invalid_grant` | — |
| Throttled | 21st start from one IP in an hour | 429 + `Retry-After` | — |
| Poll pending | first poll | 400 `authorization_pending` | — |
| Poll too fast | second poll < 5 s | 400 `slow_down`, interval 10 | — |
| Poll approved | row approved, unexpired | 200 `{station, licence}`; licence verifies against root; row `collected` | — |
| Poll again | collected | 400 `expired_token` | — |
| Expired / denied | | `expired_token` / `access_denied` | — |
| Wrong signer | poll signed by other key | 400 `invalid_grant` | — |

## Autonomous decisions

- **CSRF exemption (safe default, existing precedent).** Stations have no cookie, so the double-submit CSRF check would refuse every enrollment call. Options: exempt the two routes, or invent a station CSRF token. Chosen: exempt `POST /api/enrollments` and `/api/enrollments/poll` in `csrf::is_exempt`, as `POST /api/payments/webhook` is, because the instance-key signature is the credential and the routes set and read no cookie.
- **What start and poll sign (ADR-0008 §2 covers requests *after* enrollment).** A hub challenge needs a prior call, and start is the first call. Chosen: domain-separated messages without challenge — start signs `wipestation-enrollment-start-v1\n` + the JSON of `{instance_key_id, public_key_b64, station_id, label}` in that field order; poll signs `wipestation-enrollment-poll-v1\n` + device code. Builders, signer and verifier live in `wipe-license` so story 3's station uses the same code. A replayed start only replaces the pending enrollment, and a replayed poll yields a licence usable only with the key, which ADR-0008 already accepts.
- **Poll errors beyond RFC 8628 §3.5.** Unknown codes and bad signatures use RFC 6749's `invalid_grant`, which RFC 8628 inherits; a collected code is `expired_token`, RFC 8628's "start again" signal. An approved enrollment past its expiry is `expired_token` too; the licence stays downloadable from the portal.
- **Limits.** 20 starts per address per hour (sign-in's per-IP figure); expired rows kept 24 h so late polls still get the right error.

</intent-contract>

## Code Map

- `crates/wipe-hub/src/lead_routes.rs` -- template for a no-session route: `ConnectInfo` peer, `charge_submission` (charge, `confirm_failure`, lock audit, summary).
- `crates/wipe-hub/src/throttle.rs` -- add `ENROLL_KEY_PREFIX = "enroll:"`, limit/window/lockout consts; update module doc.
- `crates/wipe-hub/src/auth.rs:516` `ip_key`, `:528` `with_peer`, `:539` `lock_summary_action` (+ its unit test ~`:1510`), `locked_response`, `ApiError`, `JsonBody`.
- `crates/wipe-hub/src/csrf.rs` `is_exempt` -- add the two paths; doc comment.
- `crates/wipe-hub/src/accounts.rs:277` `hash_token`; `csrf::generate_token` shape for the device code.
- `crates/wipe-hub/src/stations.rs` -- `Station`, `TEXT_MAX_CHARS`, `is_instance_key_id`, `optional_text`, `register` (test approval), stored licence `document`.
- `crates/wipe-hub/src/lib.rs` `router` -- merge new routes; `config.public_base_url`.
- `crates/wipe-hub/migrations/0014_stations.sql` -- constraint/trigger style to follow.
- `crates/wipe-cert/src/sign.rs` -- `SigningKey`, `VerifyingKey::{from_bytes, public_key_id, to_base64}` (STANDARD_NO_PAD).
- `crates/wipe-hub/tests/support/hub.rs` -- `spawn_hub_with_root`, `verified_owner`, `audited`, `count`; `tests/stations.rs` for making an entitlement and registering; `tests/auth.rs:1485` for a chosen peer.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-license/src/enrollment.rs` (+ `lib.rs` `pub mod`) -- message builders, `sign_*` with `wipe_cert::SigningKey`, `verify_*` returning a typed error; unit tests incl. tampered field and wrong key.
- `crates/wipe-hub/migrations/0019_enrollments.sql` -- `enrollments`: id, instance_key_id (key-id CHECK), public_key_b64, station_id, label, requested_from (text IP, nullable), user_code, device_code_hash bytea unique, status CHECK in (pending, approved, denied, collected), poll_interval_secs, last_polled_at, created_at, expires_at, approved_by, approved_at, organization_id, registered_station, licence_id (FKs); CHECK approved/collected ⇔ approver, organization, station, licence set; partial unique indexes on instance_key_id and user_code WHERE pending; index on expires_at.
- `crates/wipe-hub/src/enrollments.rs` -- codes, `normalize_user_code`, `start`, `poll`, purge; module doc listing the rules above.
- `crates/wipe-hub/src/enrollment_routes.rs` + `lib.rs` -- routes, throttle charge, RFC error mapping.
- `crates/wipe-hub/src/{throttle,auth,csrf}.rs` -- as Code Map.
- `crates/wipe-hub/tests/enrollments.rs` -- every matrix row plus throttle (lock + audit), purge, IPv6 /64 key, 600/5 values, `normalize_user_code`, no raw device code in the DB; approve by `stations::register` + `UPDATE enrollments` (and backdate `last_polled_at`/`expires_at`) via SQL.
- `CONTEXT.md` (§10.2 row, §14 **Enrollment** entry), `README.md` hub row -- document the routes, table and limits.

**Acceptance Criteria:**
- Given a hub with a vendor root and Postgres up, when `cargo test -p wipe-hub --test enrollments` runs, then every matrix row passes against real HTTP.
- Given an approved enrollment, when the station polls, then the returned `licence` verifies with `wipe_license` against the hub's root and names the enrollment's instance key.
- Given any start or poll, when the `enrollments` table is inspected, then no column holds the raw device code.

## Spec Change Log

## Review Triage Log

### 2026-11-09 — Review pass
- verdicts: 36 findings — high 0, medium 7, low 26, false 3, maybe-false 0 (Blind's schema finding split into its two claims; the 6 intent-alignment observations are counted)
- findings:
  - Blind Hunter
    - `[low]` `[reject]` Approved enrollment can expire before collection — real but chosen in the intent contract's autonomous decisions (licence stays downloadable in the portal); a station polling every 5 s collects within seconds; fix would edit the spec.
    - `[medium]` `[patch]` Collection hands over a licence of a released station or a revoked/superseded licence — poll now answers `expired_token` unless station unreleased and licence `active`; test added.
    - `[medium]` `[patch]` No `Cache-Control: no-store` on responses carrying the device code / licence (RFC 6749 §5.1) — `no-store` and `Pragma: no-cache` set on every start and poll response; asserted in tests.
    - `[low]` `[reject]` Polls unthrottled and refused starts uncharged — signature check is cheap, lookup is a unique-index hit; charging after validation is the spec's rule (leads precedent); a poll throttle adds a new mechanism for an unlikely load.
    - `[low]` `[reject]` Poll interval uncapped — only grows when a client ignores `slow_down`; our own client (story 3) honours it; a cap adds new behaviour the spec does not set.
    - `[low]` `[reject]` Schema lets an expired enrollment be approved — no approval writer exists until story 2, which must check expiry in code; extra trigger branch guards undemonstrated state.
    - `[low]` `[patch]` Test comment attributes the `poll_interval_secs = 1` refusal to the trigger, not the CHECK — comment corrected.
    - `[low]` `[reject]` Signed start replayable to disrupt a pending enrollment — needs captured TLS traffic; accepted in the intent contract's autonomous decisions; fix would edit the spec.
    - `[false]` `[reject]` Start should refuse a key already registered — registration is per organization (`stations_active_key_idx` is on organization + key) and the organization is unknown until approval, so the hub cannot refuse at start.
    - `[low]` `[reject]` `error` is a message on start and a code on poll; malformed poll body is not `invalid_request` — start's message form is the spec's rule; malformed poll bodies come only from a broken client.
    - `[low]` `[reject]` Anonymous free text (station id, label) written into the append-only audit log — the audit detail is set by the intent contract; fix would edit the spec. Recorded as a residual risk for the owner.
    - `[low]` `[patch]` Purge subquery lacks `SKIP LOCKED` — now `ORDER BY expires_at … FOR UPDATE SKIP LOCKED`, as `throttle::purge_stale`.
    - `[medium]` `[patch]` Missing concurrency tests (concurrent starts, concurrent polls, approved `slow_down`) — grouped with the verification-gap rows below; tests added. User-code retry and purge-summary sub-items rejected as low (retry needs a 2^-34 collision; summary mapping is unit-tested).
    - `[low]` `[patch]` README hub test count off by one (174 unit, not 173) — row corrected.
    - `[false]` `[reject]` Story file and ROADMAP not in the diff — story file is committed at finalize; ROADMAP A2.1 is struck only when the epic ships.
  - Edge Case Hunter
    - `[medium]` `[patch]` Released station / non-active licence collected — same root cause as Blind's; same patch.
    - `[low]` `[reject]` Schema does not tie `licence_id` to the enrollment's key — only story 2 writes approvals; a guard here protects state nothing can produce yet.
    - `[low]` `[reject]` NUL in station id/label gives 500 instead of 400 — attacker-only input, same pre-existing behaviour as `POST /api/stations`; fix adds a guard.
    - `[low]` `[reject]` Polls unthrottled — same as Blind's; rejected.
    - `[low]` `[reject]` Invalid starts never charged — same as Blind's; rejected.
    - `[low]` `[reject]` Malformed poll body not mapped to `invalid_request` — same as Blind's; rejected.
    - `[low]` `[reject]` Start while an approved enrollment awaits collection opens a new pending one — duplicate registration in the same organization is refused by `stations::register`; story 2 concern.
    - `[low]` `[reject]` A start that fails after the charge still counts — same as the lead form; negligible.
    - `[low]` `[reject]` Claim "licence names the enrollment's key" unenforced by the hub — same root as the schema-tie row; rejected.
  - Verification Gap
    - `[medium]` `[patch]` No concurrent-start test for the advisory lock — test added.
    - `[medium]` `[patch]` No concurrent-poll test for single handover — test added.
    - `[medium]` `[patch]` Trigger's approval-freeze branch untested — approved and collected cases added.
    - `[low]` `[patch]` Denied + expired order unpinned — case added (`access_denied`).
    - `[low]` `[patch]` Approved polled too soon untested (filed `defer`) — cheap; test added asserting `slow_down` and status unchanged.
    - `[low]` `[patch]` README count (other finding) — same as Blind's; fixed.
  - Intent Alignment
    - `[low]` `[reject]` Separate unchallenged signing scheme vs CAP-3's shared §2 scheme — ADR-0008 §2 covers requests after enrollment; recorded autonomous decision; both live in `wipe-license`.
    - `[low]` `[reject]` Approved past expiry returns `expired_token` — same as Blind's first row.
    - `[low]` `[reject]` Throttle covers validated starts only — same as Blind's.
    - `[false]` `[reject]` Audit `peer` is the throttle key, not the full address — every hub audit row records `peer` this way; the full address is on the enrollment row for the approval screen.
    - `[low]` `[reject]` JSON wire format, no `grant_type`/`client_id` — ADR-0008 §5 says JSON over REST.
    - `[low]` `[reject]` No station-side caller exercised — the intent excludes it (story 3).

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass, no compile errors.
- `cargo clippy -p wipe-hub -p wipe-license --all-targets -- -D warnings` -- expected: clean.
- `rustfmt --edition 2021 <touched .rs files>` -- touched files only.

## Auto Run Result

Status: done

**Summary.** The hub half of station enrollment (ADR-0008 §1, CAP-1). A station sends its instance public key, station id and label, signed with that key, to `POST /api/enrollments`. It gets back an 8-letter user code (`XXXX-XXXX`), the `/activate` address and a secret device code. It then polls `POST /api/enrollments/poll`, which answers with RFC 8628 error codes until the enrollment is approved, and then hands over `{station, licence}` once. Starts are throttled per address (20 an hour) and audited (`enrollment.requested`). Approval itself is story 2: in these tests an approval is written straight to the database.

**Files changed**
- `crates/wipe-license/src/enrollment.rs` (new): the start and poll request shapes, the domain-separated signed messages, and the signer and verifier, shared with the station in story 3.
- `crates/wipe-hub/migrations/0019_enrollments.sql` (new): the `enrollments` table, its constraints, partial unique indexes and lifecycle trigger.
- `crates/wipe-hub/src/enrollments.rs` (new): the codes, `normalize_user_code`, `start` (replace, purge, audit) and `poll` (RFC outcomes, `slow_down`, one-time collection of an active licence for an unreleased station).
- `crates/wipe-hub/src/enrollment_routes.rs` (new): both routes, the `enroll:ip:` throttle, the `/activate` links, and `no-store`/`no-cache` on every response.
- `crates/wipe-hub/src/{lib,csrf,throttle,auth,stations}.rs`: router wiring, the CSRF exemption for the two routes, the throttle constants, the lock-summary mapping, and `optional_text` made `pub(crate)`.
- `crates/wipe-hub/tests/enrollments.rs` (new, 22 tests); `tests/isolation.rs` (both routes classified as anonymous).
- `CONTEXT.md` (§10.2 row, §14 **Enrollment**), `README.md` (hub row and test counts).

**Review.** 36 findings: 0 high, 7 medium, 26 low, 3 false.
- **Patched:** 6 items, from 4 medium entries and 4 low ones.
  - A released station's or non-active licence is no longer handed over.
  - Every start and poll response now carries `Cache-Control: no-store` and `Pragma: no-cache`.
  - The purge uses `SKIP LOCKED`.
  - New tests cover:
    - concurrent starts;
    - concurrent polls;
    - the trigger's approval freeze;
    - a denied enrollment that has also expired;
    - an approved enrollment polled too soon.
  - A misleading test comment was corrected.
  - The README test count was corrected.
- **Deferred:** none.
- **Rejected:** every rejected finding and its reason is in the Review Triage Log above. In short:
  - choices the intent contract made itself (an approved enrollment still expires, starts are charged only after validation, start replay is accepted, the audit detail shape);
  - guards for states that only story 2 can create;
  - low-likelihood input from a broken client or an attacker;
  - three false claims.

**Follow-up review recommended: true.** The patches touched 4 medium entries: the handover gate, the no-store headers, and the concurrency and trigger tests. The risk not yet re-verified is the new handover gate in `enrollments::poll`, which answers `expired_token` for a released station or a non-active licence. No independent reviewer has looked at that code since it was patched.

**Verification**
- `cargo test --workspace --no-fail-fast`: 980 passed, 1 failed. The failure is `wipe-hub` `tests/health.rs::silent_smtp_and_database_time_out_within_bound`, which took about 19 s against a 3.5 s limit. It fails the same way on the baseline commit `5e31eb3cb60fef9c42a3056b24dea8b8e34d61c0`, checked in a temporary worktree, so it is a pre-existing problem with this machine's environment and is unrelated to this story.
- `cargo clippy -p wipe-hub -p wipe-license --all-targets -- -D warnings`: clean.
- `rustfmt --edition 2021 --check` on the touched files: clean.

**Residual risks**
- `verification_uri` points at `/activate`, which does not exist until story 2.
- The poll interval has no upper limit.
- An anonymous caller's station id and label (up to 200 characters each, 20 starts per address per hour) go into the append-only audit log, where they can never be erased. The owner may want approval to be the first audited point that records free text.
- A captured signed start can be replayed to replace a bench's pending enrollment. This needs captured TLS traffic and was an accepted decision.
