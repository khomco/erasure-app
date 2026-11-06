---
title: 'wipe-hub crate, compose file, config and health'
type: 'feature'
created: '2026-11-06'
status: 'done'
baseline_revision: '832c0b738eee677919005f1a38a364c4abdc4382'
review_loop_iteration: 0
followup_review_recommended: true
context: ['{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md']
warnings: ['oversized']
deferred:
  - summary: >-
      Documented `docker compose up -d` returns before the Postgres healthcheck passes, so a warm `cargo test` can race initdb.
    evidence: |-
      compose.yaml defines a pg_isready healthcheck but AGENTS.md, README.md and the spec use plain `up -d`; `--wait` would block until healthy. Fix edits AGENTS.md (agent-context file), so deferred per triage rules. A cold build's compile time normally hides the race.
    location: >-
      AGENTS.md:25, README.md (quick start)
    severity: low
  - summary: >-
      AGENTS.md "Conventions" does not state the ADR-0007 hub dependency rule or that crates/wipe-hub/tests/deps.rs enforces it.
    evidence: |-
      The one-way dependency bullet lists only common/engine/cert/server; the hub appears only as a run command. Fix edits AGENTS.md, so deferred.
    location: >-
      AGENTS.md (Conventions that differ from defaults)
    severity: low
---

<intent-contract>

## Intent

**Problem:** The commercial platform (ADR-0006/0007) has no vendor-side service. Accounts, payments and licence delivery all need a running hub, its local dependencies and explicit config before any of them can start.

**Approach:** Add the `crates/wipe-hub` library and binary (Axum) with a `serve` subcommand. Config comes from flags and env and is validated at start-up. `GET /api/health` probes Postgres and SMTP. A root `compose.yaml` runs Postgres and Mailpit (CAP-1, CAP-6).

## Boundaries & Constraints

**Always:** `wipe-hub` depends only on `wipe-common`, `wipe-cert` and `wipe-license` among sibling crates, and no other crate depends on it. Use the workspace dependency style (new deps such as `sqlx` go in `[workspace.dependencies]`). MSRV 1.80, edition 2021. Compose publishes ports on `127.0.0.1` only, and Postgres uses a non-default host port (5432 and 5433 are taken on dev machines). The DB pool connects lazily, so the hub starts and reports `database: down` rather than crashing when Postgres is down. Health probes are time-bounded (≤2 s each). Every start logs a warning when the vendor root is file-held. AGENTS.md, README.md and CONTEXT.md are updated in the same commit.

**Never:** No migrations, test-DB harness, Mailpit API client, audit log or portal serving (stories 2–4). No accounts, payments or licence issuance. No `sqlx::query!` compile-time macros (no `DATABASE_URL` at build time). No change to station crates beyond the workspace member list. No hardware-sanitization claims.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Healthy | compose up; `GET /api/health` | 200 `{"status":"ok","database":"up","mail":"up"}` | — |
| DB or SMTP down | an unreachable DB URL or SMTP host:port | 503, `status:"degraded"`, failing component `"down"` | probe timeout ≤2 s, no panic |
| Stripe, no secret | `--payment-provider stripe`, no `--stripe-webhook-secret` | process refuses to start, error names `WIPE_HUB_STRIPE_WEBHOOK_SECRET` | non-zero exit |
| Bad provider | `--payment-provider paypal` | clap rejects the value | non-zero exit |
| Vendor root file | `--vendor-root-seed` names a valid 32-byte file | starts; a warning says it is file-held and not production custody | — |
| Bad seed file | the path is missing, or not 32 bytes | refuses to start, error names the path | non-zero exit |
| No seed | flag absent | starts; logs that licence issuance is unavailable | — |
| Bad base URL | `--public-base-url notaurl` | refuses to start | non-zero exit |

</intent-contract>

## Code Map

