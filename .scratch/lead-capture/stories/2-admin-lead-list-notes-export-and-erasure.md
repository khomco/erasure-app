---
title: 'Admin lead list, notes, export and erasure'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '184934e7894ca913fef341a256daed452a4df204'
review_loop_iteration: 1
followup_review_recommended: false
context:
  - '{project-root}/.scratch/lead-capture/SPEC.md'
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
warnings: [oversized]
deferred:
  - summary: >-
      No policy on whether sales may answer, by hand, a direct contact message from a suppressed (unsubscribed) sales lead.
    evidence: |-
      Carried from story 1. This story sends no email; the admin pages show "Unsubscribed <date>" on the list and detail without telling staff what to do about it. Wording any instruction is customer-visible policy copy, so it waits for the owner.
    location: >-
      apps/portal/src/pages/AdminLeadPage.tsx
    severity: low
  - summary: >-
      The portal's lead pages have no component tests for their audit-sensitive behaviour.
    evidence: |-
      Untested behaviours: export uses the submitted filters, a status change or note adds no refetch, and erase needs a second press. The portal has no DOM test environment; story 1's unsubscribe page and story 5's admin pages share the gap. Changing the export to post the draft filters, or wiring erase to a single press, would pass every test. Settle it with a happy-dom + testing-library setup for the portal.
    location: >-
      apps/portal/src/pages/AdminLeadsPage.tsx, apps/portal/src/pages/AdminLeadPage.tsx
    severity: medium
---

<intent-contract>

## Intent

**Problem:** Sales leads from the marketing site are stored in `wipe-hub`, but vendor staff cannot see, work, export or erase them. This covers lead-capture CAP-3 and the deletion half of CAP-4.

**Approach:** Add vendor-admin-only `/api/admin/leads*` routes, behind the existing `VendorAdmin` extractor, backed by a new data layer that audits every read and write in the caller's transaction. Add a `lead_notes` table, and portal pages `/admin/leads` (filters, table, CSV download) and `/admin/leads/$id` (submissions with consent, status, notes, erase).

## Boundaries & Constraints

**Always:**
- Access: every new route takes `VendorAdmin` as its first extractor, before any path, query or body, so customers get 403 `only vendor admins can use the admin area` and anonymous callers 401, and neither writes an audit row. Bodies use `auth::JsonBody`. Error bodies are `{"error": …}`.
- Routes, all in `admin_routes.rs`:
  - `GET /api/admin/leads?segment=&status=&from=&to=` lists leads. `segment` is one of `leads::SEGMENTS` or `none` (no segment; contact-only leads). `status` is `new|contacted|qualified|closed`. `from`/`to` are `YYYY-MM-DD`, inclusive UTC days, `from <= to`. A date range matches a lead with **any** submission whose `consented_at` falls in it. Every filter is optional and empty means any. A bad value is 400 `MSG_BAD_FILTER` with no audit row. Order is the latest submission first, then id. At most `admin::SEARCH_LIMIT` (100) rows plus `truncated`. Audited `admin.leads_listed`, subject `search:leads`, detail `{filter, lead_ids}`.
  - `GET /api/admin/leads/:id` returns `{lead, submissions, notes}`: submissions newest first, each with `{id, kind, message, consent_text, consented_at}`; notes newest first, each with `{id, body, created_at, author: {id, name, email} | null}`. Audited `admin.lead_viewed`, subject `lead:<id>`, detail `{}`.
  - `POST /api/admin/leads/:id/status {status}` returns 200 `{lead}`. It audits `admin.lead_status_changed {from, to}` and bumps `updated_at` only when the status changes. Setting the current status again is 200 with no `admin.lead_status_changed` row, but because it still returns the lead, it audits `admin.lead_viewed` (decision 8). A bad status is 400.
  - `POST /api/admin/leads/:id/notes {body}` returns 201 `{note}`. The body is checked with `leads::validate_message` and must not be blank. Audited `admin.lead_note_added {note_id}`.
  - `POST /api/admin/leads/:id/erase` answers 200 `{}`. In one transaction it locks the lead, counts its submissions and notes, deletes the lead (submissions and notes cascade), deletes the email-send throttle row keyed by its address (`mail:to:<lower(email)>`, built by the same `auth` key function `charge_mail` uses; decision 6), and audits `admin.lead_erased {submissions, notes}`.
  - `POST /api/admin/leads/export {segment?, status?, from?, to?}` uses the same filters and validation as the list, with **no** row cap and the same order. Unknown filter keys (in the query string or the body) and a body that is not a JSON object of optional strings are also 400 `MSG_BAD_FILTER`, never a silently unfiltered result. It answers 200 `text/csv; charset=utf-8` with `Content-Disposition: attachment; filename="sales-leads-<UTC date>.csv"` and `Cache-Control: no-store`. Audited `admin.leads_exported`, subject `search:leads`, detail `{filter, lead_ids}`, in the same transaction as the read, so no export leaves the hub unaudited.
