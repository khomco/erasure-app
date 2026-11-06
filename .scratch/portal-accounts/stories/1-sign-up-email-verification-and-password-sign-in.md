---
title: 'Sign-up, email verification and password sign-in'
type: 'feature'
created: '2026-11-06'
status: 'done'
baseline_revision: 'bcb60c01d8348feff7f3706cee23dfe12696abb0'
review_loop_iteration: 0
followup_review_recommended: true
context: ['{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md']
warnings: ['oversized']
deferred:
  - summary: >-
      Sign-in and sign-up issue a new session but leave the request's existing session row valid (ADR-0007 §3 "rotated on sign-in").
    evidence: |-
      sign_in/sign_up never call delete_session on the incoming wipehub_session; the old row stays valid for 14 days. Rotation is CAP-3, owned by story 2.
    location: >-
      crates/wipe-hub/src/auth.rs (sign_in, sign_up)
    severity: medium
  - summary: >-
      Portal pages (sign-up, sign-in, verify-email run-once post, home account panel, validateSearch wiring) have no tests.
    evidence: |-
      vitest runs in node env over src/**/*.test.ts only; no DOM testing setup. Renaming the verify-email search key would break every emailed link with all tests green. Interim: extract validateSearch and unit-test it.
    location: >-
      apps/portal/src/pages/, apps/portal/src/main.tsx
    severity: medium
  - summary: >-
      AGENTS.md is stale: says 209 workspace tests and does not mention --mail-from / WIPE_HUB_MAIL_FROM.
    evidence: |-
      Fix edits an agent-context file, so it is deferred by rule; workspace now runs 239 tests.
    location: >-
      AGENTS.md
    severity: low
  - summary: >-
      Unknown-email timing equalisation (dummy Argon2 verify) has no test.
    evidence: |-
      Replacing the None arm of verify_password with plain false passes every test. A deterministic check needs a seam (verification counter); wall-clock assertions would be flaky. Revisit with story 2 rate-limit/lockout work.
    location: >-
      crates/wipe-hub/src/accounts.rs (verify_password)
    severity: medium
---

<intent-contract>

## Intent

**Problem:** The hub has no accounts: nobody can register an organization, prove their email address, or sign in to the portal, so nothing commercial (orders, licences, downloads) has an owner to belong to (SPEC CAP-1, CAP-2 password part).

**Approach:** Add organizations, users (role `owner`/`member`/`vendor_admin`), single-purpose emailed tokens and server-side sessions to Postgres; expose sign-up, verify-email, sign-in, sign-out, me and resend-verification under `/api/auth/*`; hash passwords with Argon2id; send the verification email over SMTP (Mailpit locally); add sign-up, sign-in and verify-email pages to the portal.

## Boundaries & Constraints

