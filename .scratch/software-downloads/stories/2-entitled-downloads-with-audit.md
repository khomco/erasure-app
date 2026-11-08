---
title: 'Entitled downloads with audit'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '805dbe9b876930a798507aa0ce93ca9f907e4b32'
review_loop_iteration: 1
followup_review_recommended: false
context:
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
warnings: [oversized]
deferred:
  - summary: >-
      The portal's Download click flow (request a link, then navigate to it) has no test.
    evidence: |-
      Removing `window.location.assign(link.url)` from DownloadsPage passes every portal test: the portal
      has no DOM test environment (no jsdom, happy-dom or testing-library), so its tests cover pure
      helpers and static markup only. Settle it by adding a DOM environment and a click test.
    location: >-
      apps/portal/src/pages/DownloadsPage.tsx ArtifactRow
    severity: low
---

<intent-contract>

## Intent

**Problem:** Story 1 lets the vendor publish releases, but no customer can get them: there is no Downloads page, no download path, no entitlement check and no record of who downloaded what (SPEC CAP-2, CAP-3; ADR-0007 §5, §6).

**Approach:** A signed-in owner or member of an organization with a current entitlement sees every published release at `/downloads`, with each file's platform, size and full SHA-256. Pressing Download asks the hub for a short-lived signed link to that file; fetching the link re-checks the entitlement, records the download (user, organization, artifact, time) in an append-only table and the audit log, then streams the stored bytes. Vendor admins see each release's downloads on its admin page.

## Boundaries & Constraints

