---
title: 'Migrations and the per-test database harness'
type: 'feature'
created: '2026-11-06'
status: 'done'
baseline_revision: 'af77068dea9f2f686356361093bb08a964e594dc'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md']
warnings: ['oversized']
deferred: []
---

<intent-contract>

## Intent

**Problem:** `wipe-hub` has a Postgres pool but no schema versioning, and its tests have no way to get an isolated database or read captured mail. Every later hub story (audit log, accounts, payments) needs both.

**Approach:** Embed sqlx migrations from `crates/wipe-hub/migrations` in the binary and apply them before `serve` binds its listener (CAP-2). Add a shared integration-test support module that creates a fresh database per test, drops it afterwards, and wraps Mailpit's HTTP API. It fails with "run docker compose up -d" when a compose service is unreachable (CAP-3).

## Boundaries & Constraints

**Always:** Migrations are embedded at compile time with `sqlx::migrate!("./migrations")`, so the binary never reads them from disk at runtime. A `build.rs` reruns the build when `migrations/` changes. Start-up and the test harness apply migrations through the same `wipe_hub::db` function. Start-up migration makes one bounded connection attempt (≤5 s), separate from the lazy request pool, and exits non-zero with an error that names the redacted database address and says `run docker compose up -d`. The password never appears in logs or errors. Each test database gets a unique name (`wipehub_test_<uuid-simple>`) and is dropped `WITH (FORCE)` when its guard is dropped, including when the test panics. Tests never delete Mailpit messages globally. They isolate by using a unique recipient address per test. Harness failures panic (never skip), and the message names the address and says `run docker compose up -d`. `cargo test --workspace` runs these tests. Update AGENTS.md, CONTEXT.md and README.md in the same commit.

**Never:** No `audit_log` or any domain table (story 3). No portal serving (story 4). No accounts, payments or mail-sending code in `src/`. No `sqlx::query!`/`query_as!` macros and no build-time `DATABASE_URL`. No change to the health endpoint's behaviour after start-up: the pool stays lazy, and a database outage while serving still reports `database: down`. No changes to station crates. No hardware-sanitization claims.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Fresh DB start | `wipe-hub serve --database-url <empty db>` | migrations applied, then "wipe-hub listening" logged | — |
| Already migrated | same DB, second start | no-op, starts normally | — |
| DB unreachable at start | URL to a closed port | exits non-zero within ~5 s; stderr contains `run docker compose up -d`, no password | — |
| Migration test | `TestDb::empty()` then `db::migrate` | `_sqlx_migrations` rows == embedded migrations, all `success` | — |
| Two tests in parallel | two `TestDb::migrated()` | different database names; a write in one is invisible to the other | — |
| Guard dropped / test panics | `TestDb` goes out of scope | database no longer listed in `pg_database` | drop errors logged to stderr, not panicked |
| Postgres down in a test | admin connect fails | panic with `run docker compose up -d` | never skip |
| Mail capture | raw SMTP to Mailpit for a unique address | `Mailpit::wait_for_message_to(addr, timeout)` returns its subject and text body | timeout panics naming the address; Mailpit down panics with `run docker compose up -d` |

</intent-contract>

## Code Map

