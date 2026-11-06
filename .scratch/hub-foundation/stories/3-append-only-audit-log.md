---
title: 'Append-only audit log'
type: 'feature'
created: '2026-11-06'
status: 'done'
baseline_revision: 'ae371076c610227368c9919226439f576f7c924b'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md']
warnings: ['oversized']
deferred: []
---

<intent-contract>

## Intent

**Problem:** ADR-0007 §6 requires that every vendor-admin action, licence issuance, revocation, order state change and download goes into an append-only audit table with actor, time and subject (ROADMAP A1.4). The hub has no such table yet, and every later hub story needs it in place first.

**Approach:** Add a migration that creates `audit_log`. Database triggers reject `UPDATE`, `DELETE` and `TRUNCATE` on it, so append-only holds whatever the Rust code does. Add a `wipe_hub::audit` module whose only write is `record`. `record` takes any sqlx executor, so a later story can audit inside the same transaction as the action it audits (CAP-4).

## Boundaries & Constraints

**Always:** The schema goes in a new migration, `crates/wipe-hub/migrations/0002_audit_log.sql`. `0001_baseline.sql` is never edited. Columns: `id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY`, `occurred_at timestamptz NOT NULL DEFAULT now()` (UTC and set by the database, so a caller cannot backdate), `actor text NOT NULL`, `action text NOT NULL`, `subject text NOT NULL`, and `detail jsonb NOT NULL DEFAULT '{}'`. `actor`, `action` and `subject` each carry a `CHECK (btrim(col) <> '')`, and `detail` carries `CHECK (jsonb_typeof(detail) = 'object')`. One `BEFORE UPDATE OR DELETE ... FOR EACH ROW` trigger and one `BEFORE TRUNCATE ... FOR EACH STATEMENT` trigger call a function that raises an error whose message contains `audit_log is append-only`. `audit::record` is the only public function in `audit` that writes. It returns the new row's id. Its signature is generic over `sqlx::PgExecutor` (a `&PgPool`, a `&mut PgConnection`, or a `&mut *tx`). All queries use runtime `sqlx::query*`. Update CONTEXT.md §10.2, AGENTS.md (the test count) and README.md in the same commit.

**Never:** No update, delete or purge function in `audit`. No HTTP route that reads or writes the audit log, and no callers of `record` in `src/`: nothing auditable exists yet, and later stories add the call sites. No actor or user tables and no foreign keys: actors are free-form text until accounts exist. No `sqlx::query!` macros. No portal serving (story 4). No station-crate changes. No hardware-sanitization claims.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Record | `record(&pool, &AuditEntry{actor:"vendor_admin:alice", action:"licence.issue", subject:"licence:42", detail:json!({"seats":3})})` | `Ok(id)`; the row holds those values, and `occurred_at` is within a few seconds of `now()` | — |
| Ids increase | two records in a row | the second id is greater than the first | — |
| In a transaction | `record(&mut *tx, ..)`, then `tx.rollback()` | the row is absent | — |
| Blank field | actor, action or subject empty or whitespace | `Err(AuditError::Blank("actor"))` (names the field), no row written | validated in Rust before the query; the DB CHECK is the backstop |
| Detail not an object | `detail: json!([1])` or `json!("x")` | `Err(AuditError::DetailNotObject)`, no row | the DB CHECK is the backstop |
| UPDATE through the pool | `UPDATE audit_log SET actor='x'` | `Err` containing `audit_log is append-only`; the row is unchanged | trigger |
| DELETE through the pool | `DELETE FROM audit_log` | `Err` containing `audit_log is append-only`; the row count is unchanged | trigger |
| TRUNCATE through the pool | `TRUNCATE audit_log` | `Err` containing `audit_log is append-only`; the row count is unchanged | statement trigger |
| Supplied timestamp | `record` has no time parameter | `occurred_at` always comes from the database | — |

</intent-contract>

## Code Map

