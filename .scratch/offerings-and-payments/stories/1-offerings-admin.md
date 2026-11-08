---
title: 'Offerings admin'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '7c74ad2975289173f3a110e29059770653198bf3'
review_loop_iteration: 0
followup_review_recommended: false
context:
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
  - '{project-root}/.scratch/offerings-and-payments/SPEC.md'
warnings: [oversized]
deferred:
  - summary: >-
      Offering prices are ISO 4217 minor units chosen by the browser's Intl fraction digits; the hub holds no minor-unit table, and Stripe represents some currencies (ISK, UGX) with two decimals where ISO has none.
    evidence: |-
      apps/portal/src/api/adminOfferings.ts fractionDigits uses Intl; Stripe documents ISK/UGX as special cases needing x100 amounts. Harmless until story 3's Stripe provider converts price_minor to a Stripe amount; settle by a conversion table plus tests there.
    location: >-
      apps/portal/src/api/adminOfferings.ts:133
    severity: medium (unverified)
---

<intent-contract>

## Intent

**Problem:** The vendor has nothing to sell: ADR-0007 §4 makes offerings data that vendor admins manage, but the hub has no offerings table, no admin routes and no portal pages, so checkout (story 2) and the pricing page (story 5) have nothing to read.

**Approach:** Add an `offerings` table, an `offering_admin` data layer behind `/api/admin/offerings*` (vendor admins only, every read and change audited in its own transaction), and portal pages `/admin/offerings` and `/admin/offerings/$id` to create, edit, publish and retire offerings. An offering has a name, description, price (integer minor units + ISO 4217 currency), interval (`one_off` | `annual`) and grants (licence scope `machine` | `site`, seats, erasure quota or unlimited, `wipe_license::Feature` entitlements, term in months).

## Boundaries & Constraints

