---
title: 'Sessions, CSRF and rate limiting'
type: 'feature'
created: '2026-11-06'
status: 'ready-for-dev'
baseline_revision: '2c55f6864b03ebc69e896be19cabadec25248ab5'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md']
warnings: ['oversized']
deferred: []
---

<intent-contract>

## Intent

**Problem:** Story 1 shipped plain sessions: no CSRF protection on any state-changing route, unlimited password guessing, a sign-up/sign-in that leaves the request's old session valid, `Secure` only on https, and no way to end every session (SPEC CAP-3; CAP-2 "sign-in rate-limited", "wrong-password lockout"; story 1 deferral "rotated on sign-in").

**Approach:** Harden `wipe-hub` sessions to ADR-0007 §3:
- a double-submit CSRF token checked by middleware on every mutating `/api` request;
- Postgres-backed sign-in throttles (per email and per client IP) that lock out after repeated failures;
- the incoming session deleted on sign-in and sign-up;
- the `Secure` cookie attribute on every host except localhost;
- `POST /api/auth/sign-out-everywhere`.

The portal client fetches and sends the token, and home gains a "Sign out everywhere" button.

## Boundaries & Constraints

**Always:**
- **CSRF.** Every `/api/*` request whose method is not GET, HEAD or OPTIONS, including unknown `/api` paths, must carry an `X-CSRF-Token` header equal to the `wipehub_csrf` cookie, and the value must be a well-formed token (43-char base64url, 32 bytes).
  - Compare in constant time.
  - On failure answer 403 `missing or invalid CSRF token`, before any handler work or audit write.
  - `GET /api/auth/csrf` returns `{"csrf_token": "<t>"}` with `Cache-Control: no-store`. It reuses a well-formed incoming cookie; otherwise it mints one (OS RNG, 32 bytes, base64url) and sets it.
- **CSRF cookie.** `wipehub_csrf=<t>; HttpOnly; SameSite=Lax; Path=/; Max-Age=1209600`, plus `Secure` under the same rule as the session cookie.
- **`Secure` policy (ADR-0007 §3).** Both cookies carry `Secure` unless the `--public-base-url` host is `localhost`, `*.localhost` or a loopback IP and the scheme is http. So https always gets `Secure`, and http on a non-local host gets `Secure` plus a start-up warning that browsers will not keep it.
- **Rotation.** Sign-in and sign-up delete the request's incoming `wipehub_session` row, if any, in the same transaction as the new session. A successful sign-in's `auth.sign_in` detail carries `{"rotated": true|false}`.
- **Sign-in throttles**, in a new `auth_throttle` table keyed `email:<address>` and `ip:<peer>`:
  - **Email key.** The email is trimmed and lowercased by Postgres `lower()`, the same function the account lookup uses, so every spelling that reaches one account shares one key. At most 254 chars.
  - **IP key** (owner decision Q2). IPv4 peers, including IPv4-mapped IPv6, key on the full address. IPv6 peers key on their /64 prefix, e.g. `ip:2001:db8:1:2::/64`.
  - **Charge before verifying** (owner decision Q1). Each sign-in attempt is charged against both keys before any user lookup or Argon2 work, in one atomic statement per key that restarts an elapsed window. The window is fixed at 15 minutes from the first charge.
  - An attempt is refused when its key is locked, or when charges in flight have already reached the limit (5 per email, 20 per IP). So **however many attempts arrive at once, at most 5 password checks run per email (20 per IP) per window**.
  - A refused attempt answers 429 `too many failed sign-in attempts; try again later` with `Retry-After` in whole seconds (the time left on the lock, or 1 when it is refused only because checks are in flight). It is refused even with the right password, and it does not extend the lock.
  - A failed attempt keeps its charge. The failure that brings a key's count to its limit sets `locked_until = now() + 15 min`.
  - A successful sign-in deletes the email key and refunds its own charge on the IP key. A user who gets the password right on the 5th try is signed in and is not locked.
  - Unknown and known emails are throttled identically.
  - The peer IP comes from axum `ConnectInfo<SocketAddr>`. When it is absent, there is no IP key.
