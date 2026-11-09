---
title: 'GET /api/license and its TS mirror'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '788f054565a90490e6208885e21f9985a4a11301'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/docs/adr/0005-licensing-and-attestation.md', '{project-root}/docs/adr/0012-opt-in-hard-stop-on-licence-lapse.md']
warnings: ['oversized']
deferred:
  - summary: >-
      GET /api/license omits lease persistence (tier, survives_reboot), so a RAM-only lease is not visible to clients.
    evidence: |-
      LicenseContext::lease_persistence() exists but is printed only at `wipestation serve` start-up; CAP-1 does not list it. Natural home is CAP-3's banner or a later story.
    location: >-
      crates/wipe-server/src/licensing.rs
    severity: low
  - summary: >-
      The licence view lacks license_id, not_before, quota and scope, which support and renewal conversations need.
    evidence: |-
      Not in CAP-1's field list; all are in chain.license.body. Additive later.
    location: >-
      crates/wipe-server/src/licensing.rs
    severity: low
  - summary: >-
      Nothing guards the hand-mirrored TS LeaseStatus/LicenseInfo against Rust serde drift, and api.license() has no client-side test.
    evidence: |-
      Project-wide convention (AGENTS.md: TS types are hand-mirrored); only four of eight LeaseStatus variants reach the route tests. A per-variant serde snapshot shared with a TS fixture would close it.
    location: >-
      apps/desktop/src/api/types.ts
    severity: low
---

<intent-contract>

## Intent

**Problem:** Licensing decides whether certificates are marked `evaluation`, but no API route exposes the station's licence state, so no client (the bench UI in stories 2–3) can tell the operator. (SPEC CAP-1; audit 2026-11-05 §(d).)

**Approach:** Add a read-only `GET /api/license` returning a `LicenseView` built by `LicenseContext`: the current `LeaseStatus`, its `operator_message()`, customer, expiry, the licence's instance key id, lapse policy and the station fingerprint. Mirror the shape in `apps/desktop/src/api/types.ts` and add `api.license()` to `client.ts`.

## Boundaries & Constraints

**Always:** The route only reads: it never moves the clock watermark, never saves lease state, never records usage. It shows the same verdict a signing decision would make right now, including `lease_state_unavailable` when a load fault or the last save fault exists. Rust serde shape and TS mirror change in the same commit. Docs (README route table and licensing paragraph, CONTEXT §10 licensing row) updated in the same commit.

**Never:** No gating of erasure or job start (ROADMAP §0.2). No UI rendering (stories 2–3). No licence install (story 4). No authentication (SPEC non-goal). No change to `LeaseStatus` serde shape or `operator_message` wording. No hardware-sanitization claims.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Unlicensed (default `AppState`) | no chain | 200; `status.status = "unlicensed"`; `operator_message` = `LeaseStatus::Unlicensed.operator_message()`; `customer`, `not_after`, `instance_public_key_id`, `lapse_policy` all `null`; `fingerprint` = `machine_fingerprint("standalone")` | none |
| Licensed, valid | chain, `not_after` = now+30d, unlimited | 200; `status.status = "valid"` with `days_remaining`; `customer = {id, name}`; `not_after` RFC 3339; `instance_public_key_id` from licence body; `lapse_policy = "evaluation"` | none |
| Licensed, `stop` policy | chain with `lapse_policy: Stop` | `lapse_policy = "stop"` | none |
| Licensed, expired | chain with `not_after` in the past | `status.status = "expired"`, `not_after` present; message names expiry date | none |
| Lease state unavailable | licensed context whose store failed to load | `status.status = "lease_state_unavailable"` with `detail` | none |
| Repeated reads | GET twice on a file-backed context | lease file not created/modified by GET; watermark unchanged | none |

## Autonomous decisions

1. **Does reading licence state count as a clock observation?** *What:* the bench will poll this route (story 3 banner). *Problem:* `LicenseContext::status` moves the anti-rollback watermark and writes the lease file (up to 2 s) on every call. *Options:* (a) reuse `status`, so every poll writes to disk; (b) a pure read that evaluates against the stored watermark and reports existing faults. *Choice:* (b). *Why:* GET is a safe method (RFC 9110 §9.2.1); rollback detection is unchanged because the verdict is still computed against the stored watermark, and signing decisions keep observing the clock.
2. **Which "instance key id"?** *Options:* the licence body's `instance_public_key_id` or the station's own signing key id. *Choice:* the licence's (null when unlicensed); the station's own key id is already at `GET /api/public_key`. *Why:* the CAP lists licence attributes; duplicating `/api/public_key` adds nothing.
3. **Include `lapse_policy`.** *Problem:* CAP-3 must word the warning differently under `stop`, and CAP-1 is the only licence read route. *Choice:* include it (null when unlicensed). *Why:* one additive read-only field from the signed licence; avoids changing the wire shape again in story 3.

</intent-contract>

## Code Map

