---
title: 'FileCounter and honest hardware-backed flags'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '6546677d70cc8e7768416764ae4be4360280d3d0'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/docs/adr/0005-licensing-and-attestation.md']
warnings: []
deferred:
  - summary: >-
      LeaseState::counter_is_monotonic and MonotonicCounter::is_hardware_backed answer the same question and nothing derives one from the other.
    evidence: |-
      counter_is_monotonic (lease.rs, LeaseState) is hard-coded false in LeaseState::new and predates FileCounter; story 2 could pair a FileCounter with counter_is_monotonic = true. Pre-existing field, not changed by this story; wiring belongs to story 2.
    location: >-
      crates/wipe-license/src/lease.rs:38
    severity: low
  - summary: >-
      wipe-hub test silent_smtp_and_database_time_out_within_bound is timing-flaky under a full workspace run.
    evidence: |-
      Failed at crates/wipe-hub/tests/health.rs:167 with 5.5-7.9 s against a 3.5 s bound during cargo test --workspace and twice in isolation; passed on other runs at ~3.3 s. It probes SMTP/DB timeouts and does not touch wipe-license's counter code.
    location: >-
      crates/wipe-hub/tests/health.rs:167
    severity: low
---

<intent-contract>

## Intent

**Problem:** ADR-0005 §3b promises a `MonotonicCounter` with a `FileCounter` implementation (rewindable, and labelled so) and a `TpmCounter` that returns `Unsupported`. `FileCounter` does not exist, and `TpmCounter::is_hardware_backed()` returns `true` although it cannot count — a placeholder claiming a guarantee it does not give (ADR-0005 As built item 2, CAP-3).

**Approach:** Add `FileCounter` in `wipe-license`'s `lease` module: a counter persisted as a decimal integer in one file, written atomically, reporting `is_hardware_backed() == false`. Make `TpmCounter` report `false` too until it is implemented, and update the docs that say `FileCounter` is absent.

## Boundaries & Constraints

**Always:** `FileCounter` uses the trait's default `is_hardware_backed()` (false). A missing counter file reads as 0; any other read failure or unparsable content is an `Err`, never a fabricated number. `increment` writes via temp file + rename in the same directory (same pattern as `crates/wipe-server/src/store.rs:149-161`), creating the parent directory if missing, and never wraps on overflow. Increments within one process are serialized so two concurrent `increment` calls yield distinct values. `TpmCounter` keeps returning `Unsupported` from `read`/`increment`.

**Never:** Wire `FileCounter` into `LeaseState`, `AppState`, the CLI or the tiered store (story 2). No TPM work, no hub reporting, no new public API route. No new dependency in engine crates. No hardware-sanitization claim in any text.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Fresh counter | file absent | `read()` → `Ok(0)`; `increment()` → `Ok(1)` and file now holds `1` | none |
| Survives re-open | increment 3×, new `FileCounter` on same path | `read()` → `Ok(3)` | none |
| Missing parent dir | path under non-existent dir | `increment()` creates dir, → `Ok(1)` | none |
| Corrupt file | file holds `abc` | `read()` and `increment()` → `Err(LicenseError::Encoding(..))` naming the path; file unchanged | error returned |
| At max | file holds `u64::MAX` | `increment()` → `Err`, file unchanged | error returned |
| Concurrent increments | 8 threads × 25 increments on one shared counter | final `read()` == 200 | none |
| TPM placeholder | `TpmCounter` | `is_hardware_backed()` false; `read`/`increment` `Err(Unsupported)` | error returned |

</intent-contract>

## Code Map

