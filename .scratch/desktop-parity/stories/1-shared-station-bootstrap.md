---
title: 'Shared station bootstrap used by CLI and Tauri'
type: 'refactor'
created: '2026-11-09'
status: 'done'
baseline_revision: 'beb43d59f7e0f7c790fc086b4bf1dc4fc2652f2e'
review_loop_iteration: 0
followup_review_recommended: false
context: []
warnings: [oversized]
deferred:
  - summary: >-
      A newly generated signing seed is written with default file permissions and read/write errors do not name the path.
    evidence: |-
      load_or_create_signing_key (moved verbatim from the CLI) uses fs::write, so under umask 022 the
      32-byte Ed25519 seed is world-readable; fs::read/fs::write errors carry no path context.
      Pre-existing before this story.
    location: >-
      crates/wipe-server/src/bootstrap.rs load_or_create_signing_key
    severity: medium
---

<intent-contract>

## Intent

**Problem:** Station start-up is written twice. `wipestation serve` (`crates/wipe-cli/src/main.rs` `cmd_serve`) wires signing key, bay topology seed, topology/lease store detection, licence, vendor roots, concurrence policy, simulator and frontend bundle; the Tauri shell (`apps/desktop/src-tauri/src/lib.rs` `bootstrap_api`) calls only `AppState::with_static_dir`, and frontend-bundle resolution exists in both with different candidate lists. Anything added to one host silently misses the other.

**Approach:** Move the CLI's serve wiring and frontend-bundle resolution into a new `wipe_server::bootstrap` module exposing one `build_station(config, report) -> anyhow::Result<AppState>` plus `resolve_frontend_dist`. The CLI maps its flags into the config and serves the result; the Tauri shell builds a config for its current posture (nothing written to disk) and serves the same function's result.

## Boundaries & Constraints

**Always:**
- `wipestation serve`'s flags, env vars, defaults, start-up lines (same text, same order) and failure cases are unchanged; the CLI only parses flags, builds the config, prints report lines with `println!`, then calls `wipe_server::serve`.
- One frontend-bundle resolver for both hosts: explicit dir wins unchanged (even if missing — existing "API only" behaviour); else host-supplied candidates first (Tauri passes `resource_dir()/dist`); else the union of today's lists: `<cwd>/apps/desktop/dist`, `<cwd>/dist`, `<exe-dir>/static`, `<exe-dir>/../apps/desktop/dist` up to four levels up. First dir with `index.html` wins, canonicalized.
- The bootstrap always attaches the mock backend as the simulator (`with_simulator`), for both hosts.
- A saved topology still outranks a `--bay-topology`/`--bay-profile` seed; `--ephemeral-config` still persists lease state.
- Dependency direction holds: `wipe-server` gains no dependency on `wipe-cli` or Tauri.

**Never:**
- No app-data directory, persistent key or licence for the desktop (story 2), no port/URL change (story 3), no new CLI flags, no catalog-overlay wiring.
- Do not change `AppState` builder semantics or the store/licensing modules' behaviour.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| CLI default | no flags | same AppState wiring and start-up lines as before | — |
| Key file | `key_path` to a missing file, then built again | file created; second build has the same public key id | non-32-byte file → error, as today |
| Desktop posture | `Persistence::InMemory`, no key path, no licence | topology tier Ephemeral, lease RAM-only, no file written, simulator attached | — |
| Saved layout vs seed | store already holds a layout and `bay_profile` given | saved layout kept | — |
| Bad licence | `license` set, no vendor roots | error "--license needs --vendor-root-b64…" | `build_station` returns Err |
| Fleet | `Fleet::BestEffort` and mDNS start fails | station built, fleet `None`, warning reported | `Fleet::Required` returns Err (CLI) |

## Autonomous decisions

- **Simulator in the desktop now.** About: identify mode in the desktop app. Problem: CAP-1 lists the simulator among what the shared bootstrap wires, while story 2 says "enable the simulator". Options: (a) a `simulator` toggle kept off for the desktop until story 2; (b) always attach it. Choice: (b) — the mock backend is the only backend (ROADMAP §0.1), attaching it is what the CLI does, and a toggle only preserves a known-dead feature; story 2 then just verifies it.
- **Desktop keeps writing nothing to disk in this story.** About: where desktop configuration lives. Options: CLI default detection (`~/.config/wipestation`, shared with the CLI) now, or in-RAM until story 2 picks the app-data dir. Choice: in-RAM — avoids a layout/lease file in a location story 2 then abandons.
- **Fleet failure.** CLI keeps failing hard when mDNS cannot start; desktop keeps best-effort. Both behaviours exist today; the enum preserves them.

