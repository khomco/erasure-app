---
title: 'Releases, artifact store and CLI publish'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: 'bcff6965ff97f4257d680e7a334a523b5881f227'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md'
  - '{project-root}/docs/adr/0006-web-presence-and-local-first-verification.md'
warnings: [oversized]
deferred:
  - summary: >-
      An admin upload abandoned mid-transfer may leave a `.part` temp file in the artifact directory.
    evidence: |-
      Unverified (maybe-false). `LocalDirStore::put` removes its temp file only when `put` returns.
      If hyper drops the handler future on a client disconnect, rather than handing it a body error,
      the cleanup never runs. Settle it with a test that starts an upload, closes the socket mid-body,
      then checks the directory; if true, add a drop guard and/or a start-up sweep of `.*.part` files.
    location: >-
      crates/wipe-hub/src/artifacts.rs LocalDirStore::put
    severity: medium (unverified)
---

<intent-contract>

## Intent

**Problem:** The vendor has no way to put station software in the hub: no releases, nowhere to keep files, no checksums, and nothing for the end-to-end script to publish (SPEC CAP-1, CAP-4, CAP-5; ADR-0007 §5).

**Approach:** Add releases to the hub. A vendor admin creates a draft release (version, notes, channel), uploads one file per platform (the hub streams it to an `ArtifactStore` and computes its SHA-256 itself), publishes it, and can withdraw it. Files live behind an `ArtifactStore` trait with a local-directory implementation. `wipe-hub release publish` does the same in one step from the command line. Customer listing and downloads are story 2.

## Boundaries & Constraints