- **Audit volume** (owner decision Q3). Refused attempts are **not** audited one row each.
  - `auth.sign_in_locked` is written exactly once per lock (actor `anonymous`, subject the key, detail `{"until": …}`), in the transaction that engages it. "This statement engaged the lock" is determined from the row the statement itself wrote, never from an earlier snapshot.
  - Each refused attempt increments a `refused` counter on the throttle row.
  - When a row with `refused > 0` is cleared, restarted or purged, one `auth.sign_in_lock_summary` row (subject the key, detail `{"until": …, "refused": n}`) is written in the same transaction. Each lock therefore produces at most two audit rows.
  - Every sign-in audit row's detail carries `"peer": "<ip key>"` when there is one.
- Audit `auth.sign_out_everywhere` (actor/subject `user:<id>`, detail `{"sessions": n}`).
- The throttle API is reusable by story 3 (reset/magic link): `charge(key, limit, window)`, `confirm_failure(key, limit, lockout)`, `refund(key)`, `clear(key)` in `src/throttle.rs`, generic over the executor.
- Error bodies stay `{"error": "<message>"}`.

**Never:**
- Do not trust `X-Forwarded-For` or other proxy headers (deployment is undecided, ROADMAP §0.5).
- Do not edit migrations 0001–0003.
- No `sqlx::query!`.
- Never log or return raw session or CSRF tokens, except the CSRF token in its own route's body.
- Do not port throttle or CSRF rules into the portal beyond sending the header.
- Not in this story:
  - magic link and password reset, and their rate limits (story 3);
  - Origin/`Sec-Fetch-Site` checks;
  - idle-timeout sessions;
  - CSRF exemptions (none exist; a future Stripe webhook adds its own);
  - the AGENTS.md test-count drift (agent-context file);
  - rate limits on sign-up and resend-verification. Every route that sends email gets its limit in story 3, alongside magic link and reset. Until then, CONTEXT lists the gap.

## Owner decisions

2026-11-08, answering the attempt-1 intent-gap halt (questions under `## Auto Run Result`):
- **Q1 (simultaneous guesses): (a).** Charge each attempt before the password check, and refund it on success.
- **Q2 (IPv6): (a).** Key IPv6 peers by their /64 prefix.
- **Q3 (audit volume): (a).** One `auth.sign_in_locked` row per lock, plus one summary row carrying the refused count. Never one row per refused attempt.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Get token | `GET /api/auth/csrf`, no cookie | 200 `{csrf_token}`, `Set-Cookie: wipehub_csrf=<same>` | — |
| Get token again | `GET /api/auth/csrf` with well-formed cookie | 200 same token, no new `Set-Cookie` | malformed cookie → new token minted |
| Missing CSRF | any POST under `/api` (sign-up, sign-in, sign-out, verify-email, verification-email, sign-out-everywhere, unknown path) without header or cookie, or with header ≠ cookie, or malformed | 403 `missing or invalid CSRF token`; nothing written, no audit | — |
| Valid CSRF | header = cookie = well-formed token | request proceeds as in story 1 | — |
| Rotation | sign-in (or sign-up) carrying a live session cookie A | new cookie B; A now 401 on `me`; `auth.sign_in` detail `rotated:true` | no/invalid incoming cookie → `rotated:false` |
| Email lockout | 5 wrong passwords for one email within 15 min | 1st–5th: 401; 6th, even correct: 429 + `Retry-After` between 890 and 900; `auth.sign_in_locked` once | lock elapsed → sign-in works |
| Concurrent guesses | 20 wrong-password sign-ins for one email sent at once | exactly 5 answer 401 and 15 answer 429; `auth.sign_in_locked` exactly once | — |
| Right on the 5th try | 4 wrong passwords, then the right one | 200; email key gone; no `auth.sign_in_locked` | — |
| Locked hammering | 100 attempts against a locked email | all 429; no audit rows added by them; the row's `refused` = 100; after the lock elapses and the key is next cleared, restarted or purged, one `auth.sign_in_lock_summary` row with `refused: 100` | — |
| IPv6 block | 20 failures from addresses spread across one IPv6 /64, distinct emails | 21st attempt from another address in that /64 is 429 | — |
| Success resets | 4 failures, a success, 4 failures | all 4 later failures 401, no lock | — |
| IP throttle | 20 failures from one IP across distinct emails | 21st attempt (fresh email) 429 | — |
| Sign out everywhere | `POST /api/auth/sign-out-everywhere` signed in with 2+ sessions | 204, cookie cleared, every session of that user 401 on `me`, other users' sessions untouched | not signed in → 401 |
| Cookie policy | public base `http://127.0.0.1`, `http://localhost`, `http://hub.example.com`, `https://localhost` | no `Secure`, no `Secure`, `Secure` (+ start-up warning), `Secure` | — |