- `lead` shape (list rows, detail, export): `id, email, name, company, role, segment, status, created_at, updated_at, last_submitted_at, submission_count, unsubscribed_at`. Timestamps are RFC 3339 UTC strings, formatted in SQL as `admin.rs` does; nullable fields are JSON `null`.
- A lead id that is unknown, not a UUID, or erased is 404 `no such sales lead` with no audit row, on every `:id` route. Each mutation first locks the lead row (`SELECT … FOR UPDATE`), so a concurrent erase makes it a 404, never a 500.
- Audit rows written by this story hold ids, flags, filter values, counts and status names only. They never hold a name, email, company, role, message, consent text or note body, so an erased lead leaves no personal data in the append-only log (CAP-4). This is why the list has no free-text search.
- CSV: a UTF-8 byte-order mark (BOM) first, so spreadsheet apps read non-ASCII names correctly (decision 9), then RFC 4180 (comma separator, CRLF line ends, a header row, quoting any field that holds `"`, `,`, CR or LF, with `"` doubled). Formula-injection guard (OWASP CSV Injection): a field starting with `=`, `+`, `-`, `@`, TAB or CR is prefixed with `'`. Columns are the `lead` shape, in order. Messages, consent text and notes are not exported.
- Portal: `/admin` links to `/admin/leads`. The list runs only when the filter form is submitted, like the existing searches (each run is audited). The export button posts the current *submitted* filters with the CSRF token and saves the response as a file. On `/admin/leads/$id`, mutations update the page from their response, never by refetching, because a refetch is another audited view. Erasing needs a second confirming press, then returns to `/admin/leads`. An unsubscribed lead shows "Unsubscribed" with its date in the list and on the detail page.

**Never:**
- No email to sales leads, no editing or deleting single notes or submissions, no free-text search, no bulk status or bulk erase, no CRM sync.
- No suppression record kept after an erasure: it would retain the address. An erased person who submits again is a new lead.
- No editing of applied migrations, and no personal data in any audit row.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| List, filtered | 3 leads (itad/new, government/contacted, contact-only); `segment=itad` | only the itad lead; `admin.leads_listed` with its id | — |
| Date filter | lead A submitted only in a past range (backdated `consented_at`), B today | `from=to=today` → B only; the earlier range → A only | — |
| Bad filter | `status=open`, `from=2026-13-01`, `from>to`, `segment=x` | 400 `MSG_BAD_FILTER` | no audit row |
| Detail | lead with 2 submissions, 1 note | both submissions with their exact consent text; the note with its author; `admin.lead_viewed` | — |
| Status change | `new` → `contacted`, then `contacted` again | 200 both times; one `admin.lead_status_changed {from:new,to:contacted}` | bad status → 400 |
| Note | `{body:"  called, follow up  "}` | 201 trimmed note; audit detail holds `note_id` only | blank → 400 |
| Export | 2 leads match, one name `=cmd()` and one company `a,"b"` | CSV with header + 2 rows, `'=cmd()`, `"a,""b"""`; `admin.leads_exported` with both ids | no CSRF → 403 |
| Erase | lead with submissions and a note | 200; no `leads`, `lead_submissions` or `lead_notes` row left; no `auth_throttle` row keyed by its address; `admin.lead_erased {submissions:2,notes:1}`; no audit row written by `lead.*` or `admin.*` contains its email, name, company or message | then GET → 404 |
| Mutation racing an erasure | a transaction holds the lead row `FOR UPDATE`; a note POST starts; the holder deletes the lead and commits | the note POST is 404 `no such sales lead`; no `lead_notes` row; no new audit row | never 500 |
| Status no-op | status set to the current value | 200 `{lead}`; no `admin.lead_status_changed`; one `admin.lead_viewed` | — |
| Notes order and removed author | two notes, then the author's user row deleted | detail lists both newest first, each with `author: null` | — |
| Unknown filter key / bad body | `?Segment=itad`; export body `{"status": 1}` or `{"Segment":"itad"}` | 400 `MSG_BAD_FILTER` | no audit row |
| Unknown lead | random UUID / `nope` on each `:id` route | 404 `no such sales lead` | no audit row |
| Unsubscribed lead | lead unsubscribed via story 1's route | `unsubscribed_at` set in list, detail and CSV | — |
| Customer or anonymous | owner session / none, on every new route | 403 / 401 | no audit row |
| Resubmit after erasure | same address submits again | new lead id, `created:true` | — |

