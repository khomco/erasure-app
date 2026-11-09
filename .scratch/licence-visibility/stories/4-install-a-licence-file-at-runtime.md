---
title: 'Install a licence file at runtime'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '0d25600cd0559a92b8f8d1862daa81cb315bc040'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/docs/adr/0005-licensing-and-attestation.md', '{project-root}/docs/adr/0008-station-enrollment-and-licence-fetch.md']
warnings: ['oversized']
deferred:
  - summary: >-
      A licence installed with POST /api/license is held in memory only; after a restart the station silently falls back to its --license licence or to evaluation, and nothing tells the operator beforehand.
    evidence: |-
      Autonomous decision 2. GET /api/license and the banner show a runtime-installed licence exactly like a --license one, so the operator learns of the loss only after the reboot (the banner then shows evaluation). Persisting needs a stored licence file and a start-up precedence rule against --license; ADR-0008's station-enrollment epic needs the same store.
    location: >-
      crates/wipe-server/src/app.rs (AppState::install_license)
    severity: low
  - summary: >-
      The desktop (Tauri) shell can never install a licence at runtime: it wires no vendor roots, so POST /api/license always answers 409.
    evidence: |-
      apps/desktop/src-tauri/src/lib.rs builds AppState with no licence and no roots; the shell was already always unlicensed before this story. Closing it needs a vendor-root configuration for the shell.
    location: >-
      apps/desktop/src-tauri/src/lib.rs
    severity: low
---

<intent-contract>

## Intent

**Problem:** Installing a licence today means restarting `wipestation serve` with `--license`; a bench mid-shift cannot pick up a new or renewed licence without stopping, and ADR-0008's enrollment flow needs a runtime install to land licences in (SPEC CAP-4, ROADMAP L1.3).

**Approach:** Add `POST /api/license`, whose body is the licence file's JSON. It parses it, runs the same checks as start-up (`wipe_license::check_installable` against the station's configured vendor roots and its own signing key id), and on success swaps a new `LicenseContext` into the shared `AppState::license` cell, keeping the running lease state; it answers with the same `LicenseView` as `GET /api/license`. Any failure answers with the reason and leaves the installed licence untouched.

## Boundaries & Constraints

