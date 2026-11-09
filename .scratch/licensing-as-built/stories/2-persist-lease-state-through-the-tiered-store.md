---
title: 'Persist lease state through the tiered store'
type: 'feature'
created: '2026-11-08'
baseline_revision: 'c3320e7d8ffcb59b5a45f4aeaf6ce9a23eb2ec79'
status: 'done'
review_loop_iteration: 1
followup_review_recommended: true
context: ['{project-root}/docs/adr/0005-licensing-and-attestation.md', '{project-root}/docs/adr/0003-station-config-persistence.md']
warnings: ['oversized']
deferred:
  - summary: >-
      AGENTS.md still says `cargo test --workspace` runs 814 tests; the workspace now has about 835.
    evidence: |-
      README's test inventory heading was updated by this story; AGENTS.md is an agent-context file, which build-auto does not edit.
    location: >-
      AGENTS.md
    severity: low
  - summary: >-
      No mechanism recovers a station whose persisted watermark came from a clock set far in the future; only removing lease-state.json (which also restarts the count) clears it.
    evidence: |-
      Persisting the watermark (this story) means one boot with a bad RTC/NTP time leaves every later licensed decision ClockRollback across restarts. Documented in CONTEXT.md and ADR-0005 with the manual recovery; a capped watermark or vendor-signed reset is a product decision for a later story.
    location: >-
      crates/wipe-server/src/licensing.rs
    severity: medium
---

<intent-contract>

## Intent

**Problem:** Every station start builds a fresh `LeaseState` (`LicenseContext::unlicensed`, `build_license_context`), so restarting a station resets the clock watermark and the local erasure count: a restart defeats clock-rollback detection and refills a `Count` quota (ADR-0005 As built item 1, CAP-1). Nothing tells an operator whether lease state survives reboot (CAP-2).

**Approach:** Add a lease-state store in `wipe-server` beside the topology store: its own file (`lease-state.json`) in the directory the ADR-0003 tier detection resolves, written atomically, or RAM-only with a stated reason when that directory is not writable. `LicenseContext` loads lease state from the store at start-up and saves it after each signing decision and each recorded erasure; any lease I/O error makes signing `evaluation` and is logged. The context exposes its persistence tier and an operator-readable detail, which `wipestation serve` prints at start-up.

## Boundaries & Constraints

**Always:** Erasure and signing never block, wait or fail on lease state; an I/O error only marks certificates `evaluation` (ROADMAP §0.2). Lease state lives in its own file, never in the topology document; `save_bay_topology` is untouched. Writes go temp file + `sync_all` + rename + Unix directory fsync. A lease file that exists but cannot be read or parsed is never overwritten: the context keeps a sticky fault, signs `evaluation` while licensed, and the operator message names the file. A failed save is retried by the next save and signs `evaluation` until a save succeeds. Saves are serialized so an older snapshot never overwrites a newer one. The watermark is kept across licence changes; `erasures_used` counts the current licence only (see Autonomous decisions). `counter_is_monotonic` stays false: a file is not hardware-backed.

**Never:** No change to `wipe-engine*` or `wipe-common`. No new HTTP route or UI (deferred, see decisions). No evaluation-erasure counting (story 3), TPM, hub reporting, `allowed_methods` or site scope. No control-plane transport for lease state. No change to the cert hook's rule for which certificates are counted. No hardware-sanitization claim in any text.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| First start | writable dir, no file | fresh state at `now`; file written on first signing decision | none |
| Restart | file holds count 3, watermark W, same licence id | `lease_state()` count 3, watermark W | none |
| Clock rolled back after restart | persisted W, `now` = W − 1 day | status `ClockRollback`; watermark stays W | none |
| Count quota over restart | `Count { 3 }`, 3 licensed certs, restart | status `QuotaExhausted { used: 3, allowed: 3 }` | none |
| Different licence | stored `license_id` "old", count 5; open with "new" | count 0, watermark kept, `license_id` "new" | none |
| Unlicensed start on licensed file | stored "old", count 5; open with no licence | count 5 and `license_id` "old" unchanged | none |
| Corrupt file | file holds `not json` | licensed decision `evaluation` with status `LeaseStateUnavailable` naming the path; file bytes unchanged after signing | logged |
| Save fails | dir made read-only after open (skip assert as root) | that decision is `evaluation` with `LeaseStateUnavailable` | logged |
| Unwritable dir | detection probe fails | tier `Ephemeral`, `survives_reboot` false, detail says quota count and clock-rollback protection reset on reboot | none |
| No store wired | `AppState::new` / `LicenseContext::unlicensed` | tier `Ephemeral`, same reset wording | none |