## Autonomous decisions

1. **What the date filter matches.** Sales staff filter by date to find recent activity. Matching only the first submission would hide a known lead's new contact message, and matching only the latest would hide earlier ones. The choice is any submission in the range (the most inclusive reading), and the list also shows the latest submission time.
2. **Export as a CSRF-protected POST.** A GET download could be started by another site through a top-level link (the `SameSite=Lax` cookie is sent), which would drop a file of personal data into an admin's downloads and write an audit row. A bulk export of personal data is a sensitive action, so it needs the CSRF token, as OWASP advises for state-relevant actions.
3. **CSV formula-injection guard.** Lead fields are typed by the public. A name like `=HYPERLINK(…)` would run as a formula when the export is opened in a spreadsheet. The OWASP CSV Injection guidance is to prefix such cells with `'`.
4. **No free-text search, and no personal data in audit rows.** The existing admin searches audit their query. A lead search by email would put the address in the append-only log for good, which breaks CAP-4's "audit entry without the personal data". The list filters on segment, status and date only (what CAP-3 asks for).
5. **Notes are append-only and authored.** A note is attributed to the admin who wrote it, and the author link becomes null if that user is removed. Notes are deleted with the lead. Editing and deleting single notes is left out (not asked for, and an edit history would be needed to keep the audit meaningful).
6. **Erasure keeps no suppression record, and clears the address's email-send throttle row** (revised at review, pass 1). Keeping a hashed address to block re-subscription is a retained identifier, which the person asked to erase. Erasure removes everything, and a new submission starts a new lead. The first plan said the throttle rows naming the address "expire on their own". They do not: `throttle::purge_stale` runs only on some later throttled request, so on a quiet hub the row stays indefinitely. A locked row, once purged, also writes `auth.mail_lock_summary` naming the address *after* the erasure. Deleting that row in the erasure transaction is the option most protective of the person's data. Its only cost is that the address's hourly email budget restarts, and only an admin's erasure can cause that. Audit rows written before the erasure (`auth.mail_locked`, `auth.mail_lock_summary`) stay, since the log is append-only; this is recorded in CONTEXT §10.
7. **The export is minimal.** It holds the lead row only, not messages, consent text or notes. Those stay in the hub, viewable per lead and audited, so a CSV that leaves the hub carries as little as possible.
8. **Every response carrying lead data is audited** (added at review, pass 1). Setting a lead's status to the value it already has changes nothing, but the answer still carries the lead's name, email and company. Without an audit row, an admin could read any lead untraced. The no-op therefore audits `admin.lead_viewed`, the same row as opening the lead.
9. **The CSV starts with a UTF-8 BOM** (added at review, pass 1). Sales staff will open the export in a spreadsheet. Excel reads a CSV without a BOM in the legacy Windows code page and garbles names like "Müller". A BOM is the documented way to make Excel read UTF-8, and RFC 4180 parsers that expect UTF-8 accept it.
10. **Unknown filter keys are refused** (added at review, pass 1). A misspelt key (`Segment`) would otherwise be ignored, and the admin would get, or export, every lead's personal data while believing it was filtered.

</intent-contract>

## Code Map