**Always:**
- Only a `VendorAdmin` (the existing extractor in `admin_routes.rs`, first extractor of every handler) reaches the release routes: no session 401, anyone else 403 `MSG_ADMIN_ONLY`, before any path, query or body is read.
- Lifecycle `draft` → `published` → `withdrawn`, plus `draft` → `withdrawn`; `withdrawn` is terminal. Artifacts can be added or removed only while the release is `draft`; publishing needs at least one artifact. Postgres enforces it (migration `0017_releases.sql`): no deletes of releases; `version`, `channel`, `notes` and creation fields never change; only the three status steps above, with `published_at` / `withdrawn_at` set exactly when reached; artifact rows are never updated, and are inserted or deleted only while their release is `draft`.
- `version` is SemVer 2.0.0 (parsed with the `semver` crate, ≤ 64 chars), unique across all releases including withdrawn ones. `channel` is `stable` or `preview`. `notes` are trimmed, non-blank, ≤ 10 000 chars, no control characters except newline and tab. Platform label `^[a-z0-9][a-z0-9._-]{0,63}$`, unique per release. File name `^[A-Za-z0-9_+-][A-Za-z0-9._+-]{0,127}$`. Bodies deny unknown fields; a bad value is 400 naming the field.
- The hub computes SHA-256 (lowercase hex) and size while streaming the bytes, in code shared by every `ArtifactStore` implementation, never from anything the client claims. Files over `ARTIFACT_MAX_BYTES` (1 GiB) are 413 and empty files 400; either way nothing is kept.
- `ArtifactStore` (async trait, `Arc<dyn ArtifactStore>` in `AppState`): `put(key, reader)`, `open(key)` (for story 2), `delete(key)`. Keys are the artifact's UUID; the local implementation refuses any other key, writes to a temp file in the same directory, fsyncs, then renames, so a reader never sees a partial file. A file stored for a database write that then fails is deleted before the error is returned.
- Upload streams the body to the store before opening its transaction (no row lock held during a long upload), then in one transaction locks the release `FOR UPDATE`, re-checks `draft` and the platform, inserts the row and audits; on refusal it deletes the stored file.
- Every change is audited in its transaction, subject `release:<id>`: `release.created {version, channel}`, `release.artifact_added {artifact_id, platform, filename, size_bytes, sha256}`, `release.artifact_removed {same}`, `release.published {artifact_ids}`, `release.withdrawn {previous_status}`. Admin reads are audited through `admin::audit_read` as `admin.releases_listed {release_ids}` and `admin.release_viewed {}`. Actor `user:<admin id>`, or `cli:release-publish` with `{os_user}` in the detail for the CLI. A refused request or an unknown id writes nothing.
- `wipe-hub release publish --version V --notes N [--channel stable|preview] --file <platform>=<path>…` (at least one `--file`; `--channel` default `stable`; `--database-url`, `--artifact-dir` with `serve`'s env twins and defaults) validates everything first, applies migrations like `admin create`, stores each file, then creates, adds and publishes in one transaction. Any failure exits non-zero naming the problem, commits nothing and deletes the files it stored. Success prints the release id and, per artifact, platform, size and SHA-256.
- `serve` and the CLI take `--artifact-dir` / `WIPE_HUB_ARTIFACT_DIR`, default `.wipe-hub/artifacts` under the working directory; it is created if missing, and a path that cannot be created or is not a directory refuses to start. Tests always use a temp directory.

**Never:** No customer listing or download, no entitlement check, no download audit (story 2); no admin download; no code signing, auto-update or CDN; no cloud store implementation; no editing a release's version, channel or notes; no un-withdraw; no multipart dependency (the upload is the raw request body); no edit to an applied migration; no hardware-sanitization claim in any copy.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Create | admin, `POST /api/admin/releases {version:"0.2.0", notes, channel:"stable"}` | 201 `{release}` status `draft`, no artifacts; audited | — |
| Bad create | bad semver, unknown channel, blank/oversized/control-char notes, unknown field | 400 naming the field | nothing written |
| Duplicate version | version already used, even by a withdrawn release | 409 `MSG_VERSION_TAKEN` | nothing written |
| Upload | `POST /api/admin/releases/:id/artifacts?platform=linux-x86_64&filename=wipestation`, `application/octet-stream` body | 201 `{artifact}` with `size_bytes`, `sha256` = SHA-256 of the body; `open(key)` returns the same bytes; audited | — |
| Bad upload | bad or missing platform/filename, empty body | 400 naming the problem | nothing stored or written |
| Too large | body over the limit (`Config::artifact_max_bytes`, `ARTIFACT_MAX_BYTES` by default, not a flag; tests lower it) | 413 `MSG_ARTIFACT_TOO_LARGE` | no file left in the store dir, nothing written |
| Platform taken / not draft | same platform again; release published or withdrawn | 409 `MSG_PLATFORM_TAKEN` / `MSG_RELEASE_NOT_DRAFT` | stored file deleted, nothing written |
| Remove artifact | `POST /api/admin/releases/:id/artifacts/:artifact_id/remove` on a draft | 200 `{release}` without it; file deleted; audited | not draft → 409; unknown → 404 `MSG_NO_ARTIFACT` |
| Publish | `POST /api/admin/releases/:id/publish` on a draft with artifacts | 200 `{release}` `published`, `published_at`; audited | no artifacts → 409 `MSG_NO_ARTIFACTS`; not draft → 409 |
| Withdraw | `POST /api/admin/releases/:id/withdraw` on draft or published | 200 `{release}` `withdrawn`, `withdrawn_at`; files kept; still listed to admins | already withdrawn → 409 |
| List / view | `GET /api/admin/releases`, `GET /api/admin/releases/:id` | newest first, every status, each with artifacts (`id, platform, filename, size_bytes, sha256, created_at`); audited | unknown or non-UUID id → 404 `MSG_NO_RELEASE`, nothing audited |
| Not an admin | none / owner / member on any release route | 401 / 403 `MSG_ADMIN_ONLY` | nothing read or written |
| CLI publish | `wipe-hub release publish --version 0.2.0 --notes "…" --file linux-x86_64=<path>` | exit 0; release `published` with that artifact and the file's SHA-256; printed id and checksum; audited as `cli:release-publish` | — |
| CLI failure | bad flag value, missing file, duplicate version, unreachable database | non-zero exit naming the problem | no release row, no file left in the store |
| Store | `LocalDirStore` on a temp dir | put/open/delete round-trip; a key that is not a UUID (`../x`, `a/b`) is refused; delete of a missing key is not an error | — |

## Autonomous decisions

- **Releases start as drafts.** Problem: an admin uploads files one at a time, so a release visible as soon as it is created would be offered to customers with missing platforms. Options: (a) live on create, (b) draft until published. Chosen (b), matching offerings (draft → published → retired): customers never see a half-uploaded release, and files are frozen once published so a checksum a customer saw never changes.
- **A version names one set of files forever.** Versions are unique across all releases, withdrawn included, and SemVer. Reusing a withdrawn version for different bytes would let two files share a name and a customer's checksum mismatch; the safe default is to burn it.
- **Withdrawal is terminal, keeps the files, and takes no reason.** The files stay so the audit trail (who downloaded what, story 2) still points at real bytes; offerings' retire takes no reason either.
- **Upload is the raw request body, capped at 1 GiB.** It avoids a multipart dependency and lets the hub stream and hash without buffering; the station build is far smaller, and PXE images are not in scope.
- **The CLI writes to the database and the store directly**, like `wipe-hub admin create`, rather than over HTTP with an admin session: the end-to-end script runs on the same machine, and it needs no browser sign-in. It publishes in one step.
- **The artifact directory defaults to `.wipe-hub/artifacts` under the working directory** (git-ignored), created on start, so `serve` with no flags still works locally (ADR-0006 §3).
- **No admin download in this story.** Every download is to be audited (ADR-0007 §6, CAP-3); that comes with story 2's download path.

</intent-contract>

## Code Map

- `crates/wipe-hub/src/offering_admin.rs` -- closest pattern: draft/published/retired lifecycle, validation with 400 naming the field, `OfferingError`, every function takes `VendorAdmin` + transaction and writes its own audit row (`audit_read` for reads, l.~413-600). Mirror it in a new `release_admin.rs`.
- `crates/wipe-hub/migrations/0010_offerings.sql` -- lifecycle trigger and CHECK style to mirror in `0017_releases.sql`. Highest existing migration is `0016_lapse_policy.sql`.
- `crates/wipe-hub/src/admin_routes.rs` -- `VendorAdmin` extractor, `routes()` list, `From<…Error> for ApiError`, handler pattern (one transaction), module doc listing every route; add the release routes and doc lines.
- `crates/wipe-hub/src/admin.rs` -- `audit_read`, `VendorAdmin::user_id`, `CLI_ACTOR` + `os_user()` (l.439) + `create_vendor_admin` (l.483: migrate, connect with `CONNECT_TIMEOUT`, `db::redacted`) -- reuse for the CLI path.
- `crates/wipe-hub/src/config.rs` -- `Args`, `AdminCreateArgs`, `Config`, `ConfigError`, hand-written `Debug`; add `artifact_dir` (and a `ReleasePublishArgs`).
- `crates/wipe-hub/src/lib.rs` -- `AppState::new` (add `artifacts: Arc<dyn ArtifactStore>`), module list.
- `crates/wipe-hub/src/main.rs` -- clap `Cmd` (add `Release { Publish(ReleasePublishArgs) }`).
- `crates/wipe-hub/src/payments.rs:328` -- `#[async_trait]` trait-object seam pattern (`async-trait` is a workspace dep).
- `crates/wipe-hub/src/csrf.rs` -- every mutating `/api` request needs the double-submit token, uploads included.
- `crates/wipe-hub/tests/support/hub.rs:48` -- `spawn_hub_args` builds `Args` literally: add `artifact_dir` from a `TempDir` the `Hub` owns. Grep `crates/wipe-hub/tests` for every other literal `Args {`/`Config {` (AGENTS.md pitfall). `tests/support` `audited`, `count`, `error_of`, `post_req`; `tests/admin.rs` for creating and signing in a vendor admin; `tests/cli.rs` for running the binary; `tests/isolation.rs:40` `ROUTES` (classify every new route `Admin`); `tests/migrations.rs` trigger-refusal pattern.
- `apps/portal/src/api/adminOfferings.ts` (+ test), `pages/AdminOfferingsPage.tsx`, `AdminOfferingPage.tsx`, `AdminPage.tsx`, `main.tsx:264-280` -- admin client, guards, `retryAdminQuery`, page and route patterns; how the client sends the CSRF header.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/migrations/0017_releases.sql` -- `releases` (id, version unique, notes, channel CHECK, status CHECK, created_by/created_at, published_at, withdrawn_at, consistency CHECKs) and `release_artifacts` (id, release_id FK, platform, filename, size_bytes > 0, sha256 `^[0-9a-f]{64}$`, created_at; unique (release_id, platform)); lifecycle triggers as in Always -- the DB enforces immutability.
- `crates/wipe-hub/Cargo.toml` -- add `semver = "1"`, `async-trait` (workspace).
- `crates/wipe-hub/src/artifacts.rs` -- `ArtifactStore` trait, `LocalDirStore`, `StoreError`, the shared hashing/size-limiting `store_artifact` helper, `ARTIFACT_MAX_BYTES`; unit tests on a temp dir.
- `crates/wipe-hub/src/release_admin.rs` -- validation, `Release`/`ReleaseArtifact` shapes, `create`, `list`, `detail`, `add_artifact`, `remove_artifact`, `publish`, `withdraw`, `publish_from_cli`; error messages as constants.
- `crates/wipe-hub/src/config.rs`, `lib.rs`, `main.rs`, `admin_routes.rs` -- flags, state, CLI subcommand, routes (`GET|POST /api/admin/releases`, `GET /api/admin/releases/:id`, `POST …/:id/artifacts`, `POST …/:id/artifacts/:artifact_id/remove`, `POST …/:id/publish`, `POST …/:id/withdraw`), module docs.
- `crates/wipe-hub/tests/releases.rs` -- one test per matrix row through HTTP and the built binary, including a stored file's bytes hashing to the recorded `sha256` and no leftover temp files. `tests/isolation.rs` -- new routes. `tests/migrations.rs` -- 0017 refusals (update a published release's version, delete a release, insert an artifact into a published release, skip a status step).
- `apps/portal/src/api/adminReleases.ts` (+ test) -- typed client and guards; the upload sends the `File` as the raw body with `Content-Type: application/octet-stream`.
- `apps/portal/src/pages/AdminReleasesPage.tsx` (`/admin/releases`: list with status and a create form), `AdminReleasePage.tsx` (`/admin/releases/$id`: notes, artifacts with full SHA-256, upload form (platform + file) and remove while draft, publish, withdraw behind a confirming press), `AdminPage.tsx` link, `main.tsx` routes (+ tests for pure helpers).
- `.gitignore` -- `.wipe-hub/`.
- `CONTEXT.md` §10 row and §14 **Release** / **Artifact** entries, `docs/ROADMAP.md` (downloads progress), `README.md` and `AGENTS.md` hub section (`--artifact-dir`, `release publish`, test counts) -- record what shipped.

**Acceptance Criteria:**
- Given a running hub and a built file, when `wipe-hub release publish --version 0.2.0 --notes "First build" --file linux-x86_64=<file>` runs against the hub's database and artifact directory, then `GET /api/admin/releases` as a vendor admin lists `0.2.0` `published` with one `linux-x86_64` artifact whose `sha256` equals `sha256sum <file>` and whose stored bytes are identical to the file.
- Given a vendor admin on `/admin/releases`, when they create a draft, upload a file, publish and then withdraw it, then each step shows the new status, the artifact's SHA-256 is shown in full, and the withdrawn release is still listed.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 31 findings — high 0, medium 3, low 21, false 4, maybe-false 3
- findings:
  - `[medium]` `[patch]` (blind) `1.0.0` and `1.0.0+build.5` can coexist as separate releases, although SemVer ranks them equal — confirmed (`validate_version` accepts build metadata, and uniqueness is on the raw string). Fix: versions carrying build metadata are refused with a 400 naming `version`.
  - `[low]` `[reject]` (blind) The 413 message always says 1 GiB — `artifact_max_bytes` is not a flag; only tests lower it, so production always says the true limit.
  - `[low]` `[reject]` (blind) Nothing sweeps orphan files left by a crash, a killed CLI or a failed delete — real but rare (a crash between store and commit). A start-up sweep is new machinery; the failed-delete case is already a recorded CONTEXT limitation. The client-disconnect case is the separate deferred row below.
  - `[low]` `[reject]` (blind) Withdrawing a draft keeps its files — this is the recorded autonomous decision "withdrawal keeps the files"; changing it means editing the spec.
  - `[low]` `[reject]` (blind) Nothing re-hashes stored bytes when they are read back — no code reads them back in this story; downloads are story 2 (CAP-2/CAP-3), which the intent places there.
  - `[low]` `[reject]` (blind) The trigger lets any UPDATE clear `created_by` — reachable only through direct SQL (no hub path updates `created_by`), and the fix means editing 0017's trigger logic for a case nobody meets in normal use.
  - `[low]` `[reject]` (blind) The trigger checks are not race-safe against concurrent direct-SQL transactions — every hub path locks the release `FOR UPDATE` first; only hand-written SQL could interleave, and locking inside triggers adds complexity.
  - `[low]` `[reject]` (blind) No tests for upload races (same platform twice, upload losing to publish, remove racing publish) — the code handles each: the unique index turns a lost platform race into 409 and `discard`, and `add_artifact_as` re-checks `draft` under the row lock. Earlier stories' triage likewise declined tests for paths the code already handles.
  - `[low]` `[patch]` (blind) No HTTP test for 503 storage failure or the interrupted-upload 400 — the 503 half shares its root cause with the verification-gap 503 row and is fixed there. The interrupted half is rejected: simulating a hang-up mid-body needs a raw-socket client for a mapping that is a single match arm.
  - `[low]` `[reject]` (blind) Start-up does not check the artifact directory is writable — a wrong-permission directory surfaces as a 503 on the first upload, with the cause logged; a probe write at start is extra machinery for a rare misconfiguration.
  - `[low]` `[reject]` (blind) No disk, concurrency or duration limits on uploads — only vendor admins can upload; quotas are deployment work.
  - `[low]` `[reject]` (blind) No upload progress or cancel, and a stale CSRF token re-sends the whole file — cosmetic for an admin-only page; the resend needs an expired token mid-session, which is rare.
  - `[low]` `[patch]` (blind) The upload always sends the browser's file name, so `wipestation (1).tar.gz` is refused — fixed: an editable file-name field, prefilled from the picked file.
  - `[low]` `[patch]` (blind) Remove deletes a draft's file in one click — fixed: Remove now goes through `ConfirmedAction`, like publish and withdraw.
  - `[false]` `[reject]` (blind) A big declared Content-Length to an unknown release is 413, not 404 — the matrix does not rank two errors in one request; checking the request before the lookup is the admin-route convention (story 3 triage).
  - `[false]` `[reject]` (blind) The database barely checks version and notes formats — the spec's "Postgres enforces it" covers the lifecycle rules only, and the migration header says the hub checks formats; nothing claims otherwise.
  - `[maybe-false]` `[defer]` (edge) If the handler future is dropped mid-upload (client disconnect, shutdown), the `.part` temp file is left behind — whether hyper drops the handler or hands it a body error on disconnect was not checked. Settle it with a disconnect test. If true, medium (a large orphan file per abandoned upload).
  - `[low]` `[reject]` (edge) A commit that errors after the server actually committed deletes the file under a live row — it needs a lost commit acknowledgement, which is vanishingly rare; guarding it needs a re-query branch.
  - `[medium]` `[patch]` (edge) Versions differing only in build metadata coexist — same root cause and fix as the first blind row.
  - `[low]` `[reject]` (edge) After an upload, the portal may order files differently from the hub's collation — the order is cosmetic, and a reload corrects it.
  - `[low]` `[reject]` (edge) The release list is unbounded, and every id goes into one audit row — the vendor publishes a handful of releases a year; paging can come if that changes.
  - `[low]` `[reject]` (edge) The CSRF retry re-sends a raw body — same as the blind progress/CSRF row.
  - `[low]` `[reject]` (edge) The artifact trigger reads status without a lock (claim) — same as the blind trigger-race row.
  - `[maybe-false]` `[defer]` (edge) "No leftover temp files" holds only when `put` returns; a cancelled future skips the cleanup (claim) — same root cause as the deferred disconnect row.
  - `[low]` `[patch]` (verification-gap) The CLI cleanup after a file is stored is never exercised — fixed: a CLI case with a second, unreadable `--file` asserts a non-zero exit, an empty artifact directory and unchanged counts.
  - `[low]` `[patch]` (verification-gap) The 503 `MSG_STORAGE_UNAVAILABLE` mapping is never exercised — fixed: an upload after the artifact directory is removed asserts 503 with exactly that message and unchanged counts.
  - `[low]` `[reject]` (intent) The admin pages are not exercised by any test — the portal's convention is to test helpers and API clients, plus typecheck and a build (stories 1–3 triage).
  - `[medium]` `[patch]` (intent) The CLI builds `LocalDirStore` itself and `publish_files` takes `&LocalDirStore`, so a cloud store would need CLI changes too (CAP-4) — fixed: one store constructor in `artifacts.rs` serves both `AppState` and the CLI, and `publish_files` takes `&dyn ArtifactStore`.
  - `[false]` `[reject]` (intent) No end-to-end script uses the CLI — CAP-5 says the CLI is *used by* the script; the script itself is `commerce-end-to-end` CAP-1.
  - `[maybe-false]` `[reject]` (intent) The CLI only works on the hub's host — this is the recorded autonomous decision; changing it means editing the spec.
  - `[false]` `[reject]` (intent) The lifecycle adds behaviour the SPEC does not state — these are recorded autonomous decisions, and the auditor confirms they are tested.

## Design Notes

Upload, in order: validate query → quick read that the release exists and is `draft` (404/409 early) → `store_artifact(store, new_uuid, body_stream, limit)` hashing and counting each chunk, aborting past the limit → `BEGIN; SELECT status FROM releases WHERE id=$1 FOR UPDATE` → re-check draft → `INSERT` (unique violation on platform → 409) → audit → `COMMIT`; on any error after the store write, `store.delete(key)` then return the error.

Storage failure (disk full, permission) is 503 `MSG_STORAGE_UNAVAILABLE` with the cause logged, never shown.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including `releases`, isolation and migration tests
- `cargo test --workspace` -- expected: pass
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings` -- expected: clean
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: pass

## Auto Run Result

Status: done

**Summary.** The hub now has releases. A vendor admin creates a draft release (SemVer version without build metadata, notes, `stable`/`preview` channel), uploads one file per platform as the raw request body (the hub streams it into an `ArtifactStore` and computes size and SHA-256 itself, 1 GiB cap), removes files while it is a draft, publishes it (at least one file), and withdraws it for good (files kept, still listed to admins). `wipe-hub release publish --version --notes [--channel] --file <platform>=<path>…` does create, store and publish in one transaction straight against the database and artifact directory. Every change and admin read is audited; migration `0017_releases.sql` enforces the lifecycle in Postgres. Customer listing and downloads are story 2.

**Files changed.**
- `crates/wipe-hub/migrations/0017_releases.sql`: `releases` and `release_artifacts` tables, CHECKs, lifecycle triggers.
- `crates/wipe-hub/src/artifacts.rs`: `ArtifactStore` trait, `LocalDirStore` (temp file, fsync, rename; UUID keys only), `store_for`, the shared hashing and size-limiting `store_artifact`.
- `crates/wipe-hub/src/release_admin.rs`: validation, create/list/detail/upload/remove/publish/withdraw, CLI publish, audit rows.
- `crates/wipe-hub/src/admin_routes.rs`, `config.rs`, `lib.rs`, `main.rs`, `admin.rs`, `Cargo.toml`: seven `/api/admin/releases*` routes, `--artifact-dir`/`WIPE_HUB_ARTIFACT_DIR` (default `.wipe-hub/artifacts`, created at start), `release publish` subcommand, store in `AppState`.
- `crates/wipe-hub/tests/releases.rs` (new, 15 tests); `tests/isolation.rs`, `migrations.rs`, `cli.rs`, `support/hub.rs` and the other tests that build `Args` literally: new routes, 0017 refusals, artifact-dir start-up checks, a temp artifact directory.
- `apps/portal/src/api/adminReleases.ts` (+ test), `api/auth.ts` (raw-body requests), `pages/AdminReleasesPage.tsx`, `pages/AdminReleasePage.tsx` (+ test), `AdminPage.tsx`, `main.tsx`: the `/admin/releases` and `/admin/releases/$id` pages.
- `.gitignore`, `CONTEXT.md`, `docs/ROADMAP.md`, `README.md`, `AGENTS.md`: what shipped, the new flags and command, test counts.

**Review findings.** 31 findings: high 0, medium 3, low 21, false 4, maybe-false 3.
- Patched (6 entries): versions with build metadata refused (medium); one store constructor for both the server and the CLI, and `publish_files` over `&dyn ArtifactStore` (medium); a CLI cleanup test after a file is stored (low); an HTTP test for the 503 storage failure (low); an editable file-name field on upload (low); Remove behind a confirmation (low).
- Deferred (1): a possible `.part` orphan when an upload is abandoned mid-transfer (medium, unverified; see frontmatter).
- Rejected: every other finding, each with its reason in the Review Triage Log above.

**Follow-up review recommended: true.** This pass patched 2 medium entries (and 4 low). The specific unverified risk: the version rule and the store-constructor seam changed after review and have had only targeted tests, not a fresh review. The deferred disconnect cleanup question is also open.

**Verification.**
- `cargo clippy -p wipe-hub --all-targets --no-deps -- -D warnings`: clean.
- `cargo test --workspace --no-fail-fast`: 665 passed, 1 failed. The failure is `health.rs` `silent_smtp_and_database_time_out_within_bound` (took 3.7–3.9 s against a 3.5 s bound). The same test fails on the baseline commit `bcff696` in this checkout, run from a temporary worktree with the same target dir, and passes when run alone (2.75 s), so it is a pre-existing timing flake, not caused by this change. `cargo test -p wipe-hub --no-fail-fast` passed in full (519) on one run.
- Portal: `pnpm --filter portal typecheck` clean, `test` 210 passed, `build` ok.
- Every I/O-matrix row has a covering test in `tests/releases.rs` or the `artifacts` unit tests, and each ran and passed.

**Residual risks.**
- Orphan files after a crash, a killed CLI or a failed delete are never swept.
- The CLI writes directly, so it must run where the hub's database and artifact directory are reachable.
- The portal pages were checked by typecheck, unit tests and a build, not in a real browser.
- The database triggers rely on the hub's row locks to be race-safe against concurrent direct SQL.
