---
title: 'Portal app shell served by the hub'
type: 'feature'
created: '2026-11-06'
status: 'done'
baseline_revision: '60cd9af8ca4db71fcbac8dae6b74a922ec3bb80a'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/docs/adr/0006-web-presence-and-local-first-verification.md', '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md']
warnings: ['oversized']
deferred:
  - summary: >-
      AGENTS.md's Portal bullet gives no vitest count or runtime, unlike the Frontend and cargo bullets.
    evidence: |-
      apps/portal has 6 vitest tests (src/api/health.test.ts) and the bullet added in this story omits the count. The fix edits AGENTS.md, an agent-context file, so it is deferred under the triage rules. The suggested router test was not taken: it needs main.tsx split, and no harm was named.
    location: >-
      AGENTS.md:42
    severity: low
---

<intent-contract>

## Intent

**Problem:** ADR-0006 §1 puts the portal at `apps/portal`, served by `wipe-hub` from the same origin as its API (CAP-5). Neither the package nor the hub's static serving exists, so later stories (accounts, orders, licences) have no UI to land in.

**Approach:** Add an empty `apps/portal` React + Vite package that follows `apps/desktop`'s conventions (TanStack Router/Query, Tailwind 3, Vitest), with one page that shows the hub's `/api/health`. Teach `wipe-hub serve` to serve a built portal directory with SPA fallback, while `/api/*` stays the API's and never falls through to `index.html`.

## Boundaries & Constraints

**Always:** New flag `--portal-dir <PATH>` with env twin `WIPE_HUB_PORTAL_DIR`, stored as `Args::portal_dir: Option<PathBuf>`. Resolution happens in `Config::from_args` into `Config::portal_dir: Option<PathBuf>`:
- If the flag is given and `<dir>/index.html` is not a file, return `ConfigError::PortalDirInvalid`. The hub refuses to start, because an explicit path that is wrong must never silently fall back.
- If the flag is absent, use `<cwd>/apps/portal/dist` when it holds `index.html`, else `None`.
- When the result is `None`, `startup_warnings` adds a warning that the portal is not served and names `cd apps/portal && pnpm build` and `--portal-dir`.

Routing:
- Every `/api` and `/api/*` path that matches no route returns 404 JSON `{"error":"not found"}`, whether or not a portal is configured.
- `/assets/*` is served from `<dir>/assets` with no fallback, so a missing asset is a 404 and never HTML.
- Any other path serves the file if it exists. Otherwise it serves `index.html` with status 200 (SPA fallback).
- With no portal, non-API paths return 404 plain text that names the same build command.

Use `tower-http` `ServeDir`/`ServeFile` through the workspace dependency, which already enables `fs`. The portal package is named `portal`, keeps its own `pnpm-lock.yaml` (root `.npmrc` sets `shared-workspace-lockfile=false`), uses dev port 5174 and proxies `/api` to `http://127.0.0.1:7900`. Copy is neutral: product name, "portal", hub status. Update AGENTS.md, README.md and CONTEXT.md §10.2 in the same commit.

**Never:** No sign-in, accounts, or any route beyond the index and a not-found view. No change to `wipe-server`, `wipe-cli` or `apps/desktop`. No new runtime npm dependencies beyond those `apps/desktop` already uses. No hardware-sanitization, certification or compliance claims in portal copy (ROADMAP §0.1). No cache-control or compression tuning. No embedding the bundle into the binary. Do not edit `pnpm-workspace.yaml`: it already lists `apps/*` and `sites/*`.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Index | portal dir with `index.html`; `GET /` | 200, the `index.html` body | — |
| Asset | `GET /assets/app.js` that exists | 200, the file body, JS content type | — |
| Missing asset | `GET /assets/nope.js` | 404, not `index.html` | — |
| Deep link | `GET /account/orders` | 200, the `index.html` body | — |
| API unaffected | `GET /api/health` with a portal configured | the health JSON, as before | — |
| Unknown API | `GET /api/nope`, `GET /api` | 404 JSON `{"error":"not found"}` | with or without a portal |
| No portal | `portal_dir: None`; `GET /` | 404 text naming `cd apps/portal && pnpm build` | startup warning also logged |
| Bad explicit dir | `--portal-dir /does/not/exist`, or a dir without `index.html` | `Config::from_args` → `Err(PortalDirInvalid)`; the message names the path and `pnpm build` | hub exits non-zero |
| Portal health view | `/api/health` returns 200 `ok` or 503 `degraded` | `fetchHealth` returns the parsed report in both cases | network error or non-JSON → throws |