- `crates/wipe-server/src/licensing.rs` -- `LicenseContext`. `status(now)` (≈L353) folds `now` into the watermark and calls `persist()` (disk write, up to 2 s) — do NOT use it for the GET. Reuse `evaluate(self.entitlements(), &state, &self.fingerprint, now)` + `self.with_fault(..)` for a pure read. `chain.license.body` holds `instance_public_key_id`, `lapse_policy`, `entitlements.{customer_id, customer_name, not_after}`.
- `crates/wipe-license/src/lease.rs:234-325` -- `LeaseStatus` (`#[serde(tag = "status", rename_all = "snake_case")]`, variants and field names to mirror in TS) and `operator_message()`.
- `crates/wipe-license/src/chain.rs:40-96` -- `LapsePolicy` (snake_case `evaluation`/`stop`), `LicenseBody`.
- `crates/wipe-server/src/handlers.rs:968` -- `destruction_policy`: pattern for a small read-only GET handler.
- `crates/wipe-server/src/app.rs:372-446` -- router; add `.route("/api/license", get(handlers::license))` before the `/api/*rest` fallback. `AppState::license()` (L176) returns the current context.
- `crates/wipe-server/src/lib.rs:16` -- re-exports; add `LicenseView`.
- `crates/wipe-server/tests/http_e2e.rs:13` -- `spawn_server` pattern; `spawn_server_on`-style helper taking an `AppState` is needed for licensed cases.
- `crates/wipe-server/tests/lease_store.rs:45-62` -- how to mint a test `AttestationChain` (`VendorRoot::generate().issue(..)`); `LocalFileLeaseStore` for the no-write check. `wipe-license` is already a dev-dependency.
- `apps/desktop/src/api/types.ts` -- add `LapsePolicy`, `LeaseStatus` (discriminated union on `status`), `LicenseCustomer`, `LicenseInfo`; near `ConcurrencePolicy` (L323).
- `apps/desktop/src/api/client.ts:236` -- add `license: () => jsonFetch<LicenseInfo>("/api/license")`, import the type.
- `README.md:375` (route table) and `README.md:488` ("no API route or UI shows the licence"); `CONTEXT.md:918` (licensing row, "no API route or UI").

## Tasks & Acceptance

**Execution:**
- `crates/wipe-server/src/licensing.rs` -- add `#[derive(Serialize)] pub struct LicenseView { status: LeaseStatus, operator_message: String, customer: Option<LicenseCustomer{id,name}>, not_after: Option<OffsetDateTime> (rfc3339::option), instance_public_key_id: Option<String>, lapse_policy: Option<LapsePolicy>, fingerprint: String }` and `LicenseContext::view(&self, now) -> LicenseView` that evaluates without observing or persisting -- GET must be safe (RFC 9110 §9.2.1) and a polling banner must not write to disk.
- `crates/wipe-server/src/handlers.rs` -- `pub async fn license(State) -> Json<LicenseView>` calling `state.license().view(now_utc())` -- read-only route.
- `crates/wipe-server/src/app.rs` -- register `GET /api/license`.
- `crates/wipe-server/src/lib.rs` -- re-export `LicenseView`, `LicenseCustomer`.
- `crates/wipe-server/tests/http_e2e.rs` -- HTTP tests for every I/O matrix row (unlicensed, valid, stop, expired, lease_state_unavailable via a store whose `load` errors or a corrupt lease file, repeated reads leave no lease file) -- the route is the outermost surface.
- `apps/desktop/src/api/types.ts`, `apps/desktop/src/api/client.ts` -- TS mirror and client function.
- `README.md`, `CONTEXT.md` -- document the route; licensing row says route shipped, UI not yet.

