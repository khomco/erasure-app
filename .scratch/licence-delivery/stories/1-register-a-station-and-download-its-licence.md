---
title: 'Register a station and download its licence'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: 'a089113f639399c316d125726b0f8ab415465aec'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '{project-root}/docs/adr/0005-licensing-and-attestation.md'
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
warnings: [oversized]
deferred: []
---

<intent-contract>

## Intent

**Problem:** A paying customer has seats (entitlements, offerings-and-payments story 4) but no way to turn one into a licence file for a station; today only the vendor's CLI (`wipestation license issue`) can sign one. Seats are not enforced anywhere (SPEC CAP-1, CAP-2, CAP-5).

**Approach:** Add stations and licences to the hub: an owner registers a station against one of the organization's current entitlements, the hub checks a seat is free and signs a licence with `wipe_license::VendorRoot::issue` from the configured vendor root, stores the exact file, and offers it for download. Releasing a station frees its seat and marks its licence superseded. A portal "Stations" page drives it and shows how to read a station's instance key id and the vendor root public key.

## Boundaries & Constraints

**Always:**
- Sign only through `wipe_license` (`VendorRoot::issue`, `machine_fingerprint`); no copy of signing logic. The stored file is exactly the JSON `wipe_license::save_license` would write (`serde_json::to_string_pretty`), served byte-for-byte on every download.
- Entitlements in the licence: `customer_id` = organization id, `customer_name` = organization name, `quota` from `quota_erasures` (`null` → `Unlimited`), `features` parsed from the entitlement's feature names, `allowed_methods: All`, `not_before` = `issued_at` = now, `not_after` = the entitlement's `term_end`. Machine scope → `Scope::Machine { fingerprint: machine_fingerprint(station_id) }`, `machine_binding: None`. Site scope → `Scope::Site { site_id }` with the entered site, or the organization id when none.
- Seat check, row inserts, licence insert and audit rows commit in one transaction with the entitlement row locked `FOR UPDATE`, so concurrent registrations cannot exceed seats. Seats used = stations on that entitlement not released.
- Only a current entitlement (not refunded, `term_end` > now) can take a registration. Every query is scoped to the session's organization; another organization's station/entitlement is a 404 like an unknown one.
- Audit (actor `user:<id>`): `station.registered` (subject `station:<id>`), `licence.issued` (subject `licence:<id>`, detail incl. `station_id`, `entitlement_id`, `instance_key_id`, `root_key_id`, `not_after`), `station.released`, `licence.superseded` (`reason: "station released"`), `licence.downloaded`.
- No vendor root configured → registration 503 with a clear message, nothing written. ROADMAP §0.2: nothing here can stop a station erasing.
- Portal states plainly that a released station's licence file keeps working until its own expiry: the hub does not distribute revocation or supersession to stations.

**Never:** No station-side code changes; no revocation, renewal reissue, admin pages or lapse policy (stories 2–4); no online activation; never edit an applied migration; no hardware-sanitization claims in copy.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Register (machine) | owner, current machine entitlement with free seat, valid `instance_key_id`, `station_id` | 201 `{station, licence}`; licence verifies against root, names the key, fingerprint = `machine_fingerprint(station_id)`, `not_after` = term end; 2 audit rows | — |
| Register (site) | site entitlement, `site` given or blank | `Scope::Site` with site or org id | — |
| No free seat | all seats used | 409 `MSG_NO_FREE_SEAT`, nothing written | clear message |
| Machine scope, no station id | `station_id` blank | 400 `MSG_STATION_ID_REQUIRED` | nothing written |
| Bad key id | not `ed25519:` + 22 base64 chars | 400 `MSG_BAD_INSTANCE_KEY` | nothing written |
| Already registered | same key active in this org | 409 `MSG_ALREADY_REGISTERED` | nothing written |
| Not current / unknown / other org's entitlement | refunded, ended, or foreign id | 409 `MSG_ENTITLEMENT_NOT_CURRENT` / 404 `MSG_NO_ENTITLEMENT` | nothing written |
| Member registers or releases | role member | 403 owner-only message | nothing written |
| No vendor root | hub started without seed | 503 `MSG_ISSUANCE_UNAVAILABLE` | nothing written |
| Release | owner, active station | 200; station released, licence `superseded`, seat free again, 2 audit rows | re-release → 409 `MSG_ALREADY_RELEASED` |
| Download | any role, own station's licence | 200 `application/json`, `Content-Disposition: attachment`, exact stored bytes, audited | foreign/unknown → 404 |

## Autonomous decisions