</intent-contract>

## Code Map

- `crates/wipe-hub/src/lib.rs`:
  - `router()` builds `api` (health + `auth::routes()` + the `/api`, `/api/`, `/api/*rest` JSON-404 catch-alls). Add the CSRF middleware as `.layer(axum::middleware::from_fn(csrf::require))` on `api` **after** the catch-alls, so it covers them, and before `.merge(site)`.
  - `serve_on` must use `router(state).into_make_service_with_connect_info::<SocketAddr>()`.
- `crates/wipe-hub/src/auth.rs`:
  - `routes()`: add `GET /api/auth/csrf` and `POST /api/auth/sign-out-everywhere`.
  - `cookie_header`/`session_cookie`/`cleared_cookie`: generalise to take the cookie name.
  - `secure_cookies` (currently scheme == https): switch to the new policy, as a pure fn over `&Url` so it can be unit-tested.
  - `session_token` parses cookies; generalise it to `cookie_value(headers, name)`.
  - `sign_in`: charge both keys (`throttle::charge`) before the oversized/lookup branch, and answer 429 on refusal. In the failure arm, call `throttle::confirm_failure` in the same tx as the audit row. On success, clear the email key, refund the IP charge, delete the incoming session, and set `rotated`.
  - `sign_up`: delete the incoming session in its tx.
  - Unit tests at the bottom assert exact cookie strings; update them.
- `crates/wipe-hub/src/accounts.rs`:
  - `delete_session` already exists; reuse it for rotation.
  - Add `delete_user_sessions(executor, user_id) -> u64`.
  - Optionally purge the user's expired sessions in `create_session`.
- `crates/wipe-hub/src/config.rs`: `startup_warnings` gains the http-non-local `Secure` warning, with a unit test next to the existing ones.
- `crates/wipe-hub/migrations/` -- next is `0004_auth_throttle.sql`.
- `crates/wipe-hub/tests/auth.rs`:
  - The `Hub` helpers `post`/`post_empty` (lines ~75-90) send explicit `Cookie` headers. Make them fetch a CSRF token once per `Hub` (`GET /api/auth/csrf`), append `; wipehub_csrf=<t>` to the cookie and send `X-CSRF-Token`, so every story-1 test keeps passing.
  - Add raw-request helpers for the 403 cases.
  - Tests set `expires_at` in the past via `hub.db.pool` to simulate time; do the same with `auth_throttle.locked_until`.
- `crates/wipe-hub/tests/support/mod.rs` -- reuse `TestDb`, `Mailpit`, `unique_address`.
- `apps/portal/src/api/auth.ts`:
  - `request()` is the single fetch path. For POST, obtain the token through a module-level cached `csrfToken(fetchImpl)` (GET `/api/auth/csrf`) and send `X-CSRF-Token`.
  - On a 403 whose `error` is the CSRF message, drop the cache, refetch once and retry once.
  - Export `signOutEverywhere`.
  - Export a test-only `resetCsrfCache()`.