- `_bmad-output/implementation-artifacts/lead-capture-story-2-attempt-1.patch` -- review pass 1's reverted attempt. Start from it (`git apply`), then make the changes in the Spec Change Log. Everything its KEEP list names is already right there.
- `crates/wipe-hub/src/auth.rs:493` `email_key` (private; make it `pub(crate)`) and `MAIL_TO_PREFIX` -- the erasure deletes `email_key(MAIL_TO_PREFIX, lead.email)` from `auth_throttle`. `throttle.rs:293` `purge_stale` shows why the row otherwise stays.
- `crates/wipe-hub/src/admin_routes.rs` -- `VendorAdmin` extractor (`:62`), `routes()`, the `query_of` pattern, the `Path<String>` → UUID → 404 pattern (`organization_detail`). Add the lead routes here and update the module doc ("all GET and read-only" is no longer true).
- `crates/wipe-hub/src/admin.rs` -- the template for the new data layer: `VendorAdmin`, `audit_read` (`actor(admin.user_id)` → `user:<id>`), `truncate`, `SEARCH_LIMIT`, `RFC3339_UTC` (private, so copy the constant or make it `pub(crate)`), `AdminReadError`. Update its doc comment "Read-only" bullet to point at the new module.
- `crates/wipe-hub/src/leads.rs` -- `SEGMENTS`, `validate_message`, `subject(lead_id)` (`lead:<id>`), and the module doc on PII-free lead audit rows. The status values live in migration 0008's CHECK.
- `crates/wipe-hub/src/auth.rs:155` `JsonBody`; `ApiError::new` / `ApiError::internal`; `actor`.
- `crates/wipe-hub/migrations/0008_leads.sql` -- the style template; `lead_submissions` already cascades. The next migration is `0009`.
- `crates/wipe-hub/tests/admin.rs:92` `bootstrap_admin`, `:133` `admin_get`, `:500` `fixture()`. Lead tests need a vendor admin, so move `bootstrap_admin` (and the helpers it needs: `run`/`admin_create`/`reset_token`) into `tests/support/hub.rs`, or duplicate a minimal version in the new test file. Prefer moving it.
- `crates/wipe-hub/tests/leads.rs` -- how story 1 posts a lead (CSRF, body shape); reuse that to create fixtures through `POST /api/leads`. Backdate `consented_at` with SQL for the date test.
- `crates/wipe-hub/tests/isolation.rs:40` `ROUTES` -- add the six new routes as `Admin`. The sweep replaces `:id` with B's organization id and posts with no body; the admin extractor must refuse before the body is read (403).
- `crates/wipe-hub/tests/migrations.rs` -- add the `0009` check the way story 1 added `0008`.
- `apps/portal/src/api/admin.ts` -- `request`, `ensureOk`, `json`, the type guards and `retryAdminQuery`. `api/auth.ts:149` `request` adds the CSRF token on POST. `api/admin.test.ts` has the fake-fetch pattern.
- `apps/portal/src/pages/AdminPage.tsx` -- the `useSubmittedSearch` / `searchOptions` (`gcTime: 0`) pattern and the table styling. `AdminOrganizationPage.tsx` is the detail-page template. `AuthForm.tsx` has `Field`, `FormError` and `SubmitButton`.
- `apps/portal/src/main.tsx` -- add `/admin/leads` and `/admin/leads/$id` like `adminOrganizationRoute`.
- Docs: `CONTEXT.md` §10.2 (a new row; the sales-lead row's "No admin list… (story 2)" limitation; the admin-area row's "Views only") and §14 "Admin area" (no longer read-only). `README.md:48,50` (the hub and portal rows).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0009_lead_notes.sql` -- `lead_notes` (uuid PK, `lead_id` FK `ON DELETE CASCADE`, `author_id` FK users `ON DELETE SET NULL`, non-blank `body`, `created_at`, index on `lead_id`) with a comment header -- notes storage.
- `crates/wipe-hub/src/lead_admin.rs` (new, `pub mod` in `lib.rs`) -- `LeadFilter` parsing and validation with unit tests, `list`, `export_rows`, `detail`, `set_status`, `add_note`, `erase` (each takes `&VendorAdmin` and a connection and writes its own audit row), and `to_csv` with unit tests (quoting, CRLF, formula guard) -- the data layer.
- `crates/wipe-hub/src/admin_routes.rs` -- the six routes, the constants `MSG_NO_LEAD` and `MSG_BAD_FILTER`, and the CSV response headers -- the API.
- `crates/wipe-hub/tests/lead_admin.rs` (new) + `tests/support/hub.rs` (shared `bootstrap_admin`) + `tests/admin.rs` (use the shared helper) + `tests/isolation.rs` + `tests/migrations.rs` -- one test per I/O-matrix row.
- `apps/portal/src/api/adminLeads.ts` (+ `adminLeads.test.ts`) -- types mirroring the Rust shapes, guards, `listLeads`, `fetchLead`, `setLeadStatus`, `addLeadNote`, `eraseLead`, `exportLeads` (returns a `Blob` and the filename) -- the client.
- `apps/portal/src/pages/AdminLeadsPage.tsx`, `AdminLeadPage.tsx`, `AdminPage.tsx` (link), `main.tsx` (routes) -- the UI. When saving the export, revoke the object URL only after the click has been handled (`setTimeout(…, 0)` or later), never synchronously, or some browsers cancel the download.
- Tests compute "today" once, before submitting fixtures, and derive every date and the expected file name from it, so a run across UTC midnight cannot fail.
- `CONTEXT.md`, `README.md` -- docs in the same commit.

**Acceptance Criteria:**
- Given the hub, Mailpit, a vendor admin and a sales lead submitted through `POST /api/leads`, when the admin opens `/admin/leads` and presses Show leads, then the lead is listed. After its confirmation email's unsubscribe link is used, a new list run shows it as unsubscribed.
- Given the same admin, when they press Export CSV, then a `sales-leads-<date>.csv` file downloads holding the listed leads, and `admin.leads_exported` is in the audit log.
- Given a lead, when the admin erases it on `/admin/leads/$id` and confirms, then they return to the list, the lead is gone, and `SELECT … FROM audit_log WHERE detail::text ILIKE '%<email>%' OR subject ILIKE '%<email>%'` finds no row written by `lead.*` or `admin.lead*`.

## Spec Change Log

### 2026-11-08 — Review pass 1 (bad_spec loopback)
- **Triggering findings:**
  - Setting a lead's status to its current value returned the lead's personal data with no audit row (blind 1).
  - Erasure left the `mail:to:<address>` email-send throttle row, which `purge_stale` removes only on some later request; a locked one later audits the address after the erasure. The plan's "expire on their own" was wrong (blind 2, intent b).
  - Smaller items: no UTF-8 BOM (blind 4); object URL revoked before the download starts (blind 7, edge 1); unknown filter keys silently ignored (edge 2); export body type errors not `MSG_BAD_FILTER` (edge 3, edge 10); date-sensitive tests flaky across UTC midnight (edge 9); no test of note order or a removed author (gap 1); no test of a mutation racing an erasure (blind 6, gap 2).
- **Amended:**
  - Under the owner's decide-by-default rule: intent-contract decision 6 (revised), decisions 8–10 (added), and the status, erase, export and CSV rules made to agree with them.
  - Matrix rows added: the race, the status no-op, note order with a removed author, and unknown key / bad body.
  - Code Map: the attempt patch and `email_key`. Tasks: the revoke timing and the fixed "today" in tests.
- **Known-bad state avoided:** an admin reading a lead untraced; an erased person's address left in a hub table and re-audited later; garbled names in Excel; a misspelt filter exporting every lead.
- **KEEP** (right in the attempt-1 patch; must survive):
  - Migration `0009_lead_notes.sql` as written.
  - `lead_admin.rs`: module doc, `LeadFilter`/`FilterParams` parsing and `is_iso_date` with their unit tests, `lead_select`/`matching` SQL (lateral submission summary, any-submission date range, order), `lock` before every mutation, `list`/`export_rows`/`detail`/`set_status`/`add_note`/`erase`, `csv_field`/`to_csv` and their tests. Add the BOM to `to_csv` (update its tests) and `#[serde(deny_unknown_fields)]` to `FilterParams`.
  - `admin.rs`: `pub(crate)` on `RFC3339_UTC`, `audit_read`, `truncate`, and the doc change.
  - `admin_routes.rs`: module doc, constants, the six routes, the `VendorAdmin`-first extractor order, the CSV headers. The export handler takes its body as a `Result` and maps a rejection to `MSG_BAD_FILTER`.
  - `tests/lead_admin.rs` structure and its 13 tests (fix the "today" handling); `tests/support/hub.rs` shared `bootstrap_admin`; `tests/admin.rs`, `tests/isolation.rs` and `tests/migrations.rs` changes.
  - Portal: `api/adminLeads.ts` and its tests, both pages, the `/admin` link and the routes.
  - Docs: the CONTEXT §10.2 row, the sales-lead and admin-area row edits, §14 "Admin area", and README. Update the erasure limitation: the throttle row is now deleted, and only audit rows written before the erasure remain.

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 29 findings — high 0, medium 5, low 20, false 3, maybe-false 1
- findings:
  - `[medium]` `[bad_spec]` (blind 1) A status no-op returns lead data with no audit row — added decision 8: the no-op audits `admin.lead_viewed`.
  - `[medium]` `[bad_spec]` (blind 2) Erasure leaves the `mail:to:<address>` throttle row indefinitely, and a later purge of a locked row audits the address after the erasure — verified: `purge_stale` runs only inside `charge_mail` and sign-in/password failures (`auth.rs:698,804,1314`). Revised decision 6: erasure deletes the row.
  - `[low]` `[reject]` (blind 3) The export is uncapped and its audit row lists every id — a deliberate spec choice; pre-launch volumes are small, and streaming or capping is more than a direct fix.
  - `[low]` `[bad_spec]` (blind 4) No UTF-8 BOM, so Excel garbles non-ASCII names — added decision 9 and the CSV rule.
  - `[maybe-false]` `[reject]` (blind 5) The formula guard misses full-width `＝＋－＠` and a leading LF — would need checking which spreadsheet apps evaluate those; the guard follows the OWASP list, and if true it is only low.
  - `[low]` `[bad_spec]` (blind 6) No test of a mutation racing an erasure — grouped with gap 2; matrix row added. Its "last write wins between two admins" half is expected behaviour, not a defect.
  - `[low]` `[bad_spec]` (blind 7) Object URL revoked straight after `click()` — Tasks now require deferring the revoke.
  - `[false]` `[reject]` (blind 8) Note errors drift silently from `validate_message`'s wording — the integration tests pin "note must be at most 5000 characters", so a wording change fails loudly.
  - `[low]` `[reject]` (blind 9) Filters are lost when going back from a lead — real, but keeping filters in the URL is more than a direct fix, and one extra audited run is the cost.
  - `[low]` `[reject]` (blind 10) The detail page mixes date-only and full RFC 3339 timestamps — cosmetic; the full stamps carry `Z` (UTC) and are readable.
  - `[low]` `[reject]` (blind 11) The erase confirmation does not name the lead, and no request reference is stored — the confirmation sits on that lead's own page under its name; a data-subject request reference is a product choice outside the intent.
  - `[low]` `[reject]` (blind 12) An empty section banner in `tests/support/hub.rs`, and `AdminLeadPage` imports helpers from `AdminLeadsPage` — no named harm beyond tidiness.
  - `[low]` `[bad_spec]` (edge 1) Object URL revoked synchronously — grouped with blind 7.
  - `[low]` `[bad_spec]` (edge 2) Unknown filter keys are ignored, so a misspelt key lists or exports everything — added decision 10 and the matrix row.
  - `[low]` `[bad_spec]` (edge 3) An export body with a non-string value gets serde's 400 text, not `MSG_BAD_FILTER` — the spec says the export has the list's validation; the rule now names body rejections.
  - `[low]` `[reject]` (edge 4) An explicit `null` status or note body gets serde's 400 text — still a 400; our own portal never sends it, and a custom message adds a branch.
  - `[low]` `[reject]` (edge 5) An erasure committing in the middle of a detail read shows a lead without submissions — a milliseconds-wide window between two admins; the fix adds a lock mode to a shared read.
  - `[low]` `[reject]` (edge 6) Uncapped export memory and audit size — grouped with blind 3.
  - `[low]` `[reject]` (edge 7) `retryAdminQuery` retries non-`HubError` failures, each an audited request — pre-existing story 5 helper; only a malformed 200 body reaches it.
  - `[low]` `[reject]` (edge 8) An admin deleted between the extractor and the note insert gets a 500 — admins are removed only by SQL, and the window is one request.
  - `[low]` `[bad_spec]` (edge 9) Date tests can fail across UTC midnight — Tasks now fix "today" once per test.
  - `[low]` `[bad_spec]` (edge 10, claim) The export's bad-body errors contradict "a bad value is 400 `MSG_BAD_FILTER`" — grouped with edge 3.
  - `[low]` `[bad_spec]` (gap 1, pre-verified) Note order and the `author: null` path are untested through the API — matrix row added.
  - `[low]` `[bad_spec]` (gap 2, pre-verified) The "racing an erasure is a 404, never a 500" promise is untested — matrix row added.
  - `[medium]` `[defer]` (gap 3, pre-verified) The portal pages' audit-sensitive behaviour has no component test — the portal has no DOM test setup (story 1's unsubscribe page and story 5's admin pages share the gap); deferred.
  - `[medium]` `[defer]` (intent a) The intent's expectations sit at the portal pages, but tests stop at the API and the API client; the end-to-end success signal is not run — grouped with gap 3.
  - `[medium]` `[bad_spec]` (intent b) Under the strictest erasure reading, the throttle rows naming the address remain — grouped with blind 2.
  - `[false]` `[reject]` (intent c) The SPEC's singular "page" became two pages — every CAP-3 element is present; the split changes no requirement.
  - `[false]` `[reject]` (intent d) The date and notes readings are the implementer's narrowing — descriptive; they are recorded decisions 1 and 5 where the SPEC is silent.