**Always:** Trust only the vendor roots the station was started with (`--vendor-root-b64`); the request can never supply a root (ADR-0008 §4). Write through the shared cell so the already-spawned certificate generator signs with the new licence (the `AppState::license` doc comment's bug). The new context keeps the running lease: watermark, evaluation count, store, writer thread, and any load or save fault; the licensed count follows `LicenseContext::open`'s rule (restarts at 0 only when the licence id differs). A rejected install leaves `GET /api/license` and the next certificate exactly as before. Every error body is the existing `{"error": "<reason>"}` shape. Rust and TS change together (`client.ts`). Docs in the same commit: README route table and licensing paragraph, CONTEXT §10 licensing row and §11 licensing paragraph, ROADMAP L1.3 note, `--vendor-root-b64` help text.

**Never:** No gating, delaying or refusing of erasure or job start (ROADMAP §0.2; refusal is story 5). No bench UI install form (L1.3's activation screen stays unbuilt). No persistence of the installed licence across restart (autonomous decision 2). No authentication (SPEC non-goal D2). No new install checks beyond `check_installable` (SPEC: "the existing install checks"). No change to `LeaseStatus`, `operator_message` or `LicenseView` shape. No hardware-sanitization claim.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Install on unlicensed station | roots configured; licence signed by that root for this station's key id | 200 `LicenseView`, `status.status = "valid"`, customer set; GET returns the same; next erased job's certificate has `evaluation: false` and an `attestation` | none |
| Replace a licence | licensed with A; POST valid B (different id) | 200, view shows B; evaluation count and watermark kept; licensed count restarts at 0 | none |
| Reinstall same licence | licensed with A, some licensed erasures; POST A | 200; licensed count unchanged | none |
| Expired but authentic licence | passes `check_installable`, `not_after` past | 200, installed, view `status.status = "expired"` (as at start-up) | none |
| Not JSON / not a licence | `"hello"` or `{}` | 422, error names a parse failure | prior state kept |
| Untrusted | signed by another vendor root, or body tampered after signing | 422, error starts `licence did not verify` | prior state kept |
| Wrong station | valid licence for another instance key id | 422, error is `InstallProblem::WrongInstanceKey`'s text naming both keys | prior state kept |
| No vendor root configured | `AppState` with no roots (desktop shell; `serve` without `--vendor-root-b64`) | 409, error says no vendor root is configured and to restart with `--vendor-root-b64` | prior state kept |
| Unreadable lease file | context opened with a load fault; POST valid licence | 200, view `status.status = "lease_state_unavailable"`; the bad file is not written | none |

## Autonomous decisions

1. **Which vendor roots does a runtime install trust?** *What:* an operator (or, later, ADR-0008's enrollment poller) hands the station a licence file while it runs. *Problem:* today roots are only read when `--license` is given at start-up, so a station started unlicensed knows no root. *Options:* (a) accept a root in the request; (b) keep the roots from `--vendor-root-b64` in `AppState`, decode them whether or not `--license` is given, and refuse with 409 when none are configured. *Choice:* (b). *Why:* ADR-0008 §4 — "the station trusts only its configured vendor root"; a request-supplied root would let anyone license any station.
2. **Does an installed licence survive a restart?** *Problem:* persisting it needs a stored licence file and a precedence rule against `--license` at start-up, which is new scope (and ADR-0008's enrollment epic will need the same store). *Options:* (a) persist now; (b) hold it in memory until restart and document that. *Choice:* (b), recorded as a CONTEXT §10 limitation and a deferred item. *Why:* deferring is safe: after a restart the station falls back to its `--license` licence or to evaluation, which the banner (story 3) shows and every certificate marks; nothing is ever over-claimed.
3. **What happens to lease state on swap?** *Options:* (a) re-open from the lease file; (b) share the running lease. *Choice:* (b): the new context shares the same lease cell, store, writer thread and fault cells, so in-flight saves cannot land out of order and a rolled-back clock stays detected. The licensed count follows the start-up rule. *Why:* matches `open`, which ADR-0005 §3a already accepts; swapping A→B→A restarting A's count is the same speed bump a restart has (residual risk).
4. **Authentic but expired, not-yet-valid or wrong-machine licences.** *Choice:* installed, as at start-up, with the resulting status in the response. *Why:* the SPEC fixes "the existing install checks"; the status (and banner) tell the operator.

</intent-contract>

## Code Map

- `crates/wipe-server/src/licensing.rs` -- `LicenseContext` fields (`lease`, `save_fault`, `writer`) are owned; `open` holds the licence-id count rule (lines ~118-150); `view` builds `LicenseView`; `persist` skips when `load_fault` is set.
- `crates/wipe-server/src/app.rs:52-63,159-171` -- `AppState::license` shared cell and `with_license` (writes through); `spawn_cert_generator` reads `state.license()` per broadcast. Routes at `:372-447` (`/api/license` GET at `:439`). `signing_key.public_key_id()` is the station key id.
- `crates/wipe-server/src/handlers.rs:972-1001` -- `license` GET handler; `ApiError(StatusCode, String)` → `{"error"}`; `save_bay_topology` (`:154-183`) is the 422/409 precedent.
- `crates/wipe-license/src/install.rs:22-89` -- `load_license(path)` (parse inline, no `parse_license` yet), `InstallProblem` with operator wording, `check_installable(license, station_key_id, roots)`.
- `crates/wipe-cli/src/main.rs:82-90,499-512,897-945` -- `--license`/`--vendor-root-b64` flags, `build_license_context` (decodes roots only with `--license`), `decode_keys`. Desktop shell `apps/desktop/src-tauri/src/lib.rs:143` wires no licence or roots (stays 409).
- `crates/wipe-server/tests/http_e2e.rs:2266-2335` -- `spawn_server_with_license`, `license_chain` (generates its own root), `licensed_ctx`, `get_license`, `license_tmpdir`; `:30` full erase flow to copy for the certificate check; `http_post`, `http_wait_for_certificate`.
- `apps/desktop/src/api/client.ts:22-37,239` -- `jsonFetch`, `api.license()`.
- `README.md:376,485-495`, `CONTEXT.md:918,1003-1016`, `docs/ROADMAP.md:573-577` (L1.3 note) -- docs to update.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-license/src/install.rs` -- add `parse_license(raw: &str) -> LicenseResult<LicenseCertificate>`; make `load_license` use it -- one parser for file and request.
- `crates/wipe-server/src/licensing.rs` -- make `lease`, `save_fault`, `writer` `Arc`-shared; add `LicenseContext::with_chain(&self, chain) -> LicenseContext` applying the licence-id count rule to the shared lease; unit tests for count kept/reset, evaluation count and watermark kept, load fault inherited.
- `crates/wipe-server/src/app.rs` -- `vendor_roots: Arc<Vec<VerifyingKey>>` (default empty) + `with_vendor_roots`; `install_license(&self, raw: &str) -> Result<LicenseView, LicenseInstallError>` building and swapping under the cell's write lock (no disk I/O under the lock); route `POST /api/license`.
- `crates/wipe-server/src/handlers.rs` -- `install_license` handler taking the body as `String`; 409 for no roots, 422 for parse/`InstallProblem`, 200 `LicenseView`.
- `crates/wipe-server/src/lib.rs` -- re-export new public types.
- `crates/wipe-cli/src/main.rs` -- decode `--vendor-root-b64` whenever given and pass to `with_vendor_roots`; start-up line saying whether runtime licence install is enabled; update flag help.
- `crates/wipe-server/tests/http_e2e.rs` -- one test per I/O matrix row; install test also runs an erase and asserts a licensed certificate.
- `apps/desktop/src/api/client.ts` -- `installLicense(licenceJson: string)` POSTing the text, returning `LicenseInfo`.
- `README.md`, `CONTEXT.md`, `docs/ROADMAP.md` -- route, roots rule, in-memory-until-restart limitation.

**Acceptance Criteria:**
- Given an unlicensed running station started with a vendor root, when a valid licence for it is POSTed, then without a restart `GET /api/license` reports it and the next certificate is licensed.
- Given any rejected POST, when `GET /api/license` and the next certificate are inspected, then both are unchanged from before the POST.
- Given `cargo test --workspace` and `cd apps/desktop && pnpm typecheck && pnpm test`, when run, then all pass.

## Spec Change Log

## Review Triage Log

### 2026-11-09 — Review pass
- verdicts: 23 findings — high 0, medium 1, low 15, false 7, maybe-false 0
- findings:
  - `[low]` `[reject]` Blind: an install between a signing decision and its usage record counts an erasure certified under A against B's reset count — real but the window is milliseconds of signing time, it errs toward counting more (never fewer) against the new licence, and the fix adds a licence id to `SigningDecision` plus a mismatch branch.
  - `[low]` `[patch]` Blind: any authentic licence can replace the installed one over unauthenticated, CORS-permissive HTTP (e.g. a superseded `evaluation` file over a `stop` one), undocumented — authentication is a SPEC non-goal (D2) and the same posture covers every station route; the missing documentation is patched in the CONTEXT §10 limitations column.
  - `[low]` `[patch]` Blind: swapping A→B→A restarts A's licensed count with one HTTP request, and `Count` quotas are enforced (`QuotaExhausted`) — accepted by autonomous decision 3; CONTEXT §10 now states it is reachable over HTTP.
  - `[low]` `[patch]` Blind: installing B then restarting with `--license A` restarts A's count, undocumented — same root as the previous row; documented in the same CONTEXT sentence.
  - `[low]` `[patch]` Blind: module doc in `licensing.rs` still says one writer thread per context — reworded: one writer shared by contexts derived with `with_chain`.
  - `[low]` `[defer]` Blind: decision 2 promises a deferred item but `deferred` is empty — recorded the persistence item in `deferred` (grouped with the restart-warning and durability rows).
  - `[low]` `[patch]` Blind: `.scratch/STATUS.md` L row omits runtime install — added; the `spec_checkpoint` half is false (`stories/4.spec.approved` records the approval, and the checkpoint is the caller's).
  - `[low]` `[patch]` Blind: doubled "not a licence file: parsing licence:" text, dead branch, and `load_license`'s message changed unpinned — `parse_license` now says "not a licence file: …", `Unparseable` displays it once, and a malformed-file test pins `load_license`'s path plus text.
  - `[false]` `[reject]` Blind: the portal Stations page still shows only the `serve --license` restart line — that line is still accurate and the only path with operator-facing instructions; the bench has no install form, and changing customer-visible copy is not part of CAP-4.
  - `[low]` `[reject]` Blind: test gaps (replace test checks no certificate, empty/array body, no-disk-I/O, `installLicense` untested) — no defect shown: the certificate path through the shared cell is covered by the unlicensed→licensed test, an empty or array body takes the same parse branch as `{}`, and `installLicense` has no caller.
  - `[low]` `[patch]` Blind: `installLicense` doc overstates what the rejection carries — reworded: an Error whose message carries the status and the `{"error"}` body.
  - `[low]` `[defer]` Blind: nothing warns the operator that a runtime-installed licence disappears at restart — `LicenseView` is fixed by this story's Never rule; deferred with the persistence item.
  - `[low]` `[reject]` Edge: install racing a signing decision counts against the wrong licence — same as the first Blind row; rejected on the same grounds.
  - `[low]` `[patch]` Edge: a non-UTF-8 body got axum's plain-text 400 instead of the `{"error"}` 422 — handler now takes `Bytes` and answers 422 "not a licence file: the body is not UTF-8 text"; test case added. The >2 MB half (axum's 413) is rejected: a licence file is a few KB.
  - `[low]` `[reject]` Edge: a malformed `--vendor-root-b64` without `--license` now stops `serve` from starting — failing fast on an explicit, unusable trust flag is the safe default, matching `--license`'s hard error at start-up; silently ignoring it would leave runtime install broken with no sign why.
  - `[medium]` `[patch]` Verification gap: nothing tests that `serve` hands its roots to `AppState` without `--license` — `cmd_serve` now applies licence and roots through `apply_licensing`, and `serve_without_a_licence_still_trusts_its_vendor_roots_for_runtime_install` decodes a base64 root, applies it and installs (Ok), and shows no roots gives `NoVendorRoot`.
  - `[low]` `[patch]` Verification gap: `load_license`'s changed error text is not pinned — same group as the doubled-text row; `a_malformed_licence_file_names_the_file_and_the_problem` added.
  - `[false]` `[reject]` Intent: the operator cannot install from the bench UI — CAP-4's success line names only `POST /api/license`; the bench activation screen is ROADMAP L1.3, a separate item.
  - `[low]` `[defer]` Intent: desktop-shell operators can never install (always 409) — pre-existing: the shell wires no licence or roots and was always unlicensed; deferred.
  - `[false]` `[reject]` Intent: runtime install needs `--vendor-root-b64` at start-up — this is ADR-0008 §4 ("trusts only its configured vendor root"), recorded as autonomous decision 1; embedding a root in the binary is a release decision outside CAP-4.
  - `[low]` `[defer]` Intent: an installed licence does not survive restart — same root as the persistence rows; deferred.
  - `[false]` `[reject]` Intent: `with_license` is not literally called — it consumes `self` and cannot be called on a shared state from a handler; `install_license` writes through the same shared cell, which is what the epic constraint requires.
  - `[false]` `[reject]` Intent: accepted installs change lease counts — CAP-4's "leaves the old state" constrains only a bad file; rejected installs are shown unchanged by the tests (view, lease state, next certificate).

## Design Notes

```rust
pub fn with_chain(&self, chain: AttestationChain) -> LicenseContext {
    { let mut l = self.lease.write(); let id = &chain.license.body.license_id;
      if l.license_id.as_deref() != Some(id) { l.erasures_used = 0; l.license_id = Some(id.clone()); } }
    LicenseContext { chain: Some(chain), lease: self.lease.clone(), writer: self.writer.clone(), .. }
}
```

Shared cells mean a signing decision still holding the old `Arc` saves the same lease the new context does, through the same ordered writer.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass, including new POST tests.
- `cargo clippy -p wipe-server -p wipe-license -p wipe-cli --all-targets` -- expected: no new warnings in touched files.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: pass.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.

## Auto Run Result

Status: done

**Summary:** A running station now takes a licence through `POST /api/license` without a restart. The body is the licence file's JSON. It runs the start-up checks (`check_installable` against the `--vendor-root-b64` roots, which `serve` now keeps even without `--license` and which a request can never add, and the station's signing key id). On success it swaps a new `LicenseContext` into the shared cell the certificate generator reads, so the next certificate is licensed. The new context keeps the running lease (watermark, evaluation count, store, writer thread and faults); the licensed count restarts only for a different licence id. The response is the `GET /api/license` view. Errors: 409 when the station trusts no root, 422 for a body that is not UTF-8, not a licence, does not verify, or names another station's key. A refused install changes nothing. The installed licence lasts until restart.

**Files changed:**
- `crates/wipe-license/src/install.rs` -- `parse_license`, shared by `load_license` and the route.
- `crates/wipe-license/tests/attestation.rs` -- malformed-file error test.
- `crates/wipe-server/src/licensing.rs` -- lease cells shared through `Arc`; `LicenseContext::with_chain`; 5 unit tests.
- `crates/wipe-server/src/app.rs` -- `vendor_roots`, `with_vendor_roots`, `install_license`, `LicenseInstallError`, `POST /api/license` route.
- `crates/wipe-server/src/handlers.rs` -- `install_license` handler (raw bytes, UTF-8 checked).
- `crates/wipe-server/tests/http_e2e.rs` -- 9 HTTP tests, one per I/O matrix row.
- `crates/wipe-cli/src/main.rs` -- roots decoded whenever given, `apply_licensing`, start-up line, flag help, wiring test.
- `apps/desktop/src/api/client.ts` -- `api.installLicense`.
- `README.md`, `CONTEXT.md`, `docs/ROADMAP.md`, `.scratch/STATUS.md` -- route, roots rule, in-memory limitation, unauthenticated write surface, count reset on licence switch.

**Review:** 23 findings (high 0, medium 1, low 15, false 7). Patched: 1 medium (the `serve` wiring of vendor roots was untested; now `apply_licensing` plus a test) and low rows for the stale module doc, doubled parse-error text with the `load_license` message pinned, non-UTF-8 body as 422, the `installLicense` doc, the STATUS.md row, and CONTEXT documentation of the unauthenticated write surface and of the count reset on licence switching. Deferred: persistence of a runtime-installed licence and a warning before restart (3 rows); the desktop shell's permanent 409. Rejected: the install/record race (milliseconds; errs toward over-counting; fix adds a field and branch, 2 rows); test-gap list (no defect shown); fail-fast on a malformed `--vendor-root-b64` (safe default); and 7 false (portal copy still accurate, no bench UI is L1.3, roots precondition is ADR-0008 §4, `with_license` cannot be called from a handler, accepted installs are meant to change counts). Patched entries by verdict: high 0, medium 1, low 7. Follow-up review: not recommended (no high, one medium).

**Verification:** `cargo test --workspace` (Postgres and Mailpit up): 875 passed, 1 failed: `wipe-hub` `silent_smtp_and_database_time_out_within_bound`, a timing test in a crate this story does not touch. Run alone on the same tree it failed once and passed once, and it passed in the pre-patch full run, so it is flaky, not caused by this change. Every new test ran and passed. `cargo clippy -p wipe-server -p wipe-license -p wipe-cli --all-targets`: only the two existing warnings (`wipe-engine/src/runner.rs:47`, `wipe-server/src/ws.rs:18`). `rustfmt --check` clean on the 7 touched `.rs` files. `pnpm typecheck` clean, `pnpm test` 291 passed. The route was not exercised against a live `wipestation serve` binary.

**Residual risks:** The route is unauthenticated with permissive CORS, like every station route (D2): anyone reaching the API can swap in any authentic licence for this station, including a superseded `evaluation` file over a `stop` one, or restart a `Count` licence's usage by switching A→B→A. An install landing mid-signing can count one erasure against the new licence. A runtime-installed licence is lost at restart without warning. `scripts/commerce-demo.sh` still installs with `--license` at start-up.
