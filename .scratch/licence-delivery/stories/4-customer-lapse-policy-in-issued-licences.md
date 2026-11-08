---
title: 'Customer lapse policy in issued licences'
type: 'feature'
created: '2026-11-08'
baseline_revision: 'c0d892d572d6d1fc33f5b7e3483de3baf45833c5'
status: 'done'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '{project-root}/docs/adr/0012-opt-in-hard-stop-on-licence-lapse.md'
  - '{project-root}/docs/adr/0005-licensing-and-attestation.md'
warnings: [oversized]
deferred:
  - summary: >-
      Someone at the bench can reinstall an older, superseded licence file (for example the earlier evaluation one), which stays valid offline until its own not_after, so the stop choice holds only while the newest file stays installed.
    evidence: |-
      A policy-change reissue supersedes the old licence at the hub only; its not_after is the same term_end, and supersession is not distributed to stations (ADR candidates 0008/0010). ADR-0012 §3 puts the policy in the signed file so an operator cannot switch it off, and this path partly undoes that. Closing it needs station-side behaviour (for example refusing to install a licence issued before the installed one for the same key) or revocation distribution: not hub work. Recorded as a limitation in CONTEXT §10.
    location: >-
      crates/wipe-hub/src/stations.rs (reissue); station licence install path (licence-visibility)
    severity: medium
---

<intent-contract>

## Intent

**Problem:** ADR-0012 lets a customer choose that its stations stop starting new jobs once their licence lapses, instead of quietly issuing `evaluation` certificates. Nothing carries that choice today: the signed licence has no field for it, the hub has no organization setting, and the portal offers no way to set it (SPEC CAP-6).

**Approach:** `wipe-license`'s signed `LicenseBody` gains `lapse_policy` (`evaluation` | `stop`, default `evaluation`, omitted from the serialized body when `evaluation`, so every existing file and every evaluation licence is byte-identical to today's). Organizations gain a `lapse_policy` setting that only an owner changes, on the portal's Stations page, audited `org.lapse_policy_changed`. Every licence the hub signs (register, reissue, vendor issue) carries the organization's policy at signing time. A station whose active licence carries a different policy shows `lapse_policy_update_available`, and the existing reissue action issues it an updated file.

## Boundaries & Constraints

**Always:**
- `LapsePolicy` enum in `crates/wipe-license/src/chain.rs` (or `entitlement.rs`), serde `snake_case`, `Default` = `Evaluation`, exported from the crate root. Field on `LicenseBody`: `#[serde(default, skip_serializing_if = "LapsePolicy::is_evaluation")] pub lapse_policy: LapsePolicy`. Omitting the default keeps canonical bytes, and therefore `canonical_sha256_hex` and signatures, identical for every licence without `stop`. `license_format_version` stays 1.
- `VendorRoot::issue` keeps its signature and issues `Evaluation`; add `VendorRoot::issue_with_lapse_policy(…, lapse_policy)` and make `issue` delegate to it. The hub signs only through the new function, inside `stations::sign_and_store` (the one signing core).
- Migration `crates/wipe-hub/migrations/0016_lapse_policy.sql`: `organizations.lapse_policy text NOT NULL DEFAULT 'evaluation'` with a CHECK in (`evaluation`, `stop`). No `licences` column: a licence's policy is read from its stored signed document in SQL (`COALESCE((document::jsonb)->'body'->>'lapse_policy', 'evaluation')`), so it can never disagree with the file.
- `sign_and_store` reads the organization's `lapse_policy` with `SELECT … FROM organizations WHERE id = $1 FOR SHARE` in the caller's transaction, signs it in, and adds `lapse_policy` to the `licence.issued` audit detail. The policy change takes the organization row `FOR UPDATE` (`team::lock_organization`), so an issuance never signs a policy that a committed change has replaced without the station then showing the update flag.
- Routes (in `station_routes.rs`, data functions in `stations.rs` or a new `lapse_policy.rs`): `GET /api/organization/lapse-policy` (any role) → `{lapse_policy}`; `POST /api/organization/lapse-policy {lapse_policy}` (owner, `deny_unknown_fields`) → 200 `{lapse_policy}`. Role check before the body is read, then re-checked under the organization lock like `rename_organization` (`team_routes.rs`). A change audits `org.lapse_policy_changed` (actor `user:<id>`, subject `organization:<id>`, detail `{old, new}`); setting the current value is 200 with no write and no audit row.
- `Station` gains `lapse_policy_update_available`, computed in SQL in `select_stations`, never stored: not released, entitlement current, latest licence `active`, and its document's policy ≠ the organization's. `Licence` (customer shape) and `AdminLicence` gain `lapse_policy` (from the document). `AdminOrganizationDetail` gains `lapse_policy`.
- `reissue` is allowed when `renewal_available` or `lapse_policy_update_available` holds (checked in its existing post-lock statement); otherwise 409 `MSG_LICENCE_UP_TO_DATE`, whose wording now also covers the policy. Supersede reason: `"renewed"` when the licence was behind the term (also when the policy differs too), else `"lapse policy changed"`. All other reissue rules and refusal order stay as story 2 and 3 left them.
- Vendor-issued licences carry the target organization's policy too: the customer's choice covers every licence issued to it.
- Portal `/stations` gains a "When a licence lapses" section: the current policy for everyone; owners can switch between the two with a confirm before choosing `stop`. The text says: under `evaluation` (default) a lapsed station keeps erasing and marks certificates `evaluation`; under `stop` it refuses to start new jobs once its licence has expired, was revoked or its clock was rolled back, while a job already running always finishes and is certified; it never stops because the hub is unreachable; a change applies only to licence files issued afterwards, so each station needs its updated file installed. Members read "Only an organization owner can change this." Station rows show the update state and reuse the reissue action ("Issue updated licence").
- Admin organization page and admin licence page show the policy read-only.
- ROADMAP §0.2: the hub only writes the policy into licences; nothing in the hub enforces it or can stop a station erasing.

