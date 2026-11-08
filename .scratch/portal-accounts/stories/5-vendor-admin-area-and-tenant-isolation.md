---
title: 'Vendor admin area and tenant isolation'
type: 'feature'
created: '2026-11-08'
baseline_revision: '8a9834b4c5711fd88a2f743911a38aaa4a7b4e57'
status: 'done'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md']
warnings: ['oversized']
deferred:
  - summary: >-
      The admin pages (search on submit only, the vendor-admin-only home link, cache dropped on unmount) have no component tests; only the API client is tested.
    evidence: |-
      Only apps/portal/src/api/*.test.ts exist; the portal has no component-test setup (story 2, 3 and 4 deferrals). The hub enforces access and audits every read regardless, covered over HTTP by tests/admin.rs.
    location: >-
      apps/portal/src/pages/AdminPage.tsx
    severity: low
---

<intent-contract>

## Intent

**Problem:** The hub has a `vendor_admin` role but no way to create one, no admin area and no proof that tenants are isolated. Vendor staff cannot find a customer to help them, and nothing shows that organization A can never read organization B's records (SPEC CAP-5, CAP-6; ROADMAP §2.2 "vendor supertenant as an explicit, audited escalation, not an ambient superuser").

**Approach:** Mark one organization as the **vendor organization** (migration 0007). A CLI command, `wipe-hub admin create --email`, creates `vendor_admin` users in it and emails them a set-password link. Add read-only `/api/admin/*` routes that only a vendor admin can reach. They search customer organizations and users and show one organization with its users. Every one of these reads writes an audit row in the same transaction, through a separate admin data layer. Add portal pages for them, and an HTTP isolation test over every customer route, guarded by a check that every `/api` route is classified.

## Boundaries & Constraints

**Always:**
- **Vendor organization.** Migration `0007_vendor_organization.sql` adds `organizations.is_vendor boolean NOT NULL DEFAULT false`, with a partial unique index so at most one row has `is_vendor`. Sign-up and invitations never create or join it. Its name is `Vendor staff`; it is created on the first `admin create`.
- **Who is a vendor admin.** A session counts as a vendor admin only when all three hold, read live in one query: the user's role is `vendor_admin`, their organization `is_vendor`, and their email is verified. Otherwise every `/api/admin/*` route answers 403 `only vendor admins can use the admin area`. This covers owners, members, and a `vendor_admin`-role user in a customer organization. With no session the answer is 401 `not signed in`. The check runs before any query parameter is read or any data is touched.
- **`wipe-hub admin create --email <address> [--name <name>]`**, with `--database-url`, `--smtp-host`, `--smtp-port`, `--mail-from` and `--public-base-url`. These use the same `WIPE_HUB_*` env twins and defaults as `serve`.
  1. Validate first, before connecting: `validate_email`, and `validate_name`. The name defaults to the email's local part.
  2. Apply migrations, as `serve` does.
  3. One transaction:
     - get or create the vendor organization (a concurrent run must not create a second one or fail);
     - insert the user with role `vendor_admin`, the `UNUSABLE_PASSWORD_HASH` and an unverified email;
     - create a `password_reset` token (`PASSWORD_RESET_TTL`);
     - audit `admin.created`: actor `cli:admin-create`, subject `user:<id>`, detail `{email, organization_id, vendor_organization_created, os_user}`. `os_user` comes from `$USER` or `$USERNAME`, or is null; it is informational only.
  4. After commit, send the existing password-reset email with `{public_base_url}/reset-password?token=…`.
  5. Print `created vendor admin <email> (<id>)` and whether the link was emailed. Exit 0, also when SMTP is down: print a warning that the admin should use "Forgot password" on the portal.
  6. Errors exit non-zero with a message on stderr and write nothing:
     - the address belongs to a customer: `an account with this email already exists`;
     - the address is already a vendor admin: `<email> is already a vendor admin; use Forgot password on the portal`;
     - validation fails, or Postgres is unreachable.
  7. Never print, log or audit the raw token.
- **Admin data layer** (`admin.rs`) is the only code that reads across organizations. It excludes the vendor organization and `vendor_admin` users from every result. Each function takes a `&VendorAdmin` (constructible only by the extractor) and a transaction, and writes its own audit row (actor `user:<admin id>`) before returning data. So no admin read can return data without its audit row, and if the audit write fails the request is a 500 that returns no data.
- **Routes** (all GET):
  - `GET /api/admin/organizations?q=` → `{organizations: [{id, name, created_at, user_count}], truncated}`.
    - Matches: case-insensitive substring of the organization's name, or of any of its users' emails or names.
    - Order: `lower(name)`, then id. Limit 100; `truncated` is true when more matched.
    - Audit `admin.organizations_searched`, subject `search:organizations`, detail `{query, organization_ids}` (the ids returned).
  - `GET /api/admin/users?q=` → `{users: [{id, email, name, role, email_verified, created_at, organization: {id, name}}], truncated}`.
    - Matches: email or name.
    - Order: `lower(email)`. Limit 100.
    - Audit `admin.users_searched`, subject `search:users`, detail `{query, user_ids}`.
  - `GET /api/admin/organizations/:id` → `{organization: {id, name, created_at}, users: [{id, email, name, role, email_verified, created_at}]}`, users ordered by name then email.
    - Audit `admin.organization_viewed`, subject `organization:<id>`, detail `{user_ids}`.
    - A non-UUID, unknown or vendor organization id is 404 `no such organization` and writes no audit row.
  - `q` is trimmed. More than 200 characters, or any control character, is 400 `query must be at most 200 characters without control characters`. An empty `q` matches everything. LIKE wildcards (`%`, `_`, `\`) in `q` are escaped, so they match literally.
  - Timestamps are RFC 3339 UTC strings formatted in SQL, as `team.rs` does.
- **Isolation test** (`tests/isolation.rs`):
  - Fixture: two organizations, A and B, each with a verified owner, a member and a pending invitation.
  - For every route that needs a customer session, call it as A's owner and as A's member. Path ids are B's user and invitation ids. Bodies are valid, and name B's addresses where a body takes an email.
  - Assert that no response body contains any of B's identifiers (organization id, organization name, user ids, emails, names, invitation ids) other than a value the request itself carried. For example, an invite to B's member's address may echo that address back.
  - Assert that B's users, roles, names, organization name and invitations are byte-for-byte unchanged afterwards.
  - Completeness guard: a second test scans `crates/wipe-hub/src/*.rs` for string literals beginning `"/api`. It fails if any is missing from the test's classification table (`customer`, `anonymous` with a one-line reason, `admin`, `catch-all`), or if the table lists a path the source no longer has.
- **Portal:**
  - `/admin`: two search forms, organizations and users. A search runs only on submit, never per keystroke or on page load, because each search is an audited view. Results are tables, and an organization row links to its detail page.
  - `/admin/organizations/$id`: the organization and its users.
  - Home links to `/admin` when `role === "vendor_admin"`. A 403 or 404 from the hub is shown as its message.
  - Admin queries are keyed `["admin", …]`, with no refetch on focus or retry on 4xx.
  - Hidden links are a convenience only; the hub enforces.

**Never:**
- No `sqlx::query!`. Do not edit migrations 0001–0006, and add no new crate dependency.
- No web form or API route creates, promotes or demotes a `vendor_admin`. `admin create` never converts an existing customer user.
- No admin route writes customer data. This story ships views only.
- Never let a customer route take an organization id from the request. Never port the admin check into the portal beyond hiding links.
- Not in this story:
  - admin actions on customer accounts (send a reset, resend verification, change roles, disable);
  - removing or listing admins through the CLI;
  - MFA or a shorter session for admins;
  - pagination beyond the first 100 results;
  - offerings, orders and licences in the admin area;
  - portal component tests (no component-test setup exists).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Bootstrap admin | fresh DB, `admin create --email X` | exit 0; vendor org `Vendor staff` (`is_vendor`) + `vendor_admin` user; one Mailpit reset mail to X; `admin.created` `{vendor_organization_created:true}` | — |
| Set password and sign in | X completes the emailed reset, signs in | `me` 200 role `vendor_admin`, `email_verified:true`; `GET /api/admin/organizations` 200 | — |
| Second admin | `admin create --email Y` | same vendor org reused (`vendor_organization_created:false`); still one `is_vendor` row | — |
| Concurrent bootstrap | two `admin create` at once on a fresh DB | both exit 0; exactly one vendor org | — |
| Existing admin | `admin create --email X` again | non-zero, "already a vendor admin"; no new row, no mail | — |
| Existing customer | address of an organization owner | non-zero, `an account with this email already exists`; user unchanged (still `owner`, same org) | — |
| Bad input | `--email nope` or a control-char `--name` | non-zero naming the problem; nothing written | — |
| SMTP down | `--smtp-port` closed | exit 0, admin created, warning mentions Forgot password | — |
| Customer roles refused | owner and member of A call each admin route | 403 admin message; no audit row | anonymous → 401 |
| Unverified admin | `vendor_admin` in vendor org, email unverified, session forced via DB | 403 | — |
| Misplaced role | `vendor_admin`-role user in a customer org (story-4 fixture shape) | 403 on every admin route | — |
| Search organizations | orgs `Acme`, `Beta`; admin `q=acm` | only Acme, with `user_count`; `admin.organizations_searched` `{query:"acm", organization_ids:[Acme]}` | — |
| Search by member email | `q=` part of Beta member's address | Beta listed | — |
| Wildcards literal | `q=%` with no `%` in any name | empty list | — |
| Bad query | 201-char `q`, or `q` with `\n` | 400; no audit row | — |
| Search users | `q=` an email fragment | matching customer users with organization; no `vendor_admin` ever listed; `admin.users_searched` with `user_ids` | — |
| Truncation | 101 matching organizations | 100 returned, `truncated:true` | — |
| View organization | admin opens Beta | org + users; `admin.organization_viewed` subject `organization:<Beta>` `{user_ids}` | unknown/non-UUID/vendor org id → 404, no audit |
| Isolation | A's owner and member over every customer route with B's ids | no B identifier in any body; B unchanged | — |
| Route guard | a new `"/api/…"` literal added to `src` without classifying it | completeness test fails naming it | — |
| Success signal | two orgs sign up; admin bootstrapped by CLI views both | audit log holds both `auth.sign_up`, `admin.created`, the admin's `auth.sign_in`, both `admin.organization_viewed`, in order | — |

## Autonomous decisions

Recorded under the owner's "decide by default" rule (2026-11-08). None is product, pricing, legal or ADR-contradicting; wording follows story 3 decision 7 (functional, rewordable).

1. **The vendor organization is a flagged organization row.** *About:* ADR-0007 §3 puts `vendor_admin` users "on the vendor organization", but nothing marks one. *Options:* (a) an `is_vendor` flag with a one-row unique index; (b) a configured organization id; (c) a separate staff table. *Choice:* (a). It keeps one user table and one sign-in (ADR-0006 §1, one portal for both audiences), and the database refuses a second vendor organization. The admin check requires both the role and the flag, so a mis-assigned role in a customer organization reaches nothing.
2. **`admin create` emails a set-password link instead of taking a password.** *About:* vendor staff bootstrapping the first admin from a shell. *Problem:* a password passed as a flag lands in shell history and process lists, and an interactive prompt needs a new dependency and cannot be tested unattended. *Options:* (a) email the existing single-use, 1-hour reset link; (b) a `--password` flag; (c) read the password from stdin. *Choice:* (a). No password ever passes through the shell. Following the link proves the mailbox (story 3 decision 4). If SMTP is down, the admin uses "Forgot password" later. The email is the existing reset email, so it says "reset", not "set"; that is functional and can be reworded later.
3. **The admin area is read-only in this story.** *About:* CAP-6 says admins "see and help" customers. *Problem:* each power over a customer's account (sending them a reset, changing a role, disabling) needs its own rules and audit shape, which would widen the story. *Choice:* views and search only. Admins help by finding the customer's organization, users, roles and verification state; customers already self-serve resets. Every future admin action plugs into the same audited data layer. Listed as a CONTEXT limitation.
4. **Every admin read is audited, including searches, and records which records were returned.** *Problem:* CAP-6 says every admin view of a customer record is audited, and a search result page shows records. *Choice:* one row per request, holding the query and the returned ids, written in the read's transaction. An unknown id writes nothing, because nothing was seen. Searches run only on submit, so audit volume follows what the admin actually does.
5. **Vendor staff are excluded from admin results.** The admin area is for customers; staff are managed through the CLI. This also keeps the vendor organization out of customer-facing lists.
6. **Admins must have a verified email.** This is defence in depth: the CLI creates admins with an unusable password, so in practice they get in only through a mailbox link.
7. **The isolation test is guarded by a route-classification check.** *Problem:* "any customer route" stays true only if new routes are tested too. *Choice:* scan the source for `/api` path literals and fail on any unclassified one. A new route then forces a decision in the test file.

</intent-contract>

## Code Map

- `crates/wipe-hub/migrations/0007_vendor_organization.sql` (new): `ALTER TABLE organizations ADD COLUMN is_vendor boolean NOT NULL DEFAULT false;` `CREATE UNIQUE INDEX organizations_one_vendor ON organizations ((true)) WHERE is_vendor;`. Use a header comment in the style of 0003–0006.
- `crates/wipe-hub/src/config.rs` (~l.46 `Args`): add `AdminCreateArgs` (`clap::Parser`) with `email`, optional `name`, and the five connection and mail fields. Copy their `#[arg(long, env = …, default_value = DEFAULT_…)]` exactly. Validate `mail_from` and `public_base_url` with the existing parsers (`parse_base_url` ~l.277, and the `Mailbox` parse used by `from_args` ~l.135).
- `crates/wipe-hub/src/main.rs`: `Cmd::Admin { #[command(subcommand)] cmd: AdminCmd }` with `AdminCmd::Create(AdminCreateArgs)`, calling `wipe_hub::admin::create_vendor_admin`. Print outcomes; on error, print to stderr and exit 1 (`anyhow` already does this).
- `crates/wipe-hub/src/admin.rs` (new, `pub mod admin` in `lib.rs`):
  - `VendorAdmin { user_id }`, whose fields are private outside the crate;
  - `VENDOR_ORGANIZATION_NAME = "Vendor staff"`;
  - `create_vendor_admin(args) -> Result<CreatedAdmin>`, which does validation, `db::migrate_url`, the transaction (vendor org get-or-create via `INSERT … ON CONFLICT ((true)) WHERE is_vendor DO NOTHING` then `SELECT`; the user insert maps `users_email_lower_key` to the two duplicate errors by looking up the existing role first), `accounts::create_token(…, PURPOSE_PASSWORD_RESET, PASSWORD_RESET_TTL)`, `audit::record`, and the send through `Mailer::new(...).send_password_reset(to, &portal_link(base, "reset-password", token))`;
  - `search_organizations`, `search_users`, `organization_detail`, each `(conn, &VendorAdmin, …)` and each recording its audit row;
  - `escape_like(q)` and `validate_query(q)`, pure and unit-tested.
- `crates/wipe-hub/src/admin_routes.rs` (new; merged in `lib.rs` `router()` next to `team_routes::routes()`, so the CSRF layer and the `/api/*rest` 404 still cover it):
  - a `VendorAdmin` extractor (`FromRequestParts`, as `CurrentUser` does at `auth.rs` ~l.270): resolve the session with `accounts::session_user`, then one query checking `role = 'vendor_admin' AND o.is_vendor AND u.email_verified_at IS NOT NULL`;
  - `MSG_ADMIN_ONLY` and `MSG_NO_ORGANIZATION`;
  - three handlers using `Query<SearchParams { q: Option<String> }>` (the `q` extraction is after the extractor in argument order) and a path id via a `path_id`-style 404;
  - each opens a tx, calls `admin::…`, commits, and returns JSON.
- `crates/wipe-hub/src/auth.rs`: `MSG_NOT_SIGNED_IN`, `session_token`, `ApiError` and `actor` are reused. Make `portal_link` reachable from `admin.rs` (it is already `pub`).
- `crates/wipe-hub/src/mail.rs` ~l.73 `send_password_reset`: reuse as is.
- `crates/wipe-hub/tests/support/hub.rs` (new, `pub mod hub;` from `support/mod.rs`, `#![allow(dead_code)]`): move a copy of `tests/team.rs`'s harness here (`spawn_hub`, CSRF-aware `post`/`get`, `sign_up`, `sign_in`, `verified_owner`, `invite`, `accept`, `cookie_pair`, `audited`, `assert_error`). Leave `tests/team.rs` and `tests/auth.rs` untouched.
- `crates/wipe-hub/tests/admin.rs` (new):
  - CLI tests run `env!("CARGO_BIN_EXE_wipe-hub")` with every `WIPE_HUB_*` var removed (pattern in `tests/cli.rs` ~l.10), `--database-url <TestDb.url>`, an `--email unique_address()`, and a `--public-base-url` matching the spawned hub;
  - read the reset mail with `Mailpit::wait_for_message_to` and complete it via `POST /api/auth/password-reset/complete`;
  - one test per matrix row, including the success-signal test;
  - force an unverified admin session with `UPDATE users SET email_verified_at = NULL`.
- `crates/wipe-hub/tests/isolation.rs` (new): the fixture, the route table `const ROUTES: &[(&str /*method*/, &str /*path*/, Class)]`, the sweep and the completeness guard. Read sources with `std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))`; axum 0.7 path params are `:id`.
- `crates/wipe-hub/tests/migrations.rs`: check that 0007 applies and that a second `is_vendor = true` row violates `organizations_one_vendor` (existing pattern).
- `crates/wipe-hub/tests/cli.rs`: `wipe-hub admin create` with no `--email` exits non-zero (no DB needed).
- `apps/portal/src/api/admin.ts` (new) and `admin.test.ts` (the `fakeHub`/`respond` pattern from `team.test.ts`): types `AdminOrganization`, `AdminUser`, `AdminOrganizationDetail`, guards, plus `searchOrganizations(q)`, `searchUsers(q)` and `fetchOrganization(id)`, using `request`/`ensureOk`/`json` exported from `api/auth.ts`. Include a mirror comment pointing at `admin_routes.rs` and `admin.rs`.
- `apps/portal/src/main.tsx`: routes `/admin` and `/admin/organizations/$id`.
- `apps/portal/src/pages/AdminPage.tsx`, `AdminOrganizationPage.tsx` (new): reuse `Field`, `FormError` and `SubmitButton` from `AuthForm.tsx`, and `SignInPrompt` from the existing pages. In `HomePage.tsx`, `SignedIn` adds the admin link.
- `CONTEXT.md`:
  - §10 l.693: drop "No `vendor_admin` creation or admin area (story 5)".
  - Add a story-5 row with these limitations: views only, no admin actions; no CLI to list or remove admins (deleting the user row by SQL ends their sessions by cascade); admins have the same session lifetime as customers and no MFA; search queries and returned ids stay in the append-only audit log; results are capped at 100 with no paging; substring search scans tables; the set-password email reads as a reset.
  - §14: add **Vendor organization** and **Admin area**, and extend **Hub user role**.
- `docs/ROADMAP.md` l.318 (A1.3): append "*Customers part shipped 2026-11-08: search and audited views (portal-accounts story 5); licences and entitlements to come.*"
- `README.md` l.48 and l.50: `wipe-hub admin create`, the admin area, and the isolation test.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0007_vendor_organization.sql` -- `is_vendor` + one-vendor index -- identifies the vendor tenant.
- `crates/wipe-hub/src/admin.rs`, `src/config.rs`, `src/main.rs`, `src/lib.rs` -- the audited cross-org data layer, `VendorAdmin`, and the `admin create` command -- the only cross-tenant path is explicit and audited.
- `crates/wipe-hub/src/admin_routes.rs`, `src/lib.rs` -- the extractor and three routes.
- `crates/wipe-hub/tests/support/hub.rs`, `tests/admin.rs`, `tests/isolation.rs`, `tests/migrations.rs`, `tests/cli.rs` -- one test per matrix row, the isolation sweep and the route guard.
- `apps/portal/src/api/admin.ts`, `admin.test.ts`, `main.tsx`, `pages/AdminPage.tsx`, `pages/AdminOrganizationPage.tsx`, `pages/HomePage.tsx` -- client and pages.
- `CONTEXT.md`, `README.md`, `docs/ROADMAP.md` -- shipped behaviour, glossary and limitations, in this commit.