- `crates/wipe-hub/migrations/0001_baseline.sql` -- read-only. Its header says to add a new `NNNN_name.sql`.
- `crates/wipe-hub/build.rs` -- already reruns the build when `migrations/` changes. No edit needed.
- `crates/wipe-hub/src/db.rs` -- `MIGRATOR`, `migrate`, `migrate_url`, `embedded_count`. Read-only. The new migration is picked up automatically, and the existing migration tests then count 2.
- `crates/wipe-hub/src/lib.rs:8` -- module list. Add `pub mod audit;`.
- `crates/wipe-hub/tests/support/mod.rs` -- `TestDb::migrated()` (fresh database with every migration applied) exposes `pool`, `url` and `name`. Use it as the harness. It already fails with `run docker compose up -d` when Postgres is down.
- `crates/wipe-hub/tests/migrations.rs` -- reference style for a `mod support;` test file.
- `Cargo.toml:65` -- workspace `sqlx` features. Add `json`, so `serde_json::Value` binds to `jsonb`. Only wipe-hub uses sqlx. `serde_json` is already a wipe-hub dependency.
- `CONTEXT.md:692` -- §10.2 hub row. It says "no audit log … yet (`hub-foundation` stories 3–4)".
- `AGENTS.md` -- "177 tests" in Running and verifying.
- `README.md` -- crate table row for wipe-hub. Mention the audit log if the row lists hub contents.

## Tasks & Acceptance

**Execution:**
- `Cargo.toml` -- add `json` to the workspace sqlx features. Update `Cargo.lock` if it changes -- jsonb binding.
- `crates/wipe-hub/migrations/0002_audit_log.sql` -- the table, CHECKs, a `audit_log_append_only()` plpgsql function that does `RAISE EXCEPTION 'audit_log is append-only (% rejected)', TG_OP`, and the row and statement triggers. Add a header comment naming ADR-0007 §6 -- CAP-4 schema.
- `crates/wipe-hub/src/audit.rs` -- module doc (what gets audited per ADR-0007 §6, and that append-only is enforced by triggers). `pub struct AuditEntry<'a> { actor: &'a str, action: &'a str, subject: &'a str, detail: serde_json::Value }` with public fields. `#[derive(thiserror::Error)] pub enum AuditError { Blank(&'static str), DetailNotObject, Db(#[from] sqlx::Error) }`. `pub async fn record<'e, E: sqlx::PgExecutor<'e>>(executor: E, entry: &AuditEntry<'_>) -> Result<i64, AuditError>`: validate, then `INSERT … RETURNING id`. Unit tests for validation run without a database -- CAP-4 API.
- `crates/wipe-hub/src/lib.rs` -- `pub mod audit;`.
- `crates/wipe-hub/tests/audit.rs` -- `mod support;`. One test per I/O-matrix row that touches the database: record and read back, ids increase, transaction rollback, blank field and non-object detail write no row, and UPDATE/DELETE/TRUNCATE through `t.pool` each fail with `audit_log is append-only` while the rows stay unchanged. Also a test that the DB CHECKs reject a blank actor inserted with raw SQL, proving the backstop -- CAP-4 immutability test.
- `CONTEXT.md` -- §10.2 hub row: add the append-only `audit_log` (`audit::record`, with UPDATE/DELETE/TRUNCATE rejected by triggers). Remove "audit log" from the not-yet column and keep portal serving (story 4) there.
- `AGENTS.md` -- update the `cargo test --workspace` test count to the measured number.
- `README.md` -- if the wipe-hub crate row lists contents, add the audit log.

**Acceptance Criteria:**
- Given compose is up, when `cargo test --workspace` runs, then every test passes, including `tests/audit.rs`, and the migration tests report 2 embedded migrations, all applied successfully.
- Given a hub database migrated by `wipe-hub serve`, when anyone holding the hub's pool issues UPDATE, DELETE or TRUNCATE on `audit_log`, then Postgres rejects it with `audit_log is append-only`.
- Given Postgres is stopped, when `cargo test -p wipe-hub --test audit` runs, then the tests fail with `run docker compose up -d` and never pass vacuously.

## Spec Change Log

## Review Triage Log