</intent-contract>

## Code Map

- `crates/wipe-hub/src/config.rs:36` -- `Args`; add `portal_dir`. `:76` `Config`; add the resolved `portal_dir`. `:96` `ConfigError`; add `PortalDirInvalid { path }`. `:169` `startup_warnings`. `:236` is a literal `Args { … }` in the unit tests and needs the new field. Keep the hand-written `Debug` complete (the path is not a secret).
- `crates/wipe-hub/tests/health.rs:44` -- a literal `Args { … }` initializer that needs `portal_dir: None` (AGENTS.md pitfall). Grep `crates/wipe-hub` for `vendor_root_seed:` to find every one.
- `crates/wipe-hub/src/lib.rs:43` -- `router(state)`. Add the API 404 and the portal fallback here, so `serve_on` (used by tests) gets them too.
- `crates/wipe-server/src/app.rs:341` -- reference for `ServeDir::new(dir).fallback(ServeFile::new(index))`. Read-only. Its silent "API only" landing is the pitfall this story avoids.
- `Cargo.toml:56` -- workspace `tower-http` already has `fs`. Add `tower-http = { workspace = true }` to `crates/wipe-hub/Cargo.toml`. `tests/deps.rs` checks sibling crates only, so it is unaffected.
- `crates/wipe-hub/src/health.rs:46` -- `HealthReport { status: "ok"|"degraded", database, mail: "up"|"down" }`. The TS type mirrors it. A degraded report comes back as 503 with the same body.
- `apps/desktop/{package.json,vite.config.ts,tsconfig*.json,tailwind.config.js,postcss.config.js,index.html,src/main.tsx}` -- conventions to copy, with the same dependency versions. Vitest runs in the node env with `src/**/*.test.ts`.
- `.npmrc`, `pnpm-workspace.yaml` -- read-only. `.gitignore` already ignores `dist/`, `node_modules/` and `*.tsbuildinfo`.
- `AGENTS.md:24` (test count) and `:41` (Frontend bullet); `README.md:48` (hub row) and `:64` (quick start); `CONTEXT.md:692` (§10.2 hub row, which says "No portal serving yet (`hub-foundation` story 4)").

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/Cargo.toml` -- add `tower-http`.
- `crates/wipe-hub/src/config.rs` -- the flag, its resolution, `PortalDirInvalid`, and the warning. Unit tests: explicit valid dir, explicit missing dir, explicit dir without `index.html`, absent flag gives `None` or the cwd default. Avoid changing the process cwd in tests: factor the default lookup as `resolve_portal_dir(explicit, cwd)` and test that directly -- CAP-5/CAP-6.
- `crates/wipe-hub/src/lib.rs` -- routing as in Boundaries. Update the crate doc to say it serves the portal.
- `crates/wipe-hub/tests/portal.rs` -- build a tempdir with `index.html` and `assets/app.js`, run `serve_on` with a lazy pool on a closed port (no compose needed), and assert one test per serving row of the I/O matrix over real HTTP with `reqwest`.
- `crates/wipe-hub/tests/health.rs` -- add `portal_dir: None`.
- `apps/portal/` -- `package.json` (name `portal`; scripts `dev`, `build` = `tsc -b && vite build`, `preview`, `typecheck`, `test`), `vite.config.ts`, `tsconfig.json`/`tsconfig.app.json`/`tsconfig.node.json`, `tailwind.config.js`, `postcss.config.js`, `index.html` (title "Wipestation portal"), `src/index.css`, `src/main.tsx` (QueryClient, root route with header, index route, not-found component), `src/api/health.ts` (`HealthReport` type and `fetchHealth(fetchImpl = fetch)`), `src/api/health.test.ts` (200 ok, 503 degraded, non-JSON throws), `src/pages/HomePage.tsx` (shows hub status), and `pnpm-lock.yaml` from `pnpm install`.
- `AGENTS.md` -- a Portal bullet: `pnpm install` at the root, then `pnpm --filter portal build|typecheck|test`; the hub serves `apps/portal/dist` (or `--portal-dir`) and warns when it is unbuilt. Update the cargo test count to the measured number.
- `README.md` -- hub row mentions portal serving; the quick start gains the portal build line.
- `CONTEXT.md` -- §10.2 hub row: add portal serving (SPA fallback, `/api/*` 404s, `--portal-dir`). Remove "No portal serving yet" and note the portal is an empty shell.

**Acceptance Criteria:**
- Given a fresh `pnpm install` at the root, when `pnpm --filter portal build`, `pnpm --filter portal typecheck` and `pnpm --filter portal test` run, then all three succeed and `apps/portal/dist/index.html` exists.
- Given the portal is built and compose is up, when `cargo run -p wipe-hub -- serve` runs from the repo root, then `curl 127.0.0.1:7900/` and `curl 127.0.0.1:7900/some/deep/link` return the portal's `index.html`, and `curl 127.0.0.1:7900/api/health` returns the health JSON.
- Given compose is up, when `cargo test --workspace` runs, then every test passes, including `tests/portal.rs`.

## Spec Change Log

## Review Triage Log

### 2026-11-06 — Review pass
- verdicts: 23 findings — high 0, medium 0, low 19, false 4, maybe-false 0
- findings:
  - `low` `patch` B1 CONTEXT.md §14 has no "portal" entry, so it now collides with "Operator portal"/"Verification portal", and the "Hub" line still says "deferred" — added a Portal glossary line and split the Hub line into the deferred station-side hub and the shipped vendor-side `wipe-hub`
  - `low` `patch` B2 HomePage shows "Checking…" for ~7 s through React Query's default retries, and shows a stale "ok" next to "Hub unreachable" after a failed refetch — `retry: false`; render the error or the data, never both
  - `low` `patch` B3 the unbuilt-portal warning prints `./apps/portal/dist`, not the absolute path the hub looked in — the warning now prints the absolute path
  - `false` `reject` B4 `PortalDirInvalid` is wrong for a nonexistent path — the message says the path "has no index.html" (true) and offers "or fix the path"
  - `low` `patch` B5 `sourcemap: true` publishes `.map` source files from the internet-facing portal — `sourcemap: false`
  - `low` `reject` B6 `POST /api/health` returns axum's 405 with an empty body, not JSON — the spec's JSON 404 covers unmatched paths; no client sends a wrong method, and the fix adds a handler
  - `low` `reject` B7 `/favicon.ico` and other root files get `index.html` with 200 — browsers ignore a non-image favicon; 404 by extension adds a branch the spec does not call for
  - `low` `patch` B8 the hub's `/assets` no-fallback relies on Vite's default `assetsDir` — pinned `build.assetsDir: "assets"` with a comment naming `crates/wipe-hub/src/lib.rs`
  - `low` `patch` B9 no binary-level test of the cwd default, the startup warning or the bad-flag exit; no path-traversal test — binary tests added (grouped with VG-1); traversal is not tested: `ServeDir` rejects `..` components, which is its documented contract
  - `low` `patch` B10 the `.npmrc` comment lists only desktop and www lockfiles — now names apps/portal
  - `low` `patch` B11 the `VITE_API_TARGET` comment's example is the default value — non-default example
  - `low` `defer` B12 AGENTS.md's Portal bullet has no test count, and the router has no test — the fix edits AGENTS.md; a router test needs `main.tsx` split, with no named harm
  - `low` `patch` E1 stale data shown alongside the error — grouped with B2
  - `low` `reject` E2 a hung `/api/health` fetch stalls the 10 s refetch — needs a stalled proxy; the fix adds an abort-signal parameter
  - `low` `reject` E3 405 with an empty body on a wrong method — same as B6
  - `low` `reject` E4 `index.html` removed mid-rebuild gives 404s — transient: `ServeFile` resolves per request, so serving recovers when the build finishes
  - `low` `patch` VG-1 `Config::from_args`'s cwd default is never exercised: unit tests inject the cwd, and `portal.rs` overwrites the result — `tests/cli.rs` gains binary tests: built cwd (no warning), empty cwd (warning), bad `--portal-dir` (non-zero exit naming the path and `pnpm build`)
  - `low` `reject` VG-other the `WIPE_HUB_PORTAL_DIR` name is only prefix-checked — the same as every existing flag, and a rename is unlikely
  - `low` `reject` IA-a serving tests use a tempdir stand-in, not the real Vite `dist` — a cargo test that needs a pnpm build couples the Rust suite to node; the manual check served the real build
  - `false` `reject` IA-b "builds" is only an AC, not a test — the verification step ran `pnpm build`, and ADR-0006 §3 tests run under `pnpm test`; no CI exists to run a build check (ROADMAP §0.5)
  - `false` `reject` IA-c same-origin is only asserted through stand-ins — the manual run served `/` and `/api/health` from one origin on the real build, and `fetchHealth` uses a relative URL
  - `false` `reject` IA-d the workspace clause is met by existing state — CAP-5 asks that the root `pnpm-workspace.yaml` list `apps/*` and `sites/*`; it does, and `pnpm --filter portal` resolves through it
  - `low` `patch` IA-e the bad-flag refusal is tested only at `from_args`, not as a process exit — grouped with VG-1

## Design Notes

`/assets/*` has no fallback because Vite fingerprints bundle names. After a redeploy, a stale tab requesting an old hash should get a 404, not HTML parsed as JS. An unset flag falls back to a cwd default with a loud warning, which keeps `cargo run -p wipe-hub -- serve` working from the repo root. An explicit flag that is wrong is an error: that is CAP-6's "safe by default", and it avoids the `wipestation --static-dir` silent-landing pitfall.

## Verification

**Commands:**
- `cd apps/portal && pnpm install && pnpm build && pnpm typecheck && pnpm test` -- expected: all succeed.
- `docker compose up -d && cargo test --workspace` -- expected: all pass. Do not pipe the output through grep.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean.
- `rustfmt --edition 2021 --check <touched .rs files>` -- expected: clean.
- Manual: build the portal, run `cargo run -p wipe-hub -- serve`, then curl `/`, `/x/y`, `/api/health` and `/api/nope`.

## Auto Run Result

Status: done

**Summary:** `apps/portal` is a new React + Vite package (TanStack Router/Query, Tailwind 3, Vitest; the same versions as `apps/desktop`, with its own `pnpm-lock.yaml`). It has a header, a home page showing the hub's `/api/health`, and a not-found view. `wipe-hub serve` serves the built portal from the same origin as `/api/*`:
- Unknown `/api` and `/api/*` paths get a JSON 404.
- `/assets/*` never falls back to `index.html`.
- Any other unknown path gets `index.html` with a 200 (SPA fallback).

The hub finds the portal through `--portal-dir`/`WIPE_HUB_PORTAL_DIR`, or by default under `<cwd>/apps/portal/dist`. An explicit directory without `index.html` refuses to start. An unbuilt default logs a warning with the absolute path, and non-API paths then get a 404 that names the build command. The root `pnpm-workspace.yaml` already listed `apps/*` and `sites/*`, so it is unchanged.

**Files changed:**
- `crates/wipe-hub/Cargo.toml`, `Cargo.lock`: adds `tower-http`.
- `crates/wipe-hub/src/config.rs`: the `--portal-dir` flag, `resolve_portal_dir`, `PortalDirInvalid`, the unbuilt-portal warning, and unit tests.
- `crates/wipe-hub/src/lib.rs`: the API 404, the `/assets` service with no fallback, the SPA fallback, and the response when no portal is built.
- `crates/wipe-hub/tests/portal.rs`: 8 serving tests over HTTP (no compose needed).
- `crates/wipe-hub/tests/cli.rs`: 3 binary tests (cwd default used, unbuilt default warned, bad `--portal-dir` exits non-zero).
- `crates/wipe-hub/tests/health.rs`: `portal_dir: None` in its literal `Args`.
- `apps/portal/**`: the package, its configs, `src/main.tsx`, `src/pages/HomePage.tsx`, `src/api/health.ts` and its 6 vitest tests, and the lockfile.
- `.npmrc`: the comment now names the portal's lockfile.
- `AGENTS.md`: a Portal bullet, and the test count 191 → 209.
- `README.md`: the hub row, a new `apps/portal` row, the quick start, and the test count.
- `CONTEXT.md`: the §10.2 hub row now covers portal serving, and §14 gains a Portal entry and a clearer Hub line.

**Review findings:** 23 in total: 19 low, 4 false.
- **Patched (13 rows, 8 entries, all low):**
  - the glossary (B1);
  - stale status and slow retries on the home page (B2, E1);
  - the absolute path in the warning (B3);
  - source maps turned off (B5);
  - `assetsDir` pinned (B8);
  - binary-level tests (B9, VG-1, IA-e);
  - the `.npmrc` comment (B10);
  - the `VITE_API_TARGET` example (B11).
- **Deferred (1 low):** the portal's test count in AGENTS.md (B12), because the fix edits an agent-context file.
- **Rejected (6 low):**
  - a JSON body for 405 (B6, E3): no client sends a wrong method, and the fix adds a handler;
  - the root-file fallback (B7): harmless for browsers, and the fix adds a branch;
  - fetch timeout (E2): needs a stalled proxy, and the fix adds a parameter;
  - 404s during a rebuild (E4): transient, and serving recovers on its own;
  - the env-name check (VG-other): the same as every existing flag;
  - tests against the real `dist` (IA-a): would couple cargo tests to node, and the manual check covered it.
- **Rejected (4 false):** B4, IA-b, IA-c and IA-d, with reasons in the triage log.

**Follow-up review recommended: false.** This pass patched 8 low entries and no medium or high ones.

**Verification (after the patches):**
- `cd apps/portal && pnpm install --frozen-lockfile && pnpm build && pnpm typecheck && pnpm test`: exit 0, 6 vitest tests passed.
- With compose up, `cargo test --workspace` exited 0: 209 passed and no suite failed.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` is clean.
- `rustfmt --edition 2021 --check` is clean on the touched `.rs` files.
- Manual: `target/debug/wipe-hub serve` from the repo root served the real build:

  | Request | Result |
  |---|---|
  | `/`, `/x/y` | 200 `index.html` |
  | `/api/health` | 200 `ok` |
  | `/api/nope` | 404 JSON |
  | `/assets/nope.js` | 404 |
  | the hashed bundle | 200 `text/javascript` |

  It logged `portal_dir=Some(".../apps/portal/dist")`.
- Every I/O-matrix row has a covering test that passed.

**Residual risks:**
- Without the flag, the portal is found relative to the working directory, so a hub started anywhere else warns and serves no portal. That is by design, and the warning is loud.
- A wrong HTTP method on a known API route returns axum's empty 405, not JSON.
- The portal is an empty shell with no sign-in.