**Always:**
- Lifecycle `draft → published → retired`; only those two transitions; `retired` is terminal. New offerings are `draft`.
- Only the `VendorAdmin` extractor (existing, `admin_routes.rs`) admits a caller; it runs before path, query or body.
- Every response carrying offering data and every change writes one audit row (actor `user:<admin id>`, subject `offering:<id>`, or `search:offerings` for the list) in the same transaction (ADR-0007 §6; the admin area's existing "every search, view, export and change is audited" rule). Offerings hold no personal data, so audit details may carry offering values.
- A published or retired offering's commercial terms (price, currency, interval, grants) never change; a published offering's name and description may. A retired offering cannot change at all.
- Money is integer minor units (`bigint`) + uppercase 3-letter currency code; no floats anywhere, including the portal's price input.
- Features are validated against `wipe_license::Feature` (serde snake_case names), stored sorted and de-duplicated.
- Mutations lock the row (`SELECT … FOR UPDATE`) first.

**Never:**
- No customer-facing or public offering read (story 2 checkout, story 5 `GET /api/offerings`); no orders, payments, entitlements.
- No deletion of offerings, no un-retire, no re-publish.
- No editing an applied migration; no `sqlx::query!` macros.
- No feature list typed as free text: the portal mirrors the vocabulary by hand (`adminOfferings.ts`) per AGENTS.md mirroring rule.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Create | vendor admin `POST /api/admin/offerings` valid body | 201 `{offering}` status `draft`, features sorted/deduped, currency uppercased; audit `admin.offering_created {offering}` | — |
| Invalid body | blank name; name > 100 chars; description > 2000; price ≤ 0 or > 99 999 999; currency not 3 letters; interval/scope unknown; seats < 1 or > 10 000; quota ≤ 0; unknown feature; term < 1 or > 120; annual with term ≠ 12; unknown key | 400 `{error}` naming the field | no row, no audit |
| Duplicate name | another non-retired offering has the same name (case-insensitive) | 409 `an offering with this name is already in draft or for sale` | no audit |
| Edit draft | `POST /api/admin/offerings/:id` full body | 200 `{offering}`; audit `admin.offering_updated {changes: {field: {from, to}}}`; no-op edit audited `admin.offering_viewed` | — |
| Edit published terms | price/currency/interval/grants differ | 409 `a published offering's price, interval and grants cannot change; retire it and create a new one` | nothing written |
| Edit published copy | only name/description differ | 200, audited as update | — |
| Edit retired | any body | 409 `a retired offering cannot change` | — |
| Publish | draft | 200 `{offering}` status `published`, `published_at` set; audit `admin.offering_published {}` | non-draft → 409 `only a draft offering can be published` |
| Retire | published | 200, status `retired`, `retired_at` set; audit `admin.offering_retired {}` | non-published → 409 `only a published offering can be retired` |
| List | `GET /api/admin/offerings` | `{offerings}` all statuses: draft, published, retired, then lower(name), id; audit `admin.offerings_listed {offering_ids}` | — |
| View | `GET /api/admin/offerings/:id` | `{offering}`; audit `admin.offering_viewed {}` | unknown or non-UUID id → 404 `no such offering`, no audit |
| Not admin | no session / customer owner or member | 401 `not signed in` / 403 `only vendor admins can use the admin area` | no audit |

## Autonomous decisions

1. **Annual offerings have a 12-month term.** *About:* a vendor admin defining what an annual subscription grants. *Problem:* term and interval are separate fields, so an admin could publish "annual, 1-month term" and every buyer's licence would lapse 11 months before renewal (stations keep erasing but certificates turn `evaluation`). *Options:* (a) independent fields; (b) annual forces term = 12; (c) annual needs term ≥ 12. *Choice:* (b), the safe default protecting customers' certificates; renewal (story 4) then extends by exactly the term. One-off offerings take any term 1–120 months. Widening later is non-breaking.
2. **Commercial terms freeze at publication.** *Problem:* if a published price or grant could be edited, what a customer bought would depend on when they clicked, and the audit trail would be ambiguous. *Options:* editable; freeze terms but allow copy edits; freeze everything. *Choice:* freeze price, currency, interval and grants once published (Stripe's own convention: Prices are immutable); name and description stay editable while published. Changing a price means retiring and creating a new offering. Orders (story 2) should still snapshot terms.
3. **Price must be 1 to 99 999 999 minor units.** Free licences need no purchase (an unlicensed station still erases, ROADMAP §0.2), and 99 999 999 is Stripe Checkout's documented maximum amount. Currency is checked for format only (3 letters, stored uppercase), not against the ISO list; recorded as a limitation.
4. **Offering reads are audited too**, matching the admin area's existing rule, so the portal updates pages from each change's response rather than refetching (as `/admin/leads/$id` does).
5. **No published-only customer read in this story.** CAP-1's "only published offerings are visible to customers" is met because no customer can read any offering yet (every route is vendor-admin only); stories 2 and 5 add the customer reads and must filter on `published`.
6. **Seats 1–10 000, quota null (unlimited) or 1–10⁹ erasures, names unique among non-retired offerings** (case-insensitive partial unique index), so the pricing page never shows two offerings with one name.

</intent-contract>

## Code Map

- `crates/wipe-hub/src/lead_admin.rs` -- template data layer: `&VendorAdmin` + `&mut PgConnection`, `audit_read`, `lock … FOR UPDATE`, `RFC3339_UTC` `to_char` timestamps, `Option` = 404 with no audit.
- `crates/wipe-hub/src/admin.rs:62-220` -- `RFC3339_UTC`, `VendorAdmin`, `AdminReadError`, `audit_read` (pub(crate)), `truncate`.
- `crates/wipe-hub/src/admin_routes.rs` -- `VendorAdmin` extractor; handler shape (admin extractor first, `pool.begin()`, data-layer call, `commit`); `JsonBody` from `auth.rs:155` (400/415 `{error}`); `ApiError::new(StatusCode, msg)`.
- `crates/wipe-hub/src/accounts.rs:179` -- `validate_name(field, value)` (trim, non-blank, ≤ `NAME_MAX_CHARS`, no control chars): reuse only if `NAME_MAX_CHARS` == 100, else write an offering-specific check with limit 100.
- `crates/wipe-hub/src/leads.rs:143` -- `validate_message` pattern (allows `\n\r\t`) for the description rule.
- `crates/wipe-license/src/entitlement.rs:14-65` -- `Quota`, `Scope`, `Feature` (`EnterpriseMode`, `HubSync`, `ErpIntegration`, `PdfCertificates`, `CertAnchoring`, serde snake_case); `LicenseCertificate.not_after` is required, so every offering has a term.
- `crates/wipe-hub/migrations/0009_lead_notes.sql` -- latest migration; next is `0010_offerings.sql`.
- `crates/wipe-hub/src/lib.rs` -- module list and `router` merge.
- `crates/wipe-hub/tests/isolation.rs:40-130` -- `ROUTES` classification table; the guard fails on any unclassified `/api` path literal in `src/`.
- `crates/wipe-hub/tests/lead_admin.rs` + `tests/support/hub.rs` -- `spawn_hub`, `bootstrap_admin`, `Who`, `assert_error`, `post_req` helpers for integration tests.
- `apps/portal/src/api/adminLeads.ts` (+ `.test.ts`) -- client pattern: hand-mirrored types, `isX` guards, `request`/`ensureOk`/`json` from `./auth`.
- `apps/portal/src/pages/AdminLeadsPage.tsx`, `AdminLeadPage.tsx`, `AdminPage.tsx` -- page patterns; `/admin` links to sub-areas.
- `apps/portal/src/main.tsx:170-200` -- admin routes.
- `CONTEXT.md` §10.2 (rows 697-699 style), §14 glossary (Admin area entry); `docs/ROADMAP.md:316-318` (A1.3 vendor console, partial).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0010_offerings.sql` -- create `offerings` (uuid id; name, description; `price_minor bigint` CHECK 1..99999999; `currency` CHECK `^[A-Z]{3}$`; `billing_interval` CHECK in (`one_off`,`annual`); `status` CHECK in (`draft`,`published`,`retired`) default `draft`; `scope` CHECK in (`machine`,`site`); `seats int` CHECK 1..10000; `quota_erasures bigint NULL` CHECK > 0; `features text[] NOT NULL`; `term_months int` CHECK 1..120 and annual ⇒ 12; `created_by uuid` → users ON DELETE SET NULL; `created_at`, `updated_at`, `published_at`, `retired_at`; CHECKs tying `published_at`/`retired_at` to status) and partial unique index on `lower(name)` WHERE status <> 'retired' -- schema-level backstop for every rule.
- `crates/wipe-hub/src/offering_admin.rs` -- `OfferingRequest` (deny_unknown_fields), `validate` → `NewOffering` (400 messages), `Offering`/`Grants` serde shapes, `FEATURES` const with an exhaustive-match test against `Feature`; `list`, `detail`, `create`, `update`, `publish`, `retire` per the matrix; unit tests for validation.
- `crates/wipe-hub/src/admin_routes.rs` -- six routes, `MSG_NO_OFFERING`, map unique-violation to 409 and lifecycle errors to 409; extend the module doc.
- `crates/wipe-hub/src/lib.rs` -- `pub mod offering_admin;`.
- `crates/wipe-hub/tests/offering_admin.rs` -- one test per matrix row through HTTP, asserting response, stored row and audit rows (count, action, detail).
- `crates/wipe-hub/tests/isolation.rs` -- classify the six routes `Admin`.
- `apps/portal/src/api/adminOfferings.ts` + `.test.ts` -- client, mirrored types, `FEATURES`, `toMinorUnits(text, currency)` / `formatPrice(minor, currency)` using Intl fraction digits with string arithmetic; tests for parsing (`"12"`, `"12.5"`, `"12.345"` rejected for USD, JPY no decimals) and every call's path/method/body.
- `apps/portal/src/pages/AdminOfferingsPage.tsx`, `AdminOfferingPage.tsx`, shared `OfferingForm.tsx`; `apps/portal/src/main.tsx` routes; link from `AdminPage.tsx` -- list with status, new-offering form, detail with edit (term fields disabled once published), publish and retire behind a second confirming press.
- `CONTEXT.md` (§10.2 row, §14 **Offering** entry, Admin area entry), `docs/ROADMAP.md` (A1.3 note: offerings part shipped) -- docs travel with code.

**Acceptance Criteria:**
- Given a vendor admin, when they create, publish and retire an offering through the portal API, then each step returns the offering with the new status and writes exactly one audit row with the documented action.
- Given a customer owner or member, when they call any `/api/admin/offerings*` route, then they get 403 and no offering data, and `tests/isolation.rs` classifies all six routes.
- Given a published offering, when an admin posts a changed price, then the hub answers 409 and the stored offering and audit log are unchanged.
- Given the portal, when `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` run, then all pass.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 20 findings — high 0, medium 2, low 11, false 1, maybe-false 1 (5 intent-audit/duplicate rows graded low and rejected)
- findings:
  - `[medium]` `[patch]` Publish/retire with unsaved form edits left the locked terms fields showing unsaved values (Blind) — fixed: both actions bump `formKey` so the form remounts from the returned offering.
  - `[low]` `[reject]` No optimistic-concurrency token on edit/publish (Blind) — needs two admins acting at once on one offering, recoverable by retire-and-recreate; fix adds API surface.
  - `[maybe-false]` `[defer]` Minor units decided by browser Intl, not the hub; Stripe differs for ISK/UGX (Blind) — matters only when story 3 converts to Stripe amounts; deferred to frontmatter.
  - `[low]` `[reject]` No Stripe minimum amount / supported-currency check at publish (Blind) — an admin would have to type e.g. 0.01 USD; fix needs a provider limits table, belongs with the provider stories; listed as residual risk.
  - `[low]` `[reject]` Drafts cannot be discarded (Blind) — drafts are fully editable (rename frees the name); fix changes the intent contract's lifecycle.
  - `[low]` `[reject]` Grants lack `allowed_methods` (Blind) — SPEC CAP-1 enumerates grants without it; story 4 fulfilment must choose (residual risk).
  - `[low]` `[patch]` `seats` undefined (Blind) — fixed: CONTEXT.md Offering entry defines seats per ADR-0007 §5.
  - `[low]` `[reject]` Trigger backstop misses TRUNCATE, id rewrite, timestamp edits (Blind) — reachable only by direct SQL as the table owner; adding guards for paths the hub never takes.
  - `[low]` `[reject]` Portal rejects the whole list on an unknown feature; no cross-language mirror check (Blind) — the established hand-mirroring convention (AGENTS.md; `isAdminLead` does the same); fix adds infrastructure.
  - `[low]` `[reject]` `OfferingForm` has no tests (Blind) — portal has no page-test harness (pages are verified by typecheck/build, as in prior stories); the concrete bug it would catch is patched above.
  - `[low]` `[patch]` Admin copy overclaims ("on sale", "existing purchases unaffected") and a garbled list sentence (Blind) — fixed: reworded per review.
  - `[false]` `[reject]` Decisions 1–3 are pricing decisions that needed the owner (Blind) — they set no price or tier; they constrain input validity (annual-term coherence, Stripe's documented maximum, Stripe's immutable-price convention), within decide-by-default.
  - `[medium]` `[patch]` Publish with unsaved edits (Edge Case Hunter) — same root cause as the first row; fixed there.
  - `[low]` `[reject]` Client-side sort after create differs from Postgres collation (Edge Case Hunter) — display order only, corrected on reload.
  - `[medium]` `[patch]` No test that mutations lock the row (Verification Gap) — fixed: `an_edit_racing_a_publish_is_a_409_not_a_500`, confirmed to fail (500) without `FOR UPDATE`.
  - `[low]` `[reject]` Client sort vs collation (Verification Gap, other) — duplicate of the Edge Case Hunter row.
  - `[low]` `[reject]` Admin pages not tested at their UI surface (Intent audit) — same as the OfferingForm row.
  - `[low]` `[reject]` Published-only customer filter not enforced (Intent audit) — no customer read exists; stories 2 and 5 own it (autonomous decision 5).
  - `[low]` `[reject]` "Retired keeps orders intact" covered only by proxy (Intent audit) — orders do not exist yet; offerings are never deleted and terms freeze.
  - `[low]` `[reject]` Constraints beyond SPEC (frozen terms, annual term, audited reads) (Intent audit) — recorded autonomous decisions, not defects.

## Design Notes

Audit `admin.offering_updated` detail is a diff of changed fields only, e.g. `{"changes": {"price_minor": {"from": 120000, "to": 99000}}}`; `grants` diffs as one field with whole before/after objects. `admin.offering_created` carries the full created offering.

JSON shape (request omits id/status/timestamps):
```json
{"id":"…","name":"Station — annual, 1 seat","description":"…","price_minor":120000,
 "currency":"USD","interval":"annual","status":"draft",
 "grants":{"scope":"machine","seats":1,"quota_erasures":null,"features":["pdf_certificates"],"term_months":12},
 "created_at":"…Z","updated_at":"…Z","published_at":null,"retired_at":null}
```

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including `offering_admin` and `isolation`.
- `cargo test --workspace` -- expected: all pass.
- `cargo clippy -p wipe-hub --all-targets -- -D warnings` -- expected: clean.
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: all pass.

## Auto Run Result

Status: done

**Summary:** Vendor admins can create, edit, publish and retire offerings (name, description, price in minor units + currency, interval, grants: scope, seats, quota or unlimited, features, term) in the hub and portal. Every read and change is audited in its own transaction. Price, currency, interval and grants freeze once an offering is published. A database trigger and CHECK constraints back up the hub's rules.

**Files changed:**
- `crates/wipe-hub/migrations/0010_offerings.sql`: the `offerings` table, the name uniqueness index and the lifecycle trigger.
- `crates/wipe-hub/src/offering_admin.rs`: validation and the data layer (list, detail, create, update, publish, retire).
- `crates/wipe-hub/src/admin_routes.rs`, `src/lib.rs`: six `/api/admin/offerings*` routes.
- `crates/wipe-hub/tests/offering_admin.rs`: 15 HTTP tests, one per matrix row plus a lock race and a schema check.
- `crates/wipe-hub/tests/isolation.rs`: the new routes classified as admin-only.
- `apps/portal/src/api/adminOfferings.ts` (+ test): the client, mirrored types and float-free price parsing.
- `apps/portal/src/pages/AdminOfferingsPage.tsx`, `AdminOfferingPage.tsx`, `OfferingForm.tsx`, `main.tsx`, `AdminPage.tsx`: the portal pages.
- `CONTEXT.md`, `README.md`, `docs/ROADMAP.md`: the §10.2 row, the Offering glossary entry and the A1.3 note.

**Review:** 4 patches (2 medium, 2 low), 1 deferred (Stripe minor-unit mapping, for story 3), 15 rejected, with reasons in the triage log. Follow-up review recommended: false. Two medium entries were patched, but they share one root cause, and the other medium was a test-only addition.

**Verification:**
- `cargo test --workspace`: exit 0, 455 passed, 0 failed (compose Postgres and Mailpit up).
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean. Without `--no-deps`, clippy fails on a pre-existing `large_enum_variant` in the untouched `crates/wipe-common/src/job.rs`.
- rustfmt `--check` on the touched Rust files: clean.
- Portal typecheck, test and build: all pass.

**Residual risks:**
- The portal pages have not been exercised in a real browser.
- Nothing enforces Stripe's minimum amount or its supported-currency list.
- Grants carry no `allowed_methods`; story 4 must choose one.
- No customer read exists yet; stories 2 and 5 must filter on `published`.
- Currency is checked for format only.