- `apps/portal/src/api/auth.test.ts`: the `respond()` fake answers every call with one response. It needs a router-style fake that serves `/api/auth/csrf` separately.
- `apps/portal/src/pages/HomePage.tsx` -- `signOut` mutation at line ~79; add a "Sign out everywhere" button beside it.
- `CONTEXT.md` line 693 (§10 hub accounts row): its limitations column lists story-2 items and the `Secure` departure. Edit it and add a story-2 row. §14 glossary at ~1049 (**Portal session**): add **CSRF token** and **Sign-in throttle**.
- `README.md` hub/portal rows if they describe auth.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0004_auth_throttle.sql` -- `auth_throttle(key text PK CHECK (key ~ '\S'), failures int NOT NULL CHECK (failures >= 0), refused int NOT NULL DEFAULT 0, window_started_at timestamptz NOT NULL, locked_until timestamptz)` plus an index on `window_started_at` -- shared, restart-proof counters.
- `crates/wipe-hub/src/throttle.rs` -- reusable by story 3. Contents:
  - `charge(key, limit, window)`: an atomic upsert that restarts an elapsed window, refuses when the key is locked or `failures` has reached `limit` (incrementing `refused`), and otherwise increments `failures`.
  - `confirm_failure(key, limit, lockout)`: sets `locked_until` when `failures` has reached `limit`, and reports "engaged" from the row it wrote.
  - `refund(key)` and `clear(key)`.
  - A stale-row purge in its own statement (`FOR UPDATE SKIP LOCKED`).
  - Each row with `refused > 0` that is cleared, restarted or purged emits `auth.sign_in_lock_summary`.
  - Constants `SIGN_IN_EMAIL_LIMIT=5`, `SIGN_IN_IP_LIMIT=20`, `SIGN_IN_WINDOW=15 min`, and `SIGN_IN_LOCKOUT=15 min`, which is passed as the lock length.
  - Argument checks are `assert!`, not `debug_assert!`.
- `crates/wipe-hub/src/auth.rs` `ip_key` -- IPv4 and IPv4-mapped peers use the full address; IPv6 uses its /64. Unit-test both.
- Tests to add beyond the matrix:
  - the purge of a user's expired sessions on sign-in;
  - the stale-throttle purge (other keys' aged rows go, while a locked row stays);
  - a test pinning the portal's `CSRF_ERROR` to the hub's `MSG_BAD_CSRF` text.
- `crates/wipe-hub/src/csrf.rs` -- `CSRF_COOKIE`, `CSRF_HEADER`, `MSG_BAD_CSRF`, middleware `require`, constant-time compare, `GET /api/auth/csrf` handler (or put the handler in `auth.rs`), unit tests for well-formedness and compare.
- `crates/wipe-hub/src/auth.rs`, `src/accounts.rs`, `src/lib.rs`, `src/config.rs` -- per Code Map: cookie policy, rotation, throttling, sign-out-everywhere, connect info, start-up warning.
- `crates/wipe-hub/tests/auth.rs` (and a new `tests/sessions.rs` if `auth.rs` grows unwieldy) -- the CSRF helper upgrade, plus an HTTP test per matrix row and AC: 403 for each mutating route listed, with no audit row and no new rows written; the cookie-policy hosts; and the lockout expiry, which needs `locked_until` moved into the past.
- `apps/portal/src/api/auth.ts`, `auth.test.ts` -- CSRF fetch and caching, the header on every POST, one retry on CSRF 403 and no infinite loop on a second 403, `signOutEverywhere`, 429 surfaced as `HubError` with the hub's message.
- `apps/portal/src/pages/HomePage.tsx` -- "Sign out everywhere" button.
- `apps/portal/src/api/auth.ts`, sign-in page -- on a 429, read `Retry-After` and say "try again in N minutes".
- `CONTEXT.md`, `README.md` -- document the shipped behaviour and remove the story-2 limitations from the story-1 row. Record the new limitations:
  - the throttle keys on the peer IP only, so behind a reverse proxy or a shared office NAT everyone shares one key;
  - anyone who knows an email can trigger that account's lockout;
  - sign-up and resend-verification are not rate-limited until story 3.

**Acceptance Criteria:**
- Given a hub, when each state-changing auth route is posted without a valid CSRF token, then it answers 403 `missing or invalid CSRF token` and the `audit_log`, `users`, `sessions` and `auth_throttle` row counts are unchanged.
- Given the story-1 integration tests updated only to send a CSRF token, when `cargo test -p wipe-hub` runs, then they all pass unchanged in behaviour.
- Given a locked email, when the lock's `locked_until` is moved into the past, then the correct password signs in (200) and the email key is gone.
- Given the portal, when `pnpm --filter portal typecheck`, `test` and `build` run, then all pass and the client tests prove every POST carries `X-CSRF-Token` from `/api/auth/csrf`.

## Spec Change Log

## Review Triage Log

### 2026-11-06 — Review pass
- verdicts: 29 findings — high 4, medium 3, low 19, false 3, maybe-false 0
- attempted change saved at [portal-accounts-story-2-attempt-1.patch](../../../_bmad-output/implementation-artifacts/portal-accounts-story-2-attempt-1.patch) (applies cleanly to 2c55f6864b03ebc69e896be19cabadec25248ab5); code reverted
- findings:
  - `[high]` `[intent_gap]` (blind) Concurrent sign-ins bypass the lockout: `throttle::check` runs on the pool before Argon2 and `record_failure` only after, so N parallel guesses for one email (or IP) all pass the check — far more than 5 guesses per window, each paying Argon2 — Verified in `auth.rs` `sign_in`: check and record are separate statements with Argon2 between and no row/advisory lock. No trivial fix: serializing per key across Argon2 holds a pool connection per waiter (pool-exhaustion DoS), and pre-charging the attempt changes intent-contract rules ("the 5th failure", "a success deletes the email key only", lock audit "in the transaction that engages a lock").
  - `[medium]` `[intent_gap]` (blind) Locked attempts write an unbounded number of `auth.sign_in_failed`/`locked` rows to the append-only audit log on the cheapest path in the hub — Real: the locked branch does no Argon2 and has no limit of its own; but the intent mandates auditing every locked attempt, so bounding it needs an intent decision.
  - `[medium]` `[intent_gap]` (blind) Per-IP key is a full /128 for IPv6, so one /64 rotates source addresses and never reaches the 20-failure limit — Real; the intent fixes the key as `ip:<peer IP>`, so keying IPv6 by prefix needs an intent decision.
  - `[low]` `[reject]` (blind) No escalating lockout; a steady attacker gets 5 guesses per 15 min forever — Intent fixes a 15-min lock; escalation adds complexity beyond the contract.
  - `[low]` `[patch]` (blind) `SIGN_IN_LOCKOUT` is never used; `record_failure` locks for `window` — Real dead constant; moot this pass (intent_gap halt). Fix next attempt: delete it or pass it as the lock length.
  - `[low]` `[patch]` (blind) `debug_assert!(window <= PURGE_AFTER)` vanishes in release, so a longer story-3 window would let the purge delete live counters — Real; moot this pass. Fix: `assert!`, or purge by each row's own window.
  - `[low]` `[patch]` (blind) The purge `DELETE` in `record_failure` seq-scans `auth_throttle` (only PK index) on every failure — Real, grows under spraying; moot this pass. Fix: index `window_started_at` in 0004 (unapplied anywhere yet).
  - `[low]` `[reject]` (blind) Client IP is never recorded in sign-in audit rows — Story 1 already audited email only; adding forensics detail is beyond the contract and not met in everyday use.
  - `[low]` `[reject]` (blind) CSRF token is not rotated on sign-in or bound to the session — Intent prescribes plain double-submit that reuses a well-formed cookie; cookie-tossing is an accepted, documented limitation.
  - `[low]` `[reject]` (blind) CSRF cookie's 14-day Max-Age is never renewed, so a late session outlives it — Intent says a reused cookie gets no new `Set-Cookie`; the portal recovers via its one 403 refetch/retry.
  - `[low]` `[reject]` (blind) Portal drops `Retry-After` — Intent asks only that the 429 surface the hub's message; plumbing the header adds surface.
  - `[low]` `[defer]` (blind) "Sign out everywhere" has no confirmation, shares a disabled state, may show stale errors, and is untested — Stale-error claim is false (signing out unmounts `SignedIn`); no-confirmation is a product choice; the untested part groups with the verification-gap HomePage row.
  - `[low]` `[reject]` (blind) `auth.sign_up` does not record rotation — Intent requires `rotated` only on `auth.sign_in`.
  - `[false]` `[reject]` (blind) Tests hard-code `ip:127.0.0.1` and would split keys on a dual-stack listener — The test hub binds 127.0.0.1 and `ip_key` canonicalizes IPv4-mapped addresses (unit-tested); no shipped path produces split keys.
  - `[low]` `[patch]` (blind) CONTEXT limitations name only the reverse-proxy case for the shared IP lock, not an ordinary office NAT — Real doc gap; moot this pass. Fix: add "or a shared NAT".
  - `[high]` `[intent_gap]` (edge) Concurrent wrong-password requests all pass `check` while Open — Same root cause as the first blind row.
  - `[low]` `[patch]` (edge) Under concurrency `record_failure`'s `prior` CTE reads the statement snapshot, so two calls can both report engaging the lock and `auth.sign_in_locked` is written twice — Real under READ COMMITTED re-check; moot this pass. Fix: treat "engaged" as `locked_until = now() + window` from this statement.
  - `[medium]` `[patch]` (edge) Purge CTE and upsert in one statement can deadlock two concurrent failing sign-ins that lock rows the other's purge wants to delete (500, failure uncounted) — Plausible with stale rows; moot this pass. Fix: purge in its own statement with `FOR UPDATE SKIP LOCKED`.
  - `[false]` `[reject]` (edge) Serving the router without connect info silently disables the IP throttle — The intent says no IP key when ConnectInfo is absent, and the only serve path (`serve_on`, used by `main.rs`) supplies it.
  - `[low]` `[patch]` (edge) Claim that `record_failure` returns "whether this call engaged the lock" is not reliable under concurrency — Same root cause as the duplicate-lock-audit row.
  - `[high]` `[intent_gap]` (edge) Claim of at most 5 guesses per window is false under concurrency — Same root cause as the first blind row.
  - `[low]` `[patch]` (verification-gap) `create_session`'s purge of a user's expired sessions is untested — Pre-verified; moot this pass. Add `sign_in_purges_that_users_expired_sessions`.
  - `[low]` `[patch]` (verification-gap) The stale `auth_throttle` purge is untested — Pre-verified; moot this pass. Add `stale_throttle_rows_are_purged_on_the_next_failure`.
  - `[low]` `[defer]` (verification-gap) HomePage "Sign out everywhere" wiring has no test — The portal has no component-test setup; the story-1 "Sign out" button is equally untested.
  - `[low]` `[reject]` (intent) The CSRF cookie round trip is shown only with hand-built `Cookie` headers, never by a cookie-keeping client — Exact `Set-Cookie` attributes are asserted; a browser e2e harness is beyond this story.
  - `[low]` `[defer]` (intent) HomePage button and 429 display are not exercised — Groups with the verification-gap HomePage row.
  - `[high]` `[intent_gap]` (intent) The lockout is tested only sequentially; concurrent requests in flight run past the 5th failure — Same root cause as the first blind row.
  - `[false]` `[reject]` (intent) The lock-elapsed test moves only `locked_until`, a state real time never produces — The AC prescribes exactly this; the correct-password path tested is the one the AC names.
  - `[low]` `[reject]` (intent) The start-up warning is unit-tested on `startup_warnings`, not via the binary — `main.rs` prints that function's output through the existing, story-1-tested path.

## Design Notes

- **Why double-submit, not a per-session token:** sign-in and sign-up are state-changing but have no session yet, and a login-CSRF defence needs a token there too. One cookie-and-header rule covers anonymous and signed-in routes alike, with no per-route exemptions.
  - The hub sets no CORS headers, so a cross-site page cannot send the custom header.
  - Subdomain cookie-tossing is the known weakness. It is accepted until hosting is decided, and named in CONTEXT.
- **Failure-based throttles** do not punish an office signing in many people from one NAT address. The per-email key is the "wrong-password lockout"; the per-IP key stops spraying across many emails.
- **Lock before verifying** means Argon2 work stays bounded while locked, and a correct password during the lock gets 429, which is what a lockout is.
- **Charge before verifying** (Q1) is what makes the limit hold against parallel requests. Option (b), serializing per key across Argon2, was rejected: it holds a database connection per waiter, so a burst could exhaust the pool and take the portal down.
- **Secure policy example:**

  ```rust
  fn secure_cookies(base: &Url) -> bool {
      base.scheme() == "https" || !is_local_host(base) // localhost, *.localhost, 127/8, ::1
  }
  ```

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including the new CSRF, throttle, rotation and sign-out-everywhere tests.
- `cargo test --workspace` -- expected: all pass.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: all pass.

## Auto Run Result

Status: blocked
Blocking condition: intent gap

The implementation was built and passed all verification. Review found that the lockout itself can be bypassed, and fixing that needs a decision on the intent. The attempted change is saved and the code is reverted (working tree back to `2c55f6864b03ebc69e896be19cabadec25248ab5`, plus this spec).

**Saved patch:** [portal-accounts-story-2-attempt-1.patch](../../../_bmad-output/implementation-artifacts/portal-accounts-story-2-attempt-1.patch). Restore it with `git apply _bmad-output/implementation-artifacts/portal-accounts-story-2-attempt-1.patch`.

**What the patch implements:** every item in the intent contract.
- Double-submit CSRF (`csrf.rs`; `GET /api/auth/csrf`; middleware covering unknown `/api` paths).
- `0004_auth_throttle` and `throttle.rs`.
- Rotation on sign-in and sign-up, and `POST /api/auth/sign-out-everywhere`.
- The localhost-only non-`Secure` cookie policy and its start-up warning.
- ConnectInfo peer IP.
- Portal: token caching, the header on every POST, and one retry on a CSRF 403; a "Sign out everywhere" button.
- CONTEXT and README updates.

**Verification of the attempt (all passed):**
- `cargo test -p wipe-hub`: `tests/auth.rs` 30 passed; 45 lib unit tests.
- `cargo test --workspace`: 260 passed, exit 0.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean.
- `rustfmt --check` on the touched files: clean.
- Portal typecheck, test (39) and build: all passed.
- Every matrix row has a passing test.

**Unresolved questions (intent gap):**

All three questions are about one thing: what happens when someone signs in to the **customer portal** (the vendor website where customers buy and download licences) with a **wrong password**.

**How this story makes sign-in behave**
- The hub counts wrong-password attempts in two ways: per email address, and per network address (IP) the requests come from.
- **5 wrong passwords for one email** within 15 minutes locks password sign-in for that email for 15 minutes. **20 wrong passwords from one IP**, across any emails, locks that IP for 15 minutes.
- While a lock is on, sign-in answers "too many failed sign-in attempts; try again later", even if the password is right. One correct sign-in resets the email's count.
- Every sign-in attempt is written to the **audit log**: the hub's permanent record of security events. By design it can never be edited or deleted (ADR-0007 §6).

The purpose is to stop someone guessing a customer's password by trying thousands of passwords.

**Question 1: Should the 5-guess limit still hold when guesses arrive at the same moment?**
- *Today (attempt 1):* checking a password deliberately takes about a quarter of a second, because Argon2 (the password-hashing algorithm) is slow on purpose. The hub checks "is this email locked?" first, then the password, and only then counts the failure. An attacker who sends 200 guesses at once has all 200 pass the "locked?" check before any of them has been counted. They get 200 guesses instead of 5. A real browser user never does this. A script does it trivially, so the lock stops casual guessing but not a deliberate attack.
- *(a) Count the attempt before checking the password; give it back if the password was right.* 200 simultaneous guesses get exactly 5 checked, and the rest are refused immediately. Customers notice no difference. Cost: small. The rule changes from "the 5th *wrong* password locks" to "the 5th attempt locks unless one succeeded". If someone gets their own password right on the 5th try, it still succeeds because the count is given back.
- *(b) Make attempts for the same email wait in line.* The limit is exact, but every waiting request holds one of the hub's few database connections. A burst could then tie up all of them and make the whole portal unresponsive for everyone. That trades a guessing problem for an easy way to take the site down.
- *(c) Leave it and document it as a known weakness.* No work now. Password guessing stays possible for anyone who writes a simple script, and the "5 attempts" claim is untrue against attackers.
- **Recommendation: (a).** It's the only option that actually enforces the limit without creating a way to take the site down, and customers see no change.
- *If undecided:* the story stays blocked, so password sign-in protection, the CSRF protection and "sign out everywhere" don't ship, and stories 3–5 wait.
- **Answer: a, b or c?**

**Question 2: For IPv6 visitors, should the 20-per-IP limit count the visitor's whole address block instead of a single address?**
- *Background:* home and office internet connections on IPv6 (the newer internet address format) are given a block of about 18 quintillion addresses (a "/64"), not one address. The device can switch freely between addresses in that block.
- *Today (attempt 1):* the per-IP limit counts each exact address. An attacker on IPv6 changes address after every 19 guesses and never hits the limit. That lets them try a few common passwords against many customers' emails, which only the per-IP limit is meant to catch.
- *(a) Count the whole /64 block as one address.* This closes the gap. Cost: very small. Downside: devices sharing one connection (an office, a household) share one IPv6 count, just as they already share one count on IPv4 behind a router.
- *(b) Keep counting exact addresses.* No work. The per-IP limit does nothing against IPv6 attackers.
- **Recommendation: (a).** This is the standard practice for IPv6 rate limiting.
- *If undecided:* blocked, as above.
- **Answer: a or b?**

**Question 3: While an email or IP is locked, should every refused attempt still be written to the audit log?**
- *Today (attempt 1):* yes, one permanent record per refused attempt. Refusing a locked attempt costs the hub almost nothing, so anyone can send millions of requests and add millions of records the log can never shed. That fills the disk and buries the real security events.
- *(a) Record the lock once, with a running count of refused attempts kept alongside it.* The log shows "email X locked at 10:02 until 10:17; 48,211 attempts refused while locked". The audit trail stays useful and bounded. Cost: small.
- *(b) Record every refused attempt (today's behaviour).* The trail is complete, but anyone can grow it without limit.
- *(c) Record a sample, such as every 100th refusal.* Bounded, but the count is approximate and the rule is harder to explain to an auditor.
- **Recommendation: (a).**
- *If undecided:* blocked, as above.
- **Answer: a, b or c?**

**Fixes queued for the next attempt (low/medium; moot this pass, see the triage log):**
- Drop or use `SIGN_IN_LOCKOUT`.
- Make the `PURGE_AFTER` check a real `assert!`.
- Index `window_started_at`.
- Purge in its own statement with `SKIP LOCKED` to avoid deadlocks.
- Detect "engaged the lock" from this statement's `now() + window`.
- Mention a shared NAT in the CONTEXT limitations.
- Add tests for the expired-session purge and the stale-throttle purge.

**Deferred:** portal HomePage component tests. The portal has no component-test setup yet.

**Rejected:** see the triage log; each has its reason recorded.

Follow-up review recommended: false. No patches were applied this pass.

**Resolved 2026-11-08.** The owner answered (a) to all three questions. The answers are written into `<intent-contract>` (see `## Owner decisions`), together with the queued fixes above. Status is set back to `ready-for-dev`. The attempt-1 patch is a valid starting point for everything except the throttle and audit changes.