- `Cargo.toml:65` -- workspace `sqlx` (`runtime-tokio`, `postgres`). Add `migrate` and `macros` features (the `migrate!` macro needs both; sqlx is used only by wipe-hub). `uuid` (`:51`) and `tempfile` already exist.
- `crates/wipe-hub/Cargo.toml` -- add `build = "build.rs"` (or rely on auto-detect), `uuid` dependency (dev or normal as needed), `tokio` already present.
- `crates/wipe-hub/src/lib.rs:28` `AppState::new` -- lazy pool stays. `:53` `serve` -- call `db::migrate_url(&config.database_url)` before `TcpListener::bind`. `serve_on` stays migration-free, because the existing health tests point it at closed ports.
- `crates/wipe-hub/src/config.rs:18` `DEFAULT_DATABASE_URL`, `:20` `DEFAULT_SMTP_PORT` -- reuse in the harness. Redact with `url::Url::set_password(None)` (or print host:port/db).
- `crates/wipe-hub/src/health.rs` -- read-only. Probe timeouts are unchanged.
- `crates/wipe-hub/tests/health.rs:152` `green_against_compose` -- copy the "run docker compose up -d" message style from here. `tests/cli.rs:10` `hub()` -- strips `WIPE_HUB_*` env from the child process. Reuse this pattern for the binary start-up tests. Note that `file_held_vendor_root_is_logged_at_start` uses the default DB URL, and startup warnings are logged before `serve`, so it still passes when compose is up or down.
- `compose.yaml` -- Postgres `wipehub`/`wipehub` on `127.0.0.1:55432` (the role owns `wipehub` and can `CREATE DATABASE`). Mailpit API on `127.0.0.1:8025`, with `GET /api/v1/search?query=to:"<addr>"` and `GET /api/v1/message/{ID}` (fields `Subject`, `Text`).

## Tasks & Acceptance

**Execution:**
- `Cargo.toml` -- add the sqlx `migrate` and `macros` features -- `migrate!` embedding.
- `crates/wipe-hub/migrations/0001_baseline.sql` -- a header comment (the hub schema starts here; story 3 adds `audit_log`) plus `SELECT 1;`. That gives a real, applied, versioned migration without inventing domain tables.
- `crates/wipe-hub/build.rs` -- `println!("cargo:rerun-if-changed=migrations");`.
- `crates/wipe-hub/src/db.rs` -- `pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");`, `pub async fn migrate(pool: &PgPool) -> Result<(), MigrateError>`, `pub async fn migrate_url(url: &str) -> anyhow::Result<()>` (one `PgConnection::connect` under a 5 s `tokio::time::timeout`, run `MIGRATOR`, close the connection), and `pub fn redacted(url: &str) -> String`. Errors read `cannot apply migrations to <redacted>: <cause>; is Postgres running? run docker compose up -d`.
- `crates/wipe-hub/src/lib.rs` -- `pub mod db;` and migrate inside `serve` before binding. Log at info how many migrations were applied or are present.
- `crates/wipe-hub/tests/support/mod.rs` -- `#![allow(dead_code)]` shared harness. `TestDb::empty()` / `TestDb::migrated()` connect to the admin DB (`DEFAULT_DATABASE_URL`) under a 5 s timeout, `CREATE DATABASE`, and expose `pool`, `url`, `name`. `Drop` drops the database on a separate thread with its own current-thread runtime. `Mailpit` has `new()` (`http://127.0.0.1:8025`), `messages_to`, `message`, and `wait_for_message_to(addr, Duration)` (polls). `unique_address()` returns `t-<uuid>@wipehub.test`.
- `crates/wipe-hub/tests/migrations.rs` -- `mod support;`. Tests: the empty DB migrates to the embedded count with all rows `success`; migrating twice is idempotent; two `TestDb`s are isolated (create a scratch table in one, absent in the other); a dropped guard (including through a caught panic, `std::panic::catch_unwind` or a spawned task that panics) leaves no `pg_database` row.
- `crates/wipe-hub/tests/startup.rs` -- spawn the binary (`env!("CARGO_BIN_EXE_wipe-hub")`, `WIPE_HUB_*` stripped) against a `TestDb::empty()` URL with `--addr 127.0.0.1:0` and `--smtp-port` on a closed port. Wait for "wipe-hub listening" on stdout (10 s limit), kill the child, then assert `_sqlx_migrations` holds every embedded migration. A second test uses a closed-port URL with a real-looking password: non-zero exit, output contains `run docker compose up -d`, and the password is absent.
- `crates/wipe-hub/tests/mailpit.rs` -- `mod support;`. Send one message over raw SMTP (tokio `TcpStream`: EHLO, MAIL FROM, RCPT TO, DATA, `.`, QUIT) to `unique_address()`. Assert `wait_for_message_to` returns the subject and body, and `messages_to` for a different unique address is empty.
- `AGENTS.md` -- under "Running and verifying": the hub applies migrations at start-up and exits if Postgres is unreachable; hub tests use `tests/support` (`TestDb`, `Mailpit`); add new migrations as `crates/wipe-hub/migrations/NNNN_name.sql`; update the test count.
- `CONTEXT.md` §10.2 hub row -- add migrations at start-up, the per-test DB harness and the Mailpit client, and remove them from the "not yet" list. `README.md` -- mention migrations in the hub quick start if it describes start-up.