</intent-contract>

## Code Map

- `crates/wipe-cli/src/main.rs:420-547` -- `cmd_serve`: the wiring to move. Helpers moving with it: `load_bay_topology` (256), `resolve_profile` (328, also used by `cmd_bay_presets` at 363 — make it `pub` and call it from there), `detect_frontend_dist`/`has_index_html` (549-571), `load_or_create_signing_key` (827), `serve_stores` (858), `lease_store_line`, `lease_counts_line`, `apply_licensing`, `runtime_install_line`, `build_license_context` (878-976). `decode_keys` (573) stays in the CLI (also used by verify-cert/license); the config takes decoded `Vec<VerifyingKey>`.
- `crates/wipe-cli/src/main.rs:1130-1380` -- tests for the moved helpers (`serve_without_a_licence…`, `ephemeral_config_keeps…`, lease-line tests, read-only dir test): move into the bootstrap module's `#[cfg(test)]` with the helpers. `parse_feature`/verify-cert/concurrence-flag tests stay.
- `apps/desktop/src-tauri/src/lib.rs:83-150` -- `detect_static_dir` and `bootstrap_api`: replace with a config (`InMemory`, `Fleet::BestEffort`, hostname fallback `wipestation-desktop`, default timing, random station id) passed to `build_station`, logging report lines via `tracing::info!`. `api_url_cmd`, `wait_for_ready` untouched.
- `crates/wipe-server/src/lib.rs` -- add `pub mod bootstrap;`.
- `crates/wipe-server/src/store.rs:319,344,642` -- `StoreConfig`, `detect_store`, `detect_lease_store`, `EphemeralStore`, `EphemeralLeaseStore`: reuse as-is.
- `crates/wipe-server/src/app.rs:85,128-245,591` -- `AppState` builders and `serve`: reuse as-is.
- `crates/wipe-server/tests/http_e2e.rs:13` -- `spawn_server` pattern for an HTTP-level test.
- Docs: `AGENTS.md:109-110` (resolution "implemented twice"), `README.md:254`, `CONTEXT.md:917,1040-1045,1446-1449`, `docs/design/bench-setup-builder.md:5-10` (simulator now attached in the desktop; store/key/licence still not).

## Tasks & Acceptance

**Execution:**
- `crates/wipe-server/src/bootstrap.rs` -- new module: `StationConfig` (addr, station_id, hostname, fleet: `Fleet{Off,Required,BestEffort}`, key_path, fast, static_dir, host_static_candidates, bay_topology, bay_profile, persistence: `Persistence{Detect{control_plane_url, ephemeral_config}, InMemory{reason}}`, license, vendor_roots, concurrence_policy), `build_station(cfg, report: &mut dyn FnMut(String)) -> anyhow::Result<AppState>`, `resolve_frontend_dist(explicit, host_candidates)`, and the moved helpers with their tests -- one wiring for both hosts.
- `crates/wipe-server/Cargo.toml` -- add only what the moved code needs (e.g. `serde_json` already present) -- compile.
- `crates/wipe-cli/src/main.rs` -- `cmd_serve` becomes flag→config mapping + `build_station(…, &mut |l| println!("{l}"))` + "API listening" line + `serve`; delete the moved helpers -- CLI unchanged externally.
- `apps/desktop/src-tauri/src/lib.rs` -- call `build_station` and `serve`; drop `detect_static_dir`; pass the resource dir as a host candidate -- desktop uses the shared path.
- `crates/wipe-server/tests/bootstrap.rs` -- integration tests: key persistence across two builds (same key id); an `InMemory` config reports Ephemeral topology and lease tiers and creates no file in a temp dir it is pointed away from; served station answers `GET /api/sim/devices` 200; saved layout outranks a profile seed; `resolve_frontend_dist` precedence (explicit, host candidate, none) -- proves CAP-1.
- Docs listed in Code Map -- state that both hosts share `wipe_server::bootstrap`, and the desktop now has identify mode but still no saved store, persistent key or licence.

**Acceptance Criteria:**
- Given the workspace, when grepping both hosts, then `build_station` is called from `crates/wipe-cli/src/main.rs` and `apps/desktop/src-tauri/src/lib.rs`, and no other frontend-bundle resolver exists.
- Given `cargo test --workspace`, when run, then all tests pass, including the moved helper tests and `crates/wipe-server/tests/bootstrap.rs`.
- Given a station built with the desktop's config and served, when `GET /api/sim/devices` is requested, then it answers 200.
- Given `wipestation serve --help`, when compared with before, then flags and env vars are identical.

## Spec Change Log

## Review Triage Log