**Never:** no station-side enforcement, warnings or `JobRunner` change (that is `licence-visibility` story 5); no change to `license_format_version`; no edit of an applied migration; no vendor-admin route that changes a customer's policy; no CLI `license issue` flag; no automatic reissue when the policy changes; no hardware-sanitization claim in copy.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Default | new organization, `GET /api/organization/lapse-policy` | `{lapse_policy:"evaluation"}`; a registered station's file body has no `lapse_policy` key and `Licence.lapse_policy` is `evaluation` | — |
| Set stop | owner `POST {lapse_policy:"stop"}` | 200 `{lapse_policy:"stop"}`; one audit row `org.lapse_policy_changed {old:"evaluation", new:"stop"}` | — |
| Same value | owner posts the current value | 200, no audit row | — |
| Member / vendor admin | member or vendor admin POST | 403 `MSG_OWNER_ONLY_LAPSE_POLICY`; GET still 200 for a member | nothing written |
| Bad body | `"sometimes"`, missing field, unknown field, not JSON | 400 | nothing written |
| Not signed in | GET or POST | 401 | — |
| Register under stop | owner registers after setting stop | file body `lapse_policy:"stop"`; installs, `check_installable` + `evaluate` `Valid`; `licence.issued` detail has `lapse_policy:"stop"` | — |
| Update flag | station registered under evaluation, then policy set to stop | `GET /api/stations`: `lapse_policy_update_available:true`, `renewal_available:false` | — |
| Reissue for policy | owner reissues that station | 201; new licence `stop`, `not_after` = `term_end`, same key/scope/quota/features; old `superseded`, audit reason `"lapse policy changed"`; flag false | — |
| Back to evaluation | policy set back to evaluation, station reissued | reissued file has no `lapse_policy` key; flag false | — |
| Up to date | policy matches and licence runs to `term_end` | 409 `MSG_LICENCE_UP_TO_DATE` | nothing written |
| Released / revoked / not current | as story 2/3 | flag false; reissue refusals unchanged | nothing written |
| Vendor issue | vendor admin issues for an organization on stop | body `lapse_policy:"stop"`; admin licence view shows it | — |
| Old file | a licence file signed before this story (committed fixture) | verifies against its root; `lapse_policy` = `Evaluation`; re-serialized pretty JSON is byte-identical | — |
| Tamper | `lapse_policy` removed from a stop licence | `verify` → `Tampered` | — |
| Concurrent | 8 parallel reissues after a policy change | exactly one 201, one active licence | — |

## Autonomous decisions