**Acceptance Criteria:**
- Given compose is up and a fresh empty database, when `wipe-hub serve` starts against it, then every embedded migration is recorded as successful before the hub logs that it is listening.
- Given Postgres is stopped, when `cargo test -p wipe-hub` runs, then the harness-based tests fail with a message containing `run docker compose up -d`, and none of them pass vacuously.
- Given compose is up, when `cargo test --workspace` runs, then all tests pass, and no `wipehub_test_%` database is left behind.

## Spec Change Log

## Review Triage Log

### 2026-11-06 — Review pass
- verdicts: 34 findings — high 0, medium 0, low 27, false 6, maybe-false 1
- findings:
  - `low` `patch` B1 start-up (`migrate_url`/`run_direct`) and harness (`migrate`) take different db functions; docs claim they share one — TestDb::migrated now calls db::migrate_url; docs corrected
  - `low` `reject` B2 only connect is time-bounded; a held advisory lock hangs start-up — docs claim fast exit only for an unreachable Postgres, which holds; a held lock means another hub is migrating and releases it; bounding migration execution is a policy decision for later long migrations
  - `low` `reject` B3 `applied=` count read outside the migration lock — log-only, needs two concurrent starts on one DB; fix needs restructuring around sqlx's lock
  - `low` `reject` B4 no test/decision for rollback to older binary or edited migration — sqlx default (VersionMissing/checksum) fails loudly; AGENTS.md says never edit an applied migration; adds tests for library behaviour
  - `low` `patch` B5 baseline header holds a roadmap note that is part of the checksum — header reduced to timeless text; local dev DB's stale `_sqlx_migrations` row cleared
  - `false` `reject` B6 fail-fast start-up with no retry, undocumented — Design Notes explain it, and the CONTEXT §10.2 row states "start-up exits within ~5 s if Postgres is unreachable"; the hub is not in compose, so depends_on does not apply
  - `low` `reject` B7 databases orphaned by a killed run are never swept — needs SIGKILL/abort; a sweep would race a concurrent cargo test run and adds branches
  - `low` `patch` B8 password-leak test's address check is weak — now asserts the exact redacted `wipehub@127.0.0.1:<port>/wipehub`
  - `low` `patch` B9 tests/cli.rs file-held test now migrates the shared `wipehub` dev DB — test now points --database-url at a closed port
  - `low` `patch` B10 empty_on comment overstates guard coverage — comment reworded to match the code
  - `low` `reject` B11 wait_for_message_to can overrun by one 5 s request — test-only, needs a stalled Mailpit; fix adds per-request deadline plumbing
  - `low` `reject` B12 helpers duplicated across test files — cosmetic; no caller diverges today
  - `low` `reject` B13 `macros` feature makes `query!` usable — rule is a documented convention; adding a clippy disallowed-macros config is new tooling beyond this story
  - `maybe-false` `reject` B14 parallel TestDb pools may exceed max_connections — pools are lazy and tests open 1–2 connections; would need a run with many more harness tests to settle; if true only low
  - `low` `reject` E1 advisory-lock hang — same as B2
  - `low` `reject` E2 pre-lock ensure_migrations_table races two fresh-DB starts — real in sqlx ordering, but two hubs on one fresh DB is not a supported local scenario; fix needs a missing-table branch
  - `low` `reject` E3 applied-count race — same as B3
  - `low` `patch` E4 guard built after Url::parse/connect_lazy — grouped with B10
  - `low` `reject` E5 no stale-DB sweep — same as B7
  - `low` `reject` E6 harness ignores WIPE_HUB_DATABASE_URL — the harness targets compose by contract, and hub tests deliberately strip WIPE_HUB_* so a developer env cannot change outcomes
  - `low` `patch` E7 expect_reply hangs on a bare "250\r\n" final line — now ends the reply unless byte 3 is '-'
  - `low` `reject` E8 SMTP reads have no timeout — test-only, Mailpit RDNS disabled; adds timeout plumbing
  - `low` `patch` E9 start_until_listening discards child stderr — stderr now inherited
  - `low` `reject` E10 wait overrun — same as B11
  - `low` `reject` E11 claim: no-leftover AC rests on Drop only — same as B7; the verification psql check returned 0
  - `low` `reject` E12 claim: ~5 s exit not bounded past connect — same as B2
  - `low` `patch` V1 success-path migration log line never checked for redaction — startup test now asserts `database=<redacted url>` and no `:wipehub@`
  - `low` `patch` V2 cli.rs test migrates the shared dev DB — grouped with B9
  - `low` `patch` V3 empty_on comment — grouped with B10
  - `false` `reject` IA-a start-up now exits when Postgres is down, reversing story 1's lazy start — CAP-2/ADR-0007 require migrations applied at start-up, which a non-gating start cannot guarantee; story 1's after-start outage reporting is unchanged and stated in the Never list and Design Notes
  - `false` `reject` IA-b fail-not-skip never exercised at suite level with Postgres down — verified by stopping Postgres and running `cargo test -p wipe-hub` (see Auto Run Result)
  - `false` `reject` IA-c harness lives in tests/support, unavailable to src unit tests — CAP-3 asks for a test-harness module; nothing in src needs a database
  - `false` `reject` IA-d Mailpit is a shared inbox — one Mailpit is the compose contract; isolation by unique recipient is per-test logical isolation, tested (other address empty)
  - `false` `reject` IA-e 177 test count unverified — `cargo test --workspace` result lines sum to 177 passed, 0 failed