**Always:**
- Passwords hashed with Argon2id (PHC string `$argon2id$…`); hashing/verification off the async runtime (`spawn_blocking`). Password length 12–1024 chars.
- Emails unique case-insensitively; looked up by `lower(email)`.
- Session and email tokens: 32 random bytes from the OS RNG, base64url in transit, only their SHA-256 stored. Verification tokens single-use, expire after 24 h.
- Session cookie `wipehub_session`: `HttpOnly; SameSite=Lax; Path=/; Max-Age` (14 days, matching the row's `expires_at`); `Secure` when `--public-base-url` is `https`.
- Sign-in failure is one generic 401 (`invalid email or password`) whether the email is unknown or the password wrong; an unknown email still runs one Argon2 verification so timing does not reveal it.
- Every auth event is written with `audit::record` in the same transaction as its effect: `auth.sign_up`, `auth.email_verified`, `auth.sign_in`, `auth.sign_in_failed`, `auth.sign_out`. Actor `user:<uuid>` (or `anonymous` for failed sign-in, subject `email:<lowercased>`).
- Customer-side reads go through functions taking the signed-in user's organization id (data-layer scoping, SPEC Constraints).
- Every JSON error body is `{"error": "<message>"}`.

**Never:**
- No third-party auth service or SSO; no `sqlx::query!` macros; no editing migrations 0001/0002.
- Not in this story (story 2/3/4/5): CSRF tokens, session rotation policy beyond issuing a fresh session per sign-in, rate limiting and wrong-password lockout, sign-out everywhere, magic links, password reset, team/profile pages, `vendor_admin` creation, purchase gating (no purchase route exists yet; `email_verified` in `/api/auth/me` is the field later gating reads).
- Never log or return a password, a password hash or a raw token (except the token inside the email body).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Sign-up | `POST /api/auth/sign-up {email,password,name,organization_name}` new email | 201 `{user,organization,verification_email_sent:true}`, session cookie set; org created; user role `owner`, unverified; verification email to the address in Mailpit with a `{public_base_url}/verify-email?token=…` link | — |
| Duplicate email | email differing only in case from an existing user | 409 `an account with this email already exists`; no new org | nothing written |
| Invalid sign-up | password < 12 chars, blank name/org, malformed email, missing field | 400 naming the problem | nothing written |
| Mail down at sign-up | SMTP unreachable | 201 with `verification_email_sent:false`; account exists | warning logged |
| Verify | `POST /api/auth/verify-email {token}` valid token | 200 `{email_verified:true}`; `me` shows verified | — |
| Reused / expired / unknown token | same token again, or `expires_at` past, or garbage | 400 `this verification link is invalid or has expired` | user unchanged |
| Sign-in | correct email (any case) + password | 200 `{user,organization}` + new session cookie | — |
| Bad sign-in | wrong password or unknown email | 401 `invalid email or password`, identical body | `auth.sign_in_failed` audited |
| Me | `GET /api/auth/me` with/without valid cookie | 200 `{user:{id,email,name,role,email_verified},organization:{id,name}}` / 401 `not signed in` | — |
| Sign-out | `POST /api/auth/sign-out` with cookie | 204, cookie cleared (`Max-Age=0`); old cookie now 401 on `me` | without a session: 204 |
| Resend | `POST /api/auth/verification-email` signed in | 202 new email sent; 409 if already verified; 401 if not signed in; 502 if SMTP fails | — |

</intent-contract>

## Code Map

- `crates/wipe-hub/src/lib.rs` -- `AppState` (add `Mailer`), `router()` (add `/api/auth/*` before the `/api/*rest` JSON 404 catch-all).
- `crates/wipe-hub/src/config.rs` -- `Args`/`Config`; add `--mail-from` / `WIPE_HUB_MAIL_FROM` (default `Wipestation <no-reply@wipestation.localhost>`), validated as a mailbox at start-up (new `ConfigError` variant). `Args` literals at `src/config.rs:287`, `tests/health.rs:44`, `tests/portal.rs:30` must gain the field.
- `crates/wipe-hub/src/audit.rs` -- `record(executor, &AuditEntry)`; pass `&mut *tx`.
- `crates/wipe-hub/src/health.rs` -- `probe_smtp` pattern; mailer is separate.
- `crates/wipe-hub/migrations/` -- next is `0003_accounts.sql`.
- `crates/wipe-hub/tests/support/mod.rs` -- `TestDb::migrated()`, `Mailpit::wait_for_message_to`, `unique_address()`; reuse, do not duplicate.
- `crates/wipe-hub/tests/health.rs:43-63` -- pattern for building a `Config` and spawning `serve_on` on port 0.
- `apps/portal/src/main.tsx` -- TanStack route tree; `apps/portal/src/api/health.ts` -- fetch-with-injectable-`fetchImpl` + type-guard pattern and its vitest file.
- `Cargo.toml` (workspace) -- sqlx features (add `uuid`); `rand`, `sha2`, `base64`, `uuid` already in workspace deps.

## Tasks & Acceptance

**Execution:**
- `Cargo.toml`, `crates/wipe-hub/Cargo.toml` -- add sqlx `uuid` feature; hub deps `argon2 = "0.5"` (MSRV 1.65; 0.6 needs newer Rust), `lettre = "=0.11.19"` (last release with rust-version ≤ 1.80) with `default-features = false, features = ["builder","smtp-transport","tokio1","hostname"]`, plus workspace `rand`, `sha2`, `base64`, `uuid` -- dependencies within MSRV 1.80.
- `crates/wipe-hub/migrations/0003_accounts.sql` -- `organizations(id uuid pk default gen_random_uuid(), name, created_at)`, `users(id uuid pk, organization_id fk, email, name, password_hash, role check in (owner,member,vendor_admin), email_verified_at null, created_at)` + unique index on `lower(email)`, `auth_tokens(token_hash bytea pk, user_id fk, purpose check in ('verify_email'), expires_at, used_at)`, `sessions(token_hash bytea pk, user_id fk on delete cascade, created_at, expires_at)`; non-blank CHECKs like `audit_log`.
- `crates/wipe-hub/src/mail.rs` -- `Mailer` (lettre `AsyncSmtpTransport<Tokio1Executor>::builder_dangerous(host).port(port)`, 10 s timeout, plain SMTP) with `send_verification(to, link)`; plain-text body containing the link.
- `crates/wipe-hub/src/accounts.rs` -- data layer: create org+owner, find user by email, verify token, sessions create/lookup/delete, token helpers (generate, hash), Argon2id hash/verify, input validation.
- `crates/wipe-hub/src/auth.rs` -- handlers for the matrix routes, `CurrentUser` extractor (cookie → session → user+org, 401 otherwise), cookie building, audit writes.
- `crates/wipe-hub/src/config.rs`, `src/lib.rs`, `tests/health.rs`, `tests/portal.rs` -- mail-from flag, mailer in state, routes, updated `Args` literals.
- `crates/wipe-hub/tests/auth.rs` -- integration tests over HTTP against `TestDb::migrated()` + Mailpit covering every matrix row and the ACs below.
- `apps/portal/src/api/auth.ts` (+ `auth.test.ts`) -- typed client mirroring the Rust shapes: `signUp`, `signIn`, `signOut`, `fetchMe` (401 → `null`), `verifyEmail`, `resendVerification`; errors surface the hub's `error` message.
- `apps/portal/src/pages/{SignUpPage,SignInPage,VerifyEmailPage}.tsx`, `HomePage.tsx`, `main.tsx` -- routes `/sign-up`, `/sign-in`, `/verify-email?token=`; home shows the signed-in user and organization, an "email not verified" notice with a resend button, sign-out, or sign-in/sign-up links when signed out.
- `CONTEXT.md` (§10 hub row, §14 glossary: Organization, Hub user role, Portal session), `README.md` (hub/portal rows) -- document what shipped.

**Acceptance Criteria:**
- Given a fresh database, when a visitor signs up, then `organizations` and `users` each gain one row, the user's `password_hash` starts with `$argon2id$`, and the audit log holds `auth.sign_up` with actor `user:<id>`.
- Given the verification email in Mailpit, when its link's token is posted to `/api/auth/verify-email`, then `GET /api/auth/me` with the sign-up cookie reports `email_verified: true` and `auth.email_verified` is audited.
- Given a signed-up user, when they sign in with the right password, then the response cookie differs from the sign-up cookie, both work on `me` until signed out, and `auth.sign_in` is audited.
- Given a session cookie, when signed out, then that cookie gets 401 from `me` and `auth.sign_out` is audited.
- Given any set session cookie, when inspected, then it carries `HttpOnly`, `SameSite=Lax` and `Path=/`, and no `Secure` for an `http://` public base URL.
- Given the portal, when `pnpm --filter portal typecheck`, `test` and `build` run, then all pass and the client tests cover each auth function's success and error mapping.

## Spec Change Log

## Review Triage Log

### 2026-11-06 — Review pass
- verdicts: 38 findings — high 0, medium 9, low 21, false 5, maybe-false 0 (verification-gap layer findings taken as filed)
- findings:
  - `[medium]` `[patch]` Blind: sign-in email/password unbounded; huge emails written to append-only audit_log — oversized input now a generic 401 with no lookup or Argon2, audited as `oversized_input`; audit subject truncated to 254 chars; new test.
  - `[medium]` `[defer]` Blind: sign-in/sign-up leave the incoming session valid (ADR "rotated on sign-in") — CAP-3 belongs to story 2 by the stories.yaml intent; deferred.
  - `[medium]` `[patch]` Blind: sign-up page ignores `verification_email_sent:false`, so home says the link was emailed — home notice reworded to point to resend if no email arrived.
  - `[medium]` `[patch]` Blind: "only hashes stored" test compares 43 bytes against a 32-byte column and can never fail — test now asserts SHA-256(decoded token/cookie) matches exactly one auth_tokens/sessions row, raw bytes absent.
  - `[low]` `[reject]` Blind: resend leaves earlier links valid — every link verifies the same address for the same user, so there is no harm; the after-verification part is fixed by the E7 patch.
  - `[low]` `[patch]` Blind: CONTEXT does not say sign-up 409 reveals registered emails — limitation added to the §10 row.
  - `[low]` `[patch]` Blind: dummy hash built lazily, so the first unknown-email sign-in is slower — `prepare_dummy_hash()` called in `AppState::new`.
  - `[false]` `[reject]` Blind: auth responses lack Cache-Control no-store — POST responses are not cached and `me` carries no Last-Modified/Expires, so no heuristic caching applies.
  - `[medium]` `[defer]` Blind: portal pages untested — no DOM test environment; the story's ACs require client-module tests only; deferred.
  - `[low]` `[patch]` Blind: VerifyEmailPage boolean ref ignores a changed token — ref now stores the posted token.
  - `[low]` `[reject]` Blind: portal keeps showing signed-in state after a 401 on resend — 14-day sessions make this rare, and the fix adds an error branch.
  - `[low]` `[defer]` Blind: AGENTS.md test count and new flag not updated — edits an agent-context file, so deferred by rule.
  - `[false]` `[reject]` Blind: STATUS.md / stories.yaml not updated — earlier stories (bcb60c0, 60cd9af) did not touch them either; the driver owns orchestration files.
  - `[low]` `[patch]` Blind: Secure-only-on-https departs from ADR-0007 §3 without saying so — CONTEXT §10 row now states the departure until story 2.
  - `[low]` `[patch]` Edge: VerifyEmailPage token change not reposted — same fix as above (ref keyed on token).
  - `[low]` `[reject]` Edge: no retry button after a network error on verify — reloading works, and the fix adds UI.
  - `[medium]` `[patch]` Edge: oversized sign-in input hashed and audited — grouped with the first Blind finding; same fix.
  - `[low]` `[reject]` Edge: Rust to_lowercase vs Postgres lower() differ for non-ASCII in the audit subject — rare, cosmetic split of audit subjects; the fix adds complexity.
  - `[low]` `[patch]` Edge: lazy dummy hash timing leak — grouped with the Blind finding; same fix.
  - `[low]` `[reject]` Edge: resend leaves old tokens valid — same as the Blind finding; no harm.
  - `[low]` `[patch]` Edge: leftover unused token re-verifies and audits auth.email_verified again — consuming a token now marks the user's other unused verify_email tokens used; resend test asserts first token 400 and a single audit row.
  - `[medium]` `[patch]` Edge: sign-up ignores verification_email_sent — grouped with the Blind finding; same fix.
  - `[low]` `[reject]` Edge: stale `me` cache after a 401 on resend/sign-out — same as the Blind finding.
  - `[low]` `[reject]` Edge: a public_base_url with a path yields a link no SPA route matches — the hub serves the portal at root; sub-path deployment is not a supported setup.
  - `[low]` `[reject]` Edge: 405 on a wrong method has an empty body — the client never sends a wrong method, /api/health already behaves this way, and the fix adds router surface.
  - `[low]` `[reject]` Edge: two wipehub_session cookies with a stale first — needs two same-name cookies on different paths/domains, which this hub never sets.
  - `[low]` `[patch]` Edge: email limit counted in chars, not octets — now `email.len()`, with a unit test.
  - `[medium]` `[patch]` VerificationGap: token/session hash storage test can never fail — grouped with the Blind finding; same fix.
  - `[medium]` `[patch]` VerificationGap: `--mail-from` never asserted in the From header — the support Message has `from`; new test with a non-default mail_from asserts From and Subject.
  - `[medium]` `[defer]` VerificationGap: timing equalisation untested — needs a seam; deferred as filed.
  - `[low]` `[patch]` VerificationGap: 415 branch of JsonBody untested — new `non_json_body_is_415_with_a_json_error`.
  - `[medium]` `[defer]` VerificationGap: portal page behaviour untested — grouped with the Blind portal-tests finding; deferred.
  - `[low]` `[reject]` VerificationGap other: useEffect re-runs on every render because `mutation` is a dependency — the guard makes it harmless.
  - `[low]` `[reject]` VerificationGap other: verify page offers no sign-in link — "Go to the portal" leads to the sign-in links; cosmetic.
  - `[medium]` `[defer]` IntentAlignment: browser journey (pages, emailed link to route) untested — grouped with the portal-tests deferral.
  - `[low]` `[patch]` IntentAlignment: Secure on https vs "outside localhost" — grouped with the Blind ADR-departure finding; documented.
  - `[medium]` `[defer]` IntentAlignment: sign-up session not invalidated on sign-in — grouped with the Blind rotation finding; story 2.
  - `[false]` `[reject]` IntentAlignment: ROADMAP A1.1 struck including "tenant model" — organizations and roles now exist, so the strike is accurate.
  - `[false]` `[reject]` IntentAlignment: reading B expects purchase gating and lockout — no purchase route exists yet, and stories.yaml assigns rate limiting to story 2.

## Design Notes

- **Why sessions land here, not only in story 2:** "password sign-in" has no observable result without a signed-in state, so this story issues a plain server-side session per sign-up/sign-in. Story 2 owns the hardening (CSRF, rotation rules, rate limits, lockout, sign-out everywhere, Secure-outside-localhost policy) and may change cookie details.
- **Sign-up signs the user in** (unverified users can sign in, CAP-1), so the portal can show the verify notice immediately.
- **Mail after commit:** the account must not roll back because Mailpit is down; the resend route is the recovery path.
- **Plain SMTP only:** STARTTLS/auth for a production relay is a deployment decision (ROADMAP §0.5); lettre TLS features would also exceed MSRV.
- **`auth_tokens.purpose`** is a CHECK list so story 3 adds `magic_link`/`password_reset` with a new migration rather than a new table.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including `tests/auth.rs`.
- `cargo test --workspace` -- expected: all pass.
- `cargo clippy -p wipe-hub --all-targets -- -D warnings` -- expected: clean.
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: all pass.

## Auto Run Result

Status: done

**Summary:** The hub now has accounts:
- organizations and users (`owner`/`member`/`vendor_admin`);
- sign-up with an emailed, single-use 24 h verification link;
- Argon2id password sign-in;
- server-side sessions in a `wipehub_session` cookie;
- sign-out, `me` and resend under `/api/auth/*`;
- every auth event audited in the same transaction as its effect.

The portal gained sign-up, sign-in and verify-email pages and an account panel on home.

**Files changed:**
- `crates/wipe-hub/migrations/0003_accounts.sql`: organizations, users, auth_tokens, sessions (token hashes only).
- `crates/wipe-hub/src/accounts.rs`: data layer, with validation, Argon2id, tokens, sessions and queries scoped by organization.
- `crates/wipe-hub/src/auth.rs`: `/api/auth/*` handlers, the `CurrentUser` extractor, cookies and audit writes.
- `crates/wipe-hub/src/mail.rs`: lettre plain-SMTP mailer for the verification email.
- `crates/wipe-hub/src/config.rs`, `src/lib.rs`: the `--mail-from`/`WIPE_HUB_MAIL_FROM` flag, the mailer in `AppState`, the auth routes and dummy-hash warm-up.
- `crates/wipe-hub/tests/auth.rs`: 19 HTTP integration tests covering every matrix row and AC.
- `crates/wipe-hub/tests/{support/mod.rs,health.rs,portal.rs}`: Mailpit `from` field and updated `Args` literals.
- `Cargo.toml`, `crates/wipe-hub/Cargo.toml`, `Cargo.lock`: sqlx `uuid` feature, argon2 0.5, lettre =0.11.19, and transitive dependencies pinned to versions that build on Rust 1.80.
- `apps/portal/src/api/auth.ts` (+ test): typed auth client.
- `apps/portal/src/pages/*`, `main.tsx`: the new pages and routes.
- `CONTEXT.md`, `README.md`, `docs/ROADMAP.md`: §10 row, §14 glossary (Organization, Hub user role, Portal session), crate/app rows, A1.1 struck.

**Review:** 38 findings.
- 11 patch entries applied, 9 of them distinct fixes:
  - oversized sign-in input;
  - email octet limit;
  - dummy-hash warm-up;
  - sibling verify-token invalidation;
  - hash-storage test;
  - From/Subject test;
  - 415 test;
  - VerifyEmailPage ref;
  - home notice wording and CONTEXT limitations.
- 4 deferred (frontmatter `deferred`): session rotation (story 2), portal page tests, AGENTS.md drift, timing-equalisation test.
- Rejected: 5 false and 12 low, each with its reason in the triage log.

**Follow-up review recommended: true.** Patched entries by verdict: medium 4 (oversized sign-in, home notice, hash-storage test, From-header test), low 7. The unverified risk is the oversized-input sign-in branch and the sibling-token invalidation. Both are new security-relevant code paths written in the patch pass and not seen by any reviewer.

**Verification:**
- `cargo test --workspace`: exit 0, 239 tests passed.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean.
- The spec's `cargo clippy -p wipe-hub --all-targets -- -D warnings` fails only on an existing `clippy::large_enum_variant` at `crates/wipe-common/src/job.rs:201`. This story does not touch `wipe-common`.
- `rustfmt --check` on touched files: clean.
- `pnpm --filter portal typecheck/test/build`: pass, 25 tests.

**Residual risks:**
- No CSRF, rate limiting or lockout until story 2.
- Sign-up 409 reveals registered emails (documented).
- Plain SMTP only.
- Expired sessions and tokens are never purged.
- `Cargo.lock` pins older transitive dependencies (`email-encoding`, `psm`, `ar_archive_writer`) to stay within MSRV 1.80.
