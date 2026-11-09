---
title: 'Desktop persistence in the app-data directory'
type: 'feature'
created: '2026-11-09'
status: 'done'
baseline_revision: '87347ce9c52d8db91a0ecefefdac74446e539330'
review_loop_iteration: 1
followup_review_recommended: false
context: []
warnings: [oversized]
deferred:
  - summary: >-
      A corrupt saved layout is only logged, and the next save overwrites it without a backup; now reachable on the desktop.
    evidence: |-
      LocalFileStore::load returns Parse errors that AppState only logs; save renames over the path.
      Same as the CLI default path (audit 2026-11-05, ADR-0003 deviations).
    location: >-
      crates/wipe-server/src/store.rs LocalFileStore
    severity: low
  - summary: >-
      A signing seed created before this change (mode 0644 via fs::write) is read with no warning or permission tightening.
    evidence: |-
      load_or_create_signing_key only sets 0600 on files it creates; existing --key-path files keep their mode.
    location: >-
      crates/wipe-server/src/bootstrap.rs load_or_create_signing_key
    severity: low
  - summary: >-
      No test drives the desktop's run()/bootstrap_api wiring of app_local_data_dir() into station_config.
    evidence: |-
      Tests build DataDir themselves; covering run() needs a Tauri runtime harness the repo lacks.
    location: >-
      apps/desktop/src-tauri/src/lib.rs run, bootstrap_api
    severity: low
  - summary: >-
      A desktop whose station.key is corrupt or unreadable logs the error, and its window opens after 15 s over a dead API.
    evidence: |-
      build_station fails (by decision, the key is never replaced); bootstrap_api only logs it and wait_for_ready
      shows the window anyway. Making start-up failure visible is story 3 (CAP-4).
    location: >-
      apps/desktop/src-tauri/src/lib.rs run, bootstrap_api
    severity: medium
---

<intent-contract>

## Intent

**Problem:** The Tauri desktop shell builds its station with `Persistence::InMemory` and no key path, so a desktop user loses their bench layout and licence lease state (watermark, erasure counts) on quit. The shell also gets a new certificate signing key on every launch, so certificates from one launch name a key that the next launch no longer has. Story 1 already attaches the mock simulator (CAP-3); nothing yet proves it from the desktop's own config.

**Approach:** The desktop resolves Tauri's per-user app-data directory (`app.path().app_local_data_dir()`, identifier `dev.wipestation.desktop`; the machine-local app-data dir, so nothing roams) and builds its station with the shared bootstrap in `Persistence::Detect` mode. Saved configuration (`bay-topology.json`), lease state (`lease-state.json`) and the signing seed (`station.key`) all go in that directory. The bootstrap gains a way to point detection at a directory without treating the file as a `--bay-topology` seed.

## Boundaries & Constraints

**Always:**
- `Persistence::Detect` gains `config_dir: Option<PathBuf>`. When it is set and `bay_topology` is not, the store's path is `<config_dir>/bay-topology.json`. It goes through the normal ADR-0003 tier walk and write probe, and the lease file sits beside it (ADR-0003 amendment). It is never loaded as a seed, so an unreadable saved file is handled as it is on the CLI default path today: logged, and start-up is not failed. The CLI passes `None`, so its flags, env vars, defaults and start-up lines are unchanged.
- The desktop's posture is decided by one I/O helper and one pure mapping:
  - The app-data dir resolves, is created (`create_dir_all`) and passes `LocalFileStore::probe`: `Detect { control_plane_url: None, ephemeral_config: false, config_dir: Some(dir) }` and `key_path: Some(dir/station.key)`.
  - The dir resolves but cannot be created or written: the same `Detect` (the store falls to tier 3 and the UI warns that configuration cannot be saved and asks the operator to acknowledge with "Continue without saving", as ADR-0003 intends) and `key_path: None` (a fresh key for this run). The reason is logged at warn.
  - The dir does not resolve: `Persistence::InMemory` with a reason naming the error, and `key_path: None`.