- `Cargo.toml` -- add `crates/wipe-hub` to members; add `sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "postgres"] }` to workspace deps.
- `crates/wipe-server/src/app.rs:288` (`router`), `:388` (`serve`) -- pattern to mirror for the Axum router and serve function. Read-only.
- `crates/wipe-cli/src/main.rs:182` -- tracing/`EnvFilter` init and clap `Parser`/`Subcommand` + `env` style to copy. `:883` `load_seed` reads a 32-byte seed into `SigningKey::from_seed` (it is private, so replicate it, don't import it).
- `crates/wipe-license/src/chain.rs:69` -- `VendorRoot::from_signing_key`, `public_key_id`; `wipe_cert::SigningKey::from_seed`.
- `README.md:37` crate table, `:57` quick start; `AGENTS.md` "Running and verifying"; `CONTEXT.md:416` §6 stack list and §10.2 table.

## Tasks & Acceptance

**Execution:**
- `Cargo.toml` -- add the member and the workspace `sqlx` dep -- new crate.
- `crates/wipe-hub/Cargo.toml` -- package `wipe-hub` with lib and `[[bin]] wipe-hub` (`src/main.rs`); deps: wipe-common/cert/license, axum, tokio, sqlx, clap, serde(_json), tracing(-subscriber), anyhow, thiserror, url; dev: reqwest (`json`), tempfile -- CAP-1.
- `crates/wipe-hub/src/config.rs` -- clap `Args` struct (`--addr` default `127.0.0.1:7900`, `--database-url`, `--smtp-host`, `--smtp-port`, `--public-base-url`, `--payment-provider fake|stripe` default `fake`, `--stripe-webhook-secret`, `--vendor-root-seed`; every flag has an `env = "WIPE_HUB_*"`). Defaults match `compose.yaml`. `Config::from_args` validates and loads the seed into a `VendorRoot`. `startup_warnings(&Config) -> Vec<String>` -- CAP-6.
- `crates/wipe-hub/src/health.rs` -- `SELECT 1` on the lazy `PgPool` and an SMTP probe (TCP connect, then read a `220` greeting), each under `tokio::time::timeout` -- CAP-1.
- `crates/wipe-hub/src/lib.rs` -- `AppState { config, pool }`, `router(state)` with `GET /api/health`, `serve(config)` -- the seam for later stories.
- `crates/wipe-hub/src/main.rs` -- `wipe-hub serve [flags]`: init tracing, build the config, log each startup warning with `warn!`, serve.
- `crates/wipe-hub/tests/health.rs` -- the degraded test (closed ports, no network needed) and a green test against compose. The green test fails, never skips, with a message containing "run docker compose up -d" when Postgres is unreachable.
- `crates/wipe-hub/tests/deps.rs` -- asserts no other workspace crate's `Cargo.toml` names `wipe-hub`, and that `wipe-hub`'s only sibling deps are common/cert/license.
- `crates/wipe-hub/src/config.rs` (unit tests) -- cover every config row of the I/O matrix.
- `compose.yaml` -- `name: wipestation`; `postgres:16-alpine` (user/password/db `wipehub`, healthcheck, `127.0.0.1:55432:5432`); `axllent/mailpit` (`127.0.0.1:1025`, `127.0.0.1:8025`).
- `AGENTS.md`, `README.md`, `CONTEXT.md` -- hub commands (`docker compose up -d`, `cargo run -p wipe-hub -- serve`); `cargo test --workspace` now needs compose; the crate table row; the §6 stack line (hub: Postgres via sqlx, ADR-0007); a §10.2 row marked partial.

**Acceptance Criteria:**
- Given `docker compose up -d` on a fresh clone, when `cargo run -p wipe-hub -- serve` runs and `curl 127.0.0.1:7900/api/health` is called, then it returns 200 with database and mail `up`.
- Given Postgres is stopped, when `cargo test -p wipe-hub` runs, then the green health test fails with a message telling the developer to run `docker compose up -d`.
- Given the workspace, when `cargo test --workspace` runs with compose up, then all tests pass, including `wipe-hub`'s.

## Spec Change Log

## Review Triage Log

### 2026-11-06 — Review pass
- verdicts: 38 findings — high 0, medium 7, low 26, false 5, maybe-false 0
- findings:
  - `low` `patch` B1 --help prints WIPE_HUB_DATABASE_URL value incl. password — clap shows env values without hide_env_values; added hide_env_values = true
  - `false` `reject` B2 Args derives Debug so secrets leak — nothing formats Args with {:?}; only Config is logged and it redacts
  - `low` `reject` B3 webhook secret accepted as a flag (ps/history) — SPEC CAP-6 asks for env/flags; env is available and documented; dev-only today
  - `medium` `patch` B4 pool acquire_timeout tied to 2 s probe bound — would cap every later query at 2 s contention; removed it, outer tokio timeout still bounds the probe
  - `low` `reject` B5 seed file permissions unchecked — adds new branches; custody is already flagged as non-production on every start (ADR-0007 A1.5)
  - `low` `reject` B6 sqlx built without TLS — no deployment target exists (ROADMAP §0.5); local compose is plaintext loopback; adding rustls is a dependency decision for deployment
  - `low` `defer` B7 `docker compose up -d` races the Postgres healthcheck — real; fix (`--wait`) edits AGENTS.md, deferred
  - `low` `patch` B8 Mailpit image unpinned — pinned axllent/mailpit:v1.33.0 (the running version)
  - `medium` `patch` B9 Cargo.lock re-resolves windows-sys for station deps — caused by the `home` 0.5.9 pin; restored minimal lock without the pin (only redox_syscall disambiguation renames remain)
  - `low` `patch` B10 reqwest declared inline, not workspace style — added reqwest to [workspace.dependencies]; wipe-hub uses workspace = true; wipe-server untouched
  - `low` `patch` B11 green test hard-codes compose addresses in messages — messages now derived from DEFAULT_* constants
  - `low` `reject` B12 timing test flaky / closed_port reuse race — 1.5 s headroom on loopback, no flake across runs; port-reuse window negligible
  - `low` `defer` B13 AGENTS.md conventions omit hub dependency rule — real; fix edits AGENTS.md, deferred
  - `false` `reject` B14 ROADMAP A1.1 not updated — AGENTS.md requires striking only when an item ships; A1.1 (schema, migrations, tenant model) has not
  - `low` `reject` B15 public_base_url default ignores --addr — no link is built yet; a cross-check adds branches for a later story's concern
  - `low` `reject` B16 startup warnings logged in main, not serve() — main is the only start path, as the spec places it; now verified by tests/cli.rs
  - `low` `patch` E1 webhook secret stored untrimmed — now trimmed before storing; test asserts " whsec_test\n" -> "whsec_test"
  - `low` `reject` E2 secret given with fake provider silently ignored — unlikely; adds a branch
  - `low` `reject` E3 seed path to /dev/zero hangs — contrived; adds a metadata guard
  - `low` `reject` E4 base URL with userinfo/query/fragment accepted — unlikely in a vendor-set config; adds validation branches
  - `low` `reject` E5 empty smtp host / port 0 accepted — health reports mail down, which is the endpoint's job; adds branches
  - `low` `reject` E6 TLS sslmode=require fails — same as B6
  - `low` `patch` E7 probe failures only at debug — a 503 had no reason at default info level (this is how the Mailpit RDNS stall hid); now warn!
  - `low` `reject` E8 clap test reads developer env — only an invalid WIPE_HUB_* var could alter it, and it would still yield InvalidValue; cli.rs clears env
  - `low` `patch` E9 green test message misleading on credential failure — database message now adds the stale-volume hint (docker compose down -v)
  - `low` `defer` E10 compose --wait race — same root cause as B7, deferred
  - `medium` `patch` E11 Cargo.lock station-dep churn — same root cause as B9; fixed there
  - `medium` `patch` V1 binary untested: exit codes and startup warning logging — added tests/cli.rs (stripe flag, stripe via env, paypal, file-held warning on stdout)
  - `medium` `patch` V2 clap defaults never checked against constants — added clap_defaults_match_the_constants
  - `medium` `patch` V3 Cargo.lock windows-sys churn — same root cause as B9; fixed there
  - `medium` `patch` I1 tests stop below the binary surface — grouped with V1; tests/cli.rs runs the binary
  - `low` `patch` I2 bad-provider test uses Args, not Cli — grouped with V1; cli.rs runs `serve --payment-provider paypal`
  - `low` `patch` I3 env-var path never exercised — grouped with V1; cli.rs sets WIPE_HUB_PAYMENT_PROVIDER=stripe via env
  - `low` `patch` I4 defaults vs compose unpinned — grouped with V2
  - `false` `reject` I5 workspace pass and test counts not demonstrated — cargo test --workspace ran: exit 0, 161 passed, 0 failed
  - `false` `reject` I6 dependency rule stricter than SPEC — stricter is consistent with ADR-0007 §1 ("never a dependency of any station crate"); no defect
  - `low` `reject` I7 timing test bounds total, not each probe — each probe is wrapped in its own timeout; total bound with concurrent silent peers implies it
  - `false` `reject` I8 story file written in the same change as the code — workflow artifact, not a code defect

## Design Notes

The DB pool is lazy because the health endpoint's job is to report an outage, not to die from one. Making the seed optional keeps story 1 runnable without a key. Issuance stories will require it. A path that is given but invalid is an error, because a typo should never silently disable signing.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean (`--no-deps`: `wipe-common` already fails `large_enum_variant`, a pre-existing error outside this story)
- `cargo run -p wipe-hub -- serve --payment-provider stripe` -- expected: non-zero exit, error names the webhook secret

## Auto Run Result

Status: done

**Summary:** New `crates/wipe-hub` (library + `wipe-hub serve` binary): `WIPE_HUB_*` flag/env config validated at start-up (stripe refuses without a webhook secret; optional 32-byte vendor-root seed warned as file-held on every start; bad seed or base URL refuses), lazy Postgres pool, `GET /api/health` probing Postgres (`SELECT 1`) and SMTP (220 greeting) at ≤2 s each (200 `ok` / 503 `degraded`), and a root `compose.yaml` with Postgres (127.0.0.1:55432) and Mailpit v1.33.0 (1025/8025, RDNS disabled).

**Files changed:**
- `Cargo.toml` — wipe-hub member; workspace `sqlx` and `reqwest`.
- `Cargo.lock` — hub dependencies only (sqlx and friends); no station-dep re-resolution.
- `compose.yaml` — Postgres and Mailpit for local dev, loopback-only ports.
- `crates/wipe-hub/Cargo.toml` — package, lib + bin, sibling deps common/cert/license only.
- `crates/wipe-hub/src/config.rs` — Args/Config, validation, redacting Debug, startup warnings, unit tests.
- `crates/wipe-hub/src/health.rs` — probes and the health handler.
- `crates/wipe-hub/src/lib.rs` — AppState, router, serve/serve_on.
- `crates/wipe-hub/src/main.rs` — CLI entry point, logs startup warnings.
- `crates/wipe-hub/tests/health.rs` — degraded, timeout, fake-SMTP and compose-green tests.
- `crates/wipe-hub/tests/cli.rs` — binary exit codes and file-held warning.
- `crates/wipe-hub/tests/deps.rs` — ADR-0007 dependency rule via cargo metadata.
- `AGENTS.md`, `README.md`, `CONTEXT.md` — hub commands, compose requirement, crate row, §6 stack line, §10.2 partial row.

**Review findings:** 38 total. Patched 18 rows (4 medium entries: lockfile churn, untested binary surface, unpinned clap defaults, pool acquire-timeout; plus low patches for --help secret, secret trimming, warn-level probe logs, Mailpit pin, workspace reqwest, test messages). Deferred 3 rows (2 items, both AGENTS.md edits: `compose up --wait`, hub dependency rule in Conventions). Rejected 17 rows (5 false, 12 low), reasons in the triage log.

**Follow-up review recommended: true** (patched by verdict: medium 4 entries, low 7 entries, high 0). Unverified risk: `tests/cli.rs` spawns the real binary, reads its stdout on a thread with a 10 s limit and kills the child; it has not been reviewed for flakiness or orphaned processes under parallel `cargo test`.

**Verification:** with compose up, `cargo test --workspace` exited 0 (161 passed, 0 failed). `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` is clean. `cargo run -p wipe-hub -- serve --payment-provider stripe` exits 1 naming `WIPE_HUB_STRIPE_WEBHOOK_SECRET`. Every I/O-matrix row has a passing covering test.

**Residual risks:**
- `cargo clippy` without `--no-deps` fails on a `large_enum_variant` error in `wipe-common` that predates this story.
- MSRV 1.80 is not met. The workspace already needed Rust 1.88 through `time` 0.3.47, and `home` 0.5.12, which `sqlx` pulls in, also needs 1.88. Pinning `home` lower churned station dependencies, so it was dropped.
- `cargo test --workspace` now needs `docker compose up -d`, by design (ADR-0007).

