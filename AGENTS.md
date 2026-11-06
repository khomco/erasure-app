<!-- bmad:context -->
<!-- Verified 2026-11-05 against 7610905. Managed by bmad-project-context; edits inside this
     block are replaced on refresh. Keep anything you want preserved outside the markers. -->

## Wipestation

Certified drive erasure and destruction for ITAD benches: a Rust workspace (`crates/wipe-*`,
binary `wipestation`) serving `/api/*` and the React UI from one Axum origin, plus a Tauri
shell at `apps/desktop`. Domain vocabulary and product context are in `CONTEXT.md`, decisions
in `docs/adr/`, unbuilt surfaces in `docs/ROADMAP.md`.

## Where things are

- Read `CONTEXT.md` and the ADRs in `docs/adr/` touching your area before changing behavior
  there; name domain concepts with the term `CONTEXT.md` §14 defines, not a synonym. To
  contradict an ADR, say so explicitly rather than silently overriding it.
- Work to be built is markdown under `.scratch/<feature-slug>/`: BMAD spec folders (`SPEC.md`,
  `stories.yaml`, `stories/<id>-<slug>.md`) run in `.scratch/QUEUE` order by
  `scripts/bmad-driver.py`, plus older `PRD.md`/`issues/NN-slug.md` folders. Current status and
  open decisions: `.scratch/STATUS.md`, `.scratch/DECISIONS.md`. Details:
  `docs/agents/issue-tracker.md`.

## Running and verifying

- `cargo test --workspace` — 138 tests, ~30s.
- Never pipe `cargo test` through `grep`/`awk` to summarize: compile errors are discarded and
  a broken build prints as a clean run (68844bf).
- Frontend: `cd apps/desktop && pnpm typecheck && pnpm test` — vitest, 200 tests, under a second.
- `wipestation serve` with no built frontend silently serves an "API only" landing page, as
  does a `--static-dir` that does not exist; run `cd apps/desktop && pnpm build` first.
- Format only the files you touched with `rustfmt --edition 2021 <files>`. Never `--all`, and
  never `cargo fmt -- <file>`: that still formats every crate in the graph. 13 tracked files
  are unformatted, and either form buries a small change in an unrelated 13-file diff.
- Rust MSRV 1.80, edition 2021, stable toolchain with rustfmt and clippy; pnpm 9+. Read
  versions from `Cargo.toml` and `rust-toolchain.toml`, never from the shell.

## Conventions that differ from defaults

- Dependency direction is one-way: `wipe-common` depends on no sibling crate and `wipe-engine`
  depends only on `wipe-common`. Signing (`wipe-cert`) and HTTP (`wipe-server`) sit above the
  engine; the engine must never reach up into them.
- All hardware access goes through the `DeviceBackend` trait, and only `JobRunner` calls its
  mutating methods (`issue`, `unfreeze`, `cancel`, `verify`); handlers get read-only
  `enumerate`/`capabilities`. Bypassing the runner skips evidence capture.
- Certificates are produced only as a side effect of a `JobBroadcast::JobStateChanged` into
  `Erased`/`PendingCoSign` (`crates/wipe-server/src/app.rs:197`) — mutating `Job::state`
  without emitting that broadcast yields a silently uncertified job.
- A `Certificate` is immutable once signed: `verify` re-canonicalizes against the stored hash,
  so co-signing must sign the same bytes.
- All bay-topology writes go through `AppState::save_bay_topology`, never
  `*self.bay_topology.write()` — it is the only place the ADR-0003 revision check and
  persistence happen. The topology store holds configuration only; evidence never goes there.
- Bay-topology matching rules live in Rust alone (ADR-0002) — never port them to TypeScript;
  the dry-run handler `resolve_bay_topology` (`crates/wipe-server/src/handlers.rs:119`) is the
  intended seam for previewing an unsaved draft.
- TypeScript domain types are hand-mirrored from Rust serde shapes and change together:
  `wipe-common` structs with `apps/desktop/src/api/types.ts`; `JobBroadcast`/`WsEnvelope` tags
  with `src/api/ws.ts` and `crates/wipe-server/src/ws.rs`; new routes with
  `crates/wipe-server/src/app.rs` and `src/api/client.ts`. The frontend talks HTTP/WS, not
  Tauri commands.
- Frontend-bundle resolution is implemented twice — `crates/wipe-cli/src/main.rs:516` and
  `apps/desktop/src-tauri/src/lib.rs:87`. Change both or the desktop and server paths diverge.
- Update `CONTEXT.md`, `README.md`, and `docs/design/*` in the same commit as the code they
  describe; when a roadmap item ships, strike it in `docs/ROADMAP.md` and record it in
  `CONTEXT.md` §10. Docs trailing code is the repeated failure here (f7af492, a06e8ac, 7610905).

## Known pitfalls

- After adding a field to a shared domain struct, grep `crates/*/tests` for literal
  `Type { … }` initializers — updating the constructors in the defining module is not enough,
  and stale fixtures break the workspace build (68844bf).

<!-- /bmad:context -->
