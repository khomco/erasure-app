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

- `cargo test --workspace` — 946 tests, ~30s. Needs `docker compose up -d` first (root
  `compose.yaml`: Postgres on `127.0.0.1:55432`, Mailpit SMTP `:1025`, UI/API `:8025`):
  `wipe-hub`'s compose-backed tests fail, never skip, when they are down.
- Hub (ADR-0007): `docker compose up -d`, then `cargo run -p wipe-hub -- serve` and
  `curl 127.0.0.1:7900/api/health`. Flags have `WIPE_HUB_*` env twins whose defaults match the
  compose file; `--payment-provider stripe` refuses to start without both
  `WIPE_HUB_STRIPE_SECRET_KEY` and `WIPE_HUB_STRIPE_WEBHOOK_SECRET`, and `--stripe-api-base`
  accepts only https or http on a local host (tests point it at a local mock). `--site-origin` / `WIPE_HUB_SITE_ORIGIN` (default
  `http://localhost:4321`) is the one origin sent CORS headers, on `GET /api/auth/csrf` and
  `POST /api/leads` only; anything but a bare http(s) origin refuses to start. Release files go
  to `--artifact-dir` / `WIPE_HUB_ARTIFACT_DIR` (default `.wipe-hub/artifacts` under the working
  directory, git-ignored, created at start; a file path refuses to start); tests always pass a
  temp dir (`Hub::artifacts`, or `WIPE_HUB_ARTIFACT_DIR` for the binary). `wipe-hub release
  publish --version V --notes N --file <platform>=<path>` writes to the database and that
  directory directly, like `admin create`. No `sqlx::query!` macros: there is no build-time database.
- The full local platform (compose, hub, portal, marketing site, optional Stripe test mode,
  teardown) is in `docs/agents/local-stack.md`. `scripts/commerce-demo.sh` (needs `docker compose
  up -d`; ports 7910/7890) proves the customer journey end to end. `GET /api/deployment` (no
  session) drives the portal's development banner; the Fake-or-file-held-root rule lives only in
  `crates/wipe-hub/src/deployment.rs`.
- Hub migrations are embedded (`sqlx::migrate!`) and applied by `wipe-hub serve` before it
  listens; it exits non-zero within ~5 s if Postgres is unreachable. Add a migration as
  `crates/wipe-hub/migrations/NNNN_name.sql`; never edit an applied one.
- Hub tests use `crates/wipe-hub/tests/support` (`mod support;`): `TestDb::empty()` /
  `TestDb::migrated()` give each test its own `wipehub_test_<uuid>` database, dropped with the
  guard; `Mailpit` reads captured mail. Isolate mail by sending to `unique_address()`, never by
  deleting Mailpit messages.