- **Where the policy is stored per licence.** Problem: the hub must know which stations hold a file with an out-of-date policy. Options: (a) a new `licences.lapse_policy` column, protected by the lifecycle trigger; (b) read it from the stored signed document in SQL. Chosen (b): the signed file is the only authority, so a derived value can never disagree with what the station sees, and no trigger needs replacing.
- **How old files stay valid.** Problem: a field with only a serde default would make re-canonicalization of every existing licence include `"lapse_policy":"evaluation"` and fail its hash check, breaking every installed licence and every erasure certificate's chain. Chosen: omit the field when it is `evaluation` (the precedent is `machine_binding`). Consequence: a `stop` licence fails verification on a station or verifier built before this story; no such build is deployed, recorded in CONTEXT §10.
- **Vendor-issued licences.** Options: (a) always `evaluation`; (b) the organization's policy. Chosen (b): the policy is the customer's compliance choice and a vendor trial should not silently bypass it.
- **Same-value change** is a no-op with no audit row, so the audit log records only real changes (the `RenewalChanged` precedent).
- **Reissue reason when both apply** is `"renewed"`: the renewal is the larger change and the new file carries the current policy anyway.
- **The truthful-copy note.** Stations do not enforce `stop` until `licence-visibility` story 5 ships, so the portal section states, as one removable sentence, that station enforcement arrives in a later station release and until then a lapsed station keeps issuing `evaluation` certificates. Leaving it out would make the page claim behaviour the product does not have yet.

</intent-contract>

## Code Map