- **Machine-scoped licences need the station's id, not just its key.** A machine-scope licence must name a machine fingerprint, and a station derives that from its `--station-id` (`standalone` if unset); a licence with any other fingerprint installs but signs only `evaluation` certificates. Options: (a) ask for the station id when the entitlement is machine-scoped, (b) change `wipe-license` so machine scope binds only the key, (c) leave fingerprint unbound. Chosen (a): it is what ROADMAP §4.1 already specifies (bound to instance key and fingerprint), needs no change to the signed licence format, and the form explains where the id comes from.
- **Who may register and release.** Seats are paid for; registering consumes one and releasing leaves a working licence file in the wild until expiry. Options: any role, or owners only. Chosen owners only (least privilege, matches owner-only buying); every role can view and download.
- **Which entitlement a station draws from.** An organization may hold several entitlements with different terms and features. Chosen: the owner picks one explicitly (preselected when only one has a free seat); the licence mirrors that entitlement exactly, so nothing is guessed.
- **Duplicate keys.** Same instance key registered twice in one organization → 409 (would burn a seat for nothing). Not checked across organizations, which would leak that another customer holds the key; the licence is useless without the station's private key anyway.
- **Site id for site-scoped licences** defaults to the organization id when the owner leaves "site" blank, so two organizations never share a site id by accident.

</intent-contract>

## Code Map