### 2026-11-08 — Review pass
- verdicts: 27 findings — high 0, medium 1, low 24, false 2, maybe-false 0
- findings:
  - `[low]` `[patch]` (blind 1) Erasure also resets the email-send counter and any lock of a portal account with the same address, because `mail:to:` keys are shared — the cost decision 6 accepts, only an admin's erasure triggers it; CONTEXT now names the shared key.
  - `[low]` `[patch]` (blind 2) `throttle.rs` states "a cleared row with refusals is summarised" with no exception for the erasure's plain DELETE — one sentence added to its module doc.
  - `[low]` `[patch]` (blind 3) No test erases while the address's throttle row is locked, the case decision 6 exists for — the erase test now locks the row first and asserts no later `auth.mail_lock_summary` names the address.
  - `[low]` `[reject]` (blind 4) `MSG_BAD_FILTER` hand-lists the segments and statuses — adding one needs a migration and the TypeScript mirror anyway; no named failure today.
  - `[low]` `[reject]` (blind 5) The 400 text talks about values when the problem is an unknown key or body shape — still a 400 to an API caller; the portal never sends either.
  - `[low]` `[reject]` (blind 6) `audit_read` and `AdminReadError` now also serve writes — naming only; a rename is churn across story 5's code with no behaviour change.
  - `[low]` `[patch]` (blind 7) Save status is enabled when nothing changed, sending a request the hub audits as a view — Save is disabled while the value equals the current status.
  - `[low]` `[reject]` (blind 8) The list page posts `from` after `to`, and Export stays enabled after a failed run — the hub's 400 is shown; client-side checks add branches for a self-correcting mistake.
  - `[low]` `[reject]` (blind 9) The TypeScript segment and status lists are not pinned to Rust — AGENTS.md requires changing both together, and a new value needs a migration first.
  - `[low]` `[reject]` (blind 10) "Last submitted" may fall outside the filtered range — decision 1 shows the latest submission by design; showing the matching one is more than a direct fix.
  - `[low]` `[reject]` (blind 11) No index on `lead_submissions (lead_id, consented_at)` — no measured need at pre-launch volumes; an index can follow in its own migration.
  - `[low]` `[reject]` (blind 12) No "lead erased" confirmation after returning to the list, and an already-erased lead shows a 404 — cosmetic.
  - `[low]` `[reject]` (blind 13) The note box has no `maxLength`, and a whitespace-only note reaches the hub — the hub answers with a clear 400; a matching client limit has UTF-16 counting differences.
  - `[low]` `[reject]` (edge 1) carried: pass 1 edge 5 — an erasure mid-way through a detail read; same location and claim.
  - `[low]` `[reject]` (edge 2) carried: pass 1 edge 7 — `retryAdminQuery` retries non-`HubError`s.
  - `[low]` `[patch]` (edge 3) Erasure resets the shared email-send limit — grouped with blind 1.
  - `[low]` `[reject]` (edge 4) carried: pass 1 blind 3 — the uncapped export.
  - `[low]` `[reject]` (edge 5) The formula guard prefixes valid values like `+tag@x.com` or `-ish Ltd` with `'` — the trade-off the OWASP CSV Injection guidance accepts; such values are rare.
  - `[low]` `[reject]` (edge 6, claim) A non-UUID id with a malformed body gets 400, not 404 — both are refusals with no data or audit; extractor order (admin first, then body) is the spec's own rule.
  - `[low]` `[patch]` (gap 1, pre-verified) No erase test uses a mixed-case address, so dropping the key's `lower()` would pass — test added.
  - `[low]` `[patch]` (gap 2, pre-verified) Only the notes POST is raced against an erasure — the race test now covers status and erase too.
  - `[low]` `[patch]` (gap 3, pre-verified) The UTC day boundaries of the date filter are untested — submissions at 00:00:00 and 23:59:59 added.
  - `[low]` `[patch]` (gap 4, pre-verified) The portal client test never checks that the BOM survives into the downloaded blob — fixture and byte assertion added.
  - `[medium]` `[defer]` (intent a) carried: pass 1 gap 3 and intent a — no component or browser test for the portal pages or the end-to-end success signal; already deferred.
  - `[false]` `[reject]` (intent b) Auditing every list, view and change goes beyond "every export is audit-logged" — it adds the admin-area convention (ADR-0007 §6); nothing the intent asks for is missing.
  - `[low]` `[reject]` (intent c) Under the strictest erasure reading, audit rows written before the erasure that name the address remain — the audit log is append-only (ADR-0007 §6); removing them would contradict the ADR, and the limitation is in CONTEXT §10.
  - `[false]` `[reject]` (intent d) Two pages instead of one, and a settable status — every CAP-3 element is present; recorded judgement calls.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including `tests/lead_admin.rs` and the isolation guard