- Goldens: `docs/generated/http-routes.txt` pins the station router's `METHOD /path` lines
  (`crates/wipe-server/tests/route_golden.rs`, routes declared once in `app.rs` `api_routes()`).
  After an intended route change, regenerate with `UPDATE_GOLDENS=1 cargo test -p wipe-server
  --test route_golden` and commit the file. The same test fails when a routed path has no call
  in `apps/desktop/src/api/client.ts` (or `ws.ts` for `/api/events`) or the client calls an
  unrouted one; `UPDATE_GOLDENS=1` does not relax that.
  `docs/generated/cli/wipestation[-<sub>...].txt` pin the exact `--help` output of `wipestation`
  and every subcommand (`crates/wipe-cli/tests/help_golden.rs`; commands discovered from the
  binary's own `Commands:` lists, run with an emptied environment). A missing, stale or changed
  file fails; after an intended CLI change run `UPDATE_GOLDENS=1 cargo test -p wipe-cli --test
  help_golden` (also deletes goldens for removed commands) and commit the directory.
  `docs/generated/entitlements.txt` lists every licence `Feature` (serde name + one-line
  summary, names read from the derive; `crates/wipe-license/tests/feature_listing.rs`). A new
  variant fails to compile until its summary `match` arm exists, then fails the golden; run
  `UPDATE_GOLDENS=1 cargo test -p wipe-license --test feature_listing` and commit the file.
- Never pipe `cargo test` through `grep`/`awk` to summarize: compile errors are discarded and
  a broken build prints as a clean run (68844bf).
- Frontend: `cd apps/desktop && pnpm typecheck && pnpm test` — vitest, 385 tests, under a second.
- Portal (ADR-0006 §1): `pnpm install` at the root, then `pnpm --filter portal build`,
  `pnpm --filter portal typecheck`, `pnpm --filter portal test` (dev server on :5174, proxying
  `/api` to the hub on :7900). `wipe-hub serve` serves `apps/portal/dist` under the working
  directory, or `--portal-dir` / `WIPE_HUB_PORTAL_DIR`; unbuilt, it logs a warning and answers
  non-API paths with a 404 naming the build. A `--portal-dir` without `index.html` refuses to
  start. Unknown `/api/*` paths are JSON 404s and `/assets/*` never falls back to `index.html`.
- Marketing site: `pnpm install` at the root, then `pnpm --filter www build` (fails on any
  overclaim — wrap the copy in `<Capability id>`, never weaken a guarded phrase), `pnpm --filter
  www typecheck`, `pnpm --filter www test`; `pnpm --filter www lighthouse` needs local Chrome.
  Capability statuses live in `sites/www/src/data/capabilities.ts` and are a vendor sign-off.
  The lead forms stay closed (no script) unless built with `PUBLIC_HUB_ORIGIN=http://localhost:7900`
  (`pnpm --filter www dev` for a local demo); `astro build` refuses that origin while
  `site.privacyNoticePublished` is `false`. Consent wording lives in `src/data/consent.ts` and
  is pinned to `crates/wipe-hub/src/leads.rs`: change both together. `/pricing/` lists live
  prices only when built with `PRICING_HUB_ORIGIN=http://127.0.0.1:7900`; then `build` needs a
  reachable hub and fails if it cannot read `GET /api/offerings`. Unset, it needs no hub.
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
  `Erased`/`Destroyed` (ADR-0011; `crates/wipe-server/src/app.rs:216`); entering
  `PendingDestruction` signs nothing — mutating `Job::state` without emitting that broadcast
  yields a silently uncertified job. A `Destroyed` Job is certified from its run in
  `AppState::manifests`; `advance_run` builds every member's `DestructionRecord` before
  `mark_destroyed`, so never move Jobs to `Destroyed` around it.
- A `Certificate` is immutable once signed: `verify` re-canonicalizes against the stored hash,
  then checks v3 content. Nothing is appended after signing (`co_sign` is gone; v3 refuses
  co-signatures), and every field added to a type inside the signed payload must be
  `#[serde(default, skip_serializing_if = …)]` so `crates/wipe-cert/tests/fixtures/v2-*.json`
  still verify. Never edit those fixtures.
- All bay-topology writes go through `AppState::save_bay_topology`, never
  `*self.bay_topology.write()` — it is the only place the ADR-0003 revision check and
  persistence happen. The topology store holds configuration only; evidence never goes there.
- Bay-topology matching rules live in Rust alone (ADR-0002) — never port them to TypeScript;
  the dry-run handler `resolve_bay_topology` (`crates/wipe-server/src/handlers.rs:120`) is the
  intended seam for previewing an unsaved draft.
- TypeScript domain types are hand-mirrored from Rust serde shapes and change together:
  `wipe-common` structs with `apps/desktop/src/api/types.ts`; `JobBroadcast`/`WsEnvelope` tags
  with `src/api/ws.ts` and `crates/wipe-server/src/ws.rs`; new routes with
  `crates/wipe-server/src/app.rs` and `src/api/client.ts`. The frontend talks HTTP/WS, not
  Tauri commands.
- Station start-up lives once, in `wipe_server::bootstrap` (`build_station`,
  `resolve_frontend_dist`): `wipestation serve` and the Tauri shell only map their options into a
  `StationConfig`. Wire anything new there, never in one host, or the two hosts diverge.
- Update `CONTEXT.md`, `README.md`, and `docs/design/*` in the same commit as the code they
  describe; when a roadmap item ships, strike it in `docs/ROADMAP.md` and record it in
  `CONTEXT.md` §10. Docs trailing code is the repeated failure here (f7af492, a06e8ac, 7610905).

## Questions left for the human

First, decide it yourself when you can. If one option is clearly the safe default or recognised best practice, choose it, record the choice where the work is tracked (the story's intent contract, an ADR or `CONTEXT.md`) and carry on. Ask only about:
- product, pricing, licensing, legal, branding or customer-visible-copy choices;
- anything that contradicts an ADR or a `docs/ROADMAP.md` §0 invariant;
- anything irreversible or outside the repo;
- anything where no option is clearly safest or standard.

`bmad-build-auto` runs follow the same rule through `_bmad/custom/bmad-build-auto.toml`.

The rest of this section applies to every question you leave for the owner to answer: a blocked story's `## Auto Run
Result` (including `bmad-build-auto` intent-gap halts), a `.scratch/DECISIONS.md` row, a review
or story hand-off, and a question asked in chat. The owner answers these cold, without having
read the code, the spec or the review. So each question must make sense on its own and include:

1. **What it is about.** Name the feature, who uses it, and what they do. Say what currently
   happens in that situation, from the user's or operator's point of view.
2. **The problem.** Describe a concrete scenario, with who does what and what goes wrong, in
   the product terms `CONTEXT.md` §14 defines. Code names, file paths and `snake_case`
   identifiers may follow the plain explanation but never replace it.
3. **The options.** For each option, say what a user or operator would see, what it costs (in
   effort, risk or ongoing operation) and what it rules out.
4. **A recommendation** and the reason for it.
5. **The default.** Say what happens, or what ships, if nobody decides.

Every acronym or term of art gets a one-clause gloss the first time it appears. Examples: CSRF,
Argon2, `/64`, append-only audit log. Put the question itself last, as one sentence that can
be answered with an option letter.

## Known pitfalls

- After adding a field to a shared domain struct, grep `crates/*/tests` for literal
  `Type { … }` initializers — updating the constructors in the defining module is not enough,
  and stale fixtures break the workspace build (68844bf).

<!-- /bmad:context -->