- A signing seed that the bootstrap creates is written crash-safely and never overwrites: a temp file beside it (owner-only, mode `0o600` on Unix) is written and synced, then hard-linked into place (failing if the key now exists) and removed; if another process won that race (`AlreadyExists`), the winner's key is read instead. Read and write I/O errors name the path. This applies to both hosts, because the desktop now writes a key by default. It resolves story 1's deferred finding.
- The desktop's fleet, hostname, timing, random fleet station id, port and window URL are unchanged.

**Never:**
- No change to the port, window URL or busy-port behaviour (story 3). No new CLI flags. No vendor roots and no way to install a licence on the desktop. No persisted desktop station id. No catalog overlay.
- The desktop does not honour `WIPESTATION_CONFIG_DIR` or share `~/.config/wipestation` with the CLI.
- Do not change the store or lease modules' tier logic, and do not change `AppState` builders.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| First launch | empty app-data dir | topology tier LocalFile at `<dir>/bay-topology.json`, lease tier LocalFile, `station.key` created (0600 on Unix) | — |
| Relaunch | dir holds a saved layout and a key | saved layout loaded; same public key id as the first launch | — |
| Unwritable dir | dir exists, write probe fails | topology Ephemeral awaiting operator decision, lease RAM-only, fresh key | warn logged; station still starts |
| No app-data dir | `app_local_data_dir()` errors | InMemory posture; its reason names the error and says the signing key is fresh each launch; fresh key | station still starts |
| Corrupt saved layout | `bay-topology.json` not valid JSON | station starts unconfigured, error logged (CLI default-path behaviour) | not fatal |
| Corrupt key | `station.key` exists but is unreadable or not 32 bytes | start-up fails with an error naming the path; the file is left untouched | not fatal to the process; story 3 makes start-up failure visible in the window |
| CLI | any `serve` flags | identical config except `config_dir: None` | unchanged |

## Autonomous decisions