## Design Notes

"Applied at start-up" means migrate before serving, not alongside it. A hub that serves requests against an unmigrated schema breaks every later story, so start-up fails fast if Postgres is down. Story 1's lazy pool still covers what it was for: reporting an outage that happens after start. The start-up migration uses its own single connection, so the error arrives in ~5 s instead of after the pool's 30 s acquire retries.

Mailpit is shared across parallel tests, so isolation comes from unique recipient addresses. A global delete would race other tests.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean
- `psql postgres://wipehub:wipehub@127.0.0.1:55432/wipehub -tc "select count(*) from pg_database where datname like 'wipehub_test_%'"` (or the equivalent via `docker compose exec postgres psql -U wipehub`) after the test run -- expected: 0

## Auto Run Result

Status: done

**Summary:** `wipe-hub` now embeds sqlx migrations (`sqlx::migrate!`, baseline `0001_baseline.sql`). `wipe-hub serve` applies them through `db::migrate_url` before it binds, using one connection bounded at 5 s. If Postgres is unreachable it exits non-zero with a redacted address and `run docker compose up -d`. A shared integration harness, `tests/support`, provides `TestDb` and `Mailpit`. `TestDb` is a per-test `wipehub_test_<uuid>` database, migrated through the same `migrate_url` and dropped `WITH (FORCE)` with its guard, including on panic. `Mailpit` is an API client, and tests isolate mail by sending to `unique_address()`. Every harness failure panics with the compose hint and never skips.