## Autonomous decisions

- **Licence renewal and the erasure count.** A station's lease holds one erasure count. When an operator installs a new licence (a renewal with a fresh `Count` quota), carrying the old count over would start the new quota partly used; resetting the watermark would reopen clock rollback. Options: (a) keep everything; (b) reset everything; (c) keep the watermark, reset the count when the licence id differs. Chosen (c): `LeaseState`'s own doc says the count is "under the current license", and the watermark is a fact about the station's clock. Implemented as an optional `license_id` on `LeaseState` (`serde(default, skip_serializing_if)`). An unlicensed start leaves a stored count alone so a restart without `--license` loses nothing.
- **`--ephemeral-config` does not make lease state ephemeral.** The flag keeps *configuration* in RAM for demos; lease state is licensing enforcement, not configuration. Options: follow the flag (any customer can refill a quota with one flag) or keep detecting (demos write one small file). Chosen: keep detecting, the option that protects the licence's integrity.
- **Where the operator sees the tier.** No licence route or UI exists (CONTEXT §10 row: "no API route or UI"). Options: add `GET /api/license` plus TypeScript mirror and UI now, or expose it on `LicenseContext` and the `wipestation serve` start-up output (where licence status is already printed). Chosen: the latter; an HTTP/UI licence surface is a separate story and is recorded as a CONTEXT.md limitation. The desktop shell (no store wired) stays RAM-only and says so through the same API.
- **Control-plane tier.** No wire contract exists for lease state and hub reporting is a non-goal, so a station whose config directory is not writable runs lease state RAM-only even with `--control-plane-url`, and the detail says the control plane does not hold lease state.

</intent-contract>

## Code Map