- **What "licence persists" means on the desktop.** This is about whether a desktop user's licensing state survives a restart. The problem: CAP-2 says the licence persists, but the desktop has no licence file and no vendor roots. A licence installed at runtime lasts only until restart on every host (licence-visibility story 4). The options were (a) persist the lease state (watermark, licensed and evaluation counts) and the signing key, which a licence binds to; or (b) also add desktop vendor roots and keep an installed licence on disk. I chose (a). (b) is a licensing and product decision and part of station-enrollment's licence delivery, so it is deferred. The CONTEXT.md limitation stays: the desktop is always unlicensed.
- **App-data dir, not the CLI's config dir.** This is about where the desktop keeps its files. The SPEC says per-user app-data directory, and this is the Tauri convention. Sharing `~/.config/wipestation` would let a CLI station and the desktop overwrite each other's layout and lease file.
- **Unwritable dir falls to the tier-3 prompt rather than failing start-up.** This follows the never-block-a-wipe invariant and ADR-0003's operator prompt.
- **Machine-local, not roaming, app-data dir** (review loop 1). This is about where the desktop's signing key and lease file live on Windows. The problem: Tauri's `app_data_dir()` is the Roaming profile on Windows (`%APPDATA%`). On a domain with roaming profiles, `station.key` and `lease-state.json` would follow a user to other PCs, so several stations would sign with one key and share one watermark. The options were `app_data_dir()` (Roaming) or `app_local_data_dir()` (`%LOCALAPPDATA%`; on macOS and Linux it is the same directory as `app_data_dir()`). I chose `app_local_data_dir()`. A signing key must not leave its machine (NIST SP 800-57), and a licence binds to the station.
- **A corrupt `station.key` fails start-up rather than being replaced** (review loop 1). This is about what the desktop does when its saved signing key cannot be read. The options were (a) fail start-up with the path named, as the CLI does today (story 1's matrix: non-32-byte file → error); or (b) sign with a fresh key for the run. I chose (a). Silently signing with a different key breaks continuity with earlier certificates and any licence bound to the key. The file is never touched, so it can be recovered, and the crash-safe write removes the usual way it gets corrupted. Making a failed start-up visible in the window is story 3 (CAP-4).
- **Owner-only key file.** This was a medium deferred finding from story 1. It is fixed now because this story makes a written key the desktop default, and owner-only permissions are the safe default (NIST SP 800-57 key protection).

</intent-contract>

## Code Map

- `crates/wipe-server/src/bootstrap.rs:43-55` -- the `Persistence` enum: add `config_dir`. `:189-205` -- the `Detect` arm builds `StoreConfig { explicit_path: bay_topology_path, .. }`; make it `bay_topology_path.or(config_dir.map(|d| d.join("bay-topology.json")))`. `load_bay_topology` (seed) is still fed only `bay_topology`.
- `crates/wipe-server/src/bootstrap.rs:446-466` -- `load_or_create_signing_key`: add 0600 + `create_new` on Unix (`std::os::unix::fs::OpenOptionsExt`), and `with_context` naming the path on read and write.
- `crates/wipe-server/src/store.rs:122` -- `LocalFileStore::probe`: reuse it for the desktop's writability check. `:329` `default_config_path`, `:629` `lease_state_path`: these already derive from `explicit_path`, so no change.
- `crates/wipe-cli/src/main.rs` -- `station_config` mapping: add `config_dir: None`. Its `try_parse_from` unit test asserts it.
- `apps/desktop/src-tauri/src/lib.rs:38-46` -- the setup closure: also take `app.path().app_local_data_dir()` (map the error to `String`) and pass it into `bootstrap_api`. `:77-97` `station_config(addr, resource_dist)` gains the data-dir posture. Add a `prepare_data_dir(Result<PathBuf,String>) -> DataDir` helper (enum `Writable(PathBuf)` / `Unwritable(PathBuf, String)` / `Unavailable(String)`) that does the create and probe. Log its outcome. `:149-168`: update the existing unit test.
- `crates/wipe-server/tests/bootstrap.rs:36-47,84-107` -- `desktop_config()` and the in-memory test mirror the old posture. Keep an `InMemory` test (still the no-dir fallback), and repoint `desktop_config` at a temp `config_dir`.
- Docs to update: `README.md:254`; `CONTEXT.md:917`, `:919` (lease "and in the desktop shell"), `:1040-1045`, `:1402-1406` (key "always fresh in the desktop app"), `:1418`, `:1445-1449`; `docs/design/bench-setup-builder.md` (desktop as-built note); `docs/adr/0005-licensing-and-attestation.md:249` and `:322` as-built sentences; the ADR-0003 amendment directory list (`:160`) adds "or the desktop's app-data dir"; `docs/ARCHITECTURE.md:72` if it describes the desktop posture.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-server/src/bootstrap.rs` -- add `config_dir` to `Persistence::Detect` and route it into `StoreConfig.explicit_path` only. Harden `load_or_create_signing_key` (0600, `create_new`, path in errors). Add unit tests:
  - a created key file has mode 0600 (`#[cfg(unix)]`);
  - no `*.tmp*` file is left beside it;
  - an existing key is never overwritten;
  - a pre-existing key at create time (the `AlreadyExists` race, simulated by calling the create helper on a path that already holds a valid seed) is read rather than failing;
  - read errors name the path (a directory at `station.key`);
  - write errors name the path.
- `crates/wipe-cli/src/main.rs` -- `config_dir: None` in the mapping and its test, so the CLI is unchanged.
- `apps/desktop/src-tauri/src/lib.rs` -- add `prepare_data_dir` and a `station_config(addr, resource_dist, &DataDir)` that returns the posture above, wired from `app_local_data_dir()`. Unit tests: each `DataDir` variant maps to the expected persistence and key path. A test builds a station from `station_config` twice against a temp dir, with the fleet overridden to `Off`. The first build saves a layout through `AppState::save_bay_topology`. The second build has the same layout and the same `signing_key.public_key_id()`, LocalFile topology and lease tiers, and `station.key` present. Drop the first station before building the second, and check that the reloaded lease keeps the first build's evaluation count. A third test uses a temp file path as the "dir" so `prepare_data_dir` yields `Unwritable`. A fourth (`#[cfg(unix)]`, skipped when running as root) uses an existing `0o500` directory so the probe, not the create, fails. Both unwritable tests build a station: Ephemeral topology awaiting a decision, RAM-only lease, and no `station.key` written. A test serves a station built from the desktop's own `station_config` (fleet `Off`) on `127.0.0.1:0` and checks that `GET /api/sim/devices` answers 200, using the crate's `reqwest_get` helper or a dev-dependency (CAP-3 on the desktop's real config). The `Unavailable` reason and both warn logs say the signing key is fresh for that run. Neither says the UI "asks where to keep configuration": the tier-3 banner only offers "Continue without saving".
- `crates/wipe-server/tests/bootstrap.rs` -- make `desktop_config()` a `Detect` + `config_dir` temp dir. Add `config_dir` persistence across two builds. Add that a corrupt `bay-topology.json` under `config_dir` does not fail the build. Keep the `/api/sim/devices` 200 test on the new desktop config (CAP-3). Keep the in-memory test as the fallback posture.
- Docs listed in the Code Map -- the desktop now keeps layout, lease state and signing key in its per-user app-data directory (with paths per OS). It stays unlicensed (no vendor roots), keeps the random fleet id per launch, and story 3's port issue is unchanged. The README also says that `station.key` is the station's certificate identity and should be backed up: deleting or resetting the app-data dir changes the key. Use `app_local_data_dir()` paths (macOS `~/Library/Application Support/dev.wipestation.desktop`, Linux `~/.local/share/dev.wipestation.desktop`, Windows `%LOCALAPPDATA%\dev.wipestation.desktop`). Say a corrupt `station.key` stops the desktop starting.