- `crates/wipe-license/src/lease.rs:60-94` -- `MonotonicCounter` trait (default `is_hardware_backed` false) and `TpmCounter` with the `true` override to remove; add `FileCounter` here.
- `crates/wipe-license/src/lib.rs:36-54` -- `LicenseError` (`Encoding` is the variant `install.rs` uses for file I/O); `pub use lease::*` already re-exports new types.
- `crates/wipe-license/src/install.rs:30-41` -- existing file-write style (create_dir_all, error messages with `path.display()`).
- `crates/wipe-server/src/store.rs:145-162` -- reference atomic temp+rename write.
- `crates/wipe-license/tests/attestation.rs:411-428` -- `the_tpm_counter_seam_fails…` asserts `is_hardware_backed()` true; flip it. Add FileCounter tests nearby.
- `crates/wipe-license/Cargo.toml` -- no `tempfile`; workspace has `tempfile = "3"` (`Cargo.toml:82`), add as dev-dependency.
- Docs saying `FileCounter` is absent: `CONTEXT.md:916`, `CONTEXT.md:1004`, `CONTEXT.md:1390`; `docs/adr/0005-licensing-and-attestation.md:240-245` (As built item 2).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-license/src/lease.rs` -- add `pub struct FileCounter` (`new(path)`, `path()`), implement `MonotonicCounter` per the matrix; remove `TpmCounter`'s `is_hardware_backed` override and document that it turns true only once the counter really works; fix trait doc comment -- CAP-3.
- `crates/wipe-license/Cargo.toml` -- add `[dev-dependencies] tempfile = { workspace = true }` -- tests need temp dirs.
- `crates/wipe-license/tests/attestation.rs` -- flip the TPM assertion; add one test per matrix row for `FileCounter` -- covers the matrix.
- `CONTEXT.md`, `docs/adr/0005-licensing-and-attestation.md` -- state that `FileCounter` exists (rewindable, not hardware-backed) and `TpmCounter` no longer claims hardware backing; keep "lease not persisted" wording (story 2). In the ADR, annotate As built item 2 as resolved on 2026-11-08 rather than deleting it -- docs must not trail code.

**Acceptance Criteria:**
- Given any `FileCounter`, when `is_hardware_backed()` is called, then it returns false.
- Given `TpmCounter`, when `is_hardware_backed()` is called, then it returns false, and `read()` still returns `Unsupported`.
- Given the workspace, when `cargo test --workspace` runs, then it passes.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 20 findings — high 0, medium 2, low 13, false 4, maybe-false 1
- findings:
  - `[low]` `[patch]` (blind) Old test `the_file_counter_does_not_claim_hardware_backing` never builds a FileCounter — renamed to `a_fresh_lease_state_does_not_claim_a_monotonic_counter`.
  - `[low]` `[defer]` (blind) `counter_is_monotonic` and `is_hardware_backed` not tied — pre-existing field; recorded in `deferred` for story 2.
  - `[medium]` `[patch]` (blind) No parent-directory fsync after rename, so power loss can revert the counter after increment returned — added `#[cfg(unix)]` dir `sync_all()` after rename.
  - `[low]` `[patch]` (blind) Temp name per-pid only, so two instances on one path collide — temp name now pid + process-wide AtomicU64 sequence, opened `create_new`.
  - `[low]` `[reject]` (blind) I/O errors reported as `LicenseError::Encoding` — follows `install.rs`; a new variant adds public surface for a cosmetic message on a type no caller uses yet.
  - `[low]` `[patch]` (blind) ADR note "persisting the lease through it is item 1" overstates — reworded to "Nothing constructs a FileCounter yet, and lease persistence remains item 1."
  - `[low]` `[patch]` (blind) Failed-write path and temp cleanup untested — added `a_failed_counter_write_is_an_error` (parent is a regular file) and a no-leftover-temp assertion.
  - `[low]` `[reject]` (blind) Whitespace trimming untested/undocumented — tolerant parsing of hand-edited files is harmless; unlikely to regress in practice.
  - `[false]` `[reject]` (blind) No recovery path for a corrupt counter could block erasures — nothing calls FileCounter; SPEC constraint already requires signing as evaluation on any lease I/O error (story 2).
  - `[low]` `[patch]` (blind) Trait headline "A counter that only moves forward" overclaims — reworded to "meant to only move forward", pointing at `is_hardware_backed`.
  - `[medium]` `[patch]` (edge) Missing dir fsync after rename — same root cause as blind fsync row; fixed there.
  - `[low]` `[patch]` (edge) Shared temp file between instances — same root cause as blind temp-name row; fixed there.
  - `[low]` `[reject]` (edge) Rename replaces a symlinked counter path with a regular file — operators do not symlink an unwired counter; canonicalize handling adds complexity.
  - `[low]` `[patch]` (verification-gap, pre-verified) Non-NotFound read failure untested — added `an_unreadable_counter_file_is_an_error_not_zero` (directory path and non-UTF-8 bytes).
  - `[low]` `[patch]` (verification-gap other) Old test name misleading — same root cause as first blind row; renamed.
  - `[low]` `[reject]` (verification-gap other) Temp+rename vs direct write not unit-testable — informational; no practical test.
  - `[false]` `[reject]` (intent) Honest label only observable as a method — CAP-3's success line defines it at `is_hardware_backed()`; surfacing in status is story 2 (CAP-2).
  - `[maybe-false]` `[patch]` (intent) ADR forward claim about persisting through FileCounter — same root cause as blind ADR row; reworded. Settled by the reword either way.
  - `[false]` `[reject]` (intent) Concurrency narrower than CAP-1 may need — matrix row tested as written; cross-instance collision fixed via temp-name patch; CAP-1 is story 2.
  - `[false]` `[reject]` (intent) No evidence the workspace AC passes — `cargo test --workspace` exit 0 recorded in Auto Run Result.

