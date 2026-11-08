---
title: 'Magic link and password reset'
type: 'feature'
created: '2026-11-08'
baseline_revision: '0c02d7436c618a62b5a629bd963954f0558f966c'
status: 'done'
review_loop_iteration: 1
followup_review_recommended: false
context: ['{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md']
warnings: ['oversized']
deferred:
  - summary: >-
      The new portal pages have no component tests, so nothing automated proves /magic-link signs in only on a button press.
    evidence: |-
      Only apps/portal/src/api/*.test.ts exist. Adding useEffect(() => mutation.mutate(token), []) to MagicLinkPage passes typecheck, vitest and the hub tests. The portal has no component-test setup (story 2 deferral); this is the first case to cover when one lands.
    location: >-
      apps/portal/src/pages/MagicLinkPage.tsx
    severity: low
  - summary: >-
      Story 1's verification link can verify an account someone else pre-registered with the victim's address, leaving the attacker's password and session working.
    evidence: |-
      An attacker signs up with victim@corp and their own password; the victim receives "Verify your Wipestation email address" and clicks it; POST /api/auth/verify-email marks the account verified without ending the attacker's session or password. Pre-existing since story 1. Story 3 closes the same hole for magic links only, because on the verify path the clicker cannot be told apart from the legitimate new user who just set the password.
    location: >-
      crates/wipe-hub/src/auth.rs verify_email
    severity: medium
  - summary: >-
      Story 1's verification link does not audit failed uses or clear the email's sign-in lock, unlike the story-3 magic and reset links.
    evidence: |-
      POST /api/auth/verify-email with a used or expired token answers 400 with no audit row, and a successful verification leaves the email: throttle key alone, while magic-link sign-in and reset completion now do both. Pre-existing story-1 behaviour; aligning it would change that route.
    location: >-
      crates/wipe-hub/src/auth.rs verify_email
    severity: low
---

<intent-contract>

## Intent

**Problem:** Portal users can only sign in with a password. If they forget it, they have no way back in, and an attacker who locks their email out of password sign-in (story 2) locks them out completely (SPEC CAP-2: magic link and reset). Every route that sends email can also be called without limit, so anyone can flood a mailbox or the hub's SMTP relay (story 2 deferral).

**Approach:** Add emailed, single-use, expiring **magic links** (sign in) and **password reset links** to `wipe-hub`, using two new `auth_tokens` purposes. Add an **email-send throttle**, built on story 2's `throttle` module, to all four routes that send email: sign-up, resend verification, magic link and reset. Add portal pages to request and follow each link.

## Boundaries & Constraints

**Always:**
- **Tokens** reuse story 1's scheme: 32 OS-RNG bytes, base64url in the link, only the SHA-256 stored in `auth_tokens`.
  - A new migration `0005_auth_token_purposes.sql` widens the purpose CHECK to `verify_email`, `magic_link` and `password_reset`.
  - Magic links expire after **15 min** and reset links after **1 h**. Both are single-use.
  - Consuming a token is one atomic `UPDATE … WHERE used_at IS NULL AND expires_at > now() RETURNING user_id`, so two concurrent uses yield exactly one success.
  - Creating any token deletes that user's already-expired tokens in the same statement.
- **Links:** `{public_base_url}/magic-link?token=…` and `{public_base_url}/reset-password?token=…`, keeping any base path, like `verification_link`.
- **Requests never reveal whether an account exists.**
  - `POST /api/auth/magic-link` and `POST /api/auth/password-reset` take `{email}` and answer **202 `{}`** for known and unknown emails alike, including when SMTP is down.
  - The handler does only validation and the throttle charge, then answers. The account lookup, the token, the `*_requested` audit row and the send all run in a spawned task after the response is decided. So the response's timing depends on neither the account nor SMTP. A failure in that task is logged at `warn` (never the token) and never returned.
  - An unknown email creates no token, sends no mail and writes no audit row.
  - A syntactically invalid email is 400 with `validate_email`'s message, before any throttle charge.
- **Magic-link sign-in:** `POST /api/auth/magic-link/sign-in {token}`.
  - It consumes the token and retires that user's other outstanding `magic_link` tokens.
  - It marks the email verified if it was not already, and retires outstanding `verify_email` tokens.
  - It clears the sign-in throttle's `email:<address>` key, auditing any lock summary as story 2 does.
  - It rotates the session exactly like password sign-in: the incoming session is deleted in the same transaction.
  - **Pre-registered accounts.** If this sign-in is what verifies the email (the account was unverified, so whoever set its password never proved they own the mailbox), the same transaction also:
    - deletes every other session of the user;
    - replaces `password_hash` with the unusable marker `!unusable`, which never verifies, so password sign-in fails until the user sets a password through reset.

    The audit detail gains `{sessions_ended: n, password_cleared: true|false}`.
  - It answers 200 with the `Account` and the session cookie. A bad, used or expired token gets 400 `this sign-in link is invalid or has expired`.
  - Neither the sign-in throttle nor its lock blocks it: a locked-out user can get back in through their mailbox.
- **Password reset:** `POST /api/auth/password-reset/complete {token, password}`. The steps run in this order:
  1. The password is checked with `validate_password` (400). An invalid password never burns the token.
  2. The token is peeked (unused, unexpired, purpose `password_reset`); otherwise 400 `this password reset link is invalid or has expired`. Argon2 runs only after a valid peek.
  3. In one transaction:
     - consume the token (losing the race is the same 400);
     - delete the request's incoming session, whoever it belongs to, since its cookie is about to be cleared;
     - set the new hash;
     - retire the user's outstanding `password_reset` and `magic_link` tokens;
     - **delete every session of the user**;
     - mark the email verified;
     - clear the `email:` sign-in key;
     - audit.
  4. Answer 200 `{"password_reset": true}` with the cleared session cookie. The user is **not** signed in and signs in with the new password (OWASP Forgot Password Cheat Sheet).
- **Email-send throttle**, in `auth_throttle` with keys distinct from sign-in's:
  - `mail:to:<address>`: the address trimmed, lowercased by Postgres `lower()`, at most 254 characters.
  - `mail:<ip key>`, e.g. `mail:ip:192.0.2.1` or `mail:ip:2001:db8:1:2::/64`.
  - **Which routes:** sign-up, resend verification, magic link and password reset each charge both keys before any lookup, Argon2 or send. Sign-up charges the address being signed up, resend charges the account's address, and magic link and reset charge the requested address, known or not.
  - **Counting:** every charge is confirmed (`confirm_failure`) in the same transaction, so it counts whether or not mail is actually sent. Nothing is refunded.
  - **Limits:** 5 per address and 20 per IP per 1-hour window. The send that reaches a limit locks that key for 1 hour.
  - **Order:** the IP key is charged first. The IP charge is refunded if the address key refuses.
  - **Refusal:** 429 `too many emails requested; try again later` with `Retry-After`, written before any lookup, Argon2 or send.
  - **Audit:** a lock is audited once as `auth.mail_locked` (`{until, peer}`). Refusals are counted and summarised as `auth.mail_lock_summary`, never one row each.
  - A refused send writes nothing but its throttle counter. The stale-row purge runs only on a charged send.
  - Story 2's purge summaries are audited under the action that matches their key: keys starting `mail:` get `auth.mail_lock_summary`, all others `auth.sign_in_lock_summary`.
- **Audit**, each row in the same transaction as its effect, with `peer` in the detail whenever there is one:
  - `auth.magic_link_requested` and `auth.password_reset_requested`: actor `anonymous`, subject `user:<id>`.
  - `auth.magic_link_sign_in`: actor and subject `user:<id>`, detail `{rotated, email_verified}`, where `email_verified` is true only when this sign-in verified the email.
  - `auth.password_reset`: actor and subject `user:<id>`, detail `{sessions: n, email_verified}`.
  - `auth.magic_link_failed` and `auth.password_reset_failed`: actor `anonymous`, subject `user:<id>`, detail `{reason: "used"|"expired"}`. Written only when the posted token matches a stored row of that purpose. An unknown or malformed token writes nothing, so the rows are bounded by the number of links issued, which the email-send throttle limits.
- **Portal pages:**
  - `/sign-in` links to `/sign-in-link` ("Email me a sign-in link") and `/forgot-password`.
  - Both request pages post an email and then always show the same "if an account exists, we've emailed a link" confirmation.
  - `/magic-link?token=` signs in **only when the user clicks a button**, never automatically on load, so a link scanner that runs JavaScript cannot take the session. On success it caches `["me"]` and goes home.
  - `/reset-password?token=` has a new-password form. On success it clears `["me"]` and links to `/sign-in`.
  - On a 429 from an email-sending request, show "Too many emails requested. Try again in N minutes."
- Error bodies stay `{"error": "<message>"}`. The CSRF middleware already covers the new POST routes and must be listed in the CSRF test.

**Never:**
- No `sqlx::query!`; do not edit migrations 0001–0004; do not trust proxy headers.
- Never log or return a raw token, and never put one in an audit row.
- Never answer differently for known vs unknown emails on the two request routes (status, body or `Set-Cookie`).
- Do not consume a magic link on GET or on page load; do not sign the user in after a reset.
- Do not port throttle rules into the portal beyond displaying `Retry-After`.
- Not in this story: profile/password-change pages (story 4), admin area (story 5), MFA, an expired-token sweeper beyond the per-user purge, portal component tests (no setup exists; deferred in story 2).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Magic link happy path | `POST /magic-link` known email; follow emailed link; `POST /magic-link/sign-in` | 202 `{}`; one Mailpit message with `/magic-link?token=`; 200 Account + session cookie; `me` 200; `auth.magic_link_requested`, `auth.magic_link_sign_in` | — |
| Unknown email | `POST /magic-link` or `/password-reset` unknown email | 202 `{}`, byte-identical body to the known case; no mail, token or audit row | — |
| Magic link reuse | same token posted twice | 1st 200, 2nd 400 `this sign-in link is invalid or has expired` | — |
| Magic link expiry | token with `expires_at` moved into the past | 400, no session | — |
| Concurrent use | one magic token posted 10× at once | exactly one 200 | — |
| Latest link wins only once | two magic links requested, first used | second is retired → 400 | — |
| Locked-out user | email locked by 5 wrong passwords; magic link sign-in | 200; `email:` key gone; `auth.sign_in_lock_summary` if refusals were counted | — |
| Magic link verifies | unverified user signs in by magic link | `email_verified` true; earlier verification link → 400; detail `email_verified:true` | already verified → `false` |
| Rotation | magic sign-in carrying live session A | A 401 on `me`; detail `rotated:true` | — |
| Pre-registered account | attacker signs up with victim's address (keeps session S, knows password P); victim signs in by magic link | 200; S 401 on `me`; P 401 on sign-in; detail `password_cleared:true`, `sessions_ended:1`; a reset then sets a working password | already-verified account → no sessions ended, password kept, `password_cleared:false` |
| Failed token use | used or expired magic/reset token posted | 400; one `auth.magic_link_failed` / `auth.password_reset_failed` `{reason}` row | garbage token → 400, no audit row |
| Reset happy path | request reset; complete with new 12+ char password | 200 `{"password_reset":true}`, cookie cleared; every prior session 401; old password 401; new password 200; `auth.password_reset` `{sessions:n}` | — |
| Reset clears the lock | email locked by 5 wrong passwords; reset completed | new password signs in at once (200); `email:` key gone | — |
| Reset elsewhere | reset completed in a browser holding user B's session | B's session row deleted (B 401 on `me`) | — |
| Concurrent reset | one reset token completed 5× at once | exactly one 200, the rest 400 | — |
| Reset weak password | complete with 11-char password | 400 length message; token still usable afterwards | — |
| Reset bad token | garbage/used/expired token, valid password | 400 `this password reset link is invalid or has expired`; hash unchanged | — |
| Reset retires links | outstanding magic link and second reset link, then reset completes | both 400 afterwards | — |
| Mail address limit | 5 magic-link requests for one address within the hour | 1st–5th 202; 6th 429 `too many emails requested; try again later` with `Retry-After` 3590–3600; `auth.mail_locked` once; only 5 messages | lock moved into the past → 202 again |
| Shared address key | 2 sign-up attempts + 1 resend + 1 reset + 1 magic link for one address | 6th email-sending request for it is 429 | — |
| Mail IP limit | 20 requests from one IP for distinct addresses | 21st 429 | — |
| Resend when verified | verified user posts resend | 409; no `mail:` charge | — |
| Refused send writes nothing | locked address; a stale row of another key exists | 429; that stale row and the audit log are untouched | — |
| Sign-up throttled | sign-up refused by the mail throttle | 429; no user, organization, session or token row; no Argon2 | invalid sign-up body still 400 first |
| Hammering a mail lock | 50 requests against a locked address | all 429, no audit rows added; summary `auth.mail_lock_summary` `{refused:50}` when cleared/restarted/purged | — |
| CSRF | each new POST route without token | 403 `missing or invalid CSRF token` | — |

## Autonomous decisions

Recorded under the owner's "decide by default" rule (2026-11-08). Each is the safe default or recognised practice; none is product, pricing, legal or ADR-contradicting.

1. **Account enumeration on the request routes.** *About:* a visitor asks the portal to email a sign-in or reset link. *Problem:* answering "no such account" tells an attacker which emails are customers. *Options:* (a) the same 202 for every address, with mail sent in the background; (b) a 404 for unknown addresses. *Choice:* (a), per OWASP's Forgot Password and Authentication cheat sheets. Review (2026-11-08) found that writing the token and audit row before answering still let timing reveal a known account, so all per-account work moved into the background task. Sign-up's 409 still reveals existence; that is already listed in CONTEXT.
2. **Reset link lifetime: 1 hour.** The SPEC fixes only the magic link (15 min). An hour allows for slow mail delivery while keeping the window short; OWASP asks for short-lived, single-use links.
3. **No automatic sign-in after a reset, and every session ends.** *Problem:* a reset is usually done because the password may be known to someone else, so their sessions must end. *Choice:* end every session and require a fresh sign-in (OWASP Forgot Password Cheat Sheet). The alternative, signing the user in, keeps one fewer step but means one more session-issuing path to secure.
4. **A magic link or reset proves mailbox control,** so it marks the email verified and clears that email's password-sign-in lock. *Problem:* someone who knows a customer's email can lock their password sign-in for 15 minutes at a time (story 2). The emailed link is the way back in. The lock still protects the password itself.
5. **Magic link needs a click on the landing page.** Some mail security scanners open links in a real browser. An automatic sign-in on page load would hand the scanner a session. One click is the safe default.
6. **Email-send limits:** 5 emails per address and 20 per IP per hour, with a 1-hour lock, counting every request whether or not an account exists, so the limit itself reveals nothing. *Trade-off:* someone who knows an address can use up its 5 emails and block that person's reset for an hour. This is listed as a CONTEXT limitation, like the story-2 sign-in lock.
7. **Wording.** Email and page text follow the style of the story-1 verification email: plain, functional and with no product claims. It is not marketing copy and can be reworded later without any other change.
8. **Pre-registered accounts (review, 2026-11-08).** *About:* someone signs up using another person's email address and their own password. Later the real owner gets "an account with this email already exists", uses "Email me a sign-in link" and gets in. *Problem:* the impostor's password and session still work, on an account that is now verified and can purchase. *Options:*
   - (a) when a magic link is what verifies the email, end every other session and make the password unusable;
   - (b) end sessions only, which leaves the impostor able to sign in again with the password;
   - (c) refuse magic links for unverified accounts, which leaves the owner with only a reset.

   *Choice:* (a). It is the mitigation the account pre-hijacking research (Sudhodanan & Paverd, 2022) recommends. The cost: a legitimate user who signed up, never verified, and later signs in by magic link must reset their password before password sign-in works again. This is listed in CONTEXT. The same hole on story 1's verification link is pre-existing and deferred.
9. **Failed link uses are audited when the token is real.** The SPEC requires auth failures to be audited. Auditing every garbage token would let anyone grow the append-only log without bound, so only tokens that match a stored row (used or expired) are audited. Those rows are bounded by the email-send throttle.

</intent-contract>

## Code Map

- `crates/wipe-hub/migrations/0005_auth_token_purposes.sql` (new):
  - `ALTER TABLE auth_tokens DROP CONSTRAINT auth_tokens_purpose_check, ADD CONSTRAINT auth_tokens_purpose_check CHECK (purpose IN ('verify_email','magic_link','password_reset'))`.
  - Postgres auto-named the 0003 constraint; check the name with `\d auth_tokens`.
  - Header comment in the style of 0003 and 0004.
- `crates/wipe-hub/src/accounts.rs`:
  - `create_verification_token` (~l.380) becomes a wrapper over a new `create_token(executor, user_id, purpose, ttl)`. That function inserts the token and, in a CTE, deletes the user's expired tokens.
  - `consume_verification_token` (~l.401) generalises to `consume_token(conn, raw, purpose)`.
  - Add the following, each taking any executor:
    - `peek_token`;
    - `retire_tokens(user_id, &[purpose])`;
    - `mark_email_verified(conn, user_id) -> bool` (changed?), which also retires `verify_email` tokens; reuse it in the verify path;
    - `set_password_hash`;
    - `find_user_by_email -> Option<(user_id, organization_id, stored email)>`, using `lower(email) = lower($1)` like `find_sign_in_candidate` (~l.324).
  - `delete_user_sessions` (~l.491) and `hash_token`/`Token` are reused.
  - Add constants `PURPOSE_MAGIC_LINK`, `PURPOSE_PASSWORD_RESET`, `MAGIC_LINK_TTL` (15 min) and `PASSWORD_RESET_TTL` (1 h).
- `crates/wipe-hub/src/auth.rs`:
  - `routes()` (~l.53): add the four routes.
  - `verification_link` (~l.241): generalise to `portal_link(base, path, token)` and keep the old name as a wrapper.
  - `email_key` (~l.355): parametrise the prefix (`email:` / `mail:to:`).
  - `ip_key`, `with_peer`, `locked_response` and `audit_lock_summary`:
    - `audit_lock_summary` picks its action from the key prefix;
    - `locked_response` takes the message.
  - `charge_sign_in` (~l.415) is the pattern for a new `charge_mail(state, address_key, ip_key)`. It charges the IP key then the address key, refunds the IP if the address refuses, and if charged, `confirm_failure` both with `MAIL_LOCKOUT`. It audits `auth.mail_locked` on engagement, audits summaries, runs `purge_stale`, and commits.
  - `sign_up` (~l.274): call `charge_mail` after `validate_sign_up` and before `hash_password`.
  - `resend_verification` (~l.786): charge after the already-verified check, before creating the token.
  - Both need `Option<ConnectInfo<SocketAddr>>`, as `sign_in` takes it.
  - `finish_sign_in`'s success arm (~l.580) shows the rotation, clear and audit sequence that magic-link sign-in mirrors.
- `crates/wipe-hub/src/throttle.rs`:
  - Add `MAIL_ADDRESS_LIMIT=5`, `MAIL_IP_LIMIT=20`, `MAIL_WINDOW=1h` and `MAIL_LOCKOUT=1h`, plus a `MAIL_KEY_PREFIX="mail:"` constant.
  - Extend the `sign_in_constants` test.
  - Update the module docs: story 3 uses it.
- `crates/wipe-hub/src/mail.rs`: add `send_magic_link` and `send_password_reset`, with subject constants and plain-text bodies stating that the link works once, how long it lasts, and "ignore if you didn't ask".
- `crates/wipe-hub/tests/auth.rs`:
  - Reuse `Hub::post`, `count`, `audited`, `Mailpit::wait_for_message_to` and the link-extraction pattern at ~l.232.
  - Simulate time by moving `auth_tokens.expires_at` and `auth_throttle.locked_until` into the past through `hub.db.pool`, as story 2 does.
  - The CSRF route list (~l.946) gains the four new routes.
  - Unknown-email "no mail": request for the unknown address, then for a known one; wait for the known one, then assert `messages_to(unknown)` is empty.
  - Existing tests sign up at most two accounts per hub, so the 20-per-IP mail limit does not disturb them.
- `apps/portal/src/api/auth.ts`:
  - Add `requestMagicLink`, `signInWithMagicLink`, `requestPasswordReset` and `resetPassword`.
  - Add `emailRequestErrorMessage`, mirroring `signInErrorMessage`.
  - Update the mirror comment.
- `apps/portal/src/api/auth.test.ts`: the router-style fake from story 2 serves `/api/auth/csrf`.
- `apps/portal/src/main.tsx`: add routes `/sign-in-link`, `/magic-link`, `/forgot-password` and `/reset-password`. Use `validateSearch` like `verifyEmailRoute`.
- `apps/portal/src/pages/`:
  - new `SignInLinkPage.tsx`, `MagicLinkPage.tsx`, `ForgotPasswordPage.tsx` and `ResetPasswordPage.tsx`;
  - `SignInPage.tsx` gains the two links;
  - `SignUpPage.tsx` uses `emailRequestErrorMessage`;
  - reuse `Field`, `FormError` and `SubmitButton` from `AuthForm.tsx`.
- `CONTEXT.md`:
  - §10, l.693: drop "No magic link or password reset (story 3)" and "expired email tokens are never purged". Replace the latter with "expired tokens are purged per user when that user is next issued one".
  - §10, l.694: drop "sign-up and resend-verification are not rate-limited until story 3".
  - Add a story-3 row with its limitations: an address's emails can be used up by anyone who knows it; there is no global sweeper of expired tokens.
  - §14 (~l.1054): add **Magic link**, **Password reset link** and **Email-send throttle**.
- `CONTEXT.md` story-3 limitations must also say:
  - someone who knows an address can lock that person out of every route at once: 5 wrong passwords lock password sign-in for 15 min, and 5 link requests use up the address's emails for an hour, blocking magic link, reset, sign-up and resend;
  - when SMTP is down, the two request routes still answer 202 and the user is never told the link did not go out;
  - a user who never verified and then signs in by magic link loses their password and must reset it (decision 8).
- `README.md` l.48 (`wipe-hub`) and l.50 (`apps/portal`): mention magic link, reset and the email-send limits.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0005_auth_token_purposes.sql` -- widen the purpose CHECK -- new purposes without a new table, as 0003 planned.
- `crates/wipe-hub/src/accounts.rs` -- the generalised token functions, `mark_email_verified`, `set_password_hash`, `find_user_by_email` and the constants, with unit tests where pure -- the data layer stays the only SQL for accounts.
- `crates/wipe-hub/src/throttle.rs` -- the mail constants, prefix and docs.
- `crates/wipe-hub/src/mail.rs` -- the two new messages.
- `crates/wipe-hub/src/auth.rs` -- the four handlers, `charge_mail`, mail throttling on sign-up and resend, the action chosen by key prefix, `portal_link`, and unit tests for link building and the prefix-to-action mapping.
- `crates/wipe-hub/tests/auth.rs` -- one HTTP test per I/O-matrix row. These include, beyond attempt 1's tests: a reset clearing a sign-in lock, a verified user's resend leaving no `mail:` charge, concurrent reset completion, the pre-registered account, failed-token audit rows, reset in another user's browser, and a refused send leaving a stale row untouched; plus `migrations` coverage that 0005 applies (the existing `tests/migrations.rs` pattern).
- `apps/portal/src/api/auth.ts`, `auth.test.ts` -- the client functions, each POST carrying `X-CSRF-Token`, and the 429 message helper.
- `apps/portal/src/main.tsx`, `src/pages/*` -- the four pages and the sign-in links.
- `CONTEXT.md`, `README.md` -- shipped behaviour, glossary and limitations, in this commit.

**Acceptance Criteria:**
- Given a running hub with Mailpit, when a customer requests a magic link and posts the emailed token, then they are signed in (the `me` endpoint returns 200 with the new cookie), and posting the same token again is 400.
- Given a customer with two live sessions, when they complete a password reset, then both sessions are 401 on `me`, the old password is 401, and the new password signs in.
- Given known and unknown emails, when each is posted to `/api/auth/magic-link` and `/api/auth/password-reset`, then the status, body and headers (other than `Date`) are identical.
- Given 5 email-sending requests for one address in an hour, when a 6th arrives on any of the four email-sending routes, then it is 429 with `Retry-After`, sends no mail and writes no row other than the throttle counter.
- Given the workspace, when `cargo test --workspace`, clippy and the portal `typecheck`/`test`/`build` run, then all pass.

## Spec Change Log

- 2026-11-08 (review pass 1, loop 1) -- bad_spec loopback.
  - **Triggered by:**
    - a magic link that verifies a pre-registered account left the impostor's password and session working (high);
    - the request routes wrote the token and audit row before answering, so timing revealed a known account (medium).
  - **Amended in `<intent-contract>`** (under the decide-by-default rule):
    - request routes do all per-account work in the spawned task;
    - a magic link that verifies the email ends other sessions and sets `password_hash` to `!unusable`;
    - reset completion also deletes the incoming session;
    - failed uses of real tokens are audited;
    - a refused send writes only its counter (purge only on charged sends);
    - new matrix rows, and decisions 8 and 9.
  - **Amended outside it:** Code Map (CONTEXT limitations) and Tasks (extra tests).
  - **Known-bad states avoided:** a pre-hijacked account staying shared with its creator after the owner gets in; a timing oracle for which emails are customers.
  - **KEEP:** attempt 1 is saved at [portal-accounts-story-3-attempt-1.patch](../../../_bmad-output/implementation-artifacts/portal-accounts-story-3-attempt-1.patch) (applies cleanly to `0c02d7436c618a62b5a629bd963954f0558f966c`). Start by applying it with `git apply`, then change only what the amendments require.
    - Keep everything else in it: migration 0005; the `accounts` token functions; `LinkKind`, `portal_link`, `charge_mail` and `lock_summary_action`; the mail bodies; every portal page and client function; the CONTEXT/README edits; all 21 hub tests, including the CSRF route list.
    - It passed `cargo test --workspace` (297), clippy, rustfmt and the portal checks (59 tests).
- 2026-11-08 (implementation, loop 1): there is one narrow exception to "a refused send writes nothing but its throttle counter".
  - **When:** the IP key's charge restarts a window that had refusals, and the address key then refuses.
  - **What happens:** the restart's `auth.mail_lock_summary` is still written.
  - **Why kept:** dropping it would lose that window's refusal count from the append-only log for good. The stale-row purge, which writes rows for *other* keys, does run only on charged sends.

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 26 findings — high 1, medium 2, low 18, false 5, maybe-false 0
- attempted change saved at [portal-accounts-story-3-attempt-1.patch](../../../_bmad-output/implementation-artifacts/portal-accounts-story-3-attempt-1.patch); code reverted for the bad_spec loopback
- findings:
  - `[high]` `[bad_spec]` (blind) A magic link that verifies an account someone else pre-registered with the victim's address leaves the impostor's password and 14-day session working — Verified in `magic_link_sign_in`: it calls `mark_email_verified` and creates a session, and touches neither the user's other sessions nor `password_hash`. Amended: decision 8, and a magic link that verifies the email ends other sessions and sets `!unusable`.
  - `[low]` `[bad_spec]` (blind) Anyone who knows an address can still lock its owner out completely: 5 wrong passwords plus 5 link requests; sign-up 409s also count — Real; the CONTEXT row understated it. Amended: Code Map CONTEXT limitations spell out the combined lockout.
  - `[medium]` `[bad_spec]` (blind) Docs claim the request routes' timing does not depend on the account, but a known email does a token INSERT and an audit INSERT before the 202 — Verified in `request_link`; this breaks the intent rule "requests never reveal whether an account exists". Amended: all per-account work moves into the spawned task.
  - `[low]` `[reject]` (blind) The link lifetimes are written as literal strings in the mail bodies and portal pages, apart from the constants — Only drifts if a lifetime constant changes; the fix needs formatting or helper plumbing across Rust and TS for no current defect.
  - `[low]` `[bad_spec]` (blind) Concurrent reset completion is untested, and each racer pays for an Argon2 hash — The atomic consume makes it correct; Argon2 work is limited to whoever holds a valid token. Amended: a concurrent-reset matrix row and test.
  - `[low]` `[bad_spec]` (blind) Failed uses of magic or reset links are not audited — Real against SPEC "auth events (… failure …) are audited". Amended: decision 9, auditing failures only for tokens that match a stored row, which keeps the audit volume bounded.
  - `[low]` `[bad_spec]` (blind) Completing a reset in a browser signed in as another user clears that user's cookie but leaves their session row live — Verified: `complete_password_reset` deletes only the reset user's sessions. Amended: the incoming session is deleted in the same transaction.
  - `[low]` `[reject]` (blind) Tokens stay in the URL, the browser history and `Referer`; no `Referrer-Policy` — The portal loads no cross-origin resources, so `Referer` never leaves the origin; the history exposure needs local browser access to a single-use, short-lived token; a header layer is new surface.
  - `[low]` `[reject]` (blind) A spawned send is lost if the hub stops between commit and send — Rare (needs a restart within milliseconds), and the user simply asks again; a task tracker with graceful drain is new machinery.
  - `[low]` `[reject]` (blind) A magic-link sign-in leaves outstanding reset links usable — Only the mailbox owner holds those links, so retiring them protects nothing.
  - `[false]` `[reject]` (blind) The Code Map and Design Notes no longer match the code — The fix would edit this build's spec, and the code is being re-derived anyway.
  - `[low]` `[bad_spec]` (blind) CONTEXT and the glossary omit the silent 202 when SMTP is down, the lock length and the audit actions — Real doc gaps. Amended: Code Map CONTEXT limitations. The ConnectInfo-absent case is false: `serve_on` always supplies it.
  - `[low]` `[bad_spec]` (edge) Reset in another user's browser leaves their session live — Duplicate of the blind row (E1); same amendment.
  - `[medium]` `[bad_spec]` (edge) Response latency reveals whether an account exists — Duplicate of the blind timing row (E2); same amendment.
  - `[low]` `[reject]` (edge) A send is lost on shutdown — Duplicate of the blind row (E3); same reason.
  - `[low]` `[bad_spec]` (edge) A refused send can still write `*_lock_summary` audit rows for other keys through `purge_stale` — Verified in `charge_mail`: the purge runs on both paths. Amended: purge only on a charged send, plus a matrix row.
  - `[low]` `[bad_spec]` (edge) The targeted user can still be locked out completely — Duplicate of the blind lockout row (E5); same amendment.
  - `[low]` `[bad_spec]` (verification-gap) A reset clearing the email's sign-in lock is untested — Pre-verified. Amended: a matrix row and test.
  - `[low]` `[bad_spec]` (verification-gap) Resend charging only after the already-verified check is untested — Pre-verified. Amended: a matrix row and test.
  - `[low]` `[defer]` (verification-gap) No component test proves `/magic-link` signs in only on a button press — Pre-verified; the portal has no component-test setup (story 2 deferral). Added to `deferred`.
  - `[false]` `[reject]` (intent) The send throttle's counting departs from story 2's refund-on-success rule — "With story 2's throttle" is met by reusing its module; a send has no success or failure to refund, so confirming each charge is the only reading under which it limits sends.
  - `[low]` `[reject]` (intent) A 409 sign-up counts toward the address limit although no email went out — Deliberate: the charge comes before Argon2 so floods stay bounded; the 409 already reveals the address, so charging it costs nothing.
  - `[false]` `[reject]` (intent) The token routes (magic-link sign-in, reset completion) are not rate-limited — A 256-bit token cannot be guessed, and Argon2 runs only after a valid peek; the reset request route is limited, as the SPEC asks.
  - `[low]` `[defer]` (intent) Page-level promises (no sign-in on load, identical confirmation, 429 message on forms) are untested — Duplicate of the verification-gap component-test row (I4).
  - `[false]` `[reject]` (intent) Single-use and expiry live at the API — The auditor reports them covered by HTTP tests; there is no defect.
  - `[false]` `[reject]` (intent) Behaviour beyond the one-line intent comes from the story's own decisions — Descriptive only; each extra is a recorded autonomous decision with tests.

### 2026-11-08 — Review pass
- verdicts: 24 findings — high 0, medium 3, low 17, false 4, maybe-false 0
- findings:
  - `[low]` `[patch]` (blind) A password cleared to `!unusable` makes sign-in answer faster (the hash does not parse, so no Argon2 runs), showing that the account exists and was taken back — Verified in `verify_blocking`. Fixed: the parse-error branch now verifies against `dummy_hash()` before returning false, with a unit test.
  - `[low]` `[reject]` (blind) The user is never told that a magic link cleared their password — A rare path (signed up, never verified, then used a magic link), and CONTEXT lists it; telling the user needs a new API field plus customer-visible copy; the sign-in page already links to the password reset.
  - `[false]` `[reject]` (blind) The verify-link deferral rests on a false premise, since the incoming session is a signal — People often open verification mail on another device, so a missing session does not mark an impostor; clearing passwords on that signal would break ordinary sign-ups. Keeping the password when the magic link is clicked in the same user's session is a refinement for a rare case.
  - `[low]` `[patch]` (blind) Purging expired tokens removes the evidence for failure auditing, so CONTEXT's "a used or expired link is audited" overclaims — Real. Fixed: CONTEXT now says "once per token while the token is still stored (issuing a new link purges the user's expired ones)".
  - `[low]` `[defer]` (blind) The verify-email link neither audits failed uses nor clears the sign-in lock — Pre-existing story-1 route. Added to `deferred`.
  - `[false]` `[reject]` (blind) The spec's Design Notes, Code Map and audit bullet are stale against the code — The fix would edit this build's spec; the change logs record each deviation.
  - `[low]` `[reject]` (blind) The reset page asks for the new password only once — A typo is recovered by requesting another link from the same mailbox; a confirmation field adds UI and copy.
  - `[low]` `[reject]` (blind) The reset page reports a bad link only after submit — Showing it earlier needs a new read-only API route; the message on submit already offers a new link.
  - `[low]` `[patch]` (blind) No notice email when a password is reset or cleared — Real gap against OWASP guidance; a new email needs new copy. Fixed as documentation: added to the CONTEXT limitations.
  - `[low]` `[reject]` (blind) Customer-visible wording was decided without escalation — It is functional wording of the same kind as the story-1 and story-2 error strings and verification email, recorded as decision 7. It is flagged for owner review in the Auto Run Result rather than blocking the story.
  - `[medium]` `[patch]` (edge) Posting the same used or expired token repeatedly writes an unlimited number of failure rows — Verified: `token_failure` was a plain SELECT. Fixed: 0005 (applied nowhere persistent; the dev database is at version 2) adds `failure_audited_at`, and `token_failure` is one atomic UPDATE, so each token is audited at most once. Test `a_used_link_posted_again_and_again_is_audited_once`.
  - `[low]` `[patch]` (edge) The `!unusable` timing oracle — Same root cause as the blind row; fixed there.
  - `[low]` `[reject]` (edge) An IP-window restart summary is written on a refused send — The exception is deliberate and recorded in the Spec Change Log (dropping it would lose a refusal count from the append-only log); now pinned by a test.
  - `[low]` `[reject]` (edge) Mailbox control alone gives a `vendor_admin` a session — Email reset is the ADR-0007 §3 recovery for every user, and a reset gives the same power; no `vendor_admin` exists before story 5; MFA is a SPEC non-goal for now.
  - `[low]` `[patch]` (edge) The reset page shows "Email me a new reset link" on any error — Verified (`mutation.isError`). Fixed: shown only for the hub's invalid-link message.
  - `[low]` `[reject]` (edge) Client `minLength` counts UTF-16 units while the hub counts characters — Only emoji-heavy passwords reach it; the hub's 400 names the rule, and after the previous patch no misleading prompt appears.
  - `[medium]` `[patch]` (edge) The claim that failure rows are bounded by links issued did not hold — Same root cause as the repeated-token row; fixed there.
  - `[low]` `[reject]` (edge) A refused send can add an audit row — Same as the restart-summary row; deliberate and recorded.
  - `[low]` `[patch]` (verification-gap) The IP restart summary on a refused send is untested — Pre-verified. Added `an_ip_window_restart_is_summarised_even_when_the_address_refuses`.
  - `[low]` `[defer]` (intent) Portal page-level promises are untested — carried: the portal has no component-test setup; already in `deferred`.
  - `[false]` `[reject]` (intent) "Sign-in and reset rate-limited" is met only at the request routes — carried: a 256-bit token cannot be guessed, and Argon2 runs only after a valid peek.
  - `[false]` `[reject]` (intent) The mail throttle uses story 2's module but not its refund rule — carried: a send has no success or failure to refund.
  - `[low]` `[reject]` (intent) Requests are counted rather than emails actually sent — carried: a 409 sign-up is charged before Argon2 on purpose.
  - `[medium]` `[defer]` (intent) The pre-hijack fix covers magic links but not story 1's verification link — carried: pre-existing, already in `deferred`.

## Design Notes

- **Why mail charges are confirmed immediately:** `throttle::charge` alone refuses a full, unlocked key with `Retry-After: 1`, a rule meant for in-flight password checks. For email every charge is final, so confirming it in the same transaction makes the key lock at the limit, and `Retry-After` then reports the real time left. `charge`'s semantics stay unchanged for sign-in.
- **Peek, then hash, then consume** keeps Argon2 work limited to holders of a valid reset token. The token routes need no throttle of their own, because a 256-bit random token cannot be guessed.
- Background send for the request routes:

  ```rust
  tx.commit().await?;
  if let Some((to, token)) = to_send {
      let state = state.clone();
      tokio::spawn(async move { send_magic_link(&state, &to, &token).await });
  }
  Ok((StatusCode::ACCEPTED, Json(json!({}))).into_response())
  ```

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including the new tests.
- `cargo test --workspace` -- expected: all pass (never piped through grep).
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: all pass.

## Auto Run Result

Status: done

**Summary:** Story 3 adds emailed magic links (sign in) and password reset links to the hub and portal, and an email-send limit on every route that sends mail. Both kinds of link are single-use and expire: magic links after 15 min, reset links after 1 h. Choices made without the owner are recorded as decisions 1–9 in the intent contract.

- **Requests:** `POST /api/auth/magic-link` and `POST /api/auth/password-reset` answer 202 `{}` for every address. The account lookup, token, audit row and email all happen in a background task, so neither the answer nor its timing shows whether an account exists.
- **Magic-link sign-in:** `POST /api/auth/magic-link/sign-in` rotates the session, verifies the email and clears the email's sign-in lock. When it is what verifies the email, it also ends the account's other sessions and makes the password unusable. This takes back an account that someone else registered with the owner's address.
- **Password reset:** `POST /api/auth/password-reset/complete` checks the new password, then checks the token before running Argon2. It sets the password, ends every session (including the browser's own) and does not sign the user in.
- **Failed links:** a used or expired link is audited once per token.
- **Email-send limit:** 5 emails per address and 20 per IP per hour, with a 1-hour lock, on sign-up, resend verification, magic link and reset. A refused request is 429 with `Retry-After`.
- **Portal:** pages `/sign-in-link`, `/forgot-password`, `/magic-link` (signs in only on a button press) and `/reset-password`, linked from `/sign-in`, plus "try again in N minutes" messages.

**Files changed:**
- **Hub (`crates/wipe-hub`):**
  - `migrations/0005_auth_token_purposes.sql` — the new token types, and the `failure_audited_at` column.
  - `src/accounts.rs` — generalised token functions, `token_failure`, `mark_email_verified`, `set_password_hash`, the unusable-password marker, and equal Argon2 work for a hash that does not parse.
  - `src/auth.rs` — the four routes, `charge_mail`, the email limit on sign-up and resend, lock summaries named by key prefix, and `portal_link`.
  - `src/throttle.rs` — the email-limit constants and the `mail:` key prefix.
  - `src/mail.rs` — the sign-in and reset emails.
  - `tests/auth.rs` — 31 new HTTP tests (CSRF route list included).
  - `tests/migrations.rs` — checks that 0005 accepts the new token types.
- **Portal (`apps/portal/src`):**
  - `api/auth.ts`, `api/auth.test.ts` — the client functions and the 429 message helper.
  - `main.tsx` — the four new routes.
  - `pages/*` — the four new pages, `RequestLinkForm`, links on the sign-in page, and the 429 message on the sign-up and home forms.
- **Docs:** `CONTEXT.md` (§10 rows and §14 glossary) and `README.md` (the hub and portal rows).
- **Review artifact:** [portal-accounts-story-3-attempt-1.patch](../../../_bmad-output/implementation-artifacts/portal-accounts-story-3-attempt-1.patch).

**Review findings:**
- **Pass 1 (26 findings):** one spec repair loop.
  - A high finding: a magic link left an account someone had pre-registered with the owner's address still in the impostor's hands. Fixed by decision 8.
  - A medium finding: response timing showed whether an account exists. Fixed by moving all per-account work into the background task.
  - Low findings from the same pass were folded into the spec amendment:
    - audit failed link uses (decision 9);
    - a reset ends the browser's own session too;
    - a refused send skips the stale-row clean-up;
    - fuller CONTEXT limitations;
    - extra tests.
  - Code was reverted and rebuilt from the amended spec.
- **Pass 2 (24 findings):** 8 patch rows covering 6 fixes.
  - **Medium:** each token's failed use is now audited only once.
  - **Low:**
    - a cleared password costs the same Argon2 work at sign-in;
    - the reset page offers a new link only for a bad link;
    - a test for the IP-window summary on a refused send;
    - two CONTEXT wording and limitation fixes.
- **Deferred (3):**
  - portal page component tests;
  - story 1's verification link can verify a pre-registered account (medium, pre-existing);
  - the verify-email link neither audits failed uses nor clears the sign-in lock.
- **Rejected:** every rejection and its reason is in the two triage-log entries above. In short:
  - wording drift in the link lifetimes;
  - `Referrer-Policy`;
  - a send lost at shutdown;
  - a reset-page confirmation field and early bad-link check;
  - the vendor-admin recovery path;
  - UTF-16 `minLength`;
  - telling the user their password was cleared;
  - the spec's own stale notes;
  - findings the intent auditor only described.

**Follow-up review recommended: false.** This was a first-run pass. It patched no high finding and only one medium (once-per-token failure auditing), which is pinned by its own test. Patched by verdict: medium 1, low 5.

**Verification (after the pass-2 patches):**
- `cargo test --workspace`: 307 passed, 0 failed, exit 0 (Postgres and Mailpit up).
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean.
- `rustfmt --edition 2021 --check` on every touched `.rs` file: clean.
- `pnpm --filter portal typecheck`, `test` (59) and `build`: pass.
- Test-matrix audit: every row has a passing test, for example:
  - `request_routes_answer_known_and_unknown_emails_identically`;
  - `a_magic_link_takes_a_pre_registered_account_back_from_its_creator`;
  - `a_reset_completed_concurrently_succeeds_once`;
  - `a_refused_send_leaves_other_stale_rows_and_the_audit_log_alone`.

**Residual risks:**
- Anyone who knows an address can lock out every route for it at once: 5 wrong passwords plus 5 link requests. This is listed in CONTEXT.
- A user who never verified and then signs in by magic link must reset their password. Nothing in the portal tells them so (documented only).
- `accounts::tests::hashes_are_argon2id_and_verify` compares timings with a 4× margin. It could be flaky on a heavily loaded machine.
- The email subjects and bodies, page text and 429 messages were worded without the owner (decision 7). They are functional, but worth a read before launch.