**Acceptance Criteria:**
- Given the desktop's own `station_config` for a writable temp app-data dir, when a station is built, a layout saved, and a station built again, then the second station shows the saved layout and the same public key id.
- Given the desktop's config, when the built station is served and `GET /api/sim/devices` is requested, then it answers 200.
- Given `cargo test --workspace`, when run, then all tests pass. Given `wipestation serve --help`, then it is identical to the baseline.

## Spec Change Log

### 2026-11-09 — review loop 1
- **Triggering findings:**
  - Tauri's `app_data_dir()` is Roaming on Windows, so the key and lease file would roam (Blind, Edge).
  - A corrupt `station.key` makes the desktop fail to start, and the spec never decided this (Blind, Edge).
  - Patch findings were folded in as well: crash-safe key creation and the `AlreadyExists` race, tier-3 wording, the key named in the `Unavailable` reason, a README backup note, tests for key read and write errors, a `0o500` unwritable-dir test, relaunch tests that drop the first station and compare lease contents, and CAP-3 on the desktop's real `station_config`.
- **Amended:**
  - Intent contract, under decide-by-default: the Approach now uses `app_local_data_dir()`; the key write rule; the tier-3 wording; matrix rows "No app-data dir" and "Corrupt key"; two autonomous decisions.
  - Code Map and Tasks: the same API swap and the extra tests and docs.
- **Known-bad state avoided:**
  - A signing seed and lease watermark synced across machines by Windows roaming profiles.
  - A half-written key that bricks every later launch.
  - Docs promising a choice of location that the UI does not offer.