## Verification

**Commands:**
- `cargo test -p wipe-license` -- expected: all pass, including new FileCounter tests.
- `cargo clippy -p wipe-license --all-targets -- -D warnings` -- expected: clean.
- `cargo test --workspace` (with `docker compose up -d`) -- expected: all pass.
- `rustfmt --edition 2021 --check crates/wipe-license/src/lease.rs crates/wipe-license/tests/attestation.rs` -- expected: clean.

## Auto Run Result

Status: done

**Summary:** Added `wipe_license::FileCounter`, a `MonotonicCounter` kept as a decimal integer in one file (missing file = 0; unreadable or unparsable = `Encoding` error, never a fabricated number; temp file + rename + Unix directory fsync; refuses to wrap; increments serialized per instance). `TpmCounter` keeps returning `Unsupported` and no longer reports `is_hardware_backed() == true`. Nothing constructs a `FileCounter` yet (story 2).

**Files changed:**
- `crates/wipe-license/src/lease.rs` — `FileCounter`; `TpmCounter` override removed; trait docs corrected.
- `crates/wipe-license/Cargo.toml`, `Cargo.lock` — `tempfile` dev-dependency.
- `crates/wipe-license/tests/attestation.rs` — TPM assertion flipped; 8 FileCounter tests covering every matrix row plus unreadable-path and failed-write cases; misleading test renamed.
- `CONTEXT.md` — three passages now say `FileCounter` exists (rewindable, unwired) and `TpmCounter` claims no hardware backing.
- `docs/adr/0005-licensing-and-attestation.md` — As-built item 2 annotated resolved 2026-11-08.

**Review:** 20 findings. Patched: 7 entries (1 medium — directory fsync; 6 low). Deferred: 1 (flag duplication, story 2) plus the flaky hub timing test noticed during verification. Rejected: Encoding variant naming (cosmetic, new public surface), whitespace trimming (harmless), symlink replacement (unlikely, adds complexity), temp+rename untestable (informational), and 4 false (corrupt-file erasure blocking, label surface, concurrency scope, workspace AC evidence) — reasons in the triage log.

**Follow-up review recommended:** false — one medium and no high patched on a first pass.

**Verification:** `rustfmt --edition 2021 --check` on both Rust files clean; `cargo clippy -p wipe-license --all-targets -- -D warnings` clean; `cargo test -p wipe-license` 36 attestation tests pass; `cargo test --workspace --no-fail-fast` with compose up: exit 0 (57 suites ok) before and after patches. One post-patch run failed only `wipe-hub` `silent_smtp_and_database_time_out_within_bound` (timing bound, unrelated code); an immediate rerun of the workspace passed.

**Residual risks:** Two processes on one counter file are not coordinated (documented; story 2 must keep one instance). If the directory fsync fails, the new value is on disk but `increment` returns an error. The `wipe-hub` health timing test is flaky under load.