- `crates/wipe-license/src/chain.rs:35` `LicenseBody`, `:98` `VendorRoot::issue`, `:132` `verify` (re-canonicalizes the body; why the default must be skipped). `crates/wipe-license/src/entitlement.rs:135` `machine_binding` -- the `default` + `skip_serializing_if` precedent. `src/lib.rs:28` re-exports `chain::*`.
- `crates/wipe-license/tests/attestation.rs`, `end_to_end.rs` -- test patterns; add `tests/fixtures/` licence file. Generate the fixture from the **baseline** code (before adding the field) with a fixed-seed `VendorRoot::from_signing_key`, so it is a real pre-change file.
- `crates/wipe-hub/src/stations.rs` -- `Licence` (l.124) + `licence_columns` (l.143); `Station` (l.166), `StationRow` (l.190), `From<StationRow>`; `ENTITLEMENT_CURRENT` (l.247), `LATEST_LICENCE` (l.253), `LICENCE_BEHIND_TERM` (l.261), `select_stations` (l.265; join `organizations` for the policy); `sign_and_store` (l.544; the one `root.issue` call at l.553); `reissue` (l.831; post-lock statement at ~l.870 computes `behind`; supersede reason at ~l.915); messages l.81-101; module doc l.1-60.
- `crates/wipe-hub/src/station_routes.rs` -- route list l.54-58, `require_owner` l.73, `From<StationError>`; module doc lists routes.
- `crates/wipe-hub/src/team_routes.rs:196` `rename_organization` + `require_owner_locked` (l.138) + `team::lock_organization` -- the owner-change pattern; `organization_subject` (l.98).
- `crates/wipe-hub/src/admin.rs:337` `organization_detail` / `AdminOrganizationDetail`; `crates/wipe-hub/src/licence_admin.rs:230` `AdminLicence` + its `Row` query.
- `crates/wipe-hub/migrations/0003_accounts.sql:8` organizations; `0015_licence_admin.sql` -- read-only.
- `crates/wipe-hub/tests/stations.rs` -- `Fixture`, `register_machine`, `download`, `install`, key-set pins of `Station`/`Licence` (gain the new keys), concurrent-reissue pattern. `tests/licence_admin.rs` -- vendor issue/view. `tests/isolation.rs` `ROUTES` -- add both new routes. `tests/migrations.rs` -- CHECK refusal for a bad policy value. `tests/admin.rs` -- org detail key set if pinned.
- `apps/portal/src/api/stations.ts` (+ test) -- `Licence`, `Station`, guards, `reissueStation`; add `LAPSE_POLICIES`, `fetchLapsePolicy`, `setLapsePolicy`. `apps/portal/src/pages/StationsPage.tsx` (+ test) -- `stationState` (l.63), `RenewalAction` (l.192), `StationRow` (l.245), `SupersessionNote`. `apps/portal/src/api/admin.ts`, `adminLicences.ts`, `pages/AdminOrganizationPage.tsx`, `AdminLicencePage.tsx` -- read-only display.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-license/tests/fixtures/licence-before-lapse-policy.json` (+ its root public key in the test) -- generated from baseline code first.
- `crates/wipe-license/src/chain.rs`, `src/lib.rs` -- `LapsePolicy`, the field, `issue_with_lapse_policy`; doc comments citing ADR-0012.
- `crates/wipe-license/tests/attestation.rs` (or new `lapse_policy.rs`) -- old-fixture, evaluation-omitted, stop round-trip and tamper tests.
- `crates/wipe-hub/migrations/0016_lapse_policy.sql` -- the column and CHECK, with a header comment.
- `crates/wipe-hub/src/stations.rs` (and optional `lapse_policy.rs`, `lib.rs`) -- policy read/set with audit, `sign_and_store` change, `lapse_policy` on `Licence`, `lapse_policy_update_available`, `reissue` widening and reason, `MSG_OWNER_ONLY_LAPSE_POLICY`, reworded `MSG_LICENCE_UP_TO_DATE`, module doc.
- `crates/wipe-hub/src/station_routes.rs` -- the two routes.
- `crates/wipe-hub/src/admin.rs`, `licence_admin.rs` -- `lapse_policy` in the admin shapes.
- `crates/wipe-hub/tests/stations.rs` (or new `tests/lapse_policy.rs`), `tests/licence_admin.rs`, `tests/isolation.rs`, `tests/migrations.rs` -- one test per I/O row through HTTP; updated key-set pins.
- `apps/portal/src/api/stations.ts`, `admin.ts`, `adminLicences.ts` (+ tests) -- types, guards, client calls.
- `apps/portal/src/pages/StationsPage.tsx` (+ test) -- policy section, update state and action; admin pages show the policy.
- `CONTEXT.md` (§10 row; §14 entry **Lapse policy**; update story 1/2/4 rows that say the policy is pending), `docs/ROADMAP.md`, `README.md` routes, `AGENTS.md` test counts, `.scratch/STATUS.md` L row -- record what shipped.

**Acceptance Criteria:**
- Given an organization whose owner set `stop` on the Stations page, when an owner downloads a newly registered station's licence and installs it with the vendor root, then the station accepts it and its body's `lapse_policy` is `stop`.
- Given the Stations page after a policy change, when any member views it, then every registered station holding an older-policy licence shows that an updated licence is available, owners can issue and download it from that row, and members see they cannot change the policy.
- Given any licence file issued before this story, when verified by the new build, then it verifies unchanged and reads as `evaluation`.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 21 findings — high 0, medium 2, low 12, false 4, maybe-false 0 (3 not graded separately: they share a root cause with a graded row)
- findings:
  - `[medium]` `[patch]` (blind) Reinstalling the old `evaluation` file gets around `stop`, and nothing records it — patched: the limitation is now in the CONTEXT §10 row. The underlying fix is station-side or needs revocation distribution, so it is also in frontmatter `deferred`.
  - `[false]` `[reject]` (blind) Older station builds reject a `stop` licence as tampered, and this goes unrecorded — the CONTEXT §10 row and the story's autonomous decisions record it, and no older build is deployed. ADR-0012 is not contradicted: it asks only for a serde default, and the omission keeps that default's promise that existing files keep verifying.
  - `[low]` `[reject]` (blind) Customer-visible copy was written without the owner — the content of every sentence is what SPEC CAP-6 (owner-approved) prescribes, and stories 1–3 set the wording of the same page the same way. The wording sits in two constants the owner can edit.
  - `[low]` `[patch]` (blind) The stop confirmation describes enforcement that does not exist yet — `STOP_CONFIRMATION` now says it applies only from the station release that enforces it, and that until then a lapsed station keeps erasing and marks certificates evaluation.
  - `[low]` `[patch]` (blind) The policies are named differently on the same page — `LapsePolicyText` now uses `LAPSE_POLICY_LABELS` for both headings.
  - `[low]` `[patch]` (blind) Vendor-issued licences never get an update after a policy change — a flag or reissue path for them would be new surface for a rare case (a live vendor trial plus a policy change), so it was not built. Patched as a recorded limitation in the CONTEXT §10 row (the vendor admin issues a new licence).
  - `[low]` `[patch]` (blind) Reissue compares the policy without a lock, so a policy reverted mid-reissue yields a no-change licence audited "lapse policy changed" — the post-lock statement now takes `FOR SHARE OF org`. Lock order stays station/entitlement → organization; a policy change takes only the organization row, so no cycle.
  - `[false]` `[reject]` (blind) The response fields are strings, and `unwrap_or_default` could produce `""` — that branch runs only when the latest licence exists (`l.id` non-null), and the SQL `COALESCE` then always yields a value. The database CHECK holds the organization value to the two spellings.
  - `[low]` `[reject]` (blind) No test covers the locking claim, and issuance now queues behind `lock_organization` callers — those are short single-row transactions (rename, invite, role change), so the wait is negligible. A deterministic race test needs a two-connection barrier harness, out of proportion here.
  - `[low]` `[reject]` (blind) The fixture has no trailing newline and nothing protects it — a modified fixture makes its test fail loudly, which is the intended guard. It was generated from baseline c0d892d, which is recorded in this spec's frontmatter.
  - `[low]` `[patch]` (blind) The portal guesses why a licence was reissued from stale row data — `reissueKind(before, issued)` now names it from the hub's answer (a new `not_after` past the old one is a renewal), with a test.
  - `[low]` `[reject]` (blind) Lists parse every licence document as JSON — documents are about 1 KB and lists hold one organization's stations; add an expression index if that ever shows up in profiles.
  - `[low]` `[patch]` (edge) Reissue reads the policy unlocked before `sign_and_store` reads it again — same root cause and fix as the blind lock row.
  - `[low]` `[patch]` (edge) The notice kind comes from a stale row — same root cause and fix as the blind `reissueKind` row.
  - `[low]` `[patch]` (edge) The panel shows a stale setting after another owner's change or a 403 after a demotion — `onSettled` now invalidates `["lapse-policy"]`, `["stations"]` and `["me"]`, also after a refusal.
  - `[low]` `[reject]` (edge) A body with an explicit `"lapse_policy":"evaluation"` would verify as tampered — no signer produces one: `issue_with_lapse_policy` serializes through the skipping field, and a test pins that `issue` and an explicit `evaluation` give identical bytes. Rejecting it at parse would add a custom deserializer for an unreachable input.
  - `[medium]` `[patch]` (verification-gap) The confirm before `stop` and the button's POST are never exercised — the decision is extracted into `nextPolicyChange(current, confirm)`, and tests cover declining (no change), accepting and switching back without asking. The click → mutate wiring still has no DOM harness (follow-up below).
  - `[false]` `[reject]` (verification-gap) The `sign_and_store` doc overstates what the lock guarantees — the doc says the lock orders issuance after a change in flight, and that a later change leaves the flag set. Both hold: the flag is computed fresh, which is what the doc relies on.
  - `[low]` `[patch]` (verification-gap) Double blank line in `AdminLicencePage.test.ts` — removed.
  - `[false]` `[reject]` (intent) Station enforcement (reading R2) is not delivered — the story's own description scopes it to carrying the policy in issued licences. Enforcement is `licence-visibility` story 5 in stories.yaml, and the portal says so truthfully.
  - `[medium]` `[patch]` (intent) The wired portal section (fetch, mutation, confirm) is not exercised at the portal surface — same root cause and fix as the verification-gap confirm row.

## Design Notes

Back-compat is the crux. `canonical_bytes(&self.body)` re-serializes the parsed body; a field present after parse but absent at signing changes the bytes. Hence:

```rust
#[serde(default, skip_serializing_if = "LapsePolicy::is_evaluation")]
pub lapse_policy: LapsePolicy,
```

Reissue's post-lock statement becomes, in effect: `SELECT l.id, l.status, COALESCE(LICENCE_BEHIND_TERM, false), COALESCE(l.status='active' AND policy_of(l.document) <> org.lapse_policy, false) … JOIN organizations org ON org.id = s.organization_id`. Refuse up-to-date unless either is true.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-license -p wipe-hub` -- expected: all pass
- `cargo test --workspace` -- expected: pass
- `cargo clippy -p wipe-license -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean
- `cd apps/portal && pnpm typecheck && pnpm test && pnpm build` -- expected: pass

## Auto Run Result

Status: done

**Summary.**
- A licence can now carry the customer's lapse policy (ADR-0012). `wipe-license`'s signed `LicenseBody` gains `lapse_policy` (`evaluation` | `stop`). `evaluation` is left out of the serialized body, so every earlier licence file, and every evaluation licence issued now, has exactly the bytes and signature it would have had before. A committed pre-change fixture proves it.
- An organization owner sets the policy on the portal's Stations page, confirming before choosing `stop`. The change is audited `org.lapse_policy_changed {old, new}`. Members, and vendor admins on the admin pages, can see it but not change it.
- Every licence the hub signs afterwards carries the organization's policy: registration, renewal and policy reissue, and vendor issue.
- Each station holding an older-policy file shows "Updated licence available", and the existing reissue action issues and offers the updated file.
- Stations do not enforce `stop` yet (`licence-visibility` story 5). The page says so.

**Files changed**
- `crates/wipe-license/src/chain.rs` — `LapsePolicy`, the body field, `issue_with_lapse_policy`.
- `crates/wipe-license/tests/lapse_policy.rs`, `tests/fixtures/licence-before-lapse-policy.json` — back-compatibility, round-trip and tamper tests.
- `crates/wipe-hub/migrations/0016_lapse_policy.sql` — `organizations.lapse_policy`.
- `crates/wipe-hub/src/lapse_policy.rs` (new), `lib.rs`, `station_routes.rs` — get and set with an owner re-check under lock, plus the audit; `GET`/`POST /api/organization/lapse-policy`.
- `crates/wipe-hub/src/stations.rs` — the policy is signed in through `sign_and_store`, read back from the stored document, `lapse_policy_update_available`, and reissue widened (with the organization row locked `FOR SHARE`).
- `crates/wipe-hub/src/admin.rs`, `licence_admin.rs` — the policy in the admin shapes.
- `crates/wipe-hub/tests/{stations,licence_admin,isolation,migrations}.rs` — every I/O row through HTTP.
- `apps/portal/src/api/{stations,admin,adminLicences}.ts` (+ tests) — types, guards, client calls.
- `apps/portal/src/pages/StationsPage.tsx` (+ test) — the policy section, `nextPolicyChange`, `reissueKind`, and the update state and action. `AdminOrganizationPage.tsx` and `AdminLicencePage.tsx` (+ test) show the policy read-only.
- `CONTEXT.md`, `docs/ROADMAP.md`, `README.md`, `AGENTS.md`, `.scratch/STATUS.md` — what shipped, the §14 **Lapse policy** entry, the limitations and the test count (628).

**Review.** 21 findings: high 0, medium 2, low 12, false 4. Three more rows share a root cause with a graded row.
- **Patched (8 fixes, from 2 medium and 7 low entries):**
  - the reinstall and vendor-licence limitations recorded in CONTEXT;
  - truthful confirm copy;
  - consistent policy names;
  - the `FOR SHARE OF org` lock in reissue;
  - naming the reissue from the hub's answer;
  - refreshing the panel after any outcome;
  - a testable confirm decision with tests;
  - a stray blank line.
- **Deferred (1):** a reinstalled superseded file gets around `stop` until its own expiry. The fix is station-side or needs revocation distribution.
- **Rejected (9):** 5 low (copy owner sign-off, locking test, fixture newline, JSON parse cost, explicit `evaluation` body) and 4 false (old-build incompatibility unrecorded, empty-string policy, lock doc overstated, station enforcement missing). The reasons are in the Review Triage Log.
- I applied the review patches myself, not the implementer subagent. Resuming it would have run it in the background, which this workflow forbids.

**Follow-up review recommended: true.** Two medium entries were patched on a first pass. The named unverified risk: the portal's click wiring (`LapsePolicyPanel` button → `nextPolicyChange` → mutation, and `RenewalAction` → reissue) has no DOM test harness. The decisions themselves are unit-tested, but whether the button calls them is checked only by typecheck and a build.

**Verification.**
- `cargo test --workspace --no-fail-fast` passes all 628 tests.
- One earlier full run stopped on `health::silent_smtp_and_database_time_out_within_bound`, a wall-clock bound (under 3.5 s) in a file this story does not touch. It failed once when run alone, then passed 3 times out of 3 alone and in the next full run.
- `cargo clippy -p wipe-license -p wipe-hub --all-targets --no-deps -- -D warnings` is clean.
- Portal: `pnpm typecheck`, `pnpm test` (193 tests) and `pnpm build` all pass.
- Every I/O matrix row maps to a passing test.

**Residual risks**
- No station enforces `stop` until `licence-visibility` story 5.
- A `stop` licence fails verification on station or verifier builds older than this change (none are deployed).
- A superseded file can be reinstalled (deferred).
- Vendor-issued licences are not flagged after a policy change.
- Each station needs its updated file issued and installed by hand.
- The portal was not checked in a real browser.
- The vendor root is still file-held (ADR-0007).
- `health.rs` has a timing-sensitive test.
