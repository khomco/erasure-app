---
title: 'Owner approves an enrollment in the portal'
type: 'feature'
created: '2026-11-09'
status: 'done'
baseline_revision: '2ffb764e79d35a6f796bf5ad88b89843ead48a03'
review_loop_iteration: 0
followup_review_recommended: false
context:
  - '{project-root}/docs/adr/0008-station-enrollment-and-licence-fetch.md'
  - '{project-root}/.scratch/station-enrollment/SPEC.md'
  - '{project-root}/.scratch/station-enrollment/stories/1-hub-enrollment-start-and-poll.md'
warnings: [oversized]
deferred:
  - summary: >-
      An owner who approves just before the code expires uses up a seat and issues a licence that the station cannot collect, while the portal says the station collects it by itself.
    evidence: |-
      `find_pending` accepts any code with `expires_at > now()`, and story 1's poll answers `expired_token` for an approved enrollment past its expiry. Approval at 9 min 58 s followed by the next poll after 10 min leaves a registered station and an active licence in the hub that the bench never receives (the owner can download the file from /stations). Story 1's CONTEXT §10.2 row already lists this as a limitation; fixing it means changing poll (excluded by this story's Never list) or refusing approvals with little time left (a new rule for a later story).
    location: >-
      crates/wipe-hub/src/enrollments.rs find_pending; crates/wipe-hub/src/enrollments.rs poll
    severity: medium
---

<intent-contract>

## Intent

**Problem:** Story 1 lets a station start an enrollment and poll for it, and sends the owner to `{public_base_url}/activate`, but nothing exists there: no page, no hub route that approves or denies, so no enrollment can complete (CAP-2, ADR-0008 §1 steps 3–4).

**Approach:** Three session routes for organization owners — look up a pending enrollment by its user code, approve it against one of the organization's entitlements (registering the station through `stations::register` in the same transaction), or deny it — throttled per user, plus a portal `/activate` page that drives them and returns a signed-out visitor to itself after sign-in.

## Boundaries & Constraints

**Always:**
- Routes (customer session, CSRF token as usual, **not** in `csrf::is_exempt`; every response `no-store`/`no-cache` like story 1's): `POST /api/enrollments/lookup {code}` → 200 `{enrollment}`; `POST /api/enrollments/approve {code, entitlement_id}` → 201 `{station, licence}` (the `POST /api/stations` shape); `POST /api/enrollments/deny {code}` → 200 `{}`. `enrollment` = `{user_code: "XXXX-XXXX", station_id, label, instance_key_id, requested_from, requested_at, expires_at}` (RFC 3339 UTC; no row id, no device-code material).
- Order in each handler: owner check (403 `MSG_OWNER_ONLY_ENROLLMENTS`, naming organization owners as the approvers, before the body is read; members and vendor admins alike); approve only: 503 `stations::MSG_ISSUANCE_UNAVAILABLE` with no vendor root; body; throttle charge; lookup.
- A code matches only an enrollment that is `pending` and unexpired (`expires_at > now()`), read `FOR UPDATE`, via `enrollments::normalize_user_code`. Anything else — malformed, unknown, expired, approved, denied, collected — is 404 `MSG_NO_ENROLLMENT`, one message for all.
- Throttle key `enroll:user:<user uuid>` (`throttle::ENROLL_USER_KEY_PREFIX`): every lookup/approve/deny is charged; a matching code refunds its charge, a non-matching one confirms the failure. 10 per 15 minutes, then locked 15 minutes → 429 `MSG_LOOKUP_LOCKED` with `Retry-After`. Lock audited once as `enrollment.lookup_locked` (actor `user:<id>`, subject the key); summaries as `enrollment.lookup_lock_summary` (`auth::lock_summary_action` must map `enroll:user:` before the `enroll:` start prefix).
- Approve: in one transaction, `stations::register(conn, root, session org, session user, NewStation { entitlement_id, instance_key_id, station_id: Some(enrollment's), label: enrollment's, site: None })`, then set the enrollment `approved` with `approved_by`, `approved_at = now()`, `organization_id`, `registered_station`, `licence_id`, then audit `enrollment.approved` (actor `user:<id>`, subject `enrollment:<id>`, detail `{station, licence, entitlement_id, instance_key_id}`). Any `register` refusal (unknown/other org's entitlement 404, not current 409, no free seat 409, key already registered 409) is returned unchanged and rolls back: the enrollment stays `pending`.
- Deny: set `denied`, audit `enrollment.denied` (actor `user:<id>`, subject `enrollment:<id>`, detail `{instance_key_id, station_id}`). The station's next poll gets `access_denied` (story 1).
- Portal `/activate?code=…`: signed out → a sign-in link to `/sign-in?next=<this page with its code>`; signed in but not owner → the refusal naming owners, no request sent; owner → code field (prefilled from `?code`), look up, then the station id, label, key id, requesting address and request time, the current entitlements with free seats (existing `seatChoices` over `fetchSeats` + `fetchStations`; preselect via `preselected`), Approve and Deny; outcomes approved (station name, licence expiry, "the station collects its licence by itself"), denied, and hub errors (incl. 429 minutes) shown with `FormError`. The sign-in page honours `?next=` only for a same-origin path (starts with one `/`, not `//` or `/\`), else `/`.
- Hub TS mirror changes with the Rust shape in the same commit.

**Never:**
- No change to start/poll behaviour, the migration (0019 is applied), station crates, CLI or desktop app (stories 3–4); no member-approval permission; no site field on approval.
- No raw device code anywhere; never `sqlx::query!`.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected | Error |
|---|---|---|---|
| Lookup ok | owner, pending code, any case/dash | 200 details; charge refunded | — |
| Wrong / expired / used / malformed code | | 404 `MSG_NO_ENROLLMENT`, failure counted | same text |
| Member or vendor admin | any route | 403 `MSG_OWNER_ONLY_ENROLLMENTS`, nothing charged | — |
| Approve ok | free seat | 201; enrollment approved; station polls `{station, licence}`; 3 audit rows (`station.registered`, `licence.issued`, `enrollment.approved`) | — |
| Approve refused | no seat / not current / other org's entitlement / key already registered | `register`'s status+message; enrollment pending; no audit | — |
| Deny | pending | 200; poll → `access_denied`; `enrollment.denied` | — |
| Another org's owner | holds the code | may look up, approve into *their own* org, or deny | — |
| Throttled | 11th wrong code in 15 min | 429 + `Retry-After`; lock audited once | — |
| No vendor root | approve | 503 | — |

## Autonomous decisions

- **Codes go in POST bodies, approve/deny re-send the code.** Problem: the code could travel in a URL (`GET /api/enrollments/:code`) or approve could take a row id from the lookup. Options: GET with code in path; POST bodies with code. Chosen: POST bodies, every action re-proving the code, because user codes are short-lived credentials (RFC 8628 §5.1) that should stay out of access logs, and one throttled lookup path covers all three actions.
- **Throttle figures.** No figure is set by the SPEC. Chosen: 10 failures per 15 minutes, locked 15 minutes (the sign-in per-email figures' shape); with ~2.6×10^10 codes this makes guessing a live code hopeless. Correct codes are refunded, so honest owners are never locked.
- **Entitlements come from existing routes.** The lookup returns only the enrollment; the page computes free seats from `GET /api/entitlements` and `GET /api/stations` (as `/stations` already does), so the seat rule is not duplicated in a new response. `register` remains the authority.
- **No site on approval.** A site-scoped entitlement gets the organization id as its site, exactly as a portal registration with the site left blank. Adding a site field is deferred to a later story if asked for.
- **Sign-in `next`.** Only the password sign-in page returns to `next`; magic-link sign-in still lands on `/` (the link is opened from mail, possibly elsewhere). Open-redirect safe by allowing only same-origin paths (OWASP unvalidated-redirects guidance).

</intent-contract>

## Code Map

- `crates/wipe-hub/src/enrollments.rs` -- story 1 domain: `normalize_user_code`, `format_user_code`, `enrollment_subject`, `EnrollmentError` (+`From<StationError>` maps non-DB errors to `Internal` — approve must keep `StationError` intact for the route instead). Add `PendingEnrollment` (serialize shape), `find_pending` (FOR UPDATE), `approve`, `deny`; extend module doc.
- `crates/wipe-hub/src/enrollment_routes.rs` -- story 1 routes + `no_store` layer + `charge_start` (template for the per-user charge; `throttle::{charge, confirm_failure, refund}`, `auth::{audit_lock_summary, locked_response}`). Add the three routes and `MSG_*` here or in `enrollments.rs`; update module doc.
- `crates/wipe-hub/src/stations.rs:765` `register`, `:396` `NewStation`, `Registered`, `MSG_ISSUANCE_UNAVAILABLE`; `station_routes.rs` `From<StationError> for ApiError` (reuse), `register` handler (vendor-root 503 pattern).
- `crates/wipe-hub/src/auth.rs:256` `CurrentUser` (`role`, `organization_id`, `user_id`), `actor(user_id)`, `:540` `lock_summary_action` (+ unit test ~`:1490`).
- `crates/wipe-hub/src/throttle.rs:82-89` enroll consts; module doc lists key families.
- `crates/wipe-hub/migrations/0019_enrollments.sql` -- read-only: approval CHECK needs all five approval columns; trigger allows pending→approved/denied only.
- `crates/wipe-hub/tests/enrollments.rs` -- helpers `fixture`, `buy`, `start`, `poll_error`, `status_of`, `org_of`; story 1 approves by SQL — new tests approve via the route.
- `crates/wipe-hub/tests/isolation.rs:39` `ROUTES` (classify the three as `Customer`), `:530` sweep bodies (add `{code}` / `{code, entitlement_id: b_entitlement}`).
- `apps/portal/src/main.tsx` -- routes; `signInRoute` needs `validateSearch` for `next`; add `/activate` with `{code?}`.
- `apps/portal/src/pages/SignInPage.tsx` -- navigate to safe `next`.
- `apps/portal/src/pages/StationsPage.tsx:44` `seatChoices`, `preselected`, `stationName`; `AccountPage.tsx:37` `SignInPrompt`; `AuthForm.tsx` `Field`, `FormError`.
- `apps/portal/src/api/stations.ts` -- client pattern (`request`, `ensureOk`, `json`, guards, `isStation`, `isLicence`); `api/auth.ts` `HubError`, lock-minutes helper.
- `apps/portal/src/pages/StationsPage.test.ts` / `api/stations.test.ts` -- test style (renderToStaticMarkup, fetch stubs).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/src/throttle.rs`, `auth.rs` -- `ENROLL_USER_KEY_PREFIX`, `ENROLL_LOOKUP_LIMIT/WINDOW/LOCKOUT`; map `enroll:user:` → `enrollment.lookup_lock_summary` first; unit test.
- `crates/wipe-hub/src/enrollments.rs` -- `PendingEnrollment`, `find_pending`, `approve` (returns `Result<Registered, StationError>`-compatible error), `deny`; docs.
- `crates/wipe-hub/src/enrollment_routes.rs` -- three handlers with the Always order; per-user charge/refund/confirm helper; messages.
- `crates/wipe-hub/tests/enrollments.rs` (or new `tests/enrollment_approval.rs`) -- every matrix row, plus: approval then poll returns a licence verifying against the root for the enrollment's key; refusal leaves `pending`; audit rows and details; throttle lock + refund of right codes; CSRF token required.
- `crates/wipe-hub/tests/isolation.rs` -- classify and sweep the three routes.
- `apps/portal/src/api/enrollments.ts` (+ `.test.ts`) -- `PendingEnrollment`, guard, `lookupEnrollment`, `approveEnrollment`, `denyEnrollment`; header comment naming the Rust mirror.
- `apps/portal/src/pages/ActivatePage.tsx` (+ `.test.ts`) -- page and exported pure panels for each state (signed out, not owner, entry, details with choices / no free seat, approved, denied, error).
- `apps/portal/src/pages/SignInPage.tsx`, `src/main.tsx` -- `safeNext` (exported, tested), `next` search, `/activate` route; link to `/activate` from the owner's Stations page.
- `CONTEXT.md` (§10.2 row: story 2 row or update story 1 row's limitation about `/activate`; §14 **Enrollment**), `README.md` (hub row, test counts) -- document routes, throttle, audit.

**Acceptance Criteria:**
- Given Postgres up and a hub with a vendor root, when `cargo test -p wipe-hub` runs, then every matrix row passes over real HTTP and `tests/isolation.rs` passes with the routes classified.
- Given a station that started an enrollment, when an owner approves its code through `POST /api/enrollments/approve`, then the station's next poll returns `{station, licence}` whose licence verifies with `wipe_license` against the hub root and names the station's instance key.
- Given the portal, when `pnpm --filter portal typecheck && pnpm --filter portal test` run, then they pass, covering the page states and `safeNext`.

## Spec Change Log

## Review Triage Log

### 2026-11-10 — Review pass
- verdicts: 24 findings — high 0, medium 3, low 17, false 4, maybe-false 0
- findings:
  - `[medium]` `[defer]` (blind) Approval just before expiry takes a seat and issues a licence the station's poll then refuses as `expired_token` — real, but it is story 1's documented limitation, and the fix changes poll (excluded by Never) or adds an approval-time rule; recorded in `deferred`.
  - `[low]` `[reject]` (blind) Unverified owners can look up and deny, and the throttle is per user, not per IP — `POST /api/stations` has no verified-email check either, so this matches existing registration; guessing a live code among ~2.6×10^10 at 10 per 15 min per account (and sign-up is itself rate-limited) is impractical; the fix adds a guard beyond the intent's owner rule.
  - `[medium]` `[patch]` (blind) Handlers hold a transaction's connection while `match_code` takes a second from the pool, so concurrent requests up to the pool size (sqlx default 10) wait for each other for the 30 s acquire timeout and starve the whole hub — fixed: `match_code` takes the transaction and refunds or confirms on that one connection, and approve runs `enrollments::approve` in a savepoint, so a refused approval rolls back to it and still commits the refund.
  - `[low]` `[reject]` (blind) Deny is one click with no confirmation — the station can simply start again; adding a confirmation step is new UI for a rare slip.
  - `[low]` `[reject]` (blind) Lockout audit rows carry no peer address — the actor is the signed-in user id, which attributes better than an IP; adding the peer needs a new parameter threaded through.
  - `[false]` `[reject]` (blind) A concurrent approve/deny race reaches the `rows_affected() != 1` branch and returns a misleading 5xx — `find_pending` selects `WHERE status = 'pending' … FOR UPDATE`; under READ COMMITTED a waiting second request re-checks the row after the first commits and gets no row, so it answers the 404 and the branch is unreachable.
  - `[low]` `[patch]` (blind) New throttle constants and the `enroll:user:` prefix are not pinned by a unit test — added `throttle::tests::enroll_lookup_constants` (figures, window ≤ `PURGE_AFTER`, prefix under `enroll:` and clear of the other families).
  - `[low]` `[reject]` (blind) After a failed approve/deny the page keeps stale seat data and active buttons — the hub's error is shown and re-pressing gives the same answer; refetch-on-error logic is extra code for a rare case.
  - `[low]` `[reject]` (blind) Only password sign-in keeps `next`; sign-up, magic link and reset drop the code — the intent's autonomous decision chose password sign-in only; a fresh sign-up is a new organization, not the station's owner.
  - `[low]` `[patch]` (blind) `ActivatePage`'s own branching is untested — grouped with the verification-gap finding below; fixed there.
  - `[low]` `[patch]` (blind) The not-owner message tells vendor admins to ask an owner of "your organization" — reworded to "an owner of the organization the station belongs to", correct for members and vendor admins.
  - `[low]` `[patch]` (blind) The isolation test comment claims a 404 it does not assert — comment corrected to say the sweep tries the approval, `tests/enrollment_approval.rs` pins the 404, and the final `denied` status shows nothing was registered.
  - `[medium]` `[patch]` (edge) Pool starvation from a second connection while the transaction is held — same root cause as the blind finding above; same fix.
  - `[low]` `[reject]` (edge) A second owner acting on a code another owner just decided is charged one failure — rare, costs one of ten, and telling it apart needs a new branch and query.
  - `[low]` `[reject]` (edge) A database error between the charge and the refund/confirmation leaves the charge counted — needs a database failure mid-request; per-error refund branches add complexity for a negligible case.
  - `[low]` `[reject]` (edge) Malformed codes typed on the page use up throttle attempts; validate the shape client-side — the intent counts malformed codes as failures; client validation is a new guard and duplicates the hub's alphabet rule in TypeScript.
  - `[low]` `[patch]` (verification-gap) `ActivatePage` never rendered, so its signed-out / not-owner / owner choice is untested — added an `ActivatePage` describe block that seeds `["me"]` (null, member, vendor admin, owner) and asserts the sign-in link with `next`, the owner-only alert with no code field or form, and the owner's field prefilled from `code`.
  - `[low]` `[patch]` (verification-gap) Sign-in's use of `?next=` is untested — the route's search parser is now `signInSearch` in `SignInPage.tsx`, used by `main.tsx` and unit-tested; the post-sign-in `navigate` still needs a DOM harness the portal lacks.
  - `[low]` `[patch]` (verification-gap) No test that an owner's malformed body is refused before the charge — added `an_owners_malformed_body_is_400_and_charges_nothing` (all three routes, no throttle row afterwards).
  - `[low]` `[patch]` (intent-alignment) The portal's assembled behaviour (panel choice, prefill) is tested only through isolated panels — same root cause as the `ActivatePage` gap; same fix.
  - `[false]` `[reject]` (intent-alignment) The `rows_affected != 1` branch is unspecified and untested — unreachable for the `FOR UPDATE` re-check reason above.
  - `[false]` `[reject]` (intent-alignment) The not-owner state still sends `GET /api/auth/me` — "no request sent" means no enrollment request; the page must read the session to know the role, and no enrollment call is mounted for non-owners (now tested).
  - `[false]` `[reject]` (intent-alignment) The throttle could mean the 11th wrong code is itself counted — the matrix row "11th wrong code in 15 min → 429" is exactly what is built: the 10th failure locks and the 11th request is 429.
  - `[low]` `[reject]` (intent-alignment) Nothing compares the TypeScript `PendingEnrollment` with the Rust one automatically — AGENTS.md makes these hand-mirrored by convention; the hub test pins the seven keys and the guard checks the same seven.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass (story 1 noted `tests/health.rs::silent_smtp_and_database_time_out_within_bound` failing on this machine at baseline).
- `cargo clippy -p wipe-hub --all-targets -- -D warnings` -- expected: clean.
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: pass.
- `rustfmt --edition 2021 <touched .rs files>` -- touched files only.

## Auto Run Result

Status: done

**Summary.** Organization owners can now finish a station's enrollment. The hub has three customer-session routes (`POST /api/enrollments/lookup`, `/approve`, `/deny`), with the usual CSRF token and no caching. Members and vendor admins get 403. Approve without a vendor root is 503. Every code that names no pending, unexpired enrollment gets the same 404. Code entry is throttled per user (10 wrong codes in 15 minutes, then locked for 15 minutes, with the lock audited). Approval registers the station through `stations::register` in the owner's organization, marks the enrollment approved and audits `enrollment.approved`. Any refusal from `register` comes back unchanged and leaves the enrollment pending. Denial audits `enrollment.denied`. The portal has a new `/activate?code=…` page:
- signed out, it links to sign-in with `next` set, so the owner comes back to the page;
- for a member or vendor admin, it says who can approve and offers nothing to send;
- for an owner, it shows the code entry, the station's details, a seat choice, and Approve or Deny.

Password sign-in follows `?next=` only for same-origin paths, and `/stations` links owners to `/activate`.

**Files changed.**
- `crates/wipe-hub/src/enrollments.rs` — `PendingEnrollment`, `find_pending` (`FOR UPDATE`), `approve`, `deny`, the three messages, module docs.
- `crates/wipe-hub/src/enrollment_routes.rs` — the three owner routes in the required order: owner check, then (approve only) vendor root, then body, throttle charge and code. The per-user charge, refund and confirmation run on the handler's single connection, and approval runs in a savepoint.
- `crates/wipe-hub/src/throttle.rs` — `ENROLL_USER_KEY_PREFIX` and the `ENROLL_LOOKUP_*` figures, with a unit test.
- `crates/wipe-hub/src/auth.rs` — `enroll:user:` maps to `enrollment.lookup_lock_summary`, checked before the start prefix; unit test.
- `crates/wipe-hub/tests/enrollment_approval.rs` — 13 HTTP tests covering every matrix row, plus the licence collected by poll and verified against the root, CSRF, and an owner's malformed body costing no charge.
- `crates/wipe-hub/tests/isolation.rs` — the three routes classified as `Customer` and swept.
- `apps/portal/src/api/enrollments.ts` (+ test) — the TypeScript mirror of `PendingEnrollment`, its guard, and the lookup, approve and deny clients.
- `apps/portal/src/pages/ActivatePage.tsx` (+ test) — the page and its panels, with the whole page rendered for each kind of account.
- `apps/portal/src/pages/SignInPage.tsx`, `src/main.tsx` — `safeNext` and `signInSearch`, the `next` search parameter, and the `/activate` route.
- `apps/portal/src/pages/StationsPage.tsx` — `ActivateLink` for owners.
- `apps/portal/src/api/auth.ts` — exports `waitFor`.
- `CONTEXT.md` — a §10.2 row for story 2, story 1's limitation updated, and §14 **Enrollment** extended.
- `README.md` — the hub and portal rows, and the test counts.

**Review findings.** 24 findings: 0 high, 3 medium, 17 low, 4 false.

Patches applied, 7 entries (1 medium, 6 low):
- **Pool starvation (medium).** A handler held one connection while waiting for a second, which could starve the pool. Fixed by doing all the work on one connection, with a savepoint for approval.
- **`ActivatePage` panel choice untested.** Fixed with whole-page render tests.
- **`?next=` parsing untested.** Fixed by adding `signInSearch` with a unit test.
- **Malformed owner body.** Added a test that it costs no charge.
- **Throttle constants.** Added a test that pins them.
- **Vendor-admin wording.** Fixed the not-owner message's wording for vendor admins.
- **Isolation test comment.** Corrected it to match what the test asserts.

Deferred, 1 (medium): an approval made just before the code expires takes a seat that the station cannot collect. This is story 1's documented limitation, and fixing it needs a change to poll, which this story excludes. It is recorded in `deferred`.

Rejected, each with its reason in the triage log:
- low:
  - unverified owners can look up and deny (this matches `POST /api/stations`);
  - Deny has no confirmation step;
  - lockout audits carry no network address;
  - the page shows stale data after a failed approve or deny;
  - `next` survives only password sign-in;
  - a second owner on an already-decided code is charged one failure;
  - a database error mid-request leaves the charge counted;
  - no client-side check of the code's format;
  - no automatic comparison of the TypeScript mirror with the Rust shape;
- false:
  - the `rows_affected` race branch (×2; unreachable because of the `FOR UPDATE` re-check);
  - `GET /api/auth/me` counting as a request for non-owners;
  - the throttle-count reading.

**Follow-up review recommendation: false.** One medium entry and no high entry was patched, which is below the threshold. The one medium fix is checked by the whole enrollment-approval suite (refusals still leave the enrollment pending, and right codes are still refunded).

**Verification.**
- `cargo test --workspace --no-fail-fast` before the patches: 992 passed, 1 failed. The failure is `tests/health.rs::silent_smtp_and_database_time_out_within_bound`, which already failed on this machine at baseline (noted in story 1).
- After the patches, the rerun was stopped at the 10-minute limit while it was on `wipe-hub`'s `tests/team.rs`, because the machine was slow. I ran `team.rs` and the doctests separately, and they pass. `wipe-hub` now passes 593 tests (175 unit, 418 integration), again apart from that `health.rs` test. The patches touched only `wipe-hub`, which no other crate depends on.
- `cargo clippy -p wipe-hub --all-targets -- -D warnings`: clean.
- `rustfmt --edition 2021 --check` on the touched `.rs` files: clean.
- `pnpm --filter portal typecheck`, `test` (265 passed) and `build`: pass.

**Residual risks.**
- The `/activate` page has not been tried in a real browser: only static render tests cover it, and the redirect after sign-in is not run.
- The late-approval case above is deferred.
- No station sends start or poll yet (stories 3 and 4).