### 2026-11-09 — Review pass
- verdicts: 28 findings — high 0, medium 3, low 16, false 1, maybe-false 1 (7 duplicates graded with their originals: 4 low, 3 medium)
- findings:
  - `[low]` `[patch]` Blind: bench-setup-builder.md "As built" note still says the Tauri shell wires no store via `with_static_dir` — confirmed at lines 235-237; reworded to the `InMemory` posture.
  - `[low]` `[patch]` Blind: README implies the desktop honours `--static-dir`/`$WIPESTATION_STATIC_DIR` — confirmed (desktop leaves `static_dir: None`); entry marked `wipestation serve` only.
  - `[low]` `[reject]` Blind: desktop "mDNS disabled"/"no frontend" now logged at info, not warn — real, but the default filter is info so they still show; a level-aware sink adds API surface for a rare `RUST_LOG=warn` setup.
  - `[low]` `[reject]` Blind: shared bootstrap's start-up lines name CLI flags (`--vendor-root-b64`) in desktop logs — log-only wording developers rarely meet; host-neutral wording needs a parameter.
  - `[low]` `[patch]` Blind: desktop lease store's reason says "configuration" though lease state is not configuration (ADR-0005 §3a) — confirmed; desktop reason string now covers configuration and licence lease state.
  - `[low]` `[reject]` Blind: a bad `--vendor-root-b64` now fails before the earlier start-up lines and before `--key-path` is created — real, and it follows from the spec's decoded-roots config; same error text and exit, fail-fast before writing a key file is no worse; moving decode into wipe-server adds a base64 dependency and changes the config type.
  - `[low]` `[patch]` Blind: dependencies made unused by the change remain declared (desktop wipe-common/cert/engine-mock/fleet/time; CLI wipe-fleet) — confirmed by grep; deleted.
  - `[low]` `[reject]` Blind: desktop bundle search order changed (cwd before exe-relative, extra `dist`/`static`) — real but specified by the spec's union list; the resource dir stays first, so a packaged app is unaffected.
  - `[false]` `[reject]` Blind: the in-memory desktop test cannot fail — it asserts Ephemeral topology and lease tiers, which fail if the config used `Detect`.
  - `[medium]` `[patch]` Blind: no test covers the CLI flag→config mapping — same defect as the verification-gap finding below; mapping extracted and unit-tested via `Cli::try_parse_from`.
  - `[low]` `[reject]` Blind: three ACs checked only by manual commands — they were run (help identical, grep of call sites, desktop build); automating a grep or `--help` diff adds test complexity for little gain.
  - `[low]` `[reject]` Blind: `first_frontend_dist`/`default_frontend_candidates` are `pub` — the harm (a host re-assembling its own order) is speculative; restricting them means reworking the integration test.
  - `[low]` `[reject]` Blind: test temp dirs (including a throwaway test seed) are left behind — same pattern as the existing lease tests; the seed signs nothing real.
  - `[low]` `[reject]` Blind: story file's logs/Code Map look stale while in review — the fix edits this build's spec; logs are filled at triage by design.
  - `[low]` `[reject]` Edge: vendor roots decoded before other start-up work (claim) — duplicate of the Blind vendor-root row.
  - `[low]` `[reject]` Edge: "failure cases unchanged" claim contradicted by the earlier decode — duplicate of the same row.
  - `[low]` `[reject]` Edge: warn→info on the desktop — duplicate of the Blind warn/info row.
  - `[low]` `[patch]` Edge: the desktop-config tests start real mDNS on the LAN — confirmed (`Fleet::BestEffort`); test helper now uses `Fleet::Off`, and BestEffort is covered by the `start_fleet` unit test.
  - `[maybe-false]` `[reject]` Edge: `set_var` in two tests races env reads in tests that do not hold the lock — would need a test that reads env (e.g. a reqwest proxy lookup) concurrently with `set_var` to show it; if true it is only a rare test flake (low).
  - `[medium]` `[defer]` Edge: a new signing seed is written with default permissions (world-readable under umask 022), and I/O errors don't name the path — pre-existing in the moved `load_or_create_signing_key`; deferred.
  - `[low]` `[patch]` Edge: README says the desktop honours `WIPESTATION_STATIC_DIR` — duplicate of the Blind README row; same fix.
  - `[medium]` `[patch]` Verification gap: `cmd_serve`'s ServeArgs→StationConfig mapping is never run by a test — pre-verified; extracted into a private fn with a `try_parse_from` unit test.
  - `[medium]` `[patch]` Verification gap: the desktop's real StationConfig is untested (the integration test uses a hand copy) — pre-verified; extracted a private `station_config(addr, resource_dist)` in the desktop crate with a unit test (no new wipe-server API).
  - `[medium]` `[patch]` Verification gap: the default bundle candidate list is never asserted — pre-verified; unit test now asserts `default_frontend_candidates()` order.
  - `[low]` `[reject]` Verification gap (other): warn→info — duplicate of the Blind warn/info row.
  - `[low]` `[reject]` Verification gap (other): earlier vendor-root failure — duplicate of the Blind vendor-root row.
  - `[low]` `[reject]` Intent alignment: diverges from a pure-refactor reading (simulator, bundle order) — the simulator is a recorded autonomous decision and the order is spec-defined; docs match.
  - `[medium]` `[patch]` Intent alignment: tests exercise `build_station` with hand-built configs, not the hosts' own configs — same root cause as the two host-config gaps; covered by those patches.