- **KEEP:**
  - Start from `_bmad-output/implementation-artifacts/desktop-parity-story-2-loop0.patch` (`git apply` it). It was reviewed and the following were sound:
    - `Persistence::Detect.config_dir` routed only into `StoreConfig.explicit_path`;
    - the `DataDir` enum plus `prepare_data_dir`, `log_data_dir` and `station_config(addr, resource_dist, &DataDir)`;
    - `config_dir: None` in the CLI and its test;
    - `desktop_config(dir)` and `in_memory_config()` in `crates/wipe-server/tests/bootstrap.rs`;
    - the corrupt-layout test;
    - the doc edits.
  - Keep the test `an_unwritable_data_dir_starts_a_station_that_asks_the_operator`, which is in the patch.
  - Then apply only the amendments.

## Review Triage Log

### 2026-11-09 — Review pass
- verdicts: 27 findings — high 0, medium 5, low 21, false 1, maybe-false 0
- findings:
  - `[low]` `[patch]` Blind: README, the warn log and the docs say the UI "asks where to keep configuration", but the tier-3 banner (`BenchSetupPage.tsx:571-595`) only offers "Continue without saving" — confirmed; the wording is folded into the loop-1 amendment.
  - `[medium]` `[bad_spec]` Blind: `app_data_dir()` is Roaming `%APPDATA%` on Windows, so `station.key` and `lease-state.json` roam across PCs — confirmed (Tauri's data_dir is FOLDERID_RoamingAppData); the intent contract now says `app_local_data_dir()` (autonomous decision).
  - `[medium]` `[bad_spec]` Blind: a corrupt `station.key` stops the desktop starting, and the window shows over a dead port — confirmed (`build_station` returns Err, `bootstrap_api` only logs); the spec now decides on fail-with-path, story 3 makes it visible, and a matrix row is added.
  - `[low]` `[patch]` Blind: key creation is not crash-safe (create, then write, then sync) — confirmed in `write_new_secret`; the amendment now requires temp file, sync, hard_link.
  - `[low]` `[patch]` Blind: race between `exists()` and `create_new` fails with AlreadyExists — confirmed; the amendment reads the winner's key.
  - `[low]` `[reject]` Blind: two desktop instances share the app-data dir — real, but a second instance's bind fails before any job runs, so no erasure is counted; busy-port handling is story 3; a single-instance lock adds a dependency for a rare case.
  - `[low]` `[reject]` Blind: a read-only dir that already holds a key gets a fresh key anyway — real, but read-only per-user app-data dirs are rare and the fix adds a branch.
  - `[false]` `[reject]` Blind: the `Unavailable` → InMemory fallback is silent — the Bench setup page shows "Configuration is not saved." for any non-local tier (`BenchSetupPage.tsx:603`), so the operator is told.
  - `[low]` `[defer]` Blind: a corrupt saved layout is only logged and the next save overwrites it without a backup — pre-existing `LocalFileStore` behaviour on the CLI default path (audit 2026-11-05 ADR-0003 note); now reachable on the desktop.
  - `[low]` `[defer]` Blind: keys created before this change keep mode 0644, with no warning — pre-existing files; tightening or warning adds a branch.
  - `[low]` `[patch]` Blind: no test of an existing directory whose write probe fails (0o500) — confirmed; a test is added in the amendment.
  - `[low]` `[patch]` Blind: relaunch tests keep the first station alive and never compare lease contents — confirmed; the amendment drops it and compares the evaluation count.
  - `[low]` `[patch]` Blind: docs never say to back up `station.key` — confirmed; a README note is added in the amendment.
  - `[low]` `[patch]` Blind: the `Unavailable` reason omits the per-launch signing key — confirmed; the reason now says so.
  - `[low]` `[patch]` Edge: partial `station.key` after a crash — duplicate of the Blind crash-safety row.
  - `[medium]` `[bad_spec]` Edge: an unreadable or wrong-length `station.key` stops the desktop starting — duplicate of the Blind corrupt-key row.
  - `[low]` `[patch]` Edge: the `exists()`/`create_new` race — duplicate of the Blind race row.
  - `[low]` `[defer]` Edge: old 0644 keys are not reported — duplicate of the Blind 0644 row.
  - `[medium]` `[bad_spec]` Edge: Windows Roaming `app_data_dir` — duplicate of the Blind roaming row.
  - `[low]` `[patch]` Verification gap: the key read-error path is never shown to name the file — pre-verified; a test with a directory at `station.key` is added in the amendment.
  - `[low]` `[defer]` Verification gap: the `run()`/`bootstrap_api` wiring of `app_local_data_dir()` is untested — pre-verified; needs a Tauri runtime harness the repo lacks.
  - `[low]` `[reject]` Intent alignment: relaunch is proven in-process with `AppState`, not by a real relaunch or over HTTP — `AppState.bay_topology` is exactly what the topology route returns, and a real relaunch needs a Tauri harness.
  - `[low]` `[reject]` Intent alignment: the key id is compared on `AppState`, not on a signed certificate — the certificate generator signs with that same `signing_key`.
  - `[low]` `[patch]` Intent alignment: lease persistence is shown only by the file's existence — duplicate of the Blind relaunch-test row.
  - `[medium]` `[patch]` Intent alignment: the CAP-3 200 check runs on a hand copy, not the desktop's `station_config` — confirmed; the amendment serves the desktop's real config in the desktop crate.
  - `[low]` `[patch]` Intent alignment: the unwritable case is tested only via a file standing where the dir should be — duplicate of the Blind 0o500 row.
  - `[low]` `[reject]` Intent alignment: the `serve --help` comparison is a manual verification step, not a test — it was run (identical); the same rejection as story 1.

### 2026-11-09 — Review pass (loop 1)
- verdicts: 27 findings — high 0, medium 3, low 22, false 2, maybe-false 0
- findings:
  - `[low]` `[reject]` Blind: a crash leaves a `station.key.tmp-<pid>-<seq>` file that a later run never sweeps — real, but the window is microseconds after the write and the file is 0600; a sibling sweep adds a directory scan for a rare case.
  - `[medium]` `[patch]` Blind: `hard_link` fails on filesystems without hard links (FAT/exFAT, some SMB/NFS), so key creation fails where `fs::write` worked — confirmed (a CLI `--key-path` regression too); an exclusive 0600 `create_new` fallback is added (`create_exclusive`) with a unit test.
  - `[low]` `[patch]` Blind: docs say a corrupt key "stops the desktop starting", but the window opens after 15 s over a dead API — confirmed (`bootstrap_api` only logs); README, ARCHITECTURE and CONTEXT are reworded, and a deferred item is added for story 3.
  - `[low]` `[patch]` Blind: `p.exists()` is false on EACCES, so an unreadable key takes the create path with a misleading "writing" error — confirmed; now uses `symlink_metadata` (NotFound → create, other errors → "reading signing key"), with a Unix test.
  - `[low]` `[patch]` Blind: the "no station.key written" assertion in the file-as-dir test cannot fail — confirmed; replaced by asserting that two builds have different key ids.
  - `[low]` `[patch]` Blind: the hand-copied `in_memory_config` has drifted, and no test builds from the desktop's own Unavailable config — confirmed; desktop-crate test `an_unavailable_data_dir_starts_an_in_memory_station` added.
  - `[low]` `[reject]` Blind: the wipe-server relaunch test does not compare lease contents and could leak the env var on panic — the desktop-crate relaunch test compares the evaluation count on the real config, and the leak only happens on a test that has already failed.
  - `[low]` `[patch]` Blind: the ADR-0003 amendment reads as one fallback chain across hosts — confirmed; reworded per host.
  - `[low]` `[patch]` Blind: the CONTEXT caveat "in-memory store accepts saves without the tier-3 prompt" was dropped but still applies to the Unavailable fallback — confirmed; restored in CONTEXT, bench-setup-builder, README and ARCHITECTURE.
  - `[low]` `[patch]` Blind: Windows key protection is never stated — `0o600` is Unix-only; the docs now say the key relies on the `%LOCALAPPDATA%` profile ACL.
  - `[low]` `[patch]` Blind: the docs don't say an existing `station.key` is ignored when the dir is unwritable — the behaviour itself is a carried reject from pass 1; the docs now say it.
  - `[low]` `[reject]` Blind: desktop tests leave seeds and lease files in temp — the same pattern story 1 rejected; the test seeds sign nothing real.
  - `[medium]` `[patch]` Edge: hard-link unsupported — duplicate of the Blind hard-link row.
  - `[low]` `[reject]` Edge: stale temp files are only cleaned for the same pid — duplicate of the Blind temp-file row.
  - `[low]` `[patch]` Edge: `exists()` false on EACCES — duplicate of the Blind `exists()` row.
  - `[low]` `[patch]` Edge: vacuous no-key assertion — duplicate of the Blind row.
  - `[low]` `[reject]` Edge: `config_dir` is silently ignored when `bay_topology` is also set — no host sets both (the desktop never sets `bay_topology`), the precedence is documented on the field, and a guard is speculative.
  - `[low]` `[patch]` Edge: the README's corrupt-key claim — duplicate of the Blind docs row.
  - `[low]` `[defer]` Verification gap: `run()`/`bootstrap_api` wiring untested — carried: same as the pass-1 row, already in `deferred`; not deferred again.
  - `[medium]` `[patch]` Verification gap (other): hard-link unsupported — duplicate of the Blind hard-link row.
  - `[low]` `[defer]` Intent alignment: the real Tauri wiring is untested — carried: duplicate of the run() row.
  - `[low]` `[reject]` Intent alignment: relaunch is simulated in-process — carried: same as the pass-1 reject.
  - `[low]` `[reject]` Intent alignment: "licence persists" is tested only as lease state — the recorded autonomous decision (a); installing a licence on the desktop is deferred to station-enrollment.
  - `[low]` `[reject]` Intent alignment: CAP-3 is served via `axum::serve(router(state))` rather than `bootstrap_api` — `wipe_server::serve` serves the same `router(state)`; only the bind differs.
  - `[low]` `[patch]` Intent alignment: two desktop-config test surfaces (the wipe-server hand copy) — the same root cause as the Blind drift row; covered by the desktop-crate Unavailable test, and the real-config relaunch and simulator tests already exist.
  - `[false]` `[reject]` Intent alignment: the diff changes surfaces beyond the intent (bootstrap API field, key-file behaviour on both hosts) — the spec requires both (Boundaries); the flags and env vars are unchanged (help identical), and the only behavioural regression is the hard-link one, patched.
  - `[false]` `[reject]` Intent alignment: docs edited beyond the story's code surface — AGENTS.md requires docs in the same commit; no bad outcome.

## Design Notes

Why `config_dir` and not `bay_topology: Some(dir/bay-topology.json)`: `bay_topology` is also the seed, and `load_bay_topology` fails start-up on a bad file. A corrupt saved layout would then stop the desktop app opening at all, while the CLI's default path only logs it.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass (do not pipe through grep).
- `cargo build -p wipestation-desktop` -- expected: builds.
- `cargo run -p wipe-cli --bin wipestation -- serve --help` -- expected: identical to the baseline.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: pass.
- `rustfmt --edition 2021 <touched .rs files>` -- expected: no diff afterwards.

## Auto Run Result

Status: done

**Summary.** The desktop app now keeps its saved bench layout (`bay-topology.json`), lease state (`lease-state.json`) and signing key (`station.key`) in its machine-local per-user app-data directory (`app_local_data_dir()`, identifier `dev.wipestation.desktop`), so all three survive a relaunch (CAP-2). The shared bootstrap gained `Persistence::Detect.config_dir`, a store location that is never loaded as a seed. CAP-3 (`GET /api/sim/devices` returns 200) is proven on the desktop's own `station_config`.

The desktop has three postures:
- **Writable directory:** files are kept there.
- **Unwritable directory:** the ADR-0003 tier-3 banner appears, lease state is RAM-only, and the key is fresh for the run.
- **No directory:** everything is in memory, and the reason says so.

A signing seed is now created crash-safely, owner-only on Unix and never overwritten (temp file, sync, hard link, with an exclusive-create fallback where hard links are unsupported). The `AlreadyExists` race reads the winner's key. Errors name the path, which closes story 1's deferred finding. The CLI is unchanged externally (`config_dir: None`; `serve --help` is identical). The desktop stays unlicensed (no vendor roots).

**Files changed**
- `crates/wipe-server/src/bootstrap.rs`: `config_dir`, hardened key read and create, and unit tests.
- `crates/wipe-server/tests/bootstrap.rs`: desktop-style `config_dir` persistence, corrupt-layout and simulator tests, and the in-memory fallback.
- `crates/wipe-cli/src/main.rs`: `config_dir: None`, and the mapping test asserts it.
- `apps/desktop/src-tauri/src/lib.rs`: `DataDir`, `prepare_data_dir`, `log_data_dir`, `station_config(addr, resource_dist, &DataDir)`, the `app_local_data_dir()` wiring, and 11 tests (each posture, read-only dir, relaunch keeping layout, key and evaluation count, the simulator on the real config, the Unavailable build).
- `apps/desktop/src-tauri/Cargo.toml` and `Cargo.lock`: test-only dependencies (`wipe-common`, `time`, `axum`).
- `README.md`, `CONTEXT.md`, `docs/ARCHITECTURE.md`, `docs/design/bench-setup-builder.md`, the ADR-0003 amendment and the ADR-0005 as-built notes: the desktop's persistence, per-OS paths, key backup, corrupt-key behaviour, and the Windows ACL note.
- `_bmad-output/implementation-artifacts/desktop-parity-story-2-loop0.patch`: the loop-0 attempt kept for the loopback.

**Review.**
- **Pass 1** (27 findings): two medium spec-level findings went to a bad_spec loopback.
  - The Windows Roaming `app_data_dir()` was replaced with `app_local_data_dir()`.
  - The corrupt-key behaviour was decided: start-up fails and the file is left untouched.
  - The patch findings were folded into the amended spec.
  - 3 items were deferred and 7 rejected, each with its reason in the Review Triage Log.
- **Pass 2** (27 findings): 9 patch entries were applied (1 medium: the hard-link fallback; 8 low), 1 new item was deferred (a corrupt key leaves the window over a dead API, which story 3 owns), 2 rows were carried from pass 1, and 15 were rejected or duplicates, each with its reason in the Review Triage Log.
- **Patched counts this pass:** high 0, medium 1, low 8.
- **Follow-up review recommended: false.** No high entry was patched, and only one medium was.

**Verification.**
- `cargo test --workspace --no-fail-fast`: 908 passed, 1 failed. The failure is `wipe-hub` `health::silent_smtp_and_database_time_out_within_bound` (9.9 s against its time bound). It also fails on the untouched baseline (story 1 and this run's implementer confirmed it at 87347ce), and wipe-hub does not depend on any changed crate.
- `cargo build -p wipestation-desktop`: OK.
- `serve --help`: identical to the baseline.
- `pnpm typecheck` and `pnpm test` (291/291): OK.
- `rustfmt --check` on the touched files: clean.
- Every row of the edge-case table is covered by a test that ran.

**Residual risks.**
- The real Tauri `run()` path and the per-OS `app_local_data_dir()` locations were not exercised. The app was not launched, because the repo has no Tauri test harness (deferred).
- A corrupt `station.key` leaves the desktop window over a dead API until story 3 (deferred).
- Keys created before this change keep their old file mode (deferred).
- On Windows the key relies on the profile ACL, not an explicit mode.
