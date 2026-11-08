---
title: 'Account and team management'
type: 'feature'
created: '2026-11-08'
baseline_revision: '1edcb4338610437e3512884d4fb00d6a3319fd22'
status: 'done'
review_loop_iteration: 1
followup_review_recommended: false
context: ['{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md']
warnings: ['oversized']
deferred:
  - summary: >-
      The account, organization and accept-invite portal pages have no component tests, so hidden owner controls, the confirm-before-self-demotion prompt and lookup-without-consume on load are checked only by typecheck.
    evidence: |-
      Only apps/portal/src/api/*.test.ts exist; the portal has no component-test setup (story 2 and 3 deferrals). The hub enforces every rule regardless, and tests/team.rs covers it over HTTP.
    location: >-
      apps/portal/src/pages/OrganizationPage.tsx
    severity: low
  - summary: >-
      Nothing automated proves /accept-invite drops the previous browser user's cached queries after a successful accept.
    evidence: |-
      Deleting the removeQueries call in AcceptInvitePage's onSuccess passes typecheck and vitest; the portal has no component-test setup. Members and invitations caches are now keyed by organization id, which limits the exposure.
    location: >-
      apps/portal/src/pages/AcceptInvitePage.tsx
    severity: low
---

<intent-contract>

## Intent

**Problem:** A signed-in portal user can see their account but change nothing: not their name, not their password (only a reset by email), not their organization's name. An owner cannot bring colleagues into the organization, remove them or change what they may do, so every customer is a one-person organization (SPEC CAP-4).

**Approach:** Add organization-scoped hub routes and portal pages for the user's profile and password, the organization's name, and the team: a member list for everyone, plus owner-only invitations by email, role changes and member removal. Members get 403 on every team-management route. An invited person accepts through an emailed, single-use link that creates their user in the inviting organization.

## Boundaries & Constraints

**Always:**
- **Scoping.** Every query in this story takes the signed-in user's `organization_id` from the session, in the data layer (new `team` module), never from the request. A user id or invitation id from another organization behaves exactly like an unknown id: 404, nothing changes, nothing is audited.
- **Role is read live.** `CurrentUser` gains `role`, read in the same query that resolves the session, so a demotion or removal takes effect on the user's very next request.
- **Owner-only routes** (rename organization, invite, list/revoke invitations, change role, remove member) answer a `member` or `vendor_admin` 403 `only an organization owner can manage the team`, before any validation side effect, throttle charge or write.
- **Roles assignable here** are `owner` and `member` only. Any other value is 400 `role must be owner or member`. A `vendor_admin` is never a target of these routes (404 like an unknown id) and never created by them.
- **At least one owner.** A role change or removal that would leave the organization with no `owner` is 409 `an organization must keep at least one owner`. The check runs after `SELECT … FROM organizations WHERE id = $1 FOR UPDATE`, so two owners demoting each other concurrently cannot both succeed.
- **Profile:** `POST /api/account/profile {name}` → 200 `Account`. Name validated like sign-up's; audit `account.profile_updated` `{}` (the name itself stays out of the append-only log). Only the name is editable; the email is shown read-only.
- **Password change:** `POST /api/auth/password {current_password, new_password}`.
  1. `new_password` is checked with `validate_password` (400) before anything else.
  2. The attempt is charged against the sign-in throttle (`email:<the user's address>` and `ip:<peer>`) exactly like a sign-in; a refusal is 429 `MSG_LOCKED` with `Retry-After`. The remaining work runs in its own task, like `finish_sign_in`.
  3. Wrong current password (including an unusable `!unusable` hash): 400 `current password is incorrect`, `auth.password_change_failed` `{reason: "wrong_password", peer}`, and the failure is confirmed against both keys (so the 5th failure locks, audited `auth.sign_in_locked`).
  4. Right: in one transaction set the new Argon2id hash, retire outstanding `password_reset` and `magic_link` tokens, delete every session of the user (this one included), create a new session, clear the `email:` key and refund the IP charge, audit `auth.password_changed` `{sessions_ended: n, peer}`. Answer 200 `Account` with the new session cookie: this browser stays signed in, every other one is signed out.
- **Organization:** `GET /api/organization/members` (any role) → `{"members": [User…]}` ordered by name then email, own organization only. `POST /api/organization {name}` (owner) → 200 `Organization`; audit `organization.renamed` `{from, to}`.
- **Invitations** (new table, migration `0006_invitations.sql`): `id`, `organization_id`, `email`, `role` (`owner`|`member`), `token_hash` (SHA-256 of 32 OS-RNG bytes, unique), `invited_by` (`REFERENCES users ON DELETE SET NULL`), `created_at`, `expires_at` (**7 days**), `accepted_at`, `revoked_at`.
  - `POST /api/organization/invitations {email, role}` (owner). Order: owner check (403) → the inviting owner's email must be verified, else 403 `verify your email address before inviting members` → `validate_email` and role (400) → `charge_mail(address, peer)` plus the organization key below (429 as story 3) → one transaction: `lock_organization`, re-check the actor is still an owner (403), address already belongs to a user of this organization: 409 `this person is already a member of your organization` (rolls back; the charge stands, as a sign-up 409's does), revoke this organization's other pending invitations for the same address (`lower(email)`), each audited `organization.invitation_revoked` `{invitation_id, email, reason: "replaced"}`, delete its expired ones, insert, audit `organization.member_invited` `{invitation_id, email, role, peer}` → commit → send the email → 201 `{"invitation": Invitation, "invitation_email_sent": bool}`. SMTP down still creates the invitation (`false`); inviting again re-sends.
  - Whether the address has an account in **another** organization is not checked or revealed here.
  - **Per-organization invitation limit:** each invite also charges `mail:org:<organization_id>`: **20 per organization per hour**, same window, lock length, confirm-at-once rule, `auth.mail_locked`/`auth.mail_lock_summary` auditing and 429 `MSG_MAIL_LOCKED` as the other email-send keys. The organization key is charged after the address key; if it refuses, the IP and address charges of this request are refunded.
  - `GET /api/organization/invitations` (owner) → `{"invitations": [Invitation…]}`: pending (not accepted, not revoked, not expired) only.
  - `POST /api/organization/invitations/{id}/revoke` (owner) → 204; under `lock_organization` with the owner re-check, like the other owner writes; audit `organization.invitation_revoked` `{invitation_id, email, reason: "revoked"}`. Not pending or not this organization's → 404 `no such pending invitation`.
  - `Invitation` JSON: `{id, email, role, created_at, expires_at}`, timestamps as RFC 3339 UTC strings formatted in SQL (`to_char(… AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')`), so no new sqlx feature.
  - The email (subject `You're invited to join a Wipestation organization`) names the organization and the inviter, gives `{public_base_url}/accept-invite?token=…`, says it works once and expires in 7 days, and "ignore if you did not expect it". Plain text; names are user input and go only in the body, never a header.
- **Accepting:** `POST /api/invitations/lookup {token}` (anonymous) → 200 `{organization_name, email, role}` for a pending token, else 400 `this invitation is invalid or has expired`; changes nothing. `POST /api/invitations/accept {token, name, password}` (anonymous):
  1. name and password validated (400) first; an invalid one never burns the token;
  2. token peeked (pending) before Argon2, else the 400 above;
  3. one transaction: consume the invitation atomically (`UPDATE … SET accepted_at = now() WHERE token_hash = $1 AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now() RETURNING …`; losing a race is the same 400); insert the user into the invitation's organization with its role and `email_verified_at = now()` (following the link proves the mailbox); a taken email (an account anywhere) rolls everything back → 409 `an account with this email already exists`; delete the incoming session, whoever's; create a session; audit `organization.invitation_accepted` (actor and subject `user:<new id>`, `{invitation_id, organization_id, role, rotated, peer}`);
  4. 201 `Account` with the session cookie.
- **Role change:** `POST /api/organization/members/{id}/role {role}` (owner) → 200 `User`. Same role → 200, no write, no audit. Otherwise audit `organization.member_role_changed` (subject `user:<id>`, `{organization_id, from, to}`). An owner may demote themselves if another owner remains. Demoting an owner also revokes, in the same transaction, every pending invitation that user sent (`invited_by`), each audited `organization.invitation_revoked` `{invitation_id, email, reason: "sender_demoted"}`.
- **Removal:** `POST /api/organization/members/{id}/remove` (owner) → 204. Removing yourself is 409 `you cannot remove yourself from the organization`. Every pending invitation the removed user sent is revoked first (`reason: "sender_removed"`, audited as above); then the user row is deleted (sessions and tokens cascade, so they are signed out at once; `invited_by` on their past invitations becomes null; the address is free to sign up or be invited again); audit `organization.member_removed` (subject `user:<id>`, `{organization_id, email, role, sessions_ended}`), written in the same transaction.
- Invitation revocations caused by a re-invite, a demotion or a removal are written in the same transaction as their cause.
- `validate_name` additionally rejects control characters (`char::is_control`) with `{field} must not contain control characters`; it now also guards names that appear in invitation emails.
- Audit actor is always `user:<acting id>`; every write and its audit row share a transaction. Organization-level actions use subject `organization:<id>`. Error bodies stay `{"error": "<message>"}`.
- **Portal:** `/account` (profile name form, email read-only, password-change form, a note that someone who signed in by link with no password should use "Forgot password"); `/organization` (organization name, editable by owners; member list; for owners: role select and Remove per member, an invite form with role, pending invitations with Revoke; changing **your own** role asks for confirmation first, like Remove; an owner whose email is unverified sees the verify-and-resend notice in place of the invite form); `/accept-invite?token=` (looks the invitation up on load, which consumes nothing; shows organization, email and role; name and password form; joins only on submit; on success drops every cached query of the previous browser user, then caches `["me"]` and goes home). Home links to both pages when signed in. Controls hidden from members are a convenience only; the hub enforces.
- Every new mutating route is added to the CSRF test's route list.

**Never:**
- No `sqlx::query!`; do not edit migrations 0001–0005; do not trust proxy headers.
- Never return or log a raw invitation token, and never put one in an audit row.
- Never let a request choose the organization it acts on.
- Do not create, assign or modify `vendor_admin` users here; do not port role or last-owner rules into the portal beyond hiding controls.
- Not in this story: changing a user's email address; membership of more than one organization; ownership transfer flows beyond role change; deleting an organization or leaving one; organization fields beyond its name; admin area (story 5); MFA; portal component tests (no setup exists).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Profile | owner posts `{name:" Grace "}` | 200 Account with name `Grace`; `me` shows it; `account.profile_updated` | blank or control-char name → 400 |
| Password change | sessions A (this) and B; right current, valid new | 200 Account + new cookie; A and B 401 on `me`; new cookie 200; old password 401, new 200 on sign-in; `auth.password_changed {sessions_ended:2}` | new password 11 chars → 400, nothing charged |
| Wrong current password | 5 wrong attempts | each 400; 5th locks `email:` key (`auth.sign_in_locked`); 6th, even right, 429 `Retry-After`; password sign-in for that email also 429 | — |
| Change retires links | outstanding reset link, then password change | reset link → 400 afterwards | — |
| Rename organization | owner `{name}` | 200; `organization.renamed {from,to}` | member → 403, name unchanged; control-char name → 400 |
| Member list | owner of org A with 2 users; org B exists | exactly A's 2 users | not signed in → 401 |
| Invite happy path | verified owner invites new address as `member` | 201 `invitation_email_sent:true`; one Mailpit message with `/accept-invite?token=`; listed as pending; `organization.member_invited` | — |
| Invite as member | member posts invite | 403; no row, no mail, no throttle charge | — |
| Unverified owner invites | owner never verified | 403 verify message; no mail | — |
| Invite existing member | address of a user in this org (any case) | 409; no mail | — |
| Invite bad role | `role:"vendor_admin"` or `"admin"` | 400 `role must be owner or member` | — |
| Re-invite | same address invited twice | first token → 400 on accept; second works; one pending listed | — |
| Invite mail limit | 6th email to one address within an hour across any email-sending route | 429 as story 3 | — |
| Organization invite limit | 21st invitation from one organization within an hour (distinct addresses, IP limit not reached) | 429 `too many emails requested; try again later` with `Retry-After`; no invitation row or mail; `auth.mail_locked` once for `mail:org:<id>` | — |
| Concurrent invites | same address invited twice at once | exactly one pending invitation afterwards; the other audited `reason:"replaced"` | — |
| Re-invite audit | address invited twice | first invitation's `organization.invitation_revoked` `{reason:"replaced"}` | — |
| Accept | invitee looks up, then accepts with name + 12-char password | lookup 200 `{organization_name,email,role}`; accept 201 Account in org A with role, `email_verified:true`; `me` 200; owner's member list includes them | — |
| Accept reuse / revoked / expired | used, revoked, or `expires_at` moved into the past | 400 `this invitation is invalid or has expired` (lookup and accept) | — |
| Accept concurrently | one token accepted 5× at once | exactly one 201 | — |
| Accept weak password | 11 chars | 400; token still accepts afterwards | — |
| Accept taken email | address signed up elsewhere after the invite | 409; invitation still pending; no user created | — |
| Change role | owner promotes member to owner, then demotes | 200 each; `organization.member_role_changed {from,to}` | same role → 200, no audit row |
| Demotion is live | owner B demoted by owner A while B holds a session | B's next owner route → 403 | — |
| Last owner | sole owner demotes or removes themselves | 409 `an organization must keep at least one owner` / 409 self-removal | — |
| Concurrent mutual demotion | two owners demote each other at once | exactly one 200, the other 409; one owner remains | — |
| Remove member | owner removes a member holding a session | 204; member's session 401; their email can sign up again; `organization.member_removed` | — |
| Removed owner's invitations | owner B invites X (pending) and Y (accepted); owner A removes B | 204; X's invitation → 400 on accept and not listed (`reason:"sender_removed"`); Y's row survives with `invited_by` null | — |
| Demoted owner's invitations | owner B invites X; A demotes B | X's invitation revoked (`reason:"sender_demoted"`) | — |
| Accept while signed in | browser holds user C's session; invitee accepts | 201; C's session 401; detail `rotated:true` | — |
| Password change clears the lock count | 4 wrong current passwords, then the right one | 200; no `email:` throttle row; a following wrong sign-in is 401, not 429 | — |
| Cross-organization | owner of A targets B's user id or invitation id (role, remove, revoke) | 404; B unchanged; no audit row | — |
| vendor_admin target | owner targets a `vendor_admin` id | 404 | — |
| CSRF | each new POST route without token | 403 `missing or invalid CSRF token` | — |

## Autonomous decisions

Recorded under the owner's "decide by default" rule (2026-11-08). None is product, pricing, legal or ADR-contradicting; customer-visible wording follows story 3 decision 7 (functional, rewordable).

1. **Email changes are out of scope.** *About:* a user wants to change the address they sign in with. *Problem:* doing it safely needs a confirmation link to the new address and a notice to the old one (OWASP), which is a story of its own. *Options:* (a) defer, show the email read-only; (b) allow an unverified change. *Choice:* (a); listed as a CONTEXT limitation.
2. **Password change asks for the current password and shares the sign-in lock.** *Problem:* a stolen session must not be enough to take the account, and the current-password check is otherwise an unthrottled password-guessing oracle. *Choice:* require the current password (OWASP Authentication Cheat Sheet) and charge the same `email:`/`ip:` throttle as sign-in. On success every other session ends (OWASP Session Management: invalidate sessions on password change) and outstanding reset and sign-in links are retired.
3. **Invitations are a new table with a 7-day, single-use link that creates the user.** *Problem:* `auth_tokens` needs an existing user; an invitee has none. Seven days is the common lifetime for invitations, long enough for a colleague on leave. Accepting proves the mailbox, so the new user is verified.
4. **One organization per person.** ADR-0007 §3 and the schema give each user exactly one organization. An invitation to an address that already has an account fails at acceptance with 409, told only to the mailbox owner; the owner is not told at invite time, so inviting adds no new way to learn which addresses have accounts. Listed as a CONTEXT limitation.
5. **Only a verified owner can invite.** *Problem:* someone who pre-registers a victim's address could invite an accomplice; the accomplice would stay in the organization after the real owner takes the account back (story 3 decision 8). Requiring a verified email closes that path at negligible cost.
6. **An organization always keeps an owner, and owners cannot remove themselves.** Otherwise a slip leaves an organization nobody can manage, with no recovery until a vendor admin area exists. Enforced under a row lock on the organization.
7. **Removal deletes the user.** *Options:* (a) delete the row (sessions and tokens cascade; the address is free again; the audit row keeps id, email and role); (b) a soft "deactivated" flag that keeps the address taken. *Choice:* (a): nothing else references users yet, the person can be re-invited or start their own organization, and the append-only audit log is the record. Revisit when orders reference users.
8. **Organization details are its name.** It is the only organization field; billing details belong with payments.
9. **Names reject control characters.** Organization and inviter names now appear in emails to third parties; rejecting control characters is standard input hygiene and changes nothing for real names.
10. **Members can read the member list; only owners see invitations.** Colleagues seeing who is on the team is ordinary; invitations are management state.
11. **A removed or demoted owner's pending invitations are revoked (review, 2026-11-08).** *About:* an owner removes or demotes a co-owner. *Problem:* invitations outlive their sender, so an owner expecting removal could invite a personal address as `owner` and rejoin after being removed. *Options:* (a) revoke the sender's pending invitations on removal or demotion; (b) check at acceptance that the sender is still an owner; (c) leave it to the remaining owners to notice. *Choice:* (a): it is visible in the audit log and the pending list at the moment it happens, and (b) would also kill invitations sent by an owner who simply left later.
12. **Per-organization invitation limit of 20 per hour (review, 2026-11-08).** *Problem:* invitation emails carry free-text organization and inviter names to any address; per-address and per-IP limits alone let one verified account send them from rotating IPs. *Choice:* a third email-send key, `mail:org:<id>`, with the IP limit's value. A real team rarely invites 20 people in an hour; if one does, they wait.
13. **Invites and explicit revocations take the organization lock too (review, 2026-11-08).** Every owner write in this story now runs under `lock_organization` with the owner re-check, so concurrent invites for one address leave exactly one pending, and a just-demoted owner cannot invite or revoke.
14. **Invitee and removed-member addresses stay in the append-only audit log (review, 2026-11-08).** The log is the record of who was invited and removed (ADR-0007 §6); story 2 and 3 throttle subjects already store addresses. The user's own display name is kept out, as it adds nothing to the trail. Listed as a CONTEXT limitation for a later data-retention decision.

</intent-contract>

## Code Map

- `crates/wipe-hub/migrations/0006_invitations.sql` (new): `invitations` table as in the contract; `CHECK (role IN ('owner','member'))`, `CHECK (octet_length(token_hash) = 32)`, `UNIQUE (token_hash)`, index on `(organization_id, lower(email))`, and an index on `invited_by` (the `ON DELETE SET NULL` action and the sender revocation look it up). Header comment in the style of 0003–0005.
- `crates/wipe-hub/src/accounts.rs`:
  - `session_user` (~l.604) returns `(user_id, organization_id, role)`; parse with `Role::from_str` like `account` (~l.485).
  - `validate_name` (~l.300) gains the control-character rule; make it `pub`. Unit test.
  - Reuse `Token`, `hash_token`, `hash_password`, `verify_password`, `create_session`, `delete_session`, `delete_user_sessions`, `retire_tokens`, `set_password_hash`, `validate_email`, `validate_password`, `UNUSABLE_PASSWORD_HASH`.
  - `create_account` (~l.416) is the pattern for mapping `users_email_lower_key` to a duplicate-email error.
- `crates/wipe-hub/src/team.rs` (new, `pub mod team` in `lib.rs`): organization-scoped data layer. Every function takes `organization_id` first. `members`, `member` (excludes `vendor_admin`), `rename_organization`, `lock_organization` (`FOR UPDATE`), `owner_count`, `set_role`, `remove_member` (returns email, role and sessions ended), `is_member_email`, `create_invitation` (revoke same-address pending + purge expired + insert, returns raw token and `Invitation`), `pending_invitations`, `revoke_invitation`, `peek_invitation` (by token, any org: returns org name, email, role), `consume_invitation`, `create_invited_user`. `Invitation` struct with serde. Unit tests where pure.
- `crates/wipe-hub/src/auth.rs`:
  - `CurrentUser` (~l.251) gains `pub role: Role`; add a `require_owner(&self) -> Result<(), ApiError>` (403 message constant `MSG_OWNER_ONLY`).
  - `routes()` (~l.81) adds `POST /api/auth/password`.
  - `change_password` handler mirrors `sign_in`/`finish_sign_in` (~l.694–832): `email_key(SIGN_IN_EMAIL_PREFIX)`, `charge_sign_in`, `tokio::spawn`, failure arm with `confirm_failure` + `auth.sign_in_locked`, success arm with `throttle::clear` + `refund`. Fetch the hash with a scoped query (organization id + user id), never by email.
  - Make `actor`, `ip_key`, `with_peer`, `charge_mail`, `secure`, `session_cookie` usable from the new handlers module (`pub(crate)`).
- `crates/wipe-hub/src/team_routes.rs` (new, merged in `lib.rs` `router()` next to `auth::routes()`, so the CSRF layer and the `/api/*rest` 404 still cover it): handlers for `/api/account/profile`, `/api/organization`, `/api/organization/members[/{id}/role|/remove]`, `/api/organization/invitations[/{id}/revoke]`, `/api/invitations/{lookup,accept}`. Use axum 0.7 path syntax `/:id`. Reuse `ApiError`, `JsonBody`, `CurrentUser`. Path ids that do not parse as UUID → 404 `no such member` / `no such pending invitation` (not axum's plain-text 400).
- `charge_mail` (auth.rs ~l.615) gains an optional extra key with its own limit (`throttle::MAIL_ORG_LIMIT = 20` in `throttle.rs`, plus the `sign_in_constants`-style test), charged last and refunding the earlier charges when it refuses; existing callers pass `None`. The key is `mail:org:<uuid>`, so `lock_summary_action` already maps it to `auth.mail_lock_summary`.
- `team.rs` gains `revoke_invitations_sent_by(conn, organization_id, user_id) -> Vec<(id, email)>`, and `create_invitation` returns the ids and emails it replaced, so the handlers can audit each.
- `crates/wipe-hub/src/mail.rs`: `INVITATION_SUBJECT` and `send_invitation(to, organization_name, inviter_name, link)`.
- `crates/wipe-hub/tests/team.rs` (new): copy the minimal `Hub` harness from `tests/auth.rs` (~l.27–250: `spawn_hub`, `post`, `post_empty`, `me`, `sign_up`, `sign_in`, `audited`, `cookie_pair`, `error_of`, `token_in`) or move shared helpers into `tests/support`. A `get(path, cookie)` helper is new. Verify an owner by consuming the verification mail (`verification_token`) or by `UPDATE users SET email_verified_at = now()` through `hub.db.pool`. Simulate expiry by moving `invitations.expires_at` into the past. One test per matrix row.
- `crates/wipe-hub/tests/auth.rs` CSRF route list (~l.906) gains every new POST route; the password-change tests live in `tests/team.rs` or here.
- `crates/wipe-hub/tests/migrations.rs`: 0006 applies (existing pattern).
- `apps/portal/src/api/auth.ts`: export `request`, `ensureOk`, `json`, `isObject` and the `isUser`/`isOrganization`/`isAccount` guards for reuse; add `changePassword`, `updateProfile`. Update the mirror comment.
- `apps/portal/src/api/team.ts` (new, + `team.test.ts` with the `fakeHub` pattern from `auth.test.ts` ~l.40): `Invitation` type, `fetchMembers`, `renameOrganization`, `inviteMember`, `fetchInvitations`, `revokeInvitation`, `changeRole`, `removeMember`, `lookupInvitation`, `acceptInvitation`. Mirror comment pointing at `team_routes.rs` and `team.rs`.
- `apps/portal/src/main.tsx`: routes `/account`, `/organization`, `/accept-invite` (`validateSearch: tokenSearch`).
- `apps/portal/src/pages/`: new `AccountPage.tsx`, `OrganizationPage.tsx`, `AcceptInvitePage.tsx`; `HomePage.tsx` `SignedIn` links to both. Reuse `Field`, `FormError`, `SubmitButton`; `emailRequestErrorMessage` for the invite form, `signInErrorMessage` for password change.
- `CONTEXT.md`: §10 l.693 drop "No team or profile pages (story 4)"; add a story-4 row with limitations (invitee and removed-member email addresses stay in the append-only audit log; no email change; one organization per person, an invite to an address with an account fails at acceptance; removal deletes the user; organization names appear in invitation emails to third parties; inviter cannot see whether mail was delivered beyond `invitation_email_sent`). §14: add **Invitation**; extend **Hub user role** with what an owner can do and the at-least-one-owner rule.
- `README.md` l.48 and l.50: mention profile, password change, organization and team management, invitations.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0006_invitations.sql` -- create `invitations` -- the invitee has no user row yet.
- `crates/wipe-hub/src/accounts.rs` -- role in `session_user`, `validate_name` control characters, unit tests -- live role checks.
- `crates/wipe-hub/src/team.rs` -- organization-scoped data layer and `Invitation` -- scoping lives in the data layer (SPEC constraint).
- `crates/wipe-hub/src/auth.rs` -- `CurrentUser.role`, `require_owner`, `POST /api/auth/password`, visibility of shared helpers.
- `crates/wipe-hub/src/team_routes.rs`, `src/lib.rs` -- the handlers and router merge.
- `crates/wipe-hub/src/mail.rs` -- invitation email.
- `crates/wipe-hub/tests/team.rs`, `tests/auth.rs`, `tests/migrations.rs` -- one HTTP test per matrix row (including the rows added in review: organization invite limit, concurrent invites, re-invite audit, removed and demoted owners' invitations, accept while signed in, password change clearing the lock count, control-char organization name), CSRF list, migration check.
- `apps/portal/src/api/auth.ts`, `api/team.ts`, `api/team.test.ts` -- client functions, each POST with `X-CSRF-Token`, type guards.
- `apps/portal/src/main.tsx`, `src/pages/*` -- the three pages and home links.
- `CONTEXT.md`, `README.md` -- shipped behaviour, glossary and limitations, in this commit.

**Acceptance Criteria:**
- Given two organizations A and B, when A's owner calls every new organization route with B's user and invitation ids, then each answers 404 and B's members, roles, invitations and audit rows are unchanged.
- Given a `member` session, when it calls each owner-only route, then each is 403 and writes nothing (no row, no throttle charge, no mail, no audit).
- Given a verified owner, when they invite an address, the invitee accepts from the emailed link, and the owner then promotes, demotes and removes them, then each step is visible through `GET /api/organization/members` and `me`, and the audit log holds `organization.member_invited`, `organization.invitation_accepted`, `organization.member_role_changed` ×2 and `organization.member_removed` in that order.
- Given the workspace, when `cargo test --workspace`, clippy and the portal `typecheck`/`test`/`build` run, then all pass.

## Spec Change Log

- 2026-11-08 (review pass 1, loop 1) -- bad_spec loopback.
  - **Triggered by:**
    - a removed or demoted owner's pending invitations stayed live, so a departing owner could plant an `owner` invitation to themselves and rejoin (medium);
    - no per-organization limit on invitation emails, which carry free-text names to arbitrary addresses (medium);
    - invites and revocations did not take the organization lock: concurrent invites left two pending links, and a just-demoted owner could still invite (low);
    - re-invites revoked earlier invitations without an audit row (low); tests missing for a successful password change clearing the throttle, removing an owner who has sent invitations, accepting while signed in, and the control-character rule on organization names (low); a stale-cache flash on `/accept-invite`, unconfirmed self-demotion and an invite form an unverified owner cannot use (low); no index on `invited_by` (low); audit-log retention of addresses undocumented (low).
  - **Amended in `<intent-contract>`** (decide-by-default): sender-invitation revocation on removal/demotion; `mail:org:<id>` limit; lock and owner re-check on invite and revoke, membership check inside it; `reason` on every `organization.invitation_revoked`; portal confirmation, unverified-owner notice and cache drop; eight matrix rows; decisions 11–14.
  - **Amended outside it:** Code Map (`invited_by` index, `charge_mail` extra key, `MAIL_ORG_LIMIT`, `revoke_invitations_sent_by`, CONTEXT limitation) and Tasks (extra tests).
  - **Known-bad states avoided:** a removed owner rejoining through their own invitation; one account mass-mailing invitations; two live invitations for one address.
  - **KEEP:** attempt 1 is saved at [portal-accounts-story-4-attempt-1.patch](../../../_bmad-output/implementation-artifacts/portal-accounts-story-4-attempt-1.patch) (applies cleanly to `1edcb4338610437e3512884d4fb00d6a3319fd22`). Start by applying it with `git apply`, then change only what the amendments require. Keep everything else in it: migration 0006 (add only the index); `team.rs`, `team_routes.rs` and their structure, messages and `require_owner_locked`; `change_password` and its spawned-task shape; `validate_name`'s control-character rule; the invitation email; every portal page and client function; the CONTEXT/README edits; all 27 `tests/team.rs` tests, the CSRF route list and the migration test. It passed `cargo test --workspace` (341), clippy, rustfmt and the portal checks (74 tests).

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 31 findings — high 0, medium 3, low 25, false 3, maybe-false 0
- attempted change saved at [portal-accounts-story-4-attempt-1.patch](../../../_bmad-output/implementation-artifacts/portal-accounts-story-4-attempt-1.patch); code reverted for the bad_spec loopback
- findings:
  - `[medium]` `[bad_spec]` (blind) Removing or demoting an owner leaves their pending invitations live, so a removed owner can rejoin through an `owner` invitation they sent themselves — Verified: `remove_member`/`change_role` never touch `invitations`, and `invited_by` is `ON DELETE SET NULL`. Amended: decision 11 and two matrix rows.
  - `[low]` `[bad_spec]` (blind) Nothing enforces one pending invitation per address; concurrent invites both insert — Verified: `create_invitation` revokes then inserts with no lock or unique index. Amended: decision 13 (invite under `lock_organization`) and a matrix row.
  - `[low]` `[bad_spec]` (blind) Invitee and removed-member emails are stored permanently in the append-only log while the user's name is kept out, undocumented — Real inconsistency without a record. Amended: decision 14 and a CONTEXT limitation.
  - `[low]` `[reject]` (blind) "7 days" is written as a literal in the email and the portal apart from `INVITATION_TTL` — Drifts only if the constant changes (same call as story 3's link lifetimes); the mail test pins the text.
  - `[low]` `[reject]` (blind) Accepting an invitation silently signs out the browser's current user — Rotation is deliberate and matches sign-in and magic-link sign-in; a notice is new customer-visible copy for a rare case.
  - `[low]` `[bad_spec]` (blind) The role select demotes yourself on change with no confirmation, while Remove asks — Verified in `OrganizationPage`. Amended: portal bullet (confirm before changing your own role).
  - `[low]` `[reject]` (blind) The accept page shows only the error on a dead or taken-address invitation, with no next step — The hub's messages name the problem; adding guidance is new copy for a rare path.
  - `[low]` `[reject]` (blind) Failed invitation lookups and accepts are not audited, unlike story-3 links — A failed accept changes nothing and grants nothing; bounded failure auditing needs a new column and state for no current harm.
  - `[medium]` `[bad_spec]` (blind) No per-organization limit on invitations; one verified account can send branded emails with free-text names to many addresses from rotating IPs — Verified: only `mail:to:` and `mail:ip:` keys are charged. Amended: decision 12, `mail:org:<id>` key, matrix row.
  - `[low]` `[reject]` (blind) `tests/team.rs` copies the hub test harness from `tests/auth.rs` instead of sharing it through `tests/support` — Real duplication, but consolidating means rewriting `tests/auth.rs`'s helpers; a refactor outside this story's behaviour.
  - `[low]` `[bad_spec]` (blind) Missing tests: removing an owner who sent invitations, a demoted sender's invitation, concurrent invites, accepting while signed in, a demoted owner renaming — Grouped with the root-cause rows above; amended: matrix rows for each except the mid-request demotion (see the verification-gap row on `require_owner_locked`).
  - `[low]` `[bad_spec]` (blind) The invite form is shown to an unverified owner, whom the hub refuses — Verified. Amended: portal bullet (show the verify notice instead).
  - `[low]` `[reject]` (blind) A same-role change answers 200 before the locked owner re-check, and a same-name rename audits `from == to` — Neither writes anything wrong: the same-role path changes no row, and a same-name rename row is accurate.
  - `[low]` `[bad_spec]` (blind) Migration 0006 has no index on `invited_by`, so each user deletion scans `invitations` — Verified; 0006 is applied nowhere persistent yet. Amended: Code Map index.
  - `[low]` `[bad_spec]` (edge) Concurrent invites for one address leave two pending — Same root cause as the blind uniqueness row; same amendment.
  - `[medium]` `[bad_spec]` (edge) A demoted or removed owner's invitations still bring in new owners — Same root cause as the blind row; same amendment.
  - `[low]` `[bad_spec]` (edge) The address can become a member between `is_member_email` and the insert — Same root cause (invite outside the lock); amended: membership check inside the locked transaction.
  - `[low]` `[bad_spec]` (edge) A just-demoted owner can create or revoke an invitation — Same root cause; amended: owner re-check under the lock on invite and revoke.
  - `[low]` `[bad_spec]` (edge) Re-inviting revokes the earlier invitation with no audit row — Verified in `create_invitation`. Amended: `reason: "replaced"` audit and a matrix row.
  - `[low]` `[reject]` (edge) A user removed mid-request gets a wrong-password 400 and a throttle charge from password change — Needs removal inside a millisecond window of the user's own request; the outcome is a refusal, not a grant.
  - `[low]` `[reject]` (edge) A user removed after verification but before the success transaction gets a 500 from the session insert — Same narrow window; it fails loudly and grants nothing.
  - `[false]` `[reject]` (edge) `lock_organization` returning `None` lets role change and removal proceed unlocked — The acting user's session resolved, so their user row exists and its `organization_id` foreign key guarantees the organization row; organizations are never deleted.
  - `[low]` `[bad_spec]` (edge) `/accept-invite` keeps the previous browser user's cached member and invitation queries — Verified: `onSuccess` only sets `["me"]`. Amended: portal bullet (drop every cached query).
  - `[low]` `[reject]` (edge) The profile form keeps a stale name if `["me"]` changes after mount — Only another tab's edit of your own name triggers it, and saving writes what the user sees.
  - `[low]` `[bad_spec]` (verification-gap) A successful password change clearing the email key and refunding the IP charge is untested — Pre-verified. Amended: matrix row "Password change clears the lock count".
  - `[low]` `[bad_spec]` (verification-gap) Removing an owner who has sent invitations is untested — Pre-verified. Amended: matrix row "Removed owner's invitations".
  - `[low]` `[reject]` (verification-gap) No test depends on `require_owner_locked` — A deterministic test needs a hook between session extraction and the lock; the check is defensive and the window narrow (the verification-gap layer itself filed defer).
  - `[low]` `[bad_spec]` (verification-gap) The control-character rule is untested on `POST /api/organization`, whose value goes into every invitation email — Pre-verified. Amended: the rename matrix row.
  - `[low]` `[defer]` (intent) The portal pages, the surface CAP-4 names, are covered by typecheck only — The portal has no component-test setup (story 2 and 3 deferrals); added to `deferred`.
  - `[false]` `[reject]` (intent) Profile and password editing is open to every role, not only owners — Descriptive; a member managing their own account is not "managing the team", which the intent restricts.
  - `[false]` `[reject]` (intent) Organization details, inviting existing accounts, removal semantics and token-keyed lookups follow one reading among several — Descriptive; each is a recorded autonomous decision (1, 4, 7, 8) or a credential-keyed lookup.

### 2026-11-08 — Review pass
- verdicts: 27 findings — high 0, medium 2, low 22, false 3, maybe-false 0
- findings:
  - `[medium]` `[patch]` (blind) Accepting a pending invitation whose address already has an account can be repeated forever, each request running a full Argon2 hash before the 409 rolls back — Verified in `accept_invitation`. Fixed: the peeked address is checked for an account before Argon2 (same 409); the in-transaction check stays for the race.
  - `[low]` `[patch]` (blind) After demoting or removing an owner, the portal keeps showing their revoked invitations as pending — Verified: `MemberRow` refresh reloaded only members. Fixed: refresh also invalidates the invitations query.
  - `[low]` `[patch]` (blind) Member and invitation caches carry over to the next user in the same tab — Verified: unkeyed query keys. Fixed: keyed `["members", orgId]` and `["invitations", orgId]`.
  - `[low]` `[reject]` (blind) Accepting silently signs out the current user — carried: rotation is deliberate, as for sign-in.
  - `[low]` `[reject]` (blind) The accept page dead-ends on a 409 — carried: the message names the problem; guidance is new copy for a rare path.
  - `[low]` `[patch]` (blind) Three doc comments say the invitation preview names the sender; it does not — Verified. Fixed: comments corrected, no field added.
  - `[low]` `[patch]` (blind) A locked email also blocks a signed-in user's password change, undocumented — Real (shared `email:` key, by design). Fixed: CONTEXT limitation.
  - `[low]` `[patch]` (blind) `sessions_ended` in `organization.member_removed` counts expired sessions — Verified in `team::remove_member`. Fixed: counts `expires_at > now()` only.
  - `[low]` `[reject]` (blind) No partial unique index enforces one pending invitation per address — Every writer now takes `lock_organization` (tested by `concurrent_invites_for_one_address_leave_one_pending`); the index guards only hypothetical future writers.
  - `[low]` `[patch]` (blind) Member actions do not refresh the list when they fail — Verified (`onSuccess` only). Fixed: `onSettled`.
  - `[low]` `[patch]` (blind) Organization names in `organization.renamed` stay in the append-only log, not mentioned in the retention note — Fixed: CONTEXT limitation names them.
  - `[low]` `[reject]` (blind) Password change returns 500 if the user is removed mid-flow — carried: millisecond window, fails loudly, grants nothing.
  - `[low]` `[reject]` (blind) `VerifyEmailNotice` and `SignInPrompt` live in page modules — No current cycle or defect; moving them is a refactor with no behaviour change.
  - `[low]` `[reject]` (blind) Profile and rename forms keep a stale name after `["me"]` refetches — carried: needs a concurrent edit elsewhere, and saving writes what the user sees.
  - `[low]` `[patch]` (edge) Previous user's cached members and invitations shown after a user switch — Same root cause as the blind cache row; same fix.
  - `[low]` `[reject]` (edge) A demoted owner's same-role or last-owner request gets 200/409 instead of 403 — carried: neither path writes anything.
  - `[low]` `[reject]` (edge) A client disconnecting after commit can cancel the invitation email send — Rare; the invitation is listed as pending and inviting again re-sends.
  - `[medium]` `[patch]` (edge) Unlimited Argon2 through repeated accepts with a taken address — Same root cause as the blind row; same fix.
  - `[low]` `[reject]` (verification-gap) No test depends on `require_owner_locked` — carried: needs a hook between session extraction and the lock.
  - `[low]` `[patch]` (verification-gap) The stored 7-day expiry is never checked — Pre-verified. Fixed: `a_verified_owner_invites_by_email` asserts `expires_at - created_at` is 604800 s.
  - `[low]` `[patch]` (verification-gap) Purging expired invitations on re-invite is untested — Pre-verified. Fixed: `used_revoked_and_expired_invitations_are_refused` re-invites and asserts the expired row is gone and the others remain.
  - `[low]` `[defer]` (verification-gap) `/accept-invite` dropping the previous user's cache is untested — No portal component-test setup; added to `deferred`.
  - `[low]` `[patch]` (verification-gap) "A 409 keeps its email-send charge" is commented but not asserted — Pre-verified. Fixed: the test asserts the `mail:to:` charge count.
  - `[low]` `[defer]` (intent) Portal pages are proven only by typecheck — carried: already in `deferred`.
  - `[false]` `[reject]` (intent) Profile and password editing is open to every role — carried: managing your own account is not managing the team.
  - `[false]` `[reject]` (intent) Invitations, removal and token-keyed lookups follow one reading among several — carried: recorded decisions 1, 4, 7, 8; token lookups are credential-keyed.
  - `[false]` `[reject]` (intent) Role changes are audited as `organization.*`, not `auth.*` — The constraint requires the event to be audited, which it is; the action name is not specified.

## Design Notes

- **Why the last-owner check locks the organization row:** counting owners and then updating is a check-then-act race; two owners demoting each other would both see two owners. `SELECT id FROM organizations WHERE id = $1 FOR UPDATE` first serialises every role change and removal within one organization, and costs nothing across organizations.
- **Peek, hash, consume** for invitation acceptance follows the story-3 reset: Argon2 runs only for a holder of a pending token, and the atomic consume decides races.
- Scoped lookup example:

  ```rust
  sqlx::query_as("SELECT id, email, name, role, email_verified_at IS NOT NULL FROM users \
                  WHERE organization_id = $1 AND id = $2 AND role IN ('owner','member')")
      .bind(organization_id).bind(user_id)
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

**Summary:** Story 4 adds account and team management to the hub and portal (SPEC CAP-4). Choices made without the owner are decisions 1–14 in the intent contract.

- **Account:** any signed-in user edits their name (`POST /api/account/profile`) and changes their password (`POST /api/auth/password`). Changing the password needs the current one, shares the sign-in lock, ends every other session and retires outstanding reset and sign-in links.
- **Organization and team:** every member sees the member list. Owners rename the organization, invite by email (`owner` or `member`), list and revoke pending invitations, change roles and remove members. Members get 403 on all of these.
- **Invitations:** a 7-day, single-use emailed link to `/accept-invite` creates a verified user in the inviting organization. Limits: 5 emails per address, 20 per IP and 20 per organization per hour. A removed or demoted owner's pending invitations are revoked.
- **Rules:** every query is scoped to the session's organization (another organization's ids are 404). Roles are read live, so a demotion applies on the next request. An organization always keeps an owner, enforced under a row lock. Every action is audited in its own transaction.

**Files changed:**
- **Hub (`crates/wipe-hub`):**
  - `migrations/0006_invitations.sql`: the invitations table.
  - `src/team.rs`: organization-scoped data layer (new).
  - `src/team_routes.rs`: profile, organization, member and invitation routes (new).
  - `src/auth.rs`: role on `CurrentUser`, `require_owner`, password change, the extra email-send key.
  - `src/accounts.rs`: role in `session_user`, names reject control characters, scoped password-hash lookup.
  - `src/throttle.rs`: `MAIL_ORG_LIMIT`.
  - `src/mail.rs`: the invitation email.
  - `src/lib.rs`: the new modules and routes.
  - `tests/team.rs`: 34 HTTP tests (new). `tests/auth.rs`: CSRF route list. `tests/migrations.rs`: 0006.
- **Portal (`apps/portal/src`):** `api/team.ts` and its test (new); `api/auth.ts`; `main.tsx`; new `AccountPage`, `OrganizationPage`, `AcceptInvitePage`; `HomePage` links and the shared verify notice.
- **Docs:** `CONTEXT.md` (§10 story-4 row and limitations, §14 Invitation and Hub user role), `README.md`.
- **Review artifact:** [portal-accounts-story-4-attempt-1.patch](../../../_bmad-output/implementation-artifacts/portal-accounts-story-4-attempt-1.patch).

**Review findings:**
- **Pass 1 (31 findings):** one spec repair loop for three medium findings. A removed or demoted owner's invitations stayed live, and there was no per-organization invitation limit. Low findings were folded into the same amendment: invite and revoke under the lock, audited replacements, the `invited_by` index, portal fixes and extra tests. Code was reverted and rebuilt from the amended spec.
- **Pass 2 (27 findings):** patches applied for 1 medium entry and 9 low ones.
  - **Medium:** repeated accepts of an invitation whose address was taken ran unlimited Argon2 hashes. A taken address now 409s before hashing.
  - **Low:** caches keyed by organization; invitations refreshed after role changes and removals, including on failure; preview comments corrected; live-session count; two CONTEXT limitations; three test assertions.
- **Deferred:** portal page component tests, including the accept-page cache drop. The portal has no component-test setup.
- **Rejected:** each with its reason in the triage log. In short: the literal "7 days"; a notice when accepting signs out the current user; dead-end copy on the accept page; failure auditing for invitations; the test-harness duplication; same-role no-ops; a database unique index; narrow mid-request removal races; component placement; stale form state; a send lost when the client disconnects; untestable lock re-check; descriptive intent readings.

**Follow-up review recommended: false.** First run; the final pass patched one medium entry and no high ones. Patched by verdict: medium 1, low 9.

**Verification (after the pass-2 patches):**
- `cargo test --workspace`: 348 passed, 0 failed, exit 0 (Postgres and Mailpit up).
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean.
- `rustfmt --edition 2021 --check` on every touched `.rs` file: clean.
- `pnpm --filter portal typecheck`, `test` (74) and `build`: pass.
- Matrix audit: every row has a passing test in `tests/team.rs` or the CSRF test in `tests/auth.rs`.

**Residual risks:**
- The portal pages are proven only by typecheck and build (deferred).
- The owner re-check under the organization lock has no test that depends on it.
- Invitation, organization and email wording was written without the owner (decision 7's rule from story 3); worth a read before launch.
- A person can belong to one organization only; an invitation to an address with an account fails at acceptance and the inviter is not told.