## Design Notes

`report` is a sink rather than a returned `Vec` so lines print as they happen (a later failure still shows the earlier lines, as today). `Persistence::InMemory` uses `EphemeralStore::new(reason, false)` and `EphemeralLeaseStore::new(reason)` — exactly today's desktop posture — so story 2 only swaps the desktop to `Detect` with an app-data path.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass (do not pipe through grep).
- `cargo build -p wipestation-desktop` -- expected: builds.
- `cargo run -p wipe-cli --bin wipestation -- serve --help` -- expected: same flags as `git show HEAD:crates/wipe-cli/src/main.rs` declares.
- `rustfmt --edition 2021 <touched .rs files>` -- expected: no diff afterwards.

## Auto Run Result

Status: done

**Summary.** Both hosts now build their station through `wipe_server::bootstrap::build_station`. It wires the signing key, bay-topology seed, topology and lease stores, licence, vendor roots, concurrence policy, the always-attached mock simulator, mDNS fleet and frontend bundle. `resolve_frontend_dist` is the only frontend-bundle resolver. `wipestation serve` only maps its flags into a `StationConfig`. The Tauri shell builds an in-memory config, so the desktop still writes nothing to disk until story 2, and it now has identify mode.

**Files changed**
- `crates/wipe-server/src/bootstrap.rs`: new shared bootstrap module; the helpers and tests moved from the CLI; `start_fleet`; a test for the order of the default bundle locations.
- `crates/wipe-server/src/lib.rs`: exports `bootstrap`.
- `crates/wipe-server/tests/bootstrap.rs`: integration tests for key persistence, the in-memory desktop setup, `/api/sim/devices` 200, a saved layout outranking a seed, a licence without vendor roots, bundle precedence, and the CLI start-up line order.
- `crates/wipe-cli/src/main.rs`: `cmd_serve` is now `serve_args` → `station_config` → `build_station`; a flag-mapping unit test.
- `crates/wipe-cli/Cargo.toml`, `apps/desktop/src-tauri/Cargo.toml`: removed dependencies this change left unused.
- `apps/desktop/src-tauri/src/lib.rs`: `station_config(addr, resource_dist)` plus `build_station`; `detect_static_dir` removed; a unit test.
- `apps/desktop/src/api/client.ts`, `apps/desktop/src/pages/BenchSetupPage.tsx`: corrected comments that said the desktop has no simulator.
- `AGENTS.md`, `README.md`, `CONTEXT.md`, `docs/ARCHITECTURE.md`, `docs/design/bench-setup-builder.md`: both hosts share the bootstrap; the desktop has identify mode but still no saved store, persistent key or licence.

**Review.** 28 findings: 8 patch entries applied (3 medium, 5 low), 1 deferred (the signing-seed file permissions, which predate this story), 19 rejected, each with the reason recorded in the Review Triage Log. Follow-up review recommended: false. No high entry was patched, and the 3 medium patches only add tests (CLI mapping, desktop config, default bundle order). Patched counts: high 0, medium 3, low 5.

**Verification.**
- `cargo test --workspace --no-fail-fast`: everything passes except `wipe-hub --test health::silent_smtp_and_database_time_out_within_bound`. That test also fails on the untouched baseline beb43d5 (it took 6–9 s against its time bound on this machine) and is unrelated: `wipe-hub` does not depend on `wipe-server`.
- `cargo build -p wipestation-desktop`: OK.
- `serve --help`: identical to the baseline.
- `pnpm typecheck` and `pnpm test` (291/291): OK.
- clippy: no new warnings.

**Residual risks.**
- A malformed `--vendor-root-b64` now fails before the other start-up lines print and before `--key-path` is created. The error text is the same.
- The desktop's "mDNS disabled" and "no frontend" lines are now info level.
- The desktop's bundle search order now checks the working directory before paths relative to the executable. The resource directory is still first.