- `crates/wipe-license/src/lease.rs:22-61` -- `LeaseState` (doc falsely claims persistence; add `license_id`); `LeaseStatus` (~:200) gains `LeaseStateUnavailable { detail }` with an operator message saying erasure still works and certificates are marked evaluation. Only `matches!` uses exist outside the crate (`wipe-hub/tests/stations.rs`, `licence_admin.rs`).
- `crates/wipe-license/src/lease.rs:105-160` -- `FileCounter::write_unlocked`: the atomic-write pattern (unique temp name, `create_new`, `sync_all`, rename, dir fsync) to mirror.
- `crates/wipe-server/src/store.rs:42-60,95-110,323-395` -- `StoreTier`, `LocalFileStore::probe`, `default_config_path`, `detect_store`: reuse the probe and path resolution; add `LeaseStore` trait, `LocalFileLeaseStore`, `EphemeralLeaseStore`, `LeasePersistence` (tier, survives_reboot, location, detail), `lease_state_path(cfg)` and `detect_lease_store(cfg)`.
- `crates/wipe-server/src/licensing.rs` -- `LicenseContext`: hold `Arc<dyn LeaseStore>`, a fault slot and a save mutex; add `open(station_id, chain: Option<AttestationChain>, store, now)` and `lease_persistence()`; `unlicensed`/`licensed` keep their signatures with an unwired ephemeral store; `signing_decision` and `record_erasure` persist.
- `crates/wipe-server/src/app.rs:250-320` -- cert hook calls `signing_decision` then `record_erasure`; no change expected. `with_static_dir` default stays unlicensed/ephemeral.
- `crates/wipe-server/src/lib.rs:15` -- re-exports; add the new store types.
- `crates/wipe-cli/src/main.rs:478-515,850-890` -- `cmd_serve` builds `StoreConfig` then `build_license_context`; pass the lease store, use `LicenseContext::open`, print `lease store: {tier} at {location} — {detail}`.
- `crates/wipe-server/tests/topology_store.rs:85-121,312-360` -- read-only-dir pattern with `nix_running_as_root`, and the licence-wiring test showing how to issue a licence in a test.
- `crates/wipe-engine/src/runner.rs:111,140,863` -- `create_job`, `start`, `wait_for_erased` for driving erasures in the AppState-level test; certs land in `AppState::certs`.
- Docs: `CONTEXT.md:720,916,1002,1388`; `README.md:402-403,484`; `docs/ROADMAP.md:481`; `docs/adr/0005-licensing-and-attestation.md` As built item 1; `docs/adr/0003-station-config-persistence.md` (note lease state's own file).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-license/src/lease.rs` -- add `license_id` to `LeaseState`, the `LeaseStateUnavailable` variant and message, and fix the `LeaseState` doc -- CAP-1.
- `crates/wipe-server/src/store.rs` -- lease store trait, two backends, `LeasePersistence`, detection -- CAP-1/CAP-2.
- `crates/wipe-server/src/licensing.rs`, `lib.rs` -- load at open, licence-id reset, persist on decision and erasure, sticky load fault, transient save fault, `lease_persistence()` -- CAP-1/CAP-2 and the I/O constraint.
- `crates/wipe-cli/src/main.rs` -- detect the lease store from the same `StoreConfig`, open the context through it, print the lease-store line -- CAP-2.
- `crates/wipe-server/tests/lease_store.rs` (new) -- one test per matrix row; the restart and quota rows drive real erasures through an `AppState` with the mock backend -- CAP-1/CAP-2.
- `crates/wipe-license/tests/attestation.rs` -- `LeaseStateUnavailable` does not permit licensed signing; old JSON without `license_id` still parses.
- `CONTEXT.md`, `README.md`, `docs/ROADMAP.md`, ADR-0005 (annotate item 1 resolved 2026-11-08, keep the original text), ADR-0003 -- state lease persistence, the RAM-only case, and the no-route limitation.
- `crates/wipe-server/src/licensing.rs` -- bounded saves (see Design Notes): one writer thread per `LicenseContext` performs every `store.save` in submission order; `persist` sends a snapshot and waits at most `LEASE_SAVE_TIMEOUT` (2 s) for its result; a timeout is a save fault. `status()` also persists, `signing_decision` reuses `status()` instead of copying it, and `persist` returns nothing -- the spec's "never wait" rule on a hung disk.
- `crates/wipe-server/src/store.rs` -- in `detect_lease_store`, when the probe fails but `lease-state.json` there loads, seed the RAM store with it (private field, no new pub API) and say so in the detail; lease load/save errors name the path once and never call lease state "configuration".
- `crates/wipe-server/src/licensing.rs` -- the sticky load-fault detail tells the operator to repair the file and restart the station.
- `crates/wipe-cli/src/main.rs` -- extract a private `serve_stores(cfg, force_ephemeral)` returning the topology and lease stores; unit-test it (with `--ephemeral-config` a writable dir still gives a local-file lease store) and `build_license_context` (unlicensed path over a temp-dir `LocalFileLeaseStore`: tier `LocalFile`, file written after a signing decision).
- `crates/wipe-server/tests/lease_store.rs` -- add: a store whose `save` blocks makes `signing_decision` return `evaluation` with `LeaseStateUnavailable` within 5 s, and licensed signing returns once it unblocks; `status()` on a corrupt file is `LeaseStateUnavailable` naming the path; a read-only dir holding a valid lease file seeds the RAM store with its count. Chmod tests skip by checking that a write into the 0o500 dir really fails, not by reading `$USER`.
- Docs -- the ADR-0003 amendment, the `cmd_serve` comment and the CONTEXT.md lease glossary must not claim no flag can refill a quota: deleting or moving the file, or pointing `--bay-topology`/`WIPESTATION_CONFIG_DIR` at an empty directory, starts a fresh lease, the speed bump ADR-0005 §3a already admits. README's "PXE-ephemeral by design" bullet names lease state as a third kind of local state.

**Acceptance Criteria:**
- Given a licensed station with a `Count { erasures: 3 }` quota and a writable config directory, when three drives are erased to certificates and the station is rebuilt from the same directory, then its licence status is `QuotaExhausted` and the next certificate is `evaluation`.
- Given a station whose config directory is not writable, when `wipestation serve` starts, then the start-up output has a lease-store line naming the ephemeral tier and stating that the quota count and clock-rollback protection reset on reboot.
- Given the workspace, when `cargo test --workspace` runs with compose up, then it passes.

## Spec Change Log

### 2026-11-08 — loop 1 (bad_spec)
- **Trigger:** blind-hunter and edge-case-hunter (medium): `persist()` ran temp-write, `sync_all`, rename and directory fsync synchronously inside the cert-generator task, so a hung config disk (NFS on a PXE bench) stalls every certificate behind it, against the Always rule "signing never blocks, waits or fails on lease state".
- **Amended:** Tasks (bounded saves through one writer thread with a 2 s wait; plus the folded-in patch items: `status()` persists and `signing_decision` reuses it, read-only dir seeds RAM from an existing file, error text, restart hint, CLI `serve_stores` + unit tests, extra lease tests, root-check by real write, doc overclaim and README bullet); Design Notes added.
- **Known-bad state avoided:** certificate issuance hanging indefinitely on lease I/O; an older snapshot landing after a newer one once a hung write resumes.
- **KEEP:** everything in `_bmad-output/implementation-artifacts/licensing-as-built-story-2-loop0.patch` not named above: the `LeaseStore` trait, `LocalFileLeaseStore` (atomic write) and `EphemeralLeaseStore`, `LeasePersistence`, `lease_state_path`, `detect_lease_store`, `LicenseContext::open` semantics (licence-id reset keeping the watermark, unlicensed start leaving the count, sticky load fault that never overwrites, transient save fault), `LeaseState::license_id`, `LeaseStatus::LeaseStateUnavailable` and its message, the `lease store:` start-up line, all 11 `lease_store.rs` tests including the end-to-end `Count { 3 }` restart test, the 2 `attestation.rs` tests, and the doc updates. Start from that patch (`git apply`) and change only what the amended tasks require.

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 32 findings — high 0, medium 6, low 24, false 2, maybe-false 0
- findings:
  - `[low]` `[patch]` (blind) Deleting or relocating the lease file (`rm`, `--bay-topology`, `WIPESTATION_CONFIG_DIR`) refills the quota, though the ADR-0003 amendment says no flag can — real overclaim; ADR-0005 §3a already calls the file a speed bump. Folded into amendment: docs state the bypass.
  - `[low]` `[reject]` (blind) Licence swap A→B→A restarts A's count — real, but strictly harder than deleting the file, which already refills it; a per-licence map changes `LeaseState`'s shape for no added protection.
  - `[medium]` `[bad_spec]` (blind) Synchronous fsync in the async cert task stalls signing on a slow or hung disk — confirmed (`licensing.rs` persist called from `app.rs` hook); amendment: bounded saves through one writer thread.
  - `[low]` `[patch]` (blind) Lease error text repeats the path and says "stored configuration" — confirmed via `StoreError::Parse` Display; folded into amendment.
  - `[low]` `[patch]` (blind) Start-up line untested; prints `StoreTier` Debug name — untested part folded into the CLI-test amendment; Debug name rejected: it matches the adjacent `config store:` line's format.
  - `[low]` `[patch]` (blind) Root check reads `$USER`, so chmod tests fail as uid 0 without `USER` — folded into amendment (probe a real write).
  - `[low]` `[reject]` (blind) Test temp dirs never removed — same pattern as `topology_store.rs`; per-pid names; fixing needs a new dev-dependency.
  - `[low]` `[patch]` (blind) Untested: `--ephemeral-config` rule (folded into CLI-test amendment); `record_erasure` save failure and concurrent save ordering rejected (behaviour correct by construction, low value); fsync failure after rename marking evaluation is conservative, not a defect.
  - `[low]` `[patch]` (blind) `persist()` bool unused; `signing_decision` copies `status()` — confirmed; folded into amendment.
  - `[low]` `[defer]` (blind) AGENTS.md test count stale — agent-context file, deferred; STATUS.md row rejected (it lists the epic, whose story 3 is still queued); story file absent from diff is false (deliberately held back as the claims file).
  - `[low]` `[patch]` (blind) README "PXE-ephemeral by design" bullet names only evidence and configuration — confirmed; folded into amendment.
  - `[medium]` `[patch]` (edge) Unwritable dir with a readable lease file starts at count 0 — confirmed in `detect_lease_store`; folded into amendment (seed RAM store).
  - `[low]` `[reject]` (edge) Per-licence count map — same as the blind A→B→A row.
  - `[low]` `[patch]` (edge) Flag relocation gives a fresh lease — same root cause as the blind deletion row.
  - `[low]` `[reject]` (edge) Crash between licensed signing and `record_erasure`'s save loses one count — one unit per crash; counting before signing changes which certificates count, which this story must not touch.
  - `[medium]` `[bad_spec]` (edge) fsync inside the async loop — same root cause as the blind bad_spec row.
  - `[low]` `[reject]` (edge) Orphan temp files after a crash mid-write — rare; cleanup scan adds code for a cosmetic leftover.
  - `[low]` `[reject]` (edge) Two processes sharing one config dir, last writer wins — also true of the topology file; not an everyday setup.
  - `[low]` `[patch]` (edge) Sticky load-fault message lacks "restart the station" — confirmed; folded into amendment.
  - `[low]` `[patch]` (edge) Root check via `$USER` — same root cause as the blind root-check row.
  - `[low]` `[patch]` (edge) ADR-0003 amendment overclaims about flags — same root cause as the blind deletion row.
  - `[medium]` `[patch]` (edge) Read-only dir ignores existing lease contrary to persistence claim — same root cause as the edge seed row.
  - `[low]` `[patch]` (verification-gap, pre-verified) `status()` fault path untested — folded into amendment (assert on corrupt file).
  - `[medium]` `[patch]` (verification-gap, pre-verified) CLI wiring and `--ephemeral-config` rule untested — folded into amendment (`serve_stores` + unit tests).
  - `[low]` `[patch]` (verification-gap other) Root check via `$USER` — same root cause as the blind root-check row.
  - `[low]` `[reject]` (verification-gap other) Temp dirs left behind — same as the blind temp-dir row.
  - `[medium]` `[patch]` (intent) AC2 start-up output not exercised by any test — same root cause as the CLI-test row.
  - `[false]` `[reject]` (intent) CAP-2's "lease status reports the tier" met on a separate struct — the start-up output prints the tier on the `lease store:` line beside the licence line; the route/UI surface is queued as `licence-visibility` (STATUS.md).
  - `[low]` `[reject]` (intent) Rollback-after-restart tested only at context level — the end-to-end test asserts the watermark survives a real rebuild, and the rollback test reads that same persisted file format.
  - `[low]` `[reject]` (intent) `record_erasure` save failure leaves the counted cert licensed — the next save writes the in-memory count; only a crash loses it (see edge crash row).
  - `[low]` `[patch]` (intent) Watermark saved less often than it moves (`status()` does not save) — folded into amendment (`status()` persists).
  - `[false]` `[reject]` (intent) Desktop shell untested — it wires no store by design, documented RAM-only; `AppState::new` (its constructor path) is tested.

### 2026-11-08 — Review pass (after loop 1)
- verdicts: 34 findings — high 0, medium 10, low 23, false 1, maybe-false 0
- findings:
  - `[medium]` `[patch]` (blind) Lease saves wait up to 2 s on a tokio worker inside the cert task, and `while let Ok` ends certification for good on broadcast `Lagged` — confirmed in `app.rs`; both licence calls now run in `spawn_blocking`, `Lagged` logs and continues, only `Closed` stops.
  - `[medium]` `[patch]` (blind) A watermark saved while the clock was far in the future now persists, so licensed certs stay `ClockRollback` across restarts with no stated recovery — confirmed; CONTEXT.md and ADR-0005 now state it and the recovery (fix the clock, remove `lease-state.json`, restart). Mechanism deferred.
  - `[low]` `[reject]` (blind) Writer queue not flushed at shutdown — only matters after a save already timed out, which marked the fault; equivalent to the carried crash row.
  - `[low]` `[reject]` (blind) `status()` writes to disk — documented on the method; the future licence route can choose its own read path.
  - `[low]` `[reject]` (blind) A timed-out or failed save's fault can be set after a later success cleared it — needs a save failure followed by thread descheduling across a later success; sequence numbers add complexity for a self-healing state (the next save clears it).
  - `[low]` `[reject]` (blind) Unbounded writer queue while a save hangs — each caller already waits 2 s, so growth is bounded by certificate rate.
  - `[low]` `[reject]` (blind) Unlicensed station hides save failures — its certificates are `evaluation` regardless; the error is logged and in `lease_persistence()`.
  - `[low]` `[reject]` (blind) `LeaseStateUnavailable` masks the underlying verdict — deliberate: every other verdict was computed from untrusted state.
  - `[low]` `[patch]` (blind) "Repair the file" gives no workable step — message now says restore a good copy or remove it to start a fresh lease, then restart.
  - `[low]` `[defer]` (blind) AGENTS.md test count stale — carried: agent-context file, already deferred.
  - `[low]` `[patch]` (blind) `/proc/...` unwritable-path test would fail on Windows — marked `#[cfg(unix)]`.
  - `[low]` `[patch]` (blind) `--ephemeral-config` now writes the user's real config dir, undocumented — help text now says lease state is still saved; demo fixed (see verification-gap row).
  - `[low]` `[reject]` (blind) `record_erasure` save failure untested — carried: behaviour correct by construction, same as the earlier row.
  - `[medium]` `[patch]` (edge) Read-only dir with an unreadable lease file starts a fresh lease with no fault — confirmed; the RAM store now returns that load error so `open` records the sticky fault; test added.
  - `[low]` `[reject]` (edge) Per-licence count map — carried reject.
  - `[medium]` `[patch]` (edge) Future watermark permanent `ClockRollback` — same root cause as the blind watermark row.
  - `[medium]` `[patch]` (edge) Blocking `recv_timeout` in the async cert loop; `Lagged` ends the loop — same root cause as the blind cert-task row.
  - `[low]` `[patch]` (edge) Directory fsync `EINVAL`/unsupported on some FUSE/overlay filesystems fails every save — `sync_dir` now treats `InvalidInput`/`Unsupported` after the rename as success, with a warning.
  - `[low]` `[reject]` (edge) Orphan temp files after a crash — carried reject.
  - `[low]` `[reject]` (edge) Two processes sharing one config dir — carried reject.
  - `[low]` `[reject]` (edge) Stale save fault after a later success — same as the blind stale-fault row.
  - `[low]` `[reject]` (edge) `record_erasure` save failure then restart loses one count — carried reject.
  - `[medium]` `[patch]` (edge) Claim "any lease I/O error makes signing evaluation" broken for read-only dir + unreadable file — same root cause as the edge read-only row.
  - `[medium]` `[patch]` (verification-gap, pre-verified) Licensed branch of `build_license_context` untested — added `a_licensed_station_starts_from_its_saved_lease_count`.
  - `[medium]` `[patch]` (verification-gap other) Commerce demo writes `~/.config/wipestation/lease-state.json`, resetting a real station's lease on the developer's machine — `start_station` now sets `WIPESTATION_CONFIG_DIR="$WORK/station-config"`.
  - `[medium]` `[patch]` (verification-gap other) Saves block a tokio worker up to 2 s — same root cause as the blind cert-task row.
  - `[false]` `[reject]` (intent) CAP-2 tier not on `LeaseStatus` — carried false.
  - `[low]` `[patch]` (intent) AC2 not exercised from an unwritable dir through `serve_stores` — added `serve_stores_on_a_read_only_directory_runs_the_lease_in_ram_and_says_so`.
  - `[low]` `[reject]` (intent) CAP-1 test builds the station via `AppState`, rollback on a seeded file — carried reject.
  - `[low]` `[reject]` (intent) No control-plane tier for lease state — an autonomous decision in the intent contract; the fix would edit the spec.
  - `[low]` `[reject]` (intent) `--ephemeral-config` could be read as covering lease state — an autonomous decision in the intent contract; the fix would edit the spec.
  - `[low]` `[patch]` (intent) Seeded RAM store detail says counts "reset on reboot" when the station returns to the file's values — wording corrected.
  - `[medium]` `[patch]` (intent) Strict "never wait" reading vs 2 s bounded wait in the cert task — same root cause as the blind cert-task row; the wait is now off the async workers.
  - `[low]` `[reject]` (intent) Licensed `serve` writes the lease file at start-up via `status()`, earlier than the matrix's first signing decision — harmless superset; the row's expectations still hold.

## Design Notes

Bounded saves keep certificate signing from waiting on a hung disk while still marking a failed save `evaluation`. One writer thread per context gives ordering for free; a hung write only makes later saves time out, and when it resumes the queued snapshots land oldest-first, so the newest wins:

```rust
// persist(): under a mutex, so send order == snapshot order
let snapshot = self.lease.read().clone();
let (ack_tx, ack_rx) = std::sync::mpsc::channel();
self.writer.send((snapshot, ack_tx))?;           // writer thread: ack_tx.send(store.save(&s))
match ack_rx.recv_timeout(LEASE_SAVE_TIMEOUT) {   // 2 s
    Ok(Ok(())) => clear save fault,
    Ok(Err(e)) => save fault (e),
    Err(_)     => save fault ("not saved within 2 s"),
}
```

The thread ends when the context (the sender) is dropped. No tokio dependency in this path: it must work from plain `#[test]`s and the CLI start-up.

## Verification

**Commands:**
- `cargo test -p wipe-server -p wipe-license -p wipe-cli` -- expected: all pass.
- `cargo clippy -p wipe-server -p wipe-license -p wipe-cli --all-targets -- -D warnings` -- expected: clean.
- `cargo test --workspace` (with `docker compose up -d`) -- expected: all pass.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.

**Manual checks:**
- `WIPESTATION_CONFIG_DIR=<chmod 500 dir> cargo run -p wipe-cli -- serve --no-fleet --addr 127.0.0.1:0` (or any free port) prints the ephemeral lease-store line; with a writable dir it prints the local-file line and `lease-state.json` appears after the first certificate.

## Auto Run Result

Status: done

**Summary:** Lease state (clock watermark, licensed erasure count, licence id) now survives a restart. `wipestation serve` keeps it in `lease-state.json` in the directory the ADR-0003 detection resolves, written atomically by one writer thread per `LicenseContext` with a 2 s bounded wait; the certificate hook calls the licence off the async workers. A directory that is not writable runs lease state RAM-only (seeded from an existing readable file), and the `lease store:` start-up line says the quota count and clock-rollback protection reset on reboot. Any lease I/O fault (unreadable file, failed or slow save) marks licensed certificates `evaluation` via `LeaseStatus::LeaseStateUnavailable` and never stops an erasure; an unreadable file is never overwritten. A new licence id keeps the watermark and restarts the count. `--ephemeral-config` does not affect lease state.

**Files changed:**
- `crates/wipe-license/src/lease.rs` — `LeaseState::license_id`; `LeaseStateUnavailable` variant and message; corrected `LeaseState` doc.
- `crates/wipe-server/src/store.rs` — `LeaseStore` trait, `LocalFileLeaseStore`, `EphemeralLeaseStore` (seeded / load-fault carrying), `LeasePersistence`, `lease_state_path`, `detect_lease_store`, tolerant `sync_dir`.
- `crates/wipe-server/src/licensing.rs` — `LicenseContext::open`, writer thread with `LEASE_SAVE_TIMEOUT`, sticky load / transient save faults, `lease_persistence()`; `status()` saves and `signing_decision` reuses it.
- `crates/wipe-server/src/app.rs` — licence calls in `spawn_blocking`; broadcast `Lagged` no longer ends the certificate generator.
- `crates/wipe-server/src/lib.rs` — re-exports.
- `crates/wipe-cli/src/main.rs` — `serve_stores`, `lease_store_line`, licence opened through the lease store, `--ephemeral-config` help, 5 new unit tests.
- `crates/wipe-server/tests/lease_store.rs` (new, 15 tests) and `crates/wipe-license/tests/attestation.rs` (+2).
- `scripts/commerce-demo.sh` — demo station uses `$WORK/station-config`, not the developer's real config dir.
- `CONTEXT.md`, `README.md`, `docs/ROADMAP.md`, ADR-0003 amendment, ADR-0005 item 1 resolved.
- `_bmad-output/implementation-artifacts/licensing-as-built-story-2-loop0.patch` — loop-0 attempt kept as the KEEP reference.

**Review:** two passes. Pass 1 (32 findings: 6 medium, 24 low, 2 false) routed `bad_spec` for synchronous fsync in the certificate task; the spec was amended (bounded writer thread) with the patch items folded in, and the code re-derived (loop 1). Pass 2 (34 findings: 10 medium, 23 low, 1 false): patched 5 medium entries (cert task off async workers + `Lagged` handling; future-watermark recovery documented; read-only dir with an unreadable file is a sticky fault; licensed CLI path tested; commerce demo isolated) and 5 low (dir-fsync EINVAL, restore/remove wording, seeded-store wording, Windows cfg, read-only `serve_stores` test, flag help). Deferred: AGENTS.md test count (agent-context file); a recovery mechanism for a future-dated watermark (product decision). Rejected, with reasons in the triage log: per-licence count map (deleting the file is an easier bypass), orphan temp files, two processes sharing a config dir, crash between signing and saving the count, stale-fault race, unbounded queue while hung, writer not flushed at shutdown, `status()` writing, unlicensed save faults only logged, fault masking the underlying verdict, Debug tier name (matches the config-store line), temp dirs in tests, control-plane and `--ephemeral-config` readings (intent-contract decisions), and false findings on the CAP-2 surface and desktop shell.

**Follow-up review recommended:** true — pass 2 patched five medium entries after review, including a change to the certificate hook (`spawn_blocking` around the licence calls and `Lagged` recovery in `app.rs`) and the fault-carrying RAM store; neither has had an independent review. Patched entries this pass by verdict: medium 5, low 5.

**Verification:** after the final patches, `cargo test --workspace --no-fail-fast` with compose up: exit 0. `cargo clippy -p wipe-server -p wipe-license -p wipe-cli --all-targets -- -D warnings` fails only on the pre-existing `large_enum_variant` lint in untouched `wipe-engine` (`JobBroadcast`); with `-A clippy::large_enum_variant` it is clean. `rustfmt --edition 2021 --check` on every touched `.rs` file is clean (it also reports pre-existing `ws.rs`, reached through `lib.rs`, untouched). `bash -n scripts/commerce-demo.sh` ok; the demo itself was not run. Manually (loop 0 build): `wipestation serve` with a read-only `WIPESTATION_CONFIG_DIR` printed the RAM-only `lease store:` line with the reset wording. Earlier runs saw the known-flaky `wipe-hub` `health::silent_smtp_and_database_time_out_within_bound` and once `auth::concurrent_guesses_get_exactly_five_password_checks` fail under load; neither touches this code and the final run passed.

**Residual risks:** a hung disk delays each certificate by up to 2 s per save (two per licensed erasure) and marks it `evaluation`; deleting or moving `lease-state.json`, or pointing the config dir elsewhere, starts a fresh lease (the speed bump ADR-0005 §3a admits); an A→B→A licence swap restarts A's count; a future-dated watermark needs manual recovery; no API route or UI shows the lease (queued as `licence-visibility`); the desktop shell stays RAM-only.