**Acceptance Criteria:**
- Given a running `wipestation` server, when a client calls `GET /api/license`, then it receives 200 JSON whose keys are exactly `status, operator_message, customer, not_after, instance_public_key_id, lapse_policy, fingerprint`.
- Given the TS mirror, when `pnpm typecheck` runs, then `api.license()` is typed `Promise<LicenseInfo>` and `LeaseStatus` lists all eight Rust variants with their fields.
- Given any number of `GET /api/license` calls, when an erasure is later certified, then the signing decision is unaffected by those reads (no watermark move, no save).

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 19 findings — high 0, medium 0, low 13, false 6, maybe-false 0
- findings:
  - `[low]` `[reject]` Blind: unauthenticated route exposes customer, fingerprint and lease-file path under permissive CORS — the SPEC lists authentication on these endpoints (D2) as a non-goal; every other route (jobs, certificates) is equally unauthenticated.
  - `[low]` `[patch]` Blind: `fingerprint` doc implies a hardware id — doc now says it is a hash of the station id, not hardware.
  - `[false]` `[reject]` Blind: operator_message promises evaluation marking, wrong under `stop` — stop enforcement (story 5) is not built, so the station does keep erasing and marking evaluation today; the message is accurate until story 5 changes behaviour.
  - `[low]` `[patch]` Blind: "same verdict a signing decision would make" overstated (signing saves first) — `view` doc reworded to "stored lease state and last recorded fault; cannot predict the next save".
  - `[low]` `[defer]` Blind: lease persistence tier missing from the view — not in CAP-1; deferred.
  - `[low]` `[defer]` Blind: license_id, not_before, quota, scope missing — not in CAP-1; deferred.
  - `[low]` `[patch]` Blind: no test for the save-fault branch — added `license_route_reports_a_failed_lease_save` (store whose save fails, one signing decision, GET shows lease_state_unavailable).
  - `[low]` `[defer]` Blind: TS mirror not guarded against Rust drift — pre-existing hand-mirror convention; deferred.
  - `[low]` `[patch]` Blind: route tests leave temp dirs — both tests now remove their dir.
  - `[low]` `[patch]` Blind: CONTEXT prose and README paragraph omit the instance key id — added to both.
  - `[false]` `[reject]` Blind: story file stale (in-review, line numbers off) — status was mid-run; Code Map lines are pinned to baseline_revision; fix would edit the spec.
  - `[low]` `[patch]` Edge: a cleared save fault keeps showing until the next save — inherent to the pure-read decision and conservative; documented in the `view` doc (same root as the overstated-verdict row).
  - `[low]` `[patch]` Edge: banner can disagree with the next certificate's evaluation flag — same root; documented as above.
  - `[low]` `[defer]` Intent: the TS client is untested against the wire — same root as the TS-drift row; deferred.
  - `[false]` `[reject]` Intent: the CLI-built server is not exercised — `wipestation serve` calls `wipe_server::serve`, which mounts the same `router` the tests use.
  - `[false]` `[reject]` Intent: AC3 checked via `signing_decision`, not a full erasure — the cert generator (`app.rs`) calls exactly that function; the test asserts both the lease file and the decision are unchanged.
  - `[low]` `[patch]` Intent: "same verdict" wording stronger than code — same root as the overstated-verdict row; patched.
  - `[false]` `[reject]` Intent: `lapse_policy` widens CAP-1 — recorded autonomous decision 3; additive, read-only.
  - `[false]` `[reject]` Intent: README adds "nothing surfaces the lease counts over the API" — accurate statement.

## Design Notes

`view` mirrors `status` minus the side effects:

```rust
let state = self.lease.read().clone();
let status = self.with_fault(evaluate(self.entitlements(), &state, &self.fingerprint, now));
let operator_message = status.operator_message();
```

`status` stays nested (`{"status": {"status": "valid", ...}}`) rather than flattened, so the `LeaseStatus` tag cannot collide with view fields.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass, including new `/api/license` tests.
- `cargo clippy -p wipe-server --all-targets -- -D warnings` -- expected: clean.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: pass.

## Auto Run Result

Status: done

**Summary:** Added read-only `GET /api/license` returning `{status, operator_message, customer, not_after, instance_public_key_id, lapse_policy, fingerprint}` from `LicenseContext::view`, a pure read that never moves the watermark or saves lease state. Mirrored it in TS (`LapsePolicy`, `LeaseStatus`, `LicenseCustomer`, `LicenseInfo`, `api.license()`).

**Files changed:**
- `crates/wipe-server/src/licensing.rs` -- `LicenseView`, `LicenseCustomer`, `LicenseContext::view`.
- `crates/wipe-server/src/handlers.rs` -- `license` handler.
- `crates/wipe-server/src/app.rs` -- `GET /api/license` route.
- `crates/wipe-server/src/lib.rs` -- re-exports.
- `crates/wipe-server/tests/http_e2e.rs` -- 7 HTTP tests (every I/O row plus a save fault).
- `apps/desktop/src/api/types.ts`, `apps/desktop/src/api/client.ts` -- TS mirror and client function.
- `README.md`, `CONTEXT.md` -- route table, licensing paragraph, §10 row.

**Review:** 19 findings. 9 patch rows (5 fixes: view doc wording, fingerprint doc, save-fault test, temp-dir cleanup, doc field lists); 4 deferred (lease tier, extra licence fields, TS drift guard ×2 rows); 6 rejected false plus 1 low rejected as out of scope (auth non-goal D2). Patched entries by verdict: high 0, medium 0, low 5. Follow-up review: not recommended.

**Verification:** `cargo test --workspace` 860 passed, 0 failed (Postgres/Mailpit up). `pnpm typecheck` clean, `pnpm test` 256 passed. `rustfmt --check` clean on touched files. `cargo clippy -p wipe-server --all-targets -- -D warnings` fails on a pre-existing `large_enum_variant` at `crates/wipe-engine/src/runner.rs:47` (untouched); without `-D`, no warnings point at touched files.

**Residual risks:** The route is unauthenticated, like every station route (D2). `view` reports the last recorded save fault, so a banner can lag a recovered disk until the next save. The TS mirror is checked by typecheck only. ADR-0005 "as built" notes saying no API route shows the *lease* (state and counts) remain accurate: the route shows the verdict, not the lease state.