**Files changed:**
- `Cargo.toml`: adds the sqlx `migrate` and `macros` features. `Cargo.lock` is updated to match.
- `crates/wipe-hub/Cargo.toml`: adds `uuid` as a dev-dependency.
- `crates/wipe-hub/build.rs`: rebuilds when `migrations/` changes.
- `crates/wipe-hub/migrations/0001_baseline.sql`: baseline migration (`SELECT 1`).
- `crates/wipe-hub/src/db.rs`: `MIGRATOR`, `migrate`, `migrate_url`, `embedded_count` and `redacted`, with unit tests.
- `crates/wipe-hub/src/lib.rs`: `serve` migrates before binding. `serve_on` is unchanged.
- `crates/wipe-hub/tests/support/mod.rs`: the `TestDb`, `Mailpit` and `unique_address` harness.
- `crates/wipe-hub/tests/migrations.rs`: migration count and success, idempotency, isolation, drop and panic-drop, and the Postgres-down hint.
- `crates/wipe-hub/tests/startup.rs`: runs the binary. Covers migrating before listening, a restart that applies 0, the redacted success log, an unreachable database exiting fast with no password leak, and the silent-DB bound.
- `crates/wipe-hub/tests/mailpit.rs`: raw SMTP into Mailpit and back, the timeout panic, and the Mailpit-down hint.
- `crates/wipe-hub/tests/cli.rs`: the file-held test now uses a closed database port, so it no longer migrates the dev database.
- `AGENTS.md`, `README.md`, `CONTEXT.md`: start-up migration behaviour, how to add a migration, harness usage, the test count (177), and the §10.2 hub row.

**Review findings:** 34 in total (high 0, medium 0, low 27, false 6, maybe-false 1).
- **Patched:** 8 entries, all low (12 rows):
  - one migration path for start-up and the harness (B1);
  - a timeless baseline header (B5);
  - an exact redacted-address assertion (B8);
  - `cli.rs` no longer migrates the shared dev database (B9/V2);
  - a corrected `empty_on` comment (B10/E4/V3);
  - the SMTP bare-reply hang (E7);
  - child stderr inherited (E9);
  - an assertion that the success log line is redacted (V1).
- **Deferred:** none.
- **Rejected:** 22 rows, each with its reason in the triage log:
  - 15 low: unbounded advisory lock, applied-count race, rollback and checksum tests, stale-database sweep, Mailpit wait overrun, duplicated helpers, clippy `disallowed-macros`, concurrent `ensure_migrations_table`, harness env override, SMTP read timeouts, and the duplicates of each;
  - 1 maybe-false: connection growth;
  - 6 false: fail-fast was documented, story-1 start-up reversal, fail-not-skip at suite level, harness location, shared Mailpit, and the test count.

**Follow-up review recommended: false.** This pass patched 0 high, 0 medium and 8 low entries.

**Verification:**
- With compose up, `cargo test --workspace` exited 0: 177 passed, 0 failed.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` is clean.
- After the run, `pg_database` has no `wipehub_test_%` rows (count 0).
- `rustfmt --check` passes on every touched wipe-hub file.
- With Postgres stopped, `cargo test -p wipe-hub --tests --no-fail-fast` exited 101. All 8 Postgres-backed tests failed, and the compose hint was printed. Compose was restarted afterwards and is healthy.
- Every row of the I/O matrix has a covering test that passed.

**Residual risks:**
- Start-up migration is bounded only while connecting. Once connected, sqlx's advisory lock and the migration run have no time limit, which matters if two hubs share one database.
- Test databases from a run that is killed (SIGKILL or abort) are never swept.
- `wait_for_message_to` can overrun its deadline by up to one 5 s HTTP timeout.
- `cargo test -p wipe-hub` now takes about 5 s longer, because of the silent-database bound test.
- The MSRV and `wipe-common` clippy issues from story 1 are unchanged.
