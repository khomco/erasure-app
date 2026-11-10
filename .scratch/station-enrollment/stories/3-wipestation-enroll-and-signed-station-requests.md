---
title: 'wipestation enroll and signed station requests'
type: 'feature'
created: '2026-11-10'
status: 'done'
baseline_revision: '522eefc17a1f33171344a3d52b9e497e4cc8893d'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '{project-root}/docs/adr/0008-station-enrollment-and-licence-fetch.md'
  - '{project-root}/.scratch/station-enrollment/SPEC.md'
  - '{project-root}/.scratch/station-enrollment/stories/1-hub-enrollment-start-and-poll.md'
warnings: [oversized]
deferred: []
---

<intent-contract>

## Intent

**Problem:** The hub can enroll a station by device authorization (stories 1–2), but no station can ask: there is no station-side hub client, no `wipestation enroll`, nowhere to keep the hub address, and no shared signer/verifier for the challenge-signed station requests of ADR-0008 §2 (CAP-3).

**Approach:** Add `wipe_license::station_request` (the ADR-0008 §2 message, signer and verifier, used by the station now and the hub in story 5), a station-side hub client in `wipe-server` (origin rule, proxy, start/poll loop, licence verification against the station's own roots), a small station-settings file in the config store for the hub address, and `wipestation enroll`, which prints the code, polls, verifies the licence, saves it, and installs it into a running `serve` through `POST /api/license`. `scripts/commerce-demo.sh` enrolls a second station this way.

## Boundaries & Constraints

**Always:**
- `wipe_license::station_request`: message = `wipestation-station-request-v1\n` + challenge + `\n` + hub origin + `\n` + upper-case method + `\n` + path (with query, as sent) + `\n` + lowercase hex SHA-256 of the body bytes. `sign(key, challenge, origin, method, path, body) -> SignedStationRequest {instance_key_id, challenge, signature_b64}`; `verify(&VerifyingKey, &SignedStationRequest, origin, method, path, body)` refuses a key id that is not the key's, and a signature that does not `verify_strict` (unpadded standard base64, as `enrollment.rs`). Header-name consts `x-wipestation-key-id`, `x-wipestation-challenge`, `x-wipestation-signature`. Unit tests: round trip, and each of challenge / origin / method / path / body / key changed fails.
- Hub origin (`wipe_server::hub_client::HubOrigin::parse`): an absolute URL, `https`, or `http` only on `localhost`, `*.localhost`, `127.0.0.0/8` or `::1` (the hub's `auth::is_local_host` rule, reimplemented: no station crate may depend on `wipe-hub`); no userinfo, query or fragment, path empty or `/`; stored and used as `scheme://host[:port]` without a trailing slash. Any other value is refused with a message stating the rule.
- The hub client uses `reqwest` (already in the workspace) with connect timeout 10 s and request timeout 30 s, and an explicit proxy from `ProxySettings` (`HTTPS_PROXY`/`https_proxy`, `NO_PROXY`/`no_proxy`, read by `ProxySettings::from_env()`): `https` requests go through the proxy unless `NO_PROXY` matches; no other proxy source is used. Start sends `wipe_license::enrollment::sign_start`, poll `sign_poll`; it honours `interval` (and `slow_down`'s new `interval`), retries a poll after a network error or 5xx at the current interval, and stops at `expires_in` with an expired outcome.
- On `{station, licence}` the client parses `licence` with `wipe_license::parse_license` and runs `check_installable(licence, station key id, roots)` against only the roots the operator gave; failure is refused, nothing saved or installed, with a message that the hub sent a licence this station does not trust (or that names another key) and that the station trusts only its configured `--vendor-root-b64`.
- `wipestation enroll [--hub <origin>] --key-path <file> --vendor-root-b64 <b64>... [--station-id <id>] [--label <text>] [--station <url>] [--license-out <file>]`: `--key-path` and at least one root are required (refused before contacting the hub; the key file must already exist, never created here). `--station-id` defaults to `standalone` (serve's licensing default; help text says it must match serve's `--station-id`, since a machine licence is bound to it). `--hub` falls back to the saved hub address; neither → refused. Prints a stable block containing the lines `user code: XXXX-XXXX` and `open: <verification_uri_complete>` (and `address: <verification_uri>`), polls, and on a verified licence: writes it to `--license-out` (default `<config dir>/enrolled-licence.json`), saves the hub origin to station settings, then `POST`s the licence JSON to `<--station, default http://127.0.0.1:7878>/api/license`. 200 → prints the station's licence status message and exits 0. Connection refused/unreachable → prints that no station answered, the saved path and the `serve --license <path> --vendor-root-b64 …` command, exits 0. Any other station answer → prints its `error` and exits non-zero. Denied, expired, untrusted licence, hub refusal at start (incl. 429) or unreachable hub at start → message and non-zero exit.
- Station settings: `station-settings.json` `{"hub_origin": "<origin>"}` in the config directory the lease store uses (`store::lease_state_path`'s parent: `--bay-topology`'s directory, else `$WIPESTATION_CONFIG_DIR`, else `~/.config/wipestation`), written atomically like `LocalFileStore::save`; unknown fields ignored on read. `enroll` takes the same directory from `WIPESTATION_CONFIG_DIR`/default. A directory it cannot write: the hub address is not saved and `enroll` says so (licence handling continues). `wipestation serve` reports one start-up line `hub: <origin> (from <path>)` or `hub: none — run wipestation enroll --hub <origin> to register this station`; an unreadable/invalid file is reported in that line and never fails start-up.
- `scripts/commerce-demo.sh`: the demo offering grants 2 seats; after the existing licensed-certificate check, a second station (own key, own port `STATION2_PORT` default 7891, own config dir, `--vendor-root-b64`, no `--license`) is started, `wipestation enroll` runs against it in the background, the customer (owner) approves the printed code with `POST /api/enrollments/approve` through `hub_post`, `enroll` exits 0, station 2's `GET /api/license` is valid, and a job on station 2 yields a certificate that passes `verify-cert --require-licensed` with the hub's licence id. No licence file is copied by the script.
- Docs in the same commit: `docs/generated/cli/` goldens regenerated; README (CLI usage, commerce demo description, test counts), CONTEXT §10.2 row for this story (and story 1/2 rows' "no station sends these yet"), §14 **Enrollment**.

**Never:**
- No hub route, migration or hub code change (the challenge route and hub-side use of `station_request` are story 5); no station HTTP route or desktop UI (story 4); no periodic licence fetch (story 5).
- No portal credential on the station; no root from the hub or the network; never trust a licence the configured roots do not verify.
- `serve` does not auto-load `enrolled-licence.json` at start-up and the runtime install stays memory-only (licence-visibility story 4's deferred item); `--license` and `POST /api/license` behave unchanged.
- Nothing gates, delays or interrupts an erasure; no hardware-sanitization claim.
- `wipe-license` gains no network or async dependency.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected | Error |
|---|---|---|---|
| Enroll, serve running | owner approves | licence verified, saved, installed via `/api/license`; status valid; hub saved; exit 0 | — |
| Enroll, no serve | approved | licence saved; message names path and `serve --license` command; exit 0 | — |
| http non-loopback hub | `--hub http://hub.example` | refused before any request | message states rule |
| No root / no key file | | refused before any request | message |
| Untrusted licence | hub licence signed by another root | nothing saved/installed; exit ≠ 0 | "does not trust" message |
| Licence for another key | | refused | names both keys |
| Denied / expired | poll `access_denied` / `expired_token` or past `expires_in` | exit ≠ 0 | says denied / expired, start again |
| slow_down | | next poll waits the new interval | — |
| Hub unreachable at start | | exit ≠ 0 | names the hub |
| Poll network error | transient | retried at interval | — |
| Station refuses install | 409/422 | exit ≠ 0 | station's `error` text |
| Proxy | `HTTPS_PROXY` set, https hub | `CONNECT host:443` sent to proxy; with `NO_PROXY` matching, not | — |

## Autonomous decisions

- **The §2 signer has no hub caller yet.** *Problem:* ADR-0008 §2 signs every post-enrollment request over a hub challenge, but the challenge route is CAP-5. *Options:* build the shared module now, unit-tested on both sides; or defer it. *Choice:* build it in `wipe-license` now (the crate both depend on, per the stories.yaml entry) and wire the hub in story 5. *Why:* no new network surface; enrollment start/poll already share `wipe_license::enrollment` (story 1's decision).
- **Where the client lives.** *Options:* `wipe-license` (pure today), `wipe-cli`, or `wipe-server`. *Choice:* `wipe-server::hub_client`, because story 4's station routes and story 5's periodic fetch run inside the server; the CLI calls it. *Why:* one implementation for all three, and `wipe-license` stays offline-pure.
- **What a CLI enroll does without a running `serve`.** *Problem:* the runtime install exists only in a running station and is memory-only. *Options:* require `serve` running; or always save the verified licence to a file and install into `serve` when one answers. *Choice:* the latter. *Why:* the safe default: a paid, approved licence is never lost, and the offline `--license` path already exists; auto-loading the saved file at start-up needs a precedence rule against `--license` and is deferred (licence-visibility story 4's deferred item).
- **Hub address store.** *Problem:* the ADR-0003 store holds only the bay topology document. *Options:* add a field to `BayTopology` (a mirrored `wipe-common` type), or a separate settings file beside it. *Choice:* `station-settings.json` beside `lease-state.json`, local-file tier or not saved. *Why:* configuration stays out of licence state and out of the topology document; tier 2 (control plane) is unimplemented and cannot hold the address used to reach it.
- **Proxy handling.** *Choice:* read `HTTPS_PROXY`/`NO_PROXY` ourselves and configure reqwest explicitly. *Why:* ADR-0008 §5 names those two; explicit settings make the behaviour testable without mutating process environment.
- **Station id default `standalone`.** *Why:* that is the id `serve` licenses under when none is given (`bootstrap::build_station`); any other default yields a wrong-machine licence.

</intent-contract>

## Code Map

- `crates/wipe-license/src/enrollment.rs` -- story 1 start/poll: `sign_start`, `sign_poll`, `StartRequest`, `PollRequest`, B64 = `STANDARD_NO_PAD`, `verify_strict`, `EnrollmentSignatureError` style; template for `station_request.rs`. `lib.rs` module list + doc.
- `crates/wipe-license/src/install.rs` -- `parse_license`, `check_installable`, `InstallProblem` (operator wording to reuse).
- `crates/wipe-hub/src/enrollment_routes.rs:139-242` -- start response keys `{device_code, user_code, verification_uri, verification_uri_complete, expires_in, interval}`; poll 200 `{station, licence}`, 400 `{error}` codes, `slow_down` carries `interval`; 429 with `Retry-After`. Read-only.
- `crates/wipe-hub/src/auth.rs:216` `is_local_host` -- loopback rule to mirror.
- `crates/wipe-server/Cargo.toml` -- `reqwest` is dev-only; move to `[dependencies]` (workspace `reqwest = 0.12`, json; native-tls default). Add `url`, `sha2`/`hex` only if needed.
- `crates/wipe-server/src/lib.rs` -- add `pub mod hub_client;`.
- `crates/wipe-server/src/store.rs:309` `default_config_path`, `lease_state_path`/`detect_lease_store` (~`:620-680`), `LocalFileStore::save` atomic write, `LocalFileStore::probe` -- settings file location and write.
- `crates/wipe-server/src/bootstrap.rs:126-260` `build_station` -- add the `hub:` start-up line near `lease store:` (Persistence::Detect only; InMemory reports none); tests module pattern (`lease_tmpdir`, `store_cfg_in`).
- `crates/wipe-server/src/app.rs:194` `install_license`, handler 409/422 bodies `{"error"}` -- what `enroll` receives from `POST /api/license`.
- `crates/wipe-cli/src/main.rs:31-139` `Cmd` enum, `main` dispatch, `decode_keys` (`:373`), `load_seed` -- add `Enroll`; read key with the 32-byte seed rule (existing file only); module doc list.
- `crates/wipe-cli/tests/help_golden.rs` -- regenerate `docs/generated/cli/` (`UPDATE_GOLDENS=1`).
- `crates/wipe-server/tests/http_e2e.rs:2266+` -- `license_chain` (own root) for building licences in tests.
- `scripts/commerce-demo.sh:233` `start_station` (hard-codes port/key/config dir; generalise or add a second), `:378` offering `seats: 1`, `:472` registration, `:505-582` job + cert checks to reuse for station 2, `cleanup` (`:119`) must stop station 2.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-license/src/station_request.rs` (+ `lib.rs`) -- message, sign, verify, header consts; unit tests listed in Always.
- `crates/wipe-server/Cargo.toml` -- reqwest to dependencies.
- `crates/wipe-server/src/hub_client.rs` -- `HubOrigin`, `ProxySettings`, `HubClient` (`start`, `poll`), `enroll(...)` loop with injectable sleep/clock, typed `EnrollError` with operator messages, licence check; settings load/save helpers (or in `store.rs`).
- `crates/wipe-server/src/store.rs` -- `StationSettings`, `station_settings_path`, `load_station_settings`, `save_station_settings`.
- `crates/wipe-server/src/bootstrap.rs` -- `hub:` start-up line + unit tests (none, saved, invalid file).
- `crates/wipe-server/tests/hub_client.rs` -- fake hub (axum) that verifies signatures with `wipe_license::enrollment::verify_*`: approved after N polls, denied, expired, slow_down interval, transient 503 retried, untrusted root, wrong key, 429 at start, unreachable hub; origin rule cases; proxy CONNECT and NO_PROXY via a local TCP listener.
- `crates/wipe-cli/src/main.rs` -- `Enroll` subcommand per Always; install via `POST /api/license`.
- `crates/wipe-cli/tests/enroll.rs` -- spawn the binary: refuses http non-loopback, missing root, missing key file, before any request (no network needed).
- `docs/generated/cli/*` -- regenerated goldens.
- `scripts/commerce-demo.sh` -- second-station enrollment per Always.
- `README.md`, `CONTEXT.md` -- per Always.

**Acceptance Criteria:**
- Given Postgres and Mailpit up, when `scripts/commerce-demo.sh` runs, then it completes, with station 2 licensed through `wipestation enroll` and its certificate passing `verify-cert --require-licensed`.
- Given a fake hub returning a licence from an untrusted root, when `enroll` completes polling, then nothing is written to `--license-out`, no install request is sent, and the process exits non-zero with the "does not trust" message.
- Given a saved hub address, when `wipestation serve` starts, then its log contains `hub: <origin> (from <path>)`.
- Given `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, then all pass.

## Spec Change Log

## Review Triage Log

### 2026-11-10 — Review pass
- verdicts: 38 findings — high 0, medium 5, low 31, false 2, maybe-false 0
- findings:
  - Blind Hunter
    - `[low]` `[patch]` Installed arm advises `--license <path>` after the save failed — now prints the licence JSON and no `--license` advice when the save failed.
    - `[medium]` `[patch]` Refused arm loses the licence when the save also failed — now prints the licence JSON before the error.
    - `[low]` `[reject]` `enroll` writes settings/licence beside `$WIPESTATION_CONFIG_DIR`, while `serve --bay-topology <dir>` reads beside that file — the directory rule is the intent contract's; fixing it edits the spec (new flag). Residual risk below.
    - `[low]` `[reject]` Re-saving settings drops unknown fields / overwrites a corrupt file — one field exists today; keeping unknown fields adds a flatten map for a future version not yet written.
    - `[low]` `[reject]` `station_request::message` does not refuse `\n` inside fields — challenge is hub-issued and looked up by the hub, origin is the hub's own, and HTTP paths cannot carry raw newlines; no caller exists until story 5, which should add the guard if it accepts header input verbatim. Noted for story 5.
    - `[false]` `[reject]` Licence not checked for station id or validity window — `check_installable` is exactly the check `serve --license` and `POST /api/license` apply (licence-visibility story 4 decision 4: authentic expired/wrong-machine licences install and show their status); the station's status then says so.
    - `[low]` `[reject]` `enrolled-licence.json` written non-atomically and overwritten on re-enroll — the newer licence is the wanted one; a crash mid-write is negligible and the hub still holds the file (portal download).
    - `[low]` `[reject]` Response bodies read without a size cap — the only peers are the operator's https hub and local station; a cap adds code for a hostile-hub case the root check already contains.
    - `[low]` `[reject]` 429 on poll retried at the same interval — the hub does not throttle polls (only `slow_down`, which is honoured).
    - `[low]` `[reject]` Non-connect install failures print "HTTP 0" — wording only; a separate variant adds public surface for timeouts on a local station.
    - `[medium]` `[patch]` CLI-level tests missing for install refusal and untrusted licence — grouped with the verification-gap rows; tests added.
    - `[low]` `[patch]` README `enroll.rs` row count wrong — row now 8 tests, all described.
    - `[low]` `[patch]` README totals unmeasured — set from the measured workspace run (1027) plus the three tests added in this pass: 1030.
    - `[low]` `[reject]` Hub-supplied verification links printed unchecked — the hub is the TLS-authenticated origin the operator chose; the codes it prints are its own.
    - `[low]` `[reject]` `--station` accepts any http(s) URL — operator-supplied, documented as the running station; the licence is useless without the instance key.
  - Edge Case Hunter
    - `[medium]` `[patch]` Refused + failed save loses the licence — same as Blind's; same patch.
    - `[low]` `[patch]` Installed + failed save gives wrong advice — same as Blind's; same patch.
    - `[low]` `[reject]` "HTTP 0" for non-connect failures — same as Blind's.
    - `[false]` `[reject]` Approval in the last interval is never collected — the hub expires the enrollment at the same 600 s, counted from its own start (earlier than the client's), and story 1 answers `expired_token` for an approved enrollment past expiry, so a final poll would also end expired.
    - `[low]` `[reject]` Settings directory differs from `serve --bay-topology` — same as Blind's.
    - `[low]` `[reject]` Corrupt settings silently overwritten — same as Blind's.
    - `[low]` `[reject]` Unknown settings fields dropped on rewrite — same as Blind's.
    - `[low]` `[reject]` Non-atomic licence write — same as Blind's.
    - `[low]` `[reject]` Newline ambiguity in the signed message — same as Blind's.
    - `[low]` `[reject]` Upper-case `HTTP://` refused for `--station` — unusual input; message states the expected form.
    - `[low]` `[reject]` `Retry-After` in HTTP-date form gives no hint — the hub sends seconds (story 1).
    - `[low]` `[reject]` `expires_in: 0` prints "0 minutes" — the hub always sends 600.
    - `[low]` `[reject]` `InMemory` persistence reports `hub: none` though a file may exist — `InMemory` is a host's explicit no-disk choice; it reads nothing on disk by design.
    - `[low]` `[reject]` AC "serve logs the saved hub" fails under `--bay-topology` — holds for the directory the spec defines; same root as Blind's directory row.
  - Verification Gap
    - `[medium]` `[patch]` No binary test that a station refusing the install fails `enroll` — added `a_station_refusing_the_install_fails_with_its_error_and_the_saved_path`.
    - `[medium]` `[patch]` Untrusted-licence AC tested only in the library — added `an_untrusted_licence_is_neither_saved_nor_installed` (no file, no request to a witness station, non-zero exit, "does not trust").
    - `[low]` `[reject]` Settings directory mismatch — same as Blind's.
    - `[low]` `[patch]` `Enrolled::licence_json` doc claims the hub's bytes — doc now says it is re-serialized (same content, not byte-for-byte).
    - `[low]` `[reject]` "HTTP 0" — same as Blind's.
  - Intent Alignment
    - `[low]` `[reject]` §2 signer/verifier has no live caller — the intent contract's recorded autonomous decision; the challenge route and hub use are story 5.
    - `[low]` `[reject]` Without `serve`, the licence is saved, not runtime-installed — recorded autonomous decision; with `serve` running it goes through the runtime install (commerce demo proves it).
    - `[low]` `[reject]` Hub address in a settings file, not the `TopologyStore` abstraction — recorded autonomous decision; the control-plane tier cannot hold the address used to reach it.
    - `[low]` `[reject]` Rust tests use fake hubs; only the demo meets the real hub — the demo runs the binary against the real hub end to end; the fake hubs verify signatures with the hub's own verifier.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass (the known `health.rs` timing flake aside, if it recurs).
- `cargo clippy --workspace --all-targets -- -D warnings` -- expected: clean.
- `rustfmt --edition 2021 --check <touched .rs files>` -- clean.
- `scripts/commerce-demo.sh` -- expected: completes, "commerce demo complete".

## Auto Run Result

Status: done

**Summary.** The station side of enrollment (ADR-0008, CAP-3). `wipestation enroll --hub <origin> --key-path <key> --vendor-root-b64 <root>` signs a start with the station's instance key, prints the user code and activation link, polls the hub (honouring `interval` and `slow_down`, retrying transient errors, stopping at `expires_in`), checks the licence against only the given vendor roots and the station's key, saves it to `enrolled-licence.json`, saves the hub address to `station-settings.json`, and installs it into a running `serve` through `POST /api/license` (with no station running, it prints the `serve --license` command). The hub address must be https (http only on loopback); `HTTPS_PROXY`/`NO_PROXY` are honoured. `wipe_license::station_request` is the ADR-0008 §2 signer and verifier both sides will share (hub use in story 5). `serve` prints a `hub:` start-up line. The commerce demo enrolls a second station this way.

**Files changed**
- `crates/wipe-license/src/station_request.rs` (new), `enrollment.rs`, `lib.rs`: the §2 message, signer, verifier and header names.
- `crates/wipe-server/src/hub_client.rs` (new), `lib.rs`, `Cargo.toml`: hub origin rule, proxy settings, start/poll client and loop, licence check, install into a running station; `reqwest`/`url` now normal dependencies.
- `crates/wipe-server/src/store.rs`: `station-settings.json` load/save beside the lease state.
- `crates/wipe-server/src/bootstrap.rs`, `tests/bootstrap.rs`: the `hub:` start-up line.
- `crates/wipe-cli/src/main.rs`: `wipestation enroll`.
- `crates/wipe-server/tests/hub_client.rs` (17 tests), `crates/wipe-cli/tests/enroll.rs` (8 tests).
- `docs/generated/cli/wipestation{,-enroll}.txt`: help goldens.
- `scripts/commerce-demo.sh`: 2-seat offering, second station enrolled by `wipestation enroll`, approved by the owner, licensed certificate verified offline.
- `CONTEXT.md`, `README.md`: §10.2 rows, §14 **Enrollment**, CLI usage, demo description, test inventory.

**Review.** 38 findings: 0 high, 5 medium, 31 low, 2 false.
- **Patched (2 medium entries, 2 low):** a licence that could not be saved is now printed whenever it was not installed or the install was refused, and the wrong `--license` advice is gone; CLI tests added for a station refusing the install and for an untrusted licence; `Enrolled::licence_json` doc corrected; README counts corrected.
- **Deferred:** none.
- **Rejected:** every rejected finding and its reason is in the Review Triage Log: the intent contract's own decisions (no hub caller for §2 yet, saved-not-installed without `serve`, settings file location), low-likelihood inputs (newlines in signed fields, HTTP-date `Retry-After`, `expires_in: 0`, upper-case scheme), wording-only issues ("HTTP 0"), and two false claims.

**Follow-up review recommended: true.** Two medium entries were patched. The unverified risk is the new save-failed branches in `cmd_enroll` (Installed and Refused arms printing the licence JSON): no test makes `save_enrolled_licence` fail, so those branches have been read but never run.

**Verification**
- `cargo test --workspace --no-fail-fast` (before review patches): 1026 passed, 1 failed — `wipe-hub` `tests/health.rs::silent_smtp_and_database_time_out_within_bound`, the same pre-existing timing failure recorded by stories 1 and 2 on their baselines.
- After patches: `cargo test -p wipe-cli -p wipe-server -p wipe-license`: all passed (the patches touched only these crates).
- `cargo clippy -p wipe-license -p wipe-server -p wipe-cli --all-targets -- -D warnings`: one error, `clippy::large_enum_variant` in `crates/wipe-engine/src/runner.rs:47`, a file this story does not touch (pre-existing under the current toolchain); with that lint allowed, clean.
- `rustfmt --edition 2021 --check` on touched files: clean.
- `scripts/commerce-demo.sh`: completed; station 2 enrolled, licence installed (`Licensed - unlimited`), its certificate verified LICENSED offline.
- Matrix audit: every I/O row has a passing test (CLI tests for the no-serve, refusal and untrusted rows were added during this run).

**Residual risks**
- `enroll` saves the hub address and licence in `$WIPESTATION_CONFIG_DIR` (else `~/.config/wipestation`). A `serve` started with `--bay-topology` in another directory reads its settings there and reports `hub: none`. That matters once story 5 reads the address: aligning the two directories (for example an `enroll --config-dir`) is a later decision.
- An enrolled licence installed at runtime lasts until restart unless `serve` is given `--license <saved file>` (licence-visibility story 4's deferred item; `enroll` prints the hint).
- `station_request::verify` does not refuse newlines in fields; story 5 should refuse them if it takes the challenge from a header verbatim.
- `clippy -D warnings` across the workspace fails on a pre-existing `wipe-engine` lint.