**Acceptance Criteria:**
- Given an owner or member session, when it calls every `/api/admin/*` route, then each is 403 and the audit log gains no row.
- Given a vendor admin, when they search and view, then each 200 response has exactly one matching audit row, and that row's ids equal the ids in the body.
- Given the workspace, when `cargo test --workspace`, clippy, and the portal `typecheck`/`test`/`build` run, then all pass.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 24 findings — high 0, medium 2, low 19, false 3, maybe-false 0
- findings:
  - `[medium]` `[patch]` (blind) Re-submitting the same first search after a remount reuses the cached result (same `{q, n:1}` key, `staleTime: Infinity`), so the view writes no audit row — Verified in `AdminPage.tsx`. Fixed: `gcTime: 0` on every `["admin", …]` query, so results are dropped on unmount.
  - `[medium]` `[patch]` (blind) After sign-out, cached admin data stays for ~5 min and the next user in the same tab can see it beside the hub's 403 — Same root cause (admin results outlive their page); same fix.
  - `[low]` `[reject]` (blind) The route guard compares paths, not methods, so a new method on an already classified path is not swept — Real, but needs parsing the method chain next to each literal; a new method on an existing path is reviewed in the same `ROUTES` table, and the hub still scopes every query.
  - `[low]` `[reject]` (blind) The guard scans only top-level `src/*.rs` and only literal paths — No route lives in a subdirectory or is built with `format!` today; recursing guards a layout that does not exist.
  - `[low]` `[patch]` (blind) The isolation snapshot leaves out B's sessions and tokens, so a route that ended B's sessions would pass — Verified in `rows_of`. Fixed: B's `sessions` and `auth_tokens` rows are compared before and after.
  - `[low]` `[patch]` (blind) The vendor organization has no owner, contradicting CONTEXT §14 "an organization always keeps at least one owner" — Verified. Fixed: §14 says the rule applies to customer organizations; the vendor organization has only CLI-managed `vendor_admin` users. The rest of the finding (a vendor admin reading their own organization's staff list through `/api/organization/members`) is not cross-tenant and is not a customer record.
  - `[low]` `[reject]` (blind) Retrying 5xx and network errors can write a second audit row when a response is lost after commit — Each retry really read the data on the hub, so the extra row is accurate; the log errs toward over-recording.
  - `[low]` `[reject]` (blind) Going back from an organization to `/admin` loses the search — By design: showing results again is a new audited view.
  - `[low]` `[reject]` (blind) `AdminCreateArgs` copies five flags from `Args` instead of `#[command(flatten)]` — Flattening changes `Args`, whose literal initializers sit in several tests; `admin_create_shares_serves_connection_flags` pins the copies together.
  - `[low]` `[reject]` (blind) "1 hour" in the CLI output and "200 characters" in `MSG_BAD_QUERY` are literals beside their constants — Drifts only if a constant changes (same call as stories 3 and 4); tests pin the texts.
  - `[low]` `[reject]` (blind) The organization page sorts users by raw `name` while searches use `lower()` — It matches the spec ("by name then email") and the customer member list in `team::members`.
  - `[low]` `[reject]` (edge) `GET /api/admin/organizations/%FF` gets axum's plain-text 400, not the JSON 404 — Only a hand-crafted invalid-UTF-8 path reaches it; the team routes behave the same.
  - `[low]` `[reject]` (edge) An admin demoted between the extractor's check and the handler's transaction still gets one read — Millisecond window for a CLI/SQL-only change, and that read is audited; the same call was made in story 4.
  - `[low]` `[reject]` (edge) Retries write duplicate audit rows — Same root cause and reason as the blind retry row.
  - `[low]` `[reject]` (edge) Unicode format characters (bidi, zero-width) in `q` are stored in the audit log — The admin is a trusted, authenticated actor and the query is stored as a JSON string; rejecting general categories needs Unicode tables std does not provide.
  - `[low]` `[reject]` (edge) The guard misses subdirectories and `nest()` — Same as the blind top-level-scan row.
  - `[low]` `[reject]` (edge) The guard ignores methods, against the "every route classified" claim — Same as the blind method row.
  - `[low]` `[patch]` (verification-gap) Searching by a person's name is untested in both searches — Pre-verified. Fixed: `both_searches_match_a_persons_name` searches organizations and users by name fragments and checks each audit row's ids.
  - `[low]` `[reject]` (verification-gap) The concurrent bootstrap test does not force the two transactions to overlap — Forcing it needs a hook inside the private `insert_admin` transaction; the reuse path is pinned by `a_second_admin_joins_the_same_vendor_organization`, and the `ON CONFLICT` insert is the documented Postgres idiom.
  - `[false]` `[reject]` (intent) Customer scoping is proved at HTTP, not in the data layer — Every `team.rs`/`accounts.rs` customer query takes the session's organization id first and filters on it in SQL (story 4); the constraint is met in the data layer, and the HTTP sweep proves the effect.
  - `[low]` `[patch]` (intent) "Records" in the sweep cover organizations, users and invitations only — Same root cause as the blind snapshot row; same fix.
  - `[low]` `[defer]` (intent) The admin pages are proven only by typecheck and build — No portal component-test setup exists (story 2–4 deferrals); added to `deferred`.
  - `[false]` `[reject]` (intent) "Every admin action is audited" holds only because there are no actions — Descriptive; autonomous decision 3 records views-only, and any future action must go through the audited `admin` data layer.
  - `[false]` `[reject]` (intent) The success-signal test does not check isolation inside the same scenario — Descriptive; the success signal is covered by that test plus `organization_a_never_sees_or_changes_organization_b`.

## Design Notes

- **Why the audit write lives in the data layer:** if each handler recorded its own row, a new admin handler could forget it. Because `admin::search_*` takes `&VendorAdmin` and records the row itself, the type system makes "read customer data across tenants" and "audit it" one call.
- Literal substring match:

  ```rust
  fn escape_like(q: &str) -> String {
      q.chars().flat_map(|c| match c { '%' | '_' | '\\' => vec!['\\', c], c => vec![c] }).collect()
  }
  // … WHERE lower(o.name) LIKE '%' || lower($1) || '%' ESCAPE '\'
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

**Summary:** Story 5 adds the vendor admin area and proves tenant isolation (SPEC CAP-5, CAP-6). Choices made without the owner are decisions 1–7 in the intent contract.

- **Vendor organization:** migration 0007 flags at most one organization `is_vendor`. `vendor_admin` users live there.
- **Bootstrap:** `wipe-hub admin create --email <address> [--name]` applies migrations, creates the vendor organization on first use, creates the admin with an unusable password, and emails the existing single-use, 1-hour password-reset link. No password ever passes through the shell. It is audited as `admin.created` (actor `cli:admin-create`). It refuses an existing admin or customer address and writes nothing. If SMTP is down it still creates the admin and says to use "Forgot password".
- **Admin area:** `GET /api/admin/organizations?q=`, `GET /api/admin/users?q=` and `GET /api/admin/organizations/:id` are read-only. Only a verified `vendor_admin` in the vendor organization gets in, checked live: customers get 403 and anonymous callers 401. Each read writes its audit row (the query plus the ids returned) in the same transaction, through `admin.rs`, the only code that reads across organizations. Vendor staff never appear in results.
- **Isolation:** `tests/isolation.rs` calls every customer route as organization A's owner and member, using B's ids and addresses. It asserts no B identifier comes back and that B's rows, sessions and tokens are unchanged. A guard fails on any `/api` route in `src` missing from its classification table.
- **Portal:** `/admin` runs searches only on submit. `/admin/organizations/$id` shows one organization. Home links to the admin area for vendor admins only. Admin results are dropped when their page unmounts.

**Files changed:**
- **Hub (`crates/wipe-hub`):**
  - `migrations/0007_vendor_organization.sql`: the `is_vendor` flag and the one-vendor index.
  - `src/admin.rs` (new): the audited cross-organization data layer and `admin create`.
  - `src/admin_routes.rs` (new): the vendor-admin check and the three routes.
  - `src/config.rs`: `AdminCreateArgs` and the shared parsers.
  - `src/main.rs`: the `admin create` subcommand.
  - `src/lib.rs`: the new modules and routes.
- **Hub tests:**
  - `tests/support/hub.rs` (new): the shared HTTP harness.
  - `tests/admin.rs` (new): 23 tests.
  - `tests/isolation.rs` (new): the sweep and the route guard.
  - `tests/migrations.rs` and `tests/cli.rs`: one test each.
- **Portal (`apps/portal/src`):** `api/admin.ts` and its test (new); `main.tsx`; new `AdminPage` and `AdminOrganizationPage`; the `HomePage` link; an optional `required` on `Field`.
- **Docs:** `CONTEXT.md` (§10 story-5 row and limitations, §14 vendor organization, admin area and roles), `README.md`, and `docs/ROADMAP.md` (A1.3 note).

**Review findings (24):**
- **Patched:** 1 medium entry and 3 low ones.
  - **Medium:** admin results stayed in the portal cache after their page unmounted. A repeated search could then show results without an audit row, and the next user in the tab could see them. Fixed with `gcTime: 0`.
  - **Low:** B's sessions and tokens added to the isolation snapshot; CONTEXT owner rule scoped to customer organizations; a name-search test.
- **Deferred:** component tests for the admin pages. The portal has no component-test setup.
- **Rejected:** each with its reason in the triage log. In short:
  - the route guard's method and subdirectory blind spots;
  - retry over-auditing;
  - losing the search on navigation;
  - the duplicated CLI flags;
  - literal limits in messages;
  - the sort collation;
  - the invalid-UTF-8 path;
  - the demotion race;
  - Unicode format characters in queries;
  - forcing an overlap in the concurrent test;
  - three descriptive intent readings.

**Follow-up review recommended: false.** First pass; one medium and no high entries were patched. Patched by verdict: medium 1, low 3.

**Verification (after patches):**
- `cargo test --workspace`: 382 passed, 0 failed, exit 0 (Postgres and Mailpit up).
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean.
- `rustfmt --edition 2021 --check` on every touched `.rs` file: clean.
- `pnpm --filter portal typecheck`, `test` (82) and `build`: pass.
- Matrix audit: every row has a passing test in `tests/admin.rs` or `tests/isolation.rs`.

**Residual risks:**
- The admin pages are proven only by typecheck and build (deferred).
- The route guard sees literal paths in top-level `src/*.rs` only, and not methods.
- The concurrent bootstrap test does not force the two transactions to overlap.
- The new admin receives the ordinary "reset your password" email, which reads oddly for a first sign-in. It can be reworded without any other change.
- There is no command to list or remove admins. Deleting the user row by SQL ends their sessions.
- Admins have no MFA, and their sessions last as long as customers' (SPEC non-goal).
