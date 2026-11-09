---
title: 'Desktop window follows its own server'
type: 'bugfix'
created: '2026-11-09'
status: 'done'
review_loop_iteration: 0
followup_review_recommended: true
context: []
warnings: [oversized]
deferred:
  - summary: >-
      Nothing stops two copies of the desktop app on different addresses from sharing one app-data directory.
    evidence: |-
      The port bind only stops a second copy started on the same address. With WIPESTATION_API_ADDR set to another port
      (or 127.0.0.1:0), a second copy prepares the same app_local_data_dir and builds a station on the same
      bay-topology.json, lease-state.json and station.key. That gives two writers to the ADR-0003 store and the lease
      watermark. This also happened before this story. The fix is an OS file lock on a lockfile in the app-data
      directory, taken before prepare_data_dir. That is a new mechanism, beyond CAP-4.
    location: >-
      apps/desktop/src-tauri/src/lib.rs desktop_station / prepare_data_dir
    severity: medium
baseline_revision: 'a27dadae5eeb6b0700edaf8ceecf0c8748682d32'
---

<intent-contract>

## Intent

**Problem:** The desktop shell creates its window from `tauri.conf.json` already pointed at `http://127.0.0.1:7878`, then starts its server separately. If that port is taken, or start-up fails (a corrupt `station.key`, story 2's deferred finding), the error is only logged and after a 15-second wait the window shows whatever answers on 7878, which may be another program's UI, or nothing. `WIPESTATION_API_ADDR` moves the server but not the window.

**Approach:** The shell binds its listener first, before it touches the app-data directory, and builds the station with the address it actually bound. The window is created at runtime and pointed at that bound address. Any start-up failure (address unusable, invalid `WIPESTATION_API_ADDR`, station build error) opens the window on a small error page instead. That page is served by the shell itself on a free loopback port and names the reason. The window never loads a URL this process did not bind.

## Boundaries & Constraints

**Always:**
- Bind, then build: the station is built with the bound `SocketAddr` (mDNS advertises its `api_port`), and a failed bind writes nothing to the app-data directory.
- The window URL is derived from the listener's `local_addr()`, never from config or env text.
- The error page HTML-escapes the reason, answers every path (including `/api/health`) with 503, `text/html; charset=utf-8` and `Cache-Control: no-store`, and never serves the React bundle.
- The station is still built through `wipe_server::bootstrap::build_station`. The CLI's flags, env vars and `serve --help` are unchanged.

**Never:**
- Fall back silently to another port when the configured one is busy.
- Add a Tauri plugin or a Tauri command the frontend calls. The frontend stays HTTP/WS only.
- Change the frontend, the CLI default address, or the store, key or licence behaviour from story 2.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Default | env unset, 7878 free | Station on 127.0.0.1:7878, window at `http://127.0.0.1:7878/` | No error expected |
| Port 0 | `WIPESTATION_API_ADDR=127.0.0.1:0` | Bound to a free port P; station built with P; window at `http://127.0.0.1:P/` | No error expected |
| Port busy | another process (or a second desktop) on 7878 | Window shows the error page naming `127.0.0.1:7878` and `WIPESTATION_API_ADDR` | Nothing is created or written in the app-data dir |
| Bad env | `WIPESTATION_API_ADDR=nonsense` | Error page naming the rejected value | No bind is attempted |
| Build fails | `station.key` is 5 bytes | Error page with the build error, naming the key path | The key file is left untouched |
| Error page unservable | the loopback bind for the page fails | Setup returns the error; Tauri exits non-zero with it | Logged |

## Autonomous decisions

- **Busy port: fail visibly, no fallback port.** *About:* an operator launches the desktop app while something already uses 127.0.0.1:7878. *Problem:* CAP-4 allows either failing visibly or picking a free port. The most likely occupant is a second copy of the desktop app, which shares the same app-data directory: two stations writing one layout file, one lease watermark and one signing key. *Options:* (a) show an error naming the port and how to move it; (b) silently take a free port. *Choice:* (a). It is the safe default: the bound port works as a single-instance guard for the single-writer store (ADR-0003), and `WIPESTATION_API_ADDR=127.0.0.1:0` still gives a free port on request.
- **How the failure is shown: a shell-served error page, not a native dialog.** *Options:* (a) add `tauri-plugin-dialog`; (b) open the window on an HTML page that the shell serves from a free loopback port. *Choice:* (b). It adds no dependency, behaves the same on every OS's webview, can be tested over HTTP, and keeps the "window only loads what this process bound" rule.
- **Remove `api_url_cmd`.** *About:* this Tauri command returned the address from the env text. No frontend code calls it, and AGENTS.md says the frontend never uses Tauri commands. After this story its answer would be wrong whenever port 0 is used. *Choice:* delete it rather than keep an unused command that misreports the address.

</intent-contract>

## Code Map

- `apps/desktop/src-tauri/src/lib.rs:23-75` -- `run()`: the env parse (`:32-35`, silently falls back on a bad value), the setup closure, the detached `bootstrap_api` spawn and the "showing window anyway" path. `:77-82` `api_url_cmd`: delete it, along with `.invoke_handler`. `:186-198` `bootstrap_api`: today it builds, then binds via `wipe_server::serve`. `:204-237` `wait_for_ready` / `reqwest_get`: keep them; tests may need a variant that returns the status line and body.
- `apps/desktop/src-tauri/src/lib.rs:106-182` -- `prepare_data_dir`, `log_data_dir`, `station_config(addr, …)`: reuse unchanged, but call them only after a successful bind, passing the bound address.
- `apps/desktop/src-tauri/src/lib.rs:396-404` -- the `build_desktop_station` test helper (fleet `Off`): the pattern for tests that must not advertise on mDNS.
- `apps/desktop/src-tauri/tauri.conf.json:9-23` -- the `main` window: add `"create": false` and remove `"url"` (keep title, size and `visible: false`).
- Tauri 2.11 facts (read from `~/.cargo/registry/src/*/tauri-2.11.1/src`): `app.rs:2516` creates config windows whose `create` is true *before* the user `setup` hook runs, so a config URL loads first. `WebviewWindowBuilder::from_config(handle, &cfg)` (`webview/webview_window.rs:150`) builds from a cloned `WindowConfig` whose `url` is set to `WebviewUrl::External(url)`. `tauri::async_runtime::block_on` is usable inside `setup`.
- `crates/wipe-server/src/app.rs:591-597` -- `serve(state, addr)` binds and serves. Add `pub async fn serve_on(state, listener: tokio::net::TcpListener)` and make `serve` call it (the CLI is unchanged). `bootstrap.rs:178` fleet `api_port: addr.port()`: the reason to build with the bound address.
- `apps/desktop/src-tauri/Cargo.toml` -- move `axum` from dev-dependencies to dependencies (workspace version; it serves the error page).
- Docs to update: `README.md:254` (window URL, `api_url_cmd`, the "port is taken" and "until story 3" sentences); `docs/ARCHITECTURE.md:73-74,327` (the same); `CONTEXT.md:1409-1412` (key error "until story 3"); `CONTEXT.md:1040-1048` §10 gaps if the desktop-parity epic note needs it.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-server/src/app.rs` -- add `serve_on`, and have `serve` delegate to it -- so the desktop can bind first.
- `apps/desktop/src-tauri/src/lib.rs` -- implement:
  - `api_addr_from_env(Option<String>) -> Result<SocketAddr, String>`.
  - `async fn start_station(addr, build: impl FnOnce(SocketAddr) -> anyhow::Result<AppState>) -> Result<(AppState, TcpListener), String>`. It binds first, then calls `build` with `local_addr()`. A bind error message names the address and says to quit the other program or set `WIPESTATION_API_ADDR` (`127.0.0.1:0` picks a free port). A build error is `{e:#}`.
  - `fn window_url(SocketAddr) -> url::Url`.
  - `startup_error_page(&str) -> String` (escapes `& < > " '`) and `async fn serve_startup_error(reason) -> std::io::Result<SocketAddr>`, which binds `127.0.0.1:0` with an axum fallback route.
  - The setup closure: `block_on` the start. On success, spawn `serve_on` and set the window URL to the bound address. On failure, `error!` log the reason and set the URL to the error page. Build `main` from the config with that URL. Wait for ready (success only) and show. If the error page cannot be served, return the error from setup.
  - Remove `api_url_cmd` and update the module doc.
  - Unit tests: every I/O matrix row except Default (env parse and `window_url` are covered as pure functions). The busy-port case occupies a listener and checks that the app-data temp dir was not created. The port-0 case serves and gets 200 from `/api/health` at `window_url(bound)` and checks the closure saw the bound port. The corrupt-key case checks the error names `station.key` and the bytes are unchanged. The error page case checks 503 on `/` and `/api/health`, the escaped reason, and no raw `<script>`. A test parses `tauri.conf.json` (`include_str!`) and asserts `main` has `create: false` and no `url`. Production and tests share one station closure (fleet overridable to `Off`).
- `apps/desktop/src-tauri/tauri.conf.json`, `Cargo.toml` -- as in the Code Map.
- Docs in the Code Map -- the window follows the bound address. A busy port, a bad `WIPESTATION_API_ADDR` or a failed start opens the window on an error page naming the reason. `127.0.0.1:0` picks a free port. A second copy of the app shows the busy-port error. Drop `api_url_cmd` and the "until story 3" sentences.

**Acceptance Criteria:**
- Given `WIPESTATION_API_ADDR=127.0.0.1:0`, when the desktop start runs, then the station is built with and served on the bound port P, and the window URL is `http://127.0.0.1:P/`, where `/api/health` answers 200.
- Given the desktop's `tauri.conf.json`, when Tauri starts, then it creates no window before setup (`create: false`, no hard-coded URL).
- Given a busy port or a corrupt `station.key`, when the desktop starts, then the URL the window is given serves a 503 page naming the reason, and nothing is written to the app-data directory on the busy-port path.
- Given `cargo test --workspace` and `cargo build -p wipestation-desktop`, when run, then they pass (apart from the known baseline `wipe-hub` health-timing failure), and `wipestation serve --help` is identical to the baseline.

## Spec Change Log

## Review Triage Log

### 2026-11-09 — Review pass
- verdicts: 22 findings — high 0, medium 4, low 12, false 6, maybe-false 0
- findings:
  - `[medium]` `patch` (Blind) The busy-port message suggests `127.0.0.1:0`, and the docs claim a second copy always finds the port busy. A second copy on another address shares the app-data directory. — Patched: the message now tells the operator to use the other copy's window if it is Wipestation. The docs now say only a second copy on the *same* address is stopped, and that there is no lock. The missing single-instance lock predates this story and is deferred.
  - `[low]` `patch` (Blind) `wait_for_ready` has no per-attempt timeout, so `read_to_end` can outlast the 15 s deadline. — Patched: each attempt is wrapped in `tokio::time::timeout` for the time left.
  - `[low]` `reject` (Blind) If `serve_on` fails after binding, the window shows a dead page. — `axum::serve` retries accept errors and practically never returns an error after a successful bind. Fixing it would need new navigate-on-failure logic, which is not worth it for a path nobody meets in practice.
  - `[low]` `reject` (Blind) The bind error blames a busy port even for EADDRNOTAVAIL or EACCES. — The message embeds the OS error and says "probably". A non-local or privileged `WIPESTATION_API_ADDR` is rare on a desktop, and the fix would add branching on `ErrorKind`.
  - `[medium]` `patch` (Blind) The setup window wiring has no test. — Grouped with the Verification-gap row below; patched there.
  - `[low]` `patch` (Blind) `http_get` sends a `Host` header without the port. — Patched: it sends `host:port` when the URL has a port.
  - `[false]` `reject` (Blind) An empty `WIPESTATION_API_ADDR` is refused, and the docs don't say so. — The docs say an unparsable value opens the error page naming it, and an empty string is unparsable. Refusing it, visibly and by name, is the matrix's "Bad env" row.
  - `[false]` `reject` (Blind) The error-page wording should be recorded as provisional copy. — The fix would edit this build's spec. The page is functional error text, not product or marketing copy.
  - `[low]` `patch` (Edge) `wait_for_ready` can hang past its deadline. — Duplicate of the Blind row; patched there.
  - `[low]` `reject` (Edge) `window_url` drops an IPv6 scope id. — Only a link-local `WIPESTATION_API_ADDR` with a zone would trigger this. Browsers do not accept zone ids in URLs anyway, and nobody points a desktop at that.
  - `[low]` `reject` (Edge) `local_addr()` failing after the station is built makes setup error out without a page. — `local_addr` on a listener that just bound does not fail in practice, and the fix would change signatures.
  - `[low]` `reject` (Edge) `serve_on` failing leaves the window on a dead port. — Duplicate of the Blind row; rejected for the same reason.
  - `[low]` `patch` (Edge) The bad-env test's `!dir.exists()` assertion is vacuous. — Patched: the assertion is removed. The `built` flag already proves nothing was built.
  - `[medium]` `patch` (Edge) The docs' claim that a second copy finds the port busy and shares nothing is too strong. — Grouped with the first Blind row; patched there.
  - `[low]` `patch` (Edge) On Windows, WebView2 may write EBWebView under `app_local_data_dir` even on the busy-port path. — Patched: the docs now say none of the station's files (layout, lease state, key) are created or touched.
  - `[medium]` `patch` (Verification-gap) Setup's `main` window lookup and URL substitution are untested. — Patched: extracted into a pure `main_window_config` helper with unit tests against the real `tauri.conf.json` windows.
  - `[low]` `patch` (Verification-gap) No test proves the bound address reaches `StationConfig.addr`, which mDNS advertises as `api_port`. — Patched: a `desktop_config` builder, plus a test that `addr` and `fleet` are passed through.
  - `[low]` `reject` (Intent) The setup closure is untested at the window surface. — Duplicate of the Verification-gap row. Only the Tauri runtime call itself is left untested: the repo has no Tauri test harness.
  - `[false]` `reject` (Intent) A wildcard occupant (`0.0.0.0:7878`) is not exercised. — On a platform where the loopback bind still succeeds, the window reaches this process's own listener, so CAP-4 holds. Where it fails, the error page shows.
  - `[false]` `reject` (Intent) The default path has no end-to-end test. — It differs from the tested port-0 path only in the address. `api_addr_from_env(None)` and `window_url(127.0.0.1:7878)` are both pinned.
  - `[false]` `reject` (Intent) The automatic free-port fallback (R2) is not implemented. — CAP-4 allows either branch. The intent contract records "fail visibly" as an autonomous decision.
  - `[false]` `reject` (Intent) On failure the window talks to a shell-served page, not the station. — The page is this process's own listener, so nothing attaches to another process. The intent contract records this choice.

## Design Notes

Why bind before `prepare_data_dir`: its write probe creates and deletes a file, and `build_station` opens the lease and key files. A second copy of the app must fail before it touches the first copy's files.

Window flow in setup (sketch):
```rust
let (url, ready) = match block_on(start_station(addr, build)) {
    Ok((state, l)) => { let u = window_url(l.local_addr()?); spawn(serve_on(state, l)); (u, true) }
    Err(reason) => { error!(%reason); (window_url(block_on(serve_startup_error(reason))?), false) }
};
let mut wc = app.config().app.windows[0].clone(); wc.url = WebviewUrl::External(url);
WebviewWindowBuilder::from_config(app.handle(), &wc)?.build()?;
```

## Verification

**Commands:**
- `cargo test -p wipestation-desktop` -- expected: all pass, including the new start-up tests.
- `cargo build -p wipestation-desktop` -- expected: OK.
- `cargo test --workspace --no-fail-fast` (after `docker compose up -d`) -- expected: all pass except the known baseline `wipe-hub` `health::silent_smtp_and_database_time_out_within_bound`.
- `cargo run -q -p wipe-cli -- serve --help` -- expected: identical to the output at HEAD a27dada.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.

**Manual checks (if no CLI):**
- The Tauri window itself is not exercised (there is no Tauri test harness). Inspect that the setup closure gives `from_config` only a URL built by `window_url`.

## Auto Run Result

Status: done

**Summary.** The desktop shell now binds its address first, before it touches any station file in the app-data directory. It builds the station with the address it actually bound, so mDNS advertises the real port, and serves it with the new `wipe_server::serve_on`. Only then does it create the `main` window (`create: false` in `tauri.conf.json`, no URL there), pointed at that bound address (CAP-4). Three failures open the window on a shell-served error page that names the reason:
- a busy port (no other port is tried);
- an unparsable `WIPESTATION_API_ADDR` (it used to fall back silently to the default);
- a failed station build, such as a corrupt `station.key` (story 2's deferred finding).

The page listens on a free loopback port and answers every path, `/api/health` included, with 503. If even that page cannot be served, Tauri exits non-zero. `WIPESTATION_API_ADDR=127.0.0.1:0` picks a free port. The unused `api_url_cmd` Tauri command is gone. The CLI is unchanged.

**Files changed**
- `apps/desktop/src-tauri/src/lib.rs`: the start-up path: parse the address from the env, bind and then build, the window URL, the start-up error page, and `main_window_config` / `desktop_config`. `wait_for_ready` now times out each attempt. `api_url_cmd` is removed. 12 new tests (23 in the crate).
- `apps/desktop/src-tauri/tauri.conf.json`: the `main` window has `create: false` and no hard-coded URL.
- `apps/desktop/src-tauri/Cargo.toml`: `axum` is now a normal dependency, used by the error page.
- `crates/wipe-server/src/app.rs`: new `serve_on(state, listener)`; `serve` binds and then calls it.
- `README.md`, `docs/ARCHITECTURE.md`, `CONTEXT.md`:
  - the window follows the bound address, and a failed start shows the error page;
  - the "until story 3" and `api_url_cmd` passages are gone;
  - copies of the app on different addresses are not prevented: there is no lock on the app-data directory.

**Review.** 22 findings (high 0, medium 4, low 12, false 6), all recorded in the Review Triage Log.
- **Patches applied** (8 entries: 2 medium, 6 low):
  - busy-port wording and doc claims (medium);
  - setup's window wiring extracted and tested (medium);
  - a per-attempt timeout in `wait_for_ready`;
  - `Host` header with the port;
  - a vacuous assertion removed;
  - Windows WebView2 doc wording;
  - a test that the bound address reaches `StationConfig`.
- **Deferred:** no single-instance lock on the app-data directory (medium; predates this story).
- **Rejected:**
  - 6 false: an empty env value is covered as unparsable; the copy-sign-off fix would edit the spec; a wildcard occupant still leaves CAP-4 holding; the default path is pinned by pure functions; no fallback port and the shell-served page are both recorded decisions.
  - 7 low, unlikely and not worth extra logic: `serve_on` failing after bind (twice); bind-error wording for EADDRNOTAVAIL or EACCES; IPv6 scope ids; `local_addr` failing; and a duplicate of the untested-setup finding.

**Follow-up review recommended: true.** Two medium entries were patched on this first pass. The specific risk not yet checked: the real Tauri `run()` setup path has never run. That path calls `block_on` inside `setup`, builds the window from the config with an external URL, and shows the error page at once. The repo has no Tauri test harness, so the logic is covered only through the pure helpers. A launch on macOS, Windows and Linux, with a free port, a busy port and a corrupt key, would settle it.

**Verification**
- `cargo test -p wipestation-desktop`: 23/23 passed.
- `cargo build -p wipestation-desktop`: OK.
- `cargo test --workspace --no-fail-fast` with compose up: everything passed except the known baseline `wipe-hub` `health::silent_smtp_and_database_time_out_within_bound` (9.7 s against its time bound). That test also fails at a27dada, and `wipe-hub` depends on none of the changed crates.
- `wipestation serve --help`: identical to a27dada.
- `rustfmt --edition 2021 --check` on the touched `.rs` files: clean.
- Every row of the I/O matrix is covered by a test that ran.

**Residual risks**
- The Tauri window itself was never launched (see the follow-up above).
- Two copies started on different addresses still share one app-data directory (deferred).
- On macOS and BSD, binding `127.0.0.1:7878` can succeed beside a `0.0.0.0:7878` listener, because tokio sets SO_REUSEADDR. The window then still reaches its own listener, so CAP-4 holds.
- `Fleet::BestEffort` in `run()` is still checked only by reading the code.