- `cargo test --workspace` -- expected: passes (never piped through grep)
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean
- `cd apps/portal && pnpm typecheck && pnpm test && pnpm build` -- expected: pass
- `rustfmt --edition 2021 <touched .rs files>` -- touched files only

## Auto Run Result

**Status:** done (2 review passes; 1 bad_spec loopback)

**Summary:** Vendor admins can now work the sales-lead list in the portal.
- `/admin/leads` lists leads filtered by segment (or none), status and submission date, running only when the form is submitted. Export CSV downloads every matching lead.
- `/admin/leads/$id` shows each submission with its exact consent text and time, the status (settable), append-only notes with their author, and erase behind a second press.
- The hub routes are `/api/admin/leads*`, for vendor admins only, through the existing `VendorAdmin` extractor.
- Every list, view, status change, note, export and erasure is audited in its own transaction, with ids, filters, counts and status names only, so an erased lead leaves no personal data in the audit log.
- The export is a CSRF-protected POST: RFC 4180 CSV with a UTF-8 byte-order mark, a formula-injection guard and lead-row columns only.
- Erasure deletes the lead, its submissions, its notes and its address's email-send throttle row.

**Files changed:**
- Hub:
  - `migrations/0009_lead_notes.sql` — the notes table.
  - `src/lead_admin.rs` — filters, list, export, detail, status, notes, erasure and CSV.
  - `src/admin_routes.rs` — the six routes.
  - `src/admin.rs`, `src/auth.rs` — `pub(crate)` helpers.
  - `src/lib.rs` — the module.
  - `src/throttle.rs` — a doc note on the erasure exception.