**Always:**
- "Entitled" means the organization has at least one **current** entitlement: not refunded and `term_end > now()` — the same rule as `entitlements::list`'s `current` (a cancelled one stays current until its term ends). The check is repeated when the link is fetched, not only when it is issued.
- Customers only ever see `published` releases, both channels (channel shown); drafts and withdrawn releases are listed to admins only, and their files cannot be linked or fetched (404 `MSG_NO_DOWNLOAD`, identical to an unknown or non-UUID id).
- `GET /api/downloads` and `POST /api/downloads/:artifact_id/link` need a session (no session: the hub-wide 401); any role of an entitled customer organization; unentitled (including the vendor organization) is 403 `MSG_NOT_ENTITLED`, checked before the artifact id is looked at.
- A link is `/api/downloads/:artifact_id/file?user=<uuid>&expires=<unix seconds>&sig=<base64url HMAC-SHA256>` over a domain-separated string of artifact id, user id and expiry, keyed by 32 random bytes the hub draws from the OS at start (`AppState`), compared in constant time (`Mac::verify_slice`). Lifetime `Config::download_link_ttl` (300 s; not a flag; tests set 0 to get an already-expired link). The link response is `{url, expires_at}` (path-relative URL, RFC 3339 UTC).
- The file route needs no session; the link is the credential. A missing, malformed, tampered (any of artifact, user, expiry or sig changed) or expired link is 403 `MSG_LINK_INVALID`, nothing written. A valid link whose user no longer exists, or whose organization is no longer entitled, is 403 (`MSG_LINK_INVALID` / `MSG_NOT_ENTITLED`), nothing written. Release no longer published: 404 `MSG_NO_DOWNLOAD`.
- On success, in this order: open the stored file (store missing/failing → 503 `MSG_STORAGE_UNAVAILABLE`, cause logged, nothing written); then one transaction inserts a `release_downloads` row and an audit row (actor `user:<id>`, action `release.downloaded`, subject `release:<id>`, detail `{artifact_id, platform, filename, sha256, organization_id}`) and commits; then the body streams with `Content-Type: application/octet-stream`, `Content-Disposition: attachment; filename="<filename>"`, `Content-Length`, `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`.
- Migration `0018_release_downloads.sql`: `release_downloads(id, release_id → releases, artifact_id → release_artifacts, organization_id → organizations, user_id uuid NOT NULL without FK (team removal deletes users and must keep working), downloaded_at default now())` — no email or other personal data is copied into it; triggers refuse UPDATE, DELETE and TRUNCATE, and an INSERT whose artifact is not in `release_id` or whose release is not `published`.
- `GET /api/admin/releases/:id/downloads` (`VendorAdmin` first, as every admin route): 200 `{downloads: [{id, artifact_id, platform, filename, user_id, user_email, organization_id, organization_name, downloaded_at}]}` (`user_email` is the user's current email joined from `users`, `null` once they have been removed) newest first, any release status; audited via `audit_read` as `admin.release_downloads_viewed {}`; unknown or non-UUID id 404 `MSG_NO_RELEASE`, nothing audited.
- Customer listing and link issuing are not audited (licence and station reads are not); only the fetch is.
- The file route answers `GET` only: `HEAD` (and every other method) is 405 and writes nothing, so a link-preview bot probing a pasted link never records a download.

**Never:** No admin or vendor-organization download of release files; no auto-update, code signing, CDN or cloud store; no `Range`/resume support; no change to story 1's admin routes or lifecycle besides the new read; no email verification gate beyond what sign-in already has; no edit to an applied migration; no hardware-sanitization claim in any copy.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| List | owner or member, org with a current entitlement; releases in every status | 200 `{releases}`: published only, newest `published_at` first, each with artifacts (`id, platform, filename, size_bytes, sha256`) | — |
| List unentitled | no entitlement / only refunded / term ended / vendor admin | 403 `MSG_NOT_ENTITLED` | nothing written |
| Link | entitled user, published artifact | 200 `{url, expires_at}`; `expires_at` ≈ now + 300 s | — |
| Link refused | draft, withdrawn, unknown or non-UUID artifact | 404 `MSG_NO_DOWNLOAD` | — |
| Fetch | GET the link, no cookie | 200, body byte-identical to the uploaded file, SHA-256 = listed `sha256`, headers as in Always; one `release_downloads` row and one `release.downloaded` audit row | — |
| Fetch twice | same link again before expiry | 200 again; a second row each | — |
| Bad link | no query; changed sig, user, expiry or artifact; ttl 0 | 403 `MSG_LINK_INVALID` | no row written |
| Lapsed since issue | link issued, then the entitlement refunded (or user removed from the team) | 403 `MSG_NOT_ENTITLED` (`MSG_LINK_INVALID` for a removed user) | no row written |
| Withdrawn since issue | link issued, release withdrawn | 404 `MSG_NO_DOWNLOAD` | no row written |
| File missing | stored file deleted from the artifact directory | 503 `MSG_STORAGE_UNAVAILABLE` | no row written |
| Signed out | list or link without a session | 401 (hub-wide) | — |
| Admin downloads view | admin, `GET /api/admin/releases/:id/downloads` | 200 rows with user email (`null` for a removed user) and organization name, newest first; audited | unknown id 404 no audit; customer 403 `MSG_ADMIN_ONLY`; none 401 |
| HEAD probe | `HEAD` on a valid link | 405 | no row written |
| Isolation | org B's user fetches a link issued to org A's user | allowed (the link is a bearer credential, re-checked against A's user and entitlement) and recorded as A's user | — |

## Autonomous decisions

- **"Signed-out gets 403" applies to the file route; session routes keep 401.** Problem: the SPEC says a signed-out request gets 403, while every session route in the hub answers 401 and the isolation sweep relies on it. Options: (a) make the new session routes 403 when signed out, (b) 401 on session routes and 403 on the file route for any request without a valid link. Chosen (b): the file route is the "download request"; it has no session concept, so a signed-out fetch without a valid link is 403, and the portal's existing 401 handling (redirect to sign-in) keeps working.
- **The link is a short-lived bearer credential, not bound to the session.** Options: (a) require the session cookie too, (b) link alone, like an object-store presigned URL. Chosen (b), with a 5-minute lifetime, a per-user signature, `no-referrer`, and the entitlement re-checked at fetch: a link copied to a bench machine's `curl` works, which is the reason the SPEC asks for signed links at all; requiring the cookie would make the signature redundant.
- **The signing key is random per hub process.** Options: (a) a new secret flag, (b) 32 random bytes drawn at start. Chosen (b): nothing to configure or leak, and a restart only voids links younger than 5 minutes. A second hub instance behind a load balancer would need a shared key; recorded as a CONTEXT limitation.
- **Downloads get their own append-only table as well as an audit row.** The admin per-release view needs user, organization and artifact; no product view reads `audit_log`. `user_id` has no foreign key, because removing a team member deletes the user row.
- **The download record keeps the user's id, not their email** (review pass 1). Problem: the append-only table refuses deletes, so a snapshotted email would outlive the person's removal from their team with no way to erase it; nothing else in the hub keeps a removed member's email. Options: (a) snapshot the email, (b) store the id only and show the current email while the user exists. Chosen (b), the option most protective of personal data and the hub's existing ids-only audit convention: the admin view shows a removed user's id with no email, and the audit row (`user:<id>`) still names them.
- **`HEAD` on the file route is refused** (review pass 1): axum's `get` also answers `HEAD`, which would record a download for a link-preview bot's probe in a table that cannot be corrected.
- **The download is recorded once the file is open and before bytes stream**, so a store failure records nothing, and an interrupted transfer still counts as a download (the bytes left the hub).
- **Vendor admins cannot download through this path** (the vendor organization has no entitlement); story 1 deferred admin download here, but CAP-2 does not ask for it and the vendor holds its own builds.
- **Both channels are listed to every entitled customer**, labelled; no entitlement feature gates `preview`.

</intent-contract>

## Code Map

- `crates/wipe-hub/src/release_admin.rs` -- `Release`/`ReleaseArtifact` shapes, `select()`, `with_artifacts`, `subject()`, `MSG_NO_RELEASE`, `MSG_STORAGE_UNAVAILABLE`, `ReleaseError`; add the admin downloads read (`downloads(conn, admin, id)`) here, and reuse the shapes from a new customer module.
- `crates/wipe-hub/src/entitlements.rs:139-150` -- the `current` predicate `(e.status <> 'refunded' AND e.term_end > now())`; express the "entitled" check with the same SQL (one `EXISTS` query), next to it or in the new module.
- `crates/wipe-hub/src/station_routes.rs` -- customer route pattern (`CurrentUser`, `ApiError`, `From<…Error>`), `licence_file_response` header style; licence download audit in `src/stations.rs:1097` (`licence.downloaded`).
- `crates/wipe-hub/src/artifacts.rs` -- `ArtifactStore::open(key)` (key = artifact id string) returns a boxed `AsyncRead`; `StoreError::NotFound`. Stream with `tokio_util::io::ReaderStream` (dep already present) into `axum::body::Body::from_stream`.
- `crates/wipe-hub/src/payments.rs:419` -- `HmacSha256 = Hmac<Sha256>` (`hmac` 0.12, `rand`, `base64` are deps); use `Mac::verify_slice` for the constant-time check.
- `crates/wipe-hub/src/lib.rs` -- `AppState` (add the download signer), `router()` merge list, module list. `src/config.rs:223,316,404` -- `artifact_max_bytes` is the not-a-flag `Config` field pattern for `download_link_ttl` (default, `Debug`, test at l.1106).
- `crates/wipe-hub/src/admin_routes.rs:152-163` -- release routes; add `GET /api/admin/releases/:id/downloads` and its module-doc line; `VendorAdmin` first extractor.
- `crates/wipe-hub/src/auth.rs` -- `CurrentUser`, `ApiError::new`, `actor(user_id)`. `src/team.rs:232` deletes users (why no FK on `user_id`).
- `crates/wipe-hub/migrations/0017_releases.sql` -- trigger style for 0018; highest migration is 0017.
- `crates/wipe-hub/tests/releases.rs` -- admin sign-in, release creation/publish helpers, `spawn_hub_config(|args|…, |config|…)` for `download_link_ttl = 0`; `tests/stations.rs` / `tests/entitlements.rs` -- how a test gets an organization a paid order and entitlement (Fake provider) and refunds it; `tests/isolation.rs` `ROUTES` with `Class::{Customer, Anonymous(reason), Admin}` and its sweep; `tests/migrations.rs` trigger-refusal pattern; `tests/team.rs` for removing a member.
- `apps/portal/src/api/stations.ts`, `api/adminReleases.ts`, `api/auth.ts` (`request`, `ensureOk`, `json`, CSRF) -- client and guard patterns; `pages/StationsPage.tsx` customer page pattern; `pages/AdminReleasePage.tsx` for the downloads section; `pages/HomePage.tsx:131` nav link; `main.tsx:210-328` routes.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0018_release_downloads.sql` -- table, index `(release_id, downloaded_at DESC)`, append-only and insert-guard triggers.
- `crates/wipe-hub/src/downloads.rs` (new) -- `DownloadSigner` (random key, `sign`, `verify`), `is_entitled`, `list_for(conn, org)`, `issue_link`, `resolve_fetch` (user → org → entitled → published artifact), `record` (row + audit), error enum and `MSG_NOT_ENTITLED`, `MSG_NO_DOWNLOAD`, `MSG_LINK_INVALID`; unit tests for sign/verify (each field tampered, expired, malformed base64).
- `crates/wipe-hub/src/download_routes.rs` (new) -- the three customer routes and the file response; `lib.rs`, `config.rs` -- wiring and `download_link_ttl`.
- `crates/wipe-hub/src/release_admin.rs`, `admin_routes.rs` -- admin downloads read and route.
- `crates/wipe-hub/tests/downloads.rs` (new) -- one test per matrix row over HTTP, including SHA-256 of the fetched body equal to the listed checksum, header values, exact row and audit counts; `tests/isolation.rs` -- classify the four routes (file route `Anonymous("the signed link is the credential")`); `tests/migrations.rs` -- 0018 refusals (update, delete, insert for a draft release, artifact of another release); fix any literal `Config {`/`Args {` that breaks.
- `apps/portal/src/api/downloads.ts` (+ test) -- types, guards, `listDownloads`, `requestDownloadLink`; `api/adminReleases.ts` (+ test) -- `listReleaseDownloads`.
- `apps/portal/src/pages/DownloadsPage.tsx` (`/downloads`: releases with channel, published date, notes, files with platform, human size and full SHA-256, Download → request link → `window.location.assign(url)`; 403 shows a not-entitled message linking to `/buy`), `HomePage.tsx` link, `main.tsx` route; `AdminReleasePage.tsx` -- Downloads table (when, user email, organization, platform); tests for pure helpers.
- `CONTEXT.md` (§10 row, §11 limitation on the per-process key, §14 **Download link**), `docs/ROADMAP.md` (strike downloads), `README.md`, `AGENTS.md` (test counts) -- record what shipped.

- Review pass 1 additions (apply on top of the attempt-1 patch, see Spec Change Log):
  - `crates/wipe-hub/src/entitlements.rs` -- one shared SQL fragment (e.g. `pub(crate) const CURRENT_SQL`) for the `current` predicate, used by `select_entitlements` and by `downloads::is_entitled`, so seats and downloads cannot diverge.
  - `crates/wipe-hub/tests/downloads.rs` -- a withdrawal racing a fetch: a transaction locks the release `FOR UPDATE` and withdraws it, the fetch starts, the transaction commits; the fetch is 404 `MSG_NO_DOWNLOAD` with nothing written (pins the `FOR SHARE` lock). A `HEAD` on a valid link is 405 with nothing written. A removed user's download shows `user_email: null` in the admin view.
  - `apps/portal/src/api/downloads.ts` -- `isNotEntitled` matches the hub's not-entitled message, not every 403 (a CSRF 403 must show as an error, not as "buy a licence").
  - `apps/portal/src/pages/DownloadsPage.tsx` -- reset the "Download started." flag when a new press begins; `apps/portal/src/pages/AdminReleasePage.tsx` -- a removed user's row shows their id instead of an email.
  - Portal tests -- render `NotEntitled` inside a memory router (text and `href="/buy"`, as `AdminLicencePage.test.ts` does) and the admin downloads table with one row (time, email, organization link to `/admin/organizations/<id>`, platform) and with a removed user.
  - `CONTEXT.md` §14 **Download link** -- say that a link stays usable until it expires even after its user signs out everywhere or resets their password; only removal from the team or a lapsed entitlement voids it early.

**Acceptance Criteria:**
- Given a release published with `wipe-hub release publish --file linux-x86_64=<file>` and a customer organization with a paid order, when its member requests a link from `GET /api/downloads` and fetches it, then the body's SHA-256 equals both the listed `sha256` and `sha256sum <file>`, and `GET /api/admin/releases/:id/downloads` lists that member's email, organization and the artifact.
- Given a member of an organization without a current entitlement, when they open `/downloads`, then they see that their organization has no current entitlement and a link to buy, and no file is listed.

## Spec Change Log

### 2026-11-08 — review pass 1 (loop 1)
- **Trigger:** blind review: `release_downloads.user_email` kept a removed team member's email forever in an append-only table that cannot erase it. Also folded in: `HEAD` on the file route recorded a download (blind, edge), plus the patch-level findings listed in the triage log.
- **Amended:** intent contract (autonomous decisions "The download record keeps the user's id, not their email" and "`HEAD` on the file route is refused"; migration column list; admin view `user_email` is joined and nullable; new Always rule and matrix row for `HEAD`); Tasks gain "Review pass 1 additions".
- **Known-bad state avoided:** personal data stored where it can never be deleted; downloads recorded for link-preview probes that sent no bytes.
- **KEEP:** start from `_bmad-output/implementation-artifacts/software-downloads-story-2-attempt-1.patch` (`git apply` it at the baseline), which passed every verification command: clippy clean, `cargo test --workspace` green except the known `health.rs` timing flake, portal typecheck/test (222)/build green, and all 14 `tests/downloads.rs` tests covering every matrix row. Keep its module layout (`downloads.rs`, `download_routes.rs`), the signer and its unit tests, the open-then-record-then-stream order with the `FOR SHARE` lock, the isolation classifications, the 0018 triggers, the portal pages and the docs; change only what this entry and the "Review pass 1 additions" name. Update the test count in `AGENTS.md`/`README.md` to the new total.

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 34 findings — high 0, medium 3, low 26, false 3, maybe-false 2
- findings:
  - `[low]` `[patch]` (blind) `isNotEntitled` treats any 403, including a repeated CSRF refusal, as "not entitled" — confirmed (`downloads.ts:86` checks status only; `csrf.rs:113` also answers 403). Folded into the loopback: match the hub's not-entitled message.
  - `[low]` `[reject]` (blind) `window.location.assign` leaves the portal for a raw JSON page when the fetch fails — the link was verified a moment earlier, so failure needs a withdrawal, refund or store loss within seconds; a probe or error page is new machinery.
  - `[medium]` `[bad_spec]` (blind) `HEAD` on a link records a download that sent no bytes — confirmed: axum 0.7 `get()` also answers `HEAD`, and the handler records before the body. Amended: Always rule and matrix row, `HEAD` is 405.
  - `[low]` `[reject]` (blind) A truncated or replaced stored file is recorded and served with the database's `Content-Length` — needs out-of-band tampering with the artifact directory; checking the length needs a new `ArtifactStore` method.
  - `[medium]` `[bad_spec]` (blind) `release_downloads.user_email` keeps a removed member's email forever in an append-only table — confirmed: no other hub table keeps a removed user's email, and 0018's triggers refuse deletes. Amended: autonomous decision, store the id only and join the current email (null once removed).
  - `[low]` `[patch]` (blind) `is_entitled` re-types the `current` predicate instead of sharing it with `entitlements::list` — a later change to one (e.g. a grace period) would split `/seats` from downloads. Folded into the loopback: one shared SQL fragment.
  - `[low]` `[reject]` (blind) Members and vendor admins are offered "Buy a licence" though only owners can buy — the wording is customer-visible copy, and `/buy` itself explains who may buy; cosmetic.
  - `[low]` `[patch]` (blind) Acceptance criterion 2 (the not-entitled view) has no rendering test — confirmed by the verification-gap layer. Folded into the loopback: render `NotEntitled` in a memory router.
  - `[low]` `[reject]` (blind) No test for a term that ends between issuing and fetching a link — same `is_entitled` query as the refund case, which is tested at fetch time.
  - `[low]` `[reject]` (blind) The admin downloads view is unbounded — story 1 rejected paging for the release list on the same grounds; paging can come with volume.
  - `[low]` `[patch]` (blind) A link outlives "sign out everywhere" and a password reset for up to five minutes — it is the recorded bearer decision; the fix chosen is the direct one, a CONTEXT §14 sentence (folded into the loopback).
  - `[low]` `[reject]` (blind) The customer page imports `CHANNEL_LABELS` and `formatSize` from admin modules — no named breakage; a move is a refactor with no user effect.
  - `[false]` `[reject]` (blind) `expires_at` silently becomes `""` if formatting fails — `now + 300 s` is always inside `OffsetDateTime`'s range, so the `unwrap_or_default` branch is unreachable.
  - `[low]` `[reject]` (blind) Migration 0018 has no index on `organization_id` / `user_id` — no query uses them yet; an index is a new migration later, not an edit.
  - `[medium]` `[bad_spec]` (edge) `HEAD` records a download — same root cause and amendment as the blind `HEAD` row.
  - `[low]` `[reject]` (edge) A missing stored file answers 503 before user, entitlement and status are checked — the spec orders open-first so a store failure records nothing; the link's signature is already valid, so nothing leaks.
  - `[low]` `[reject]` (edge) Stored length differing from `size_bytes` — same as the blind truncated-file row.
  - `[low]` `[patch]` (edge) A link survives sign-out-everywhere — same as the blind row; CONTEXT sentence.
  - `[low]` `[reject]` (edge) `expires` accepts non-canonical numbers (`+N`, `0N`), so one signed link has several URLs — each still names the same artifact, user and expiry under the same signature; no harm.
  - `[low]` `[reject]` (edge) The customer list's two queries run outside a transaction, so a release withdrawn in between may list — its link then answers 404; a reload corrects it.
  - `[low]` `[reject]` (edge) Navigation to a failing link leaves the portal — same as the blind navigation row.
  - `[low]` `[patch]` (edge) Any 403 counts as not entitled — same as the blind CSRF row.
  - `[low]` `[patch]` (edge) "Download started." stays shown after a later press fails — confirmed (`opened` is never reset). Folded into the loopback: reset on a new press.
  - `[false]` `[reject]` (edge) The module doc promises an error order the code breaks — the doc lists outcomes, not precedence; the spec's Always rule sets open-first, which the code follows.
  - `[low]` `[patch]` (verification-gap) The `FOR SHARE` lock against a racing withdrawal is untested — pre-verified. Folded into the loopback: a lock-hold test.
  - `[low]` `[patch]` (verification-gap) The not-entitled view is never rendered — same as the blind AC 2 row.
  - `[low]` `[patch]` (verification-gap) The admin downloads table is tested only empty — pre-verified. Folded into the loopback: one-row and removed-user renders.
  - `[low]` `[defer]` (verification-gap) The Download click flow (link then `window.location.assign`) is untested — the portal has no DOM test environment; adding one is separate from this story.
  - `[false]` `[reject]` (verification-gap) `AGENTS.md`'s 686 is off by one — the run printed 685 passed plus the 1 flaky failure, 686 tests.
  - `[maybe-false]` `[reject]` (intent) Signed-out requests get 401 on the session routes, not the SPEC's 403 — the recorded autonomous decision; changing it means editing the spec.
  - `[maybe-false]` `[reject]` (intent) "Others cannot": a link works for anyone who holds it for five minutes — the recorded bearer decision.
  - `[low]` `[reject]` (intent) The record names the user the link was issued to, not whoever fetched it — the recorded bearer decision, tested.
  - `[low]` `[patch]` (intent) Portal surfaces (not-entitled view, admin rows) are proven only at the API level — same as the verification-gap render rows.
  - `[low]` `[reject]` (intent) "Active" read as current, vendor excluded, preview shown to all — recorded autonomous decisions.
- loopback: bad_spec → review_loop_iteration 1; attempt saved as `_bmad-output/implementation-artifacts/software-downloads-story-2-attempt-1.patch`; patch rows above are carried into "Review pass 1 additions" for re-derivation.

### 2026-11-08 — Review pass (loop 1)
- verdicts: 23 findings — high 0, medium 0, low 19, false 2, maybe-false 2
- findings:
  - `[low]` `[reject]` (blind) A valid link can be fetched any number of times in its five minutes, each adding a row — only the link's own entitled user can do it, inflating only their own records; a per-link nonce or rate limit is new machinery. The matrix records "Fetch twice → a second row each".
  - `[low]` `[patch]` (blind) CONTEXT §14 **Download link** says only removal or a lapse voids a link early, omitting restart and withdrawal — patch: entry lists every early end.
  - `[low]` `[reject]` (blind) The portal offers no "copy link" for the bench-`curl` case — a new UI affordance with customer-visible copy; the link is still reachable from the browser's downloads.
  - `[low]` `[patch]` (blind) No test that a vendor admin is refused a link — patch: admin `POST …/link` asserted 403 `MSG_NOT_ENTITLED`, nothing written.
  - `[low]` `[reject]` (blind) No per-organization download view — CAP-3 asks for an admin view per release; a per-customer view widens scope.
  - `[false]` `[reject]` (blind) Keeping download records by user id forever is an undecided personal-data choice — ADR-0007 §6 decided that every download goes to an append-only audit with its actor; ids only, as pass 1 chose, is that convention.
  - `[low]` `[reject]` (blind) A refund committing during a fetch can still record and stream one download — a millisecond window; share-locking entitlements adds lock ordering with the payment path for no customer harm.
  - `[false]` `[reject]` (blind) The story is still `in-review` and the log covers pass 1 only — the run is mid-review; this entry and finalization close it.
  - `[low]` `[patch]` (blind) Empty action header and identical Download button names — patch: visually hidden header and per-file `aria-label`.
  - `[low]` `[reject]` (blind) Admin table tests sit in the customer page's test file — no behaviour change; a file move.
  - `[low]` `[patch]` (blind) The config test never asserts `download_link_ttl` in `Debug` — patch: one assertion.
  - `[low]` `[patch]` (blind) `isDownloadLink` accepts any `/api/downloads/` path — patch: require the `<uuid>/file?` shape.
  - `[low]` `[reject]` (edge) carried — navigation to a failing link leaves the portal (pass-1 navigation row).
  - `[low]` `[reject]` (edge) Repeated fetches record repeatedly and `Range` is ignored — repeated rows are the matrix's "Fetch twice"; `Range` is a Never.
  - `[low]` `[reject]` (edge) carried — stored length differing from `size_bytes` (pass-1 truncated-file row).
  - `[low]` `[reject]` (edge) carried — open-before-resolve answers 503 for an invalid user (pass-1 row).
  - `[low]` `[patch]` (edge) `DownloadsSection` keeps release A's table when the router moves straight to release B — patch: keyed by release id.
  - `[low]` `[reject]` (edge) carried — vendor admins are told to buy a licence (pass-1 "Buy a licence" copy row).
  - `[low]` `[patch]` (verification-gap) Hub and portal copies of the not-entitled message are not pinned — pre-verified; patch: an `include_str!` test like `csrf.rs:233`.
  - `[maybe-false]` `[reject]` (intent) carried — signed-out session routes answer 401 (recorded decision).
  - `[maybe-false]` `[reject]` (intent) carried — bearer link, recorded as the issued-to user (recorded decision).
  - `[low]` `[reject]` (intent) The composed Downloads page and admin wiring are tested only as components — same root as the deferred DOM-environment item.
  - `[low]` `[reject]` (intent) The success-signal test uses synthetic bytes over HTTP, not a real build through the portal — the end-to-end script is `commerce-end-to-end` CAP-1 (story 1 triage).

## Design Notes

Signed string: `"wipe-hub/download/v1\n{artifact_id}\n{user_id}\n{expires}"`; signature base64url without padding. Verify first parses `user` as UUID and `expires` as `i64`, recomputes, `verify_slice`, then `expires > now`. Never log the signature.

Fetch: `verify` → `store.open(artifact_id)` → `BEGIN` → user's org (none → 403) → entitled (no → 403) → artifact joined to a `published` release (no → 404) → insert row + audit → `COMMIT` → stream. A store error after commit cannot happen because the reader is already open.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including `downloads`, isolation and migration tests
- `cargo test --workspace` -- expected: pass (known flake: `health.rs` `silent_smtp_and_database_time_out_within_bound`)
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: pass

## Auto Run Result

Status: done

**Summary.** Customers can now download the station software. Any owner or member of an organization with a current entitlement (not refunded, term not ended) sees every published release at `/downloads`, with each file's platform, size and full SHA-256. Download asks the hub for a five-minute HMAC-signed link and opens it. Fetching the link needs no session. It re-checks the user and the entitlement, records the download in an append-only `release_downloads` table (user id, organization, artifact, time; no email) and the audit log, then streams the file. Vendor admins see each release's downloads, with the user's current email (or "Removed user <id>"), on its admin page. `HEAD` on a link is 405, so link-preview probes record nothing.

**Files changed.**
- `crates/wipe-hub/migrations/0018_release_downloads.sql`: the append-only downloads table and its insert guard.
- `crates/wipe-hub/src/downloads.rs` (new): signer, entitlement check, listing, link issue, fetch resolution and recording.
- `crates/wipe-hub/src/download_routes.rs` (new): `GET /api/downloads`, `POST /api/downloads/:artifact_id/link`, `GET /api/downloads/:artifact_id/file` (`HEAD` refused).
- `crates/wipe-hub/src/entitlements.rs`, `stations.rs`: one shared `CURRENT_SQL` predicate.
- `crates/wipe-hub/src/release_admin.rs`, `admin_routes.rs`: `GET /api/admin/releases/:id/downloads`, audited.
- `crates/wipe-hub/src/lib.rs`, `config.rs`: signer in `AppState`, `download_link_ttl` (300 s, not a flag).
- `crates/wipe-hub/tests/downloads.rs` (new, 16 tests), `tests/isolation.rs`, `tests/migrations.rs`.
- `apps/portal/src/api/downloads.ts` (+ test), `pages/DownloadsPage.tsx` (+ test), `api/adminReleases.ts` (+ test), `pages/AdminReleasePage.tsx`, `AdminReleasesPage.tsx`, `HomePage.tsx`, `main.tsx`.
- `CONTEXT.md`, `docs/ROADMAP.md`, `README.md`, `AGENTS.md`: what shipped, the **Download link** term, the per-process key limitation, test counts (689).
- `_bmad-output/implementation-artifacts/software-downloads-story-2-attempt-1.patch`: the attempt reverted by the pass-1 loopback.

**Review findings.**
- Pass 1: 34 findings (high 0, medium 3, low 26, false 3, maybe-false 2). Two spec loopbacks (bad_spec): emails are not stored in the append-only record, and `HEAD` is refused. Eight patch rows were carried into the re-derivation: CSRF-vs-not-entitled matching, the shared `current` rule, the stale "Download started." flag, the `FOR SHARE` race test, not-entitled and admin-table renders, and the sign-out note. One item was deferred (the Download click flow is untested; the portal has no DOM test environment). Every rejection and its reason is in the Review Triage Log.
- Pass 2: 23 findings (high 0, medium 0, low 19, false 2, maybe-false 2). Seven low patches were applied: an accurate §14 link entry, a vendor-admin link refusal test, a hub↔portal message pin test, accessible Download labels, a strict `isDownloadLink`, a keyed admin downloads section, and the TTL `Debug` assertion. Every rejection and its reason is in the Review Triage Log.

**Follow-up review recommended: false.** Pass 2 patched 7 low entries and no medium or high ones. Pass 1's medium findings were fixed by re-derivation and then reviewed again in pass 2, which found no medium issues.

**Verification.**
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean.
- `cargo test --workspace --no-fail-fast`: 688 passed, 1 failed. The failure is `health.rs` `silent_smtp_and_database_time_out_within_bound` (4.2 s against a 3.5 s bound, load average about 6). It passed twice when rerun alone (2.83 s). It is the timing flake story 1 showed failing on the baseline, and it touches no download code.
- Portal: `typecheck` clean, `test` 225 passed, `build` ok.
- Matrix audit: every I/O-matrix row, including the `HEAD` probe, has a covering test in `tests/downloads.rs`, and each passed in the full run.

**Residual risks.**
- A link is a bearer credential for five minutes. It survives sign-out everywhere and password resets (documented in CONTEXT §14).
- The signing key is per process: a restart voids outstanding links, and a second hub instance would need a shared key (CONTEXT §11).
- The portal pages are checked by typecheck, static renders and a build, not in a browser. The click-to-download flow is untested (deferred).
- An interrupted or repeated transfer records one row per fetch. There is no `Range` support.