### 2026-11-06 — Review pass
- verdicts: 22 findings — high 0, medium 0, low 17, false 5, maybe-false 0
- findings:
  - `low` `patch` B1 `btrim(col)` strips only spaces, so a tab- or newline-only value passes the DB CHECK that Rust `trim()` rejects — confirmed in psql (`btrim(E'\t') <> ''` is true). 0002 was not yet applied to the dev DB, so it was edited in place: CHECKs are now `col ~ '\S'`, and the raw-SQL backstop test gains `E'\t'`, `E'\n'` and `E'\t\n'` cases
  - `low` `patch` B2 "a caller cannot backdate an entry" is false for a raw INSERT that supplies `occurred_at` — docs now say `record` never supplies a time, so `occurred_at` is the database's `now()`
  - `low` `patch` B3 `now()` is the transaction start time, and id order is not commit order or gap-free; neither was documented — added to the audit.rs module doc
  - `low` `reject` B4 tests match English error text, not SQLSTATE — the compose Postgres uses the default English locale; switching needs `USING ERRCODE` plus code plumbing in tests, which nobody meets in everyday use
  - `low` `reject` B5 no defined action/actor vocabulary, so names may drift — no call sites exist yet; the spec keeps actors free-form until accounts exist; an enum or consts adds public surface without a caller
  - `low` `patch` B6 the CONTEXT.md §10.2 reference column omits ROADMAP A1.4 — the column now reads `ROADMAP A1.1, A1.4`; A1.4 is not struck because no call sites exist
  - `low` `reject` B7 no indexes beyond the PK — no read path exists to size an index for; the table is empty, so a later migration adds one without lock cost
  - `low` `patch` B8 the module doc says actions "are written" though nothing calls `record`, and `AuditError::Db` has no doc comment — reworded as what callers must record; doc comment added
  - `false` `reject` B9 trigger enablement untested — `update_is_rejected`, `delete_is_rejected` and `truncate_is_rejected` run on the fully migrated schema, so any later migration that disables a trigger fails them
  - `low` `patch` E1 btrim whitespace gap — grouped with B1
  - `low` `reject` E2 a NUL byte in a field reaches the DB despite "never reaches the database" — `record` returns `AuditError::Db`, no row is written, it needs deliberately malformed input, and the fix adds a recursive JSON scan
  - `low` `reject` E3 field sizes are unbounded — callers are hub code, not user input; caps add branches and a policy decision with no caller
  - `low` `patch` E4 `now()` is transaction start — grouped with B3 (documented, not changed: sharing the transaction's time is acceptable)
  - `low` `reject` E5 locale-dependent "check constraint" match — same as B4
  - `low` `patch` E6 docs say only a superuser can bypass, but the owner (the hub's own role) can disable or drop triggers — migration header, audit.rs doc and CONTEXT.md now name "the table owner (the hub's own role) or a superuser"
  - `low` `patch` E7 backdating via raw INSERT — grouped with B2
  - `low` `patch` E8 the Rust checks are claimed to mirror the DB CHECKs — grouped with B1
  - `low` `patch` VG-1 the docs understate who can disable the triggers — grouped with E6
  - `false` `reject` IA-a the immutability tests prove DB refusal, not that the Rust layer lacks a mutating path — `audit` has only `record` (review-confirmed), and CAP-4's "through the hub's data layer" is met by the hub's pool being refused
  - `false` `reject` IA-b the owner or a superuser can still alter rows (strong reading R3) — CAP-4 scopes to "through the hub's data layer"; DDL is not a data-layer path, and the limitation is documented with its ROADMAP §0.5 owner
  - `false` `reject` IA-c no action is recorded yet and there is no read path — CAP-4's success asks for the table, `record` and the immutability test only; call sites belong to later stories
  - `false` `reject` IA-d enabling sqlx `json` adds `sqlx-mysql`/`sqlx-sqlite` to Cargo.lock — `cargo tree -p wipe-hub -e normal` shows neither is compiled; the lock only lists weak optional deps


## Design Notes

Triggers enforce append-only, not `REVOKE`. The compose `wipehub` role owns the table and is a superuser, so a revoked privilege would not stop it, but a trigger fires for the owner too. A superuser could still `ALTER TABLE … DISABLE TRIGGER` or drop the table. Guarding against an operator who controls the database needs a separate, unprivileged runtime role, and that is a deployment decision (ROADMAP §0.5), not this story's. CAP-4 asks for "through the hub's data layer": the data layer has no mutating path, and the pool it uses is refused.

`record` is generic over the executor so later stories write the audit row in the same transaction as the action. A rolled-back licence issuance then leaves no audit row, and a committed one cannot lose its row.

## Verification

**Commands:**
- `docker compose up -d && cargo test --workspace` -- expected: all pass. Do not pipe the output through grep.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean.
- `rustfmt --edition 2021 --check crates/wipe-hub/src/audit.rs crates/wipe-hub/src/lib.rs crates/wipe-hub/tests/audit.rs` -- expected: clean.

## Auto Run Result

Status: done

**Summary:** Migration `0002_audit_log.sql` creates the append-only `audit_log` table. `occurred_at` defaults to the database's `now()`. `actor`, `action` and `subject` are text that must contain a non-whitespace character, and `detail` must be a jsonb object. Triggers reject `UPDATE`, `DELETE` and `TRUNCATE` with `audit_log is append-only`, including for the table owner. `wipe_hub::audit::record` is the module's only write. It checks the entry, then returns the new id, and it accepts a pool, a connection or a transaction, so an action and its audit row can commit or roll back together. Nothing calls it yet.

**Files changed:**
- `Cargo.toml` and `Cargo.lock`: adds the sqlx `json` feature, so jsonb binds. No new backends are compiled.
- `crates/wipe-hub/migrations/0002_audit_log.sql`: the table, its checks, the trigger function and both triggers.
- `crates/wipe-hub/src/audit.rs`: `AuditEntry`, `AuditError` and `record`, plus validation unit tests.
- `crates/wipe-hub/src/lib.rs`: adds `pub mod audit;`.
- `crates/wipe-hub/tests/audit.rs`: 11 integration tests covering the round trip, increasing ids, a single connection, transaction rollback and commit, blank fields, non-object detail, the DB checks as a backstop, and UPDATE, DELETE and TRUNCATE being rejected.
- `CONTEXT.md` (§10.2 hub row), `README.md` (crate row and test count), `AGENTS.md` (test count 191).

**Review findings:** 22 in total: 17 low, 5 false.
- **Patched (6 low entries, covering 11 rows):**
  - The DB check missed tab- and newline-only values. It now uses `~ '\S'`, with tests (B1, E1, E8).
  - The docs said entries could not be backdated. They now say `record` never supplies a time (B2, E7).
  - Documented that `now()` is the transaction's start time and that ids can have gaps (B3, E4).
  - Added A1.4 to the CONTEXT.md reference column (B6).
  - The module doc said actions are written to the log. It now says callers must record them, and `AuditError::Db` has a doc comment (B8).
  - The docs named only a superuser as able to disable the triggers. They now name the owner or a superuser (E6, VG-1).
- **Deferred:** none.
- **Rejected (6 low):**
  - English error text instead of SQLSTATE (B4, E5): the default locale is English, and the fix adds plumbing.
  - Action vocabulary (B5): there are no callers yet, and it would add public surface.
  - Indexes (B7): there is no read path yet, and the table is empty.
  - NUL bytes (E2): needs malformed input, and the error is already returned.
  - Size caps (E3): no untrusted callers, and it adds a policy decision.
- **Rejected (5 false):**
  - Trigger enablement (B9): already covered by the rejection tests on the fully migrated schema.
  - IA-a to IA-c: CAP-4 is met by the hub's pool being refused. Owner DDL and call sites are outside it.
  - IA-d: `cargo tree` shows no mysql or sqlite crates are compiled.

**Follow-up review recommended: false.** This pass patched 6 low entries and no high or medium ones.

**Verification:**
- With compose up, `cargo test --workspace` exited 0: 191 passed, 0 failed, both before and after the patches.
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` is clean.
- `rustfmt --edition 2021 --check` is clean on the touched Rust files.
- The implementer stopped Postgres and ran `cargo test -p wipe-hub --test audit`. All 11 tests failed with `run docker compose up -d`, and compose was restarted afterwards.
- Every I/O-matrix row has a covering test that passed.

**Residual risks:**
- The hub's own role owns the table and is a superuser in compose, so it can still disable or drop the triggers. A separate unprivileged runtime role is a deployment decision (ROADMAP §0.5).
- No action, actor or subject vocabulary is defined yet, so later stories should agree on names when they add call sites.
- The MSRV and `wipe-common` clippy issues from story 1 are unchanged.