- Hub tests:
  - `tests/lead_admin.rs` — 17 tests.
  - `tests/support/hub.rs` — a shared `bootstrap_admin`.
  - `tests/admin.rs`, `tests/isolation.rs`, `tests/migrations.rs`.
- Portal:
  - `api/adminLeads.ts` and its tests.
  - `pages/AdminLeadsPage.tsx`, `pages/AdminLeadPage.tsx`.
  - `pages/AdminPage.tsx` (link), `pages/AuthForm.tsx` (`SubmitButton` `disabled`), `main.tsx` (routes).
- Docs: `CONTEXT.md` (§10.2 row, sales-lead and admin-area rows, §14 "Admin area"), `README.md`.
- `_bmad-output/implementation-artifacts/lead-capture-story-2-attempt-1.patch` — the reverted first attempt.

**Review findings:**
- **Pass 1:** 29 findings, routed to bad_spec and re-derived:
  - the status no-op returned lead data unaudited;
  - erasure left the address's throttle row;
  - the CSV byte-order mark, revoke timing, unknown filter keys and export body errors;
  - missing tests (race, note order and removed author, midnight flakiness).

  Deferred: no component tests for the portal pages. Rejected with reasons in the triage log: the uncapped export, the full-width formula characters, filter persistence, timestamp display, the erase confirmation, test-helper tidiness, null bodies, the detail race window, retries and a deleted author.
- **Pass 2:** 27 findings. Patches applied, all `low`:
  - a CONTEXT clause on the shared `mail:to:` key;
  - a `throttle.rs` doc note;
  - tests for a locked throttle row, a mixed-case address, races on status and erase, UTC day boundaries, and the byte-order mark in the portal blob;
  - Save status disabled when unchanged.

  Rejected with reasons in the triage log; 3 rows carried from pass 1.
- **Patched counts (pass 2):** high 0, medium 0, low 8.

**Follow-up review recommendation:** false. No high-verdict patch, and no two medium-verdict patches.

**Verification:**
- `cargo test --workspace`: exit 0, with `tests/lead_admin.rs` at 17 passed and the isolation guard passing.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean.
- Portal typecheck, 97 tests and build pass.
- `rustfmt --edition 2021 --check` is clean on the touched files.

**Residual risks:**
- The portal pages have not been clicked through in a real browser and have no component tests (deferred). The acceptance criteria are proven at the hub API.
- What sales may do with a direct message from an unsubscribed sales lead is still an open owner decision. The pages show "Unsubscribed <date>" with no instruction.
- Erasure cannot reach CSVs already downloaded, backups, or audit rows written before it that name the address (the log is append-only, ADR-0007 §6).