- `crates/wipe-license/src/chain.rs:66-120` -- `VendorRoot::issue`, `LicenseCertificate::verify`; `install.rs` `check_installable`, `save_license` format; `lib.rs:63` `machine_fingerprint`; `entitlement.rs` `Entitlements`, `Quota`, `Scope`, `Feature` (serde snake_case names = hub `FEATURES`).
- `crates/wipe-cli/src/main.rs:716-756` -- station's `--license`/`--vendor-root-b64` path (read-only reference: `check_installable` then `LicenseContext::licensed` → `evaluate` with `machine_fingerprint(station_id)`, station id default `"standalone"`).
- `crates/wipe-hub/src/config.rs:93,151-171,237,331` -- `vendor_root: Option<FileVendorRoot>` already loaded with its file-held warning; reuse.
- `crates/wipe-hub/src/entitlements.rs` -- `Entitlement`, `list`, `lock_for_organization` (pattern), `subject`; `current` = `status <> 'refunded' AND term_end > now()`.
- `crates/wipe-hub/src/order_routes.rs` -- route module pattern, `ApiError`, `CurrentUser`, `require_owner`, `MSG_NO_ENTITLEMENT`.
- `crates/wipe-hub/src/audit.rs` `record`; `auth.rs:292` `actor(user_id)`.
- `crates/wipe-hub/src/lib.rs:84` -- `router`: merge new routes.
- `crates/wipe-hub/migrations/0013_entitlements.sql` -- trigger/CHECK style to mirror.
- `crates/wipe-hub/tests/entitlements.rs`, `tests/support/hub.rs` -- fixture (`verified_owner`, `add`, `published_offering`, buy via Fake), `spawn_hub_args` (`vendor_root_seed`), `audited`, `assert_error`.
- `apps/portal/src/pages/SeatsPage.tsx`, `api/orders.ts`, `main.tsx:199,286`, `pages/HomePage.tsx:128` -- page/API/route patterns.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0014_stations.sql` -- `stations` (org, entitlement, instance_key_id, station_id, label, site, registered_by/at, released_by/at; partial unique (org, instance_key_id) where not released; never deleted, only release may change) and `licences` (id = licence id, org, station, entitlement, instance_key_id, root_key_id, not_after, issued_at, `document text`, status `active|superseded`, superseded_at; never deleted, only active→superseded) -- DB enforces lifecycle like 0013.
- `crates/wipe-hub/src/stations.rs` -- validation, `register` (lock, seat count, build `Entitlements`, `issue`, insert, audit), `release`, `list`, `licence_document`; message constants -- one writer for both tables.
- `crates/wipe-hub/src/station_routes.rs` -- `GET/POST /api/stations`, `POST /api/stations/:id/release`, `GET /api/licences/:id/download`, `GET /api/licensing/root` (`{root: {public_key_id, public_key_b64} | null}`); wire into `lib.rs` -- HTTP surface.
- `crates/wipe-hub/tests/stations.rs` -- one test per I/O row through HTTP, incl. downloaded file → `load`/`check_installable` with root + `evaluate(.., machine_fingerprint(station_id), now)` is `Valid`, org isolation, and concurrent registrations on a 1-seat entitlement yield exactly one 201.
- `apps/portal/src/api/stations.ts` (+ `.test.ts`) -- typed client mirroring the shapes.
- `apps/portal/src/pages/StationsPage.tsx` (+ test), `main.tsx`, `HomePage.tsx`, `SeatsPage.tsx` -- `/stations`: list, register form (entitlement select, key id, station id, label, site), download, release, how-to-read key id, vendor root key, supersession note.
- `CONTEXT.md` §10, `docs/ROADMAP.md`, `AGENTS.md` test counts if changed -- record what shipped.

**Acceptance Criteria:**
- Given a customer who bought one machine seat and a station started with `--station-id bench-1`, when the owner registers the station's key with station id `bench-1` and downloads the file, then `wipestation serve --license <file> --vendor-root-b64 <root>` accepts it and its certificates are licensed to the organization (verified by the library-level test and the manual check).
- Given the portal Stations page, when any member opens it, then it shows how to read a station's instance key id (`GET /api/public_key` → `public_key_id`, the `signing key public id:` startup line) and the vendor root public key, and says released licences keep working until expiry.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 26 findings — high 0, medium 8, low 14, false 4, maybe-false 0
- findings:
  - `[medium]` `[patch]` (blind) Expired active licence still reads "Licensed to <past date>" — `stationState` checked only `status === "active"`; now shows "Expired on <date>" when `not_after` has passed.
  - `[medium]` `[patch]` (blind) Hidden `site` posted for a machine registration after switching entitlement — form now sends `site` only for site scope.
  - `[low]` `[patch]` (blind) Register form enabled with no warning while vendor-root query pending/failed — now `root.isSuccess && root.data !== null`.
  - `[low]` `[reject]` (blind) DB does not force a licence's entitlement/key to equal its station's — only `stations::register` writes both rows in one statement path; fix needs new composite constraints, not worth it for an unreachable state.
  - `[low]` `[reject]` (blind) Portal key-id hint is shape-only, hub also checks canonical trailing bits — real station ids are always canonical; a pasted non-canonical id still gets the hub's clear 400.
  - `[medium]` `[patch]` (blind) Same-key race across two entitlements untested — added a concurrent test expecting one 201 and one 409 `MSG_ALREADY_REGISTERED`.
  - `[low]` `[patch]` (blind) Release test claims "still downloadable" without downloading — test now downloads the superseded licence over HTTP and compares bytes.
  - `[low]` `[patch]` (blind) Install guidance tells site-scoped owners to reuse a station id — sentence now applies to machine licences only.
  - `[low]` `[reject]` (blind) `/seats` lacks in-use counts and a renewal-reissue note — Stations page shows free seats per entitlement; renewal reissue is story 2 (CAP-3).
  - `[low]` `[reject]` (blind) Download GET audits each request; anchor shows raw JSON on 401 — auditing every download is required by ADR-0007 §6; browsers do not prefetch plain anchors by default; a stale-session JSON page is cosmetic.
  - `[low]` `[reject]` (blind) Station-id requirement checked twice — second check is an unreachable defensive branch with the same message; harmless.
  - `[low]` `[reject]` (blind) `GET /api/stations` unpaged — organizations hold tens of stations; paging adds surface for no everyday benefit.
  - `[low]` `[patch]` (edge) Form enabled while root query pending/failed — same root cause and fix as the blind row above.
  - `[medium]` `[patch]` (edge) Hidden site posted for machine scope — same root cause and fix as the blind row above.
  - `[low]` `[reject]` (edge) Hub accepts `site` on a machine registration — stored as an informational label only, never in the signed licence; with the client fixed, the extra guard is not worth adding.
  - `[medium]` `[patch]` (edge) Expired licence shown as licensed — same root cause and fix as the blind row above.
  - `[low]` `[reject]` (edge) Trimming `station_id` could mismatch a bench id with surrounding whitespace — a `--station-id` with leading/trailing spaces is not a realistic configuration; trimming pasted input is the standard behaviour.
  - `[medium]` `[patch]` (verification-gap) Station/licence JSON key sets not pinned — test now asserts exact key sets with nullable keys present.
  - `[medium]` `[patch]` (verification-gap) Same-key race 409 mapping unexercised — same root cause and fix as the blind race row.
  - `[low]` `[patch]` (verification-gap) Download `Cache-Control: no-store` not asserted — assertion added.
  - `[low]` `[patch]` (verification-gap) "Released last" ordering indistinguishable from newest-first — added a case with a newer released and older active station.
  - `[medium]` `[patch]` (verification-gap, other) Hidden site posted for machine scope — same root cause and fix as the blind row above.
  - `[false]` `[reject]` (intent) Real `wipestation serve` path not exercised automatically — the story's AC names the library-level test plus manual check, both done (manual run printed `licence: <id> for <org>`); the scripted end-to-end is the `commerce-end-to-end` spec's job, and the hub must not depend on station crates.
  - `[false]` `[reject]` (intent) Interactive portal page not tested — portal convention tests helpers and static components; typecheck, 148 tests and a production build pass.
  - `[false]` `[reject]` (intent) Machine scope requires a station id beyond SPEC's "key id, label, site" — recorded autonomous decision in the intent contract (ROADMAP §4.1 binds machine licences to the fingerprint).
  - `[false]` `[reject]` (intent) `spec_checkpoint` approval predates the story file — checkpoint orchestration is the caller's (`bmad-driver`); build-auto never reads it.

## Design Notes

Instance key id format is `ed25519:` + 22 chars of unpadded standard base64 (`wipe-cert` `public_key_id`); validate with that shape, trimmed. Store the licence `document` as text, not jsonb, so downloads are byte-identical to what was signed and stored.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, new `stations` tests included
- `cargo test --workspace` -- expected: pass
- `cargo clippy -p wipe-hub --all-targets -- -D warnings` -- expected: clean
- `cd apps/portal && pnpm typecheck && pnpm test` -- expected: pass

**Manual checks (if no CLI):**
- Hub with `--vendor-root-seed`, buy via Fake, register a running `wipestation serve --station-id bench-1` key, restart it with the downloaded licence: startup prints `licence: <id> for <org>`.

## Auto Run Result

Status: done

**Summary.** An organization owner can now register a station on the portal's new `/stations` page. They choose a current entitlement, paste the station's instance key id, and give its `--station-id` (needed for machine licences). The hub checks that a seat is free, with the entitlement row locked so parallel requests can't overbook. It then signs a licence through `wipe_license::VendorRoot::issue` with the file-held vendor root and stores the exact file for anyone in the organization to download. Releasing a station frees its seat and marks its licence superseded; the page says the old file keeps working until it expires. Registering, issuing, releasing, superseding and downloading are each audited. The page also shows how to read a station's key id and shows the vendor root public key.

**Files changed**
- `crates/wipe-hub/migrations/0014_stations.sql` — `stations` and `licences` tables, with lifecycle triggers and organization-consistent foreign keys.
- `crates/wipe-hub/src/stations.rs` — the only code that writes those tables: register, release, list and download.
- `crates/wipe-hub/src/station_routes.rs`, `src/lib.rs` — the five `/api` routes.
- `crates/wipe-hub/tests/stations.rs` (16 tests), `tests/isolation.rs`, `tests/support/hub.rs` — HTTP tests for every row of the I/O matrix, plus tests for races, ordering, organization isolation and the database triggers.
- `apps/portal/src/api/stations.ts`, `pages/StationsPage.tsx` (each with tests), `main.tsx`, `HomePage.tsx`, `SeatsPage.tsx` — the `/stations` page.
- `CONTEXT.md`, `docs/ROADMAP.md`, `README.md`, `AGENTS.md` — records what shipped and the updated test count.

**Review.** 26 findings: 13 patched in 8 fixes (5 medium, 3 low), 13 rejected (9 low, 4 false), none deferred. Patches: the expired-licence state, the hidden site field, the issuance flag while loading, the site install guidance, and four test gaps (pinned JSON key sets, the same-key race, the `no-store` header, the listing order), plus downloading the superseded licence over HTTP. The reasons for each rejection are in the Review Triage Log.

**Follow-up review recommended: true.** Five medium findings were patched. Two of the portal fixes (the expired state and sending site only for site scope) are covered by unit tests only, and the interactive page has never been opened in a browser.

**Verification.** `cargo test --workspace` passes all 582 tests, including the 16 new station tests. `cargo clippy -p wipe-hub --all-targets --no-deps -D warnings` is clean. Clippy without `--no-deps` fails on an existing `large_enum_variant` lint in `crates/wipe-common/src/job.rs:201`, which this story does not touch. Portal typecheck passes and all 149 tests pass. The implementer did a manual run against a temporary database: the station restarted with the downloaded licence printed `licence: <id> for <org>`, "Licensed - unlimited".

**Residual risks**
- The vendor root is still file-held (ADR-0007: production must not launch on it).
- A released station's licence file stays valid offline until it expires.
- A machine licence is bound to the customer-chosen `--station-id`, not to hardware.
- No automated test starts a real `wipestation`; that end-to-end check belongs to `commerce-end-to-end`.
