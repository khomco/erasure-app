---
title: 'Commerce demo script'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '05e222a3c656fc925030dbe0ffe49c796c3b6b22'
review_loop_iteration: 0
followup_review_recommended: true
context: ['{project-root}/scripts/demo.sh', '{project-root}/docs/adr/0006-web-presence-and-local-first-verification.md']
warnings: ['oversized']
deferred:
  - summary: >-
      No automated check runs scripts/commerce-demo.sh, so a change to the station or hub output it matches can break the demo silently.
    evidence: |-
      The script matches exact wipestation output ("signing key public id: ", "PUBLISH THIS: ", "licence: <id> for <org> (", "LICENSED", "  customer: <org> (") and hub JSON shapes. No cargo or pnpm test runs it or pins those lines (crates/wipe-cli has no tests/ directory), and there is no CI. One cheap partial fix is a wipe-cli integration test that pins the verify-cert LICENSED/customer lines.
    location: >-
      scripts/commerce-demo.sh
    severity: medium
---

<intent-contract>

## Intent

**Problem:** ADR-0006 §3 requires one command that proves the commercial platform works end to end on a laptop, the way `scripts/demo.sh` proves the station. No such script exists, and no test runs a real `wipestation` binary against a hub-issued licence.

**Approach:** Add `scripts/commerce-demo.sh` (bash, curl, jq). It builds `wipe-hub` and `wipestation`, starts a hub with the Fake payment provider on its own fresh database, and starts a station. It then walks the whole customer journey over the hub's real HTTP API and CLIs, in the order CAP-1 lists. It finishes by verifying the certificate offline with `wipestation verify-cert` against the vendor root, and exits non-zero with a `FAILED: <step>: <reason>` message at the first failure.

## Boundaries & Constraints

**Always:**
- Only localhost: the hub, the station, compose Postgres (`127.0.0.1:55432`) and Mailpit (`127.0.0.1:8025`). No third-party account and no outbound network.
- Drive the hub only through its public surfaces: HTTP with a cookie jar and the double-submit CSRF header `X-CSRF-Token`, plus the `wipe-hub admin create` and `wipe-hub release publish` CLIs. Never write SQL to shortcut a step.
- Read emailed tokens from Mailpit, isolated by unique per-run addresses. Never delete Mailpit messages.
- Use a run-scoped temporary work directory for the vendor root seed, the station key, the artifact dir, logs, the licence and the certificate. Pass every path explicitly, by flag or `WIPE_HUB_*` env var, so the hub and the CLIs share one artifact dir.
- Build before running (`cargo build -p wipe-hub -p wipe-cli`) so stale binaries are never exercised.
- Clean up on every exit: kill both processes and drop the run database. On success, delete the work dir. On failure, keep it, print its path, and tail both logs.
- Before starting anything, check the preconditions and fail with clear messages: `jq`, `curl` and `docker` are present; Postgres and Mailpit are reachable (otherwise "run `docker compose up -d`"); and nothing already answers on the hub or station port.

**Never:**
- Edit hub or station Rust code, or add a runtime licence-install endpoint. Installing a licence is the existing offline ADR-0005 path: restart the station with `--license` and `--vendor-root-b64`.
- Use the shared `wipehub` dev database.
- Write CI configuration or deployment steps (SPEC non-goals).
- Make a hardware-sanitization claim in script output (ROADMAP §0.1). The station runs the mock backend, and the closing summary says so.
- Do story 2 (CAP-2/CAP-3) docs work beyond the minimal README and CONTEXT entries below.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Happy path | `docker compose up -d`, free ports | Each step prints `==> <step>`. The run ends with a summary naming the customer, licence id and release version. Exit 0. | — |
| Compose down | Postgres or Mailpit unreachable | `FAILED: preconditions: … run docker compose up -d`. Exit 1, and the hub never starts. | precondition check |
| Port taken | Something already answers on the hub or station port | `FAILED: … port N already in use`. Exit 1, with an `lsof` hint. | precondition check |
| Any HTTP step fails | Non-2xx or unexpected JSON | `FAILED: <step>: <method path> returned <code>: <body>`. Exit 1, with the work dir kept. | `ERR` trap and `fail` helper |
| Licence not accepted | The station does not log `licence: <id> for <org>` after the restart | `FAILED: install licence: …` | log check |
| Evaluation cert | `.certificate.evaluation` is true, or the customer name differs | `FAILED: verify certificate: …` | jq assertion |
| Wrong root | Verify the cert against a freshly generated, unrelated root | `verify-cert` must exit non-zero. If it passes, the script fails. | negative check |

## Autonomous decisions

- **Licence install method.** CAP-1 says the script "downloads and installs the licence" on a station, but a running station cannot load a licence: there is no install endpoint (a hot-swap is planned in `.scratch/licence-visibility`). The options were to add an install endpoint (widens scope, touches the station API) or to restart the station with the downloaded file, which is the documented ADR-0005 offline activation path. **Chose the restart**, keeping the same `--key-path` and `--station-id` so the instance key and machine binding hold. That is the existing behaviour, and the safe one.
- **Database isolation.** Running against the shared `wipehub` dev database would mix demo orders into a developer's data, and repeated runs would hit the per-IP email-send throttle (20 per hour). **Chose a fresh `wipehub_demo_<run>` database**, created and dropped (`WITH (FORCE)`) through `docker compose exec -T postgres psql`. `psql` is not assumed on the host, and the compose file is already required. This matches how hub tests isolate (`TestDb`).
- **Default ports.** Defaults are hub `127.0.0.1:7910` and station `127.0.0.1:7890`, overridable with `HUB_PORT` and `STATION_PORT`, so the script does not collide with a dev hub on 7900 or `demo.sh` on 7878/7879. `--public-base-url` follows the hub port, because Fake pay requires a local base URL.
- **What gets released.** The release artifact is the freshly built `wipestation` binary, on platform `$(uname -s)-$(uname -m)` lower-cased, with version `0.0.0-demo.<epoch>`. The checksum check therefore covers a real build, not a dummy file.

</intent-contract>

## Code Map

- `scripts/demo.sh` -- house style to mirror: `set -euo pipefail`, repo-root `cd`, source `~/.cargo/env`, `cleanup` trap, a readiness loop with the load-bearing `|| true`, the job create/start/poll flow (`dev-nvme-0`, poll `.state.state` until `erased`).
- `crates/wipe-hub/src/config.rs:20-181` -- `serve` flags and env: `--addr`, `--database-url`, `--payment-provider fake`, `--vendor-root-seed` (32 raw bytes, never generated), `--public-base-url`, `--artifact-dir`. `admin create --email --name`; `release publish --version --notes --file P=PATH` publishes immediately.
- `crates/wipe-hub/src/csrf.rs:42-136` -- `GET /api/auth/csrf` returns `{csrf_token}` and sets the `wipehub_csrf` cookie. Non-GET requests need `X-CSRF-Token`. `Content-Type: application/json` is required (otherwise 415). Cookies are not `Secure` on `127.0.0.1`; use one host consistently.
- `crates/wipe-hub/tests/support/hub.rs:529-560` -- `bootstrap_admin`: CLI create, then the Mailpit reset email (`/reset-password?token=<43 b64url>`), then `POST /api/auth/password-reset/complete {token,password}` (this verifies the email), then `POST /api/auth/sign-in`.
- `crates/wipe-hub/tests/offering_admin.rs:34-49` -- offering body. `POST /api/admin/offerings` (`deny_unknown_fields`; `quota_erasures` is required, `null` means unlimited; `term_months` must be 12 for `annual`), then `POST /api/admin/offerings/:id/publish`.
- `crates/wipe-hub/src/auth.rs:382-479` -- sign-up `{email,password(>=12),name,organization_name}` returns 201 and signs the user in. The mail contains `/verify-email?token=`; `POST /api/auth/verify-email {token}`.
- `crates/wipe-hub/tests/stations.rs:63-145` -- `buy`: `POST /api/checkout {offering_id}` returns `checkout_url ...?session=fake_cs_…`; `POST /api/payments/fake/sessions/:s/pay` is synchronous and returns `paid`; `GET /api/entitlements` matched by `order_id`. `register_machine`: `POST /api/stations {entitlement_id, instance_key_id, station_id}` returns `.licence.id`.
- `crates/wipe-hub/tests/downloads.rs:979-1023` -- CLI publish, then `GET /api/downloads` (`artifacts[].sha256` lowercase hex), then `POST /api/downloads/:id/link` (relative `url`, 300 s), then GET the file (no HEAD) and compare SHA-256.
- `crates/wipe-hub/src/station_routes.rs:199-247` -- `GET /api/licences/:id/download` returns the licence JSON the station loads unchanged. `GET /api/licensing/root` returns `.root.public_key_b64`.
- `crates/wipe-cli/src/main.rs:36-116,413-420,724-754,760-784` -- station `serve` flags (`--key-path`, `--station-id`, `--no-fleet`, `--fast`, `--license`, `--vendor-root-b64`). Startup log line `licence: <id> for <customer_name> (<id>)`. `license new-root --out` prints `PUBLISH THIS: <b64>`. `GET /api/public_key` returns `{public_key_id, public_key_b64}`.
- `crates/wipe-cli/src/main.rs:600-676` -- `verify-cert CERT --public-key-b64 PK --vendor-root-b64 ROOT --require-licensed`. It prints `LICENSED — …` and `  customer: <name> (<id>)`; an invalid chain exits 1.
- `crates/wipe-license/src/lib.rs:63-69` -- the machine fingerprint comes from `station_id`. The hub `station_id` must equal the station's `--station-id`, or the cert falls back to evaluation.
- Cert jq paths: `.certificate.evaluation`, `.attestation.license.body.entitlements.customer_name`, `.attestation.license.body.license_id`.

## Tasks & Acceptance

**Execution:**
- `scripts/commerce-demo.sh` -- new executable script. Helpers: `step` (prints `==> <step>` and records the current step for the `ERR` trap); `fail` (prints `FAILED: <step>: <msg>`, exit 1); `hub_get` and `hub_post` (per-actor cookie jar, CSRF header, JSON content type, `-w` status capture, `fail` on an unexpected status with the body); `mail_token` (polls Mailpit `GET /api/v1/search?query=to:"<addr>"` and then `/api/v1/message/<ID>` `.Text`, and extracts `<path>?token=([A-Za-z0-9_-]{43})`); `sha256_of` (`shasum -a 256` or `sha256sum`). Steps, in order:
  1. Check preconditions.
  2. Build.
  3. Create the run DB.
  4. Generate the vendor root (`license new-root`).
  5. Start the hub (`--payment-provider fake`, seed, run DB, artifact dir) and wait for `/api/health` 200 while the process is alive.
  6. Start the station unlicensed and read its key.
  7. Bootstrap the vendor admin.
  8. Create and publish the offering, and assert it appears in public `GET /api/offerings`.
  9. Sign up the customer.
  10. Verify the email via Mailpit.
  11. Check out and pay with Fake (assert `provider == "fake"` and `status == "paid"`).
  12. Publish the release with the CLI.
  13. Download the release as the customer and compare SHA-256.
  14. Register the station and download the licence.
  15. Fetch the root from `/api/licensing/root` and assert it equals the `new-root` output.
  16. Restart the station with the licence, assert the log line names the organization, and assert the key id is unchanged.
  17. Run the erasure and poll for the certificate.
  18. Assert with jq that the cert is not evaluation and its customer name equals the organization.
  19. Run `verify-cert --require-licensed` and assert the output has `LICENSED` and `customer: <org>`.
  20. Run the negative check against a wrong root.
  21. Print the summary.
- `README.md` -- next to the `./scripts/demo.sh` line (~:80), add `./scripts/commerce-demo.sh` with its `docker compose up -d` prerequisite -- AGENTS.md requires docs in the same commit (story 2 writes the full developer guide).
- `CONTEXT.md` §10.2 -- append a row "Commerce end-to-end demo — `scripts/commerce-demo.sh`" naming ADR-0006 §3, with as-built gaps: the licence is installed by station restart (no hot install), and it runs on the mock backend only.

**Acceptance Criteria:**
- Given `docker compose up -d` and free ports, when `scripts/commerce-demo.sh` runs, then it exits 0. Its output shows each CAP-1 step in order, and it ends with `verify-cert` printing `LICENSED` and `customer: <the organization name the script signed up>`.
- Given a successful run, when it finishes, then no `wipe-hub` or `wipestation` process it started is still running, no `wipehub_demo_*` database it created remains, and the shared `wipehub` database is untouched.
- Given Postgres is stopped (or `HUB_PORT` points at a port already serving), when the script runs, then it exits non-zero with a `FAILED:` line naming the cause before it starts a hub.
- Given the script runs twice in a row, when the second run starts, then it passes too: there are no collisions on emails, release version or database.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 24 findings — high 0, medium 4, low 11, false 9, maybe-false 0
- findings:
  - `medium` `patch` A single readiness probe after a fresh `docker compose up -d` can fail while Postgres or Mailpit is still starting — fixed: a `wait_for` helper retries the PG port, `pg_isready`, Mailpit info and SMTP for up to 30 s. Postgres restarted mid-run, and the run waited and passed.
  - `low` `patch` The CONTEXT row did not name the ADR-0006 §3 clause ("install it on a running station") that the restart install departs from — fixed: the gaps column now names the clause and the planned hot install in `.scratch/licence-visibility`.
  - `false` `reject` ADR-0006 §3's "test CI will later simply run" is unmet — the ADR lists the end-to-end script itself as the artifact, and a future CI can run the script as is.
  - `low` `patch` The CONTEXT row lacked the story and CAP reference that neighbouring rows carry — fixed: "commerce-end-to-end story 1 (CAP-1) / ADR-0006 §3".
  - `low` `patch` The wrong-root check accepted any non-zero exit — fixed: it now requires `attestation chain INVALID` in the output.
  - `medium` `patch` Hard-coded `target/debug` paths can run stale binaries when `CARGO_TARGET_DIR` is set — fixed: binary paths come from `cargo metadata .target_directory`.
  - `low` `patch` The licence file was rebuilt through a shell variable, which strips trailing newlines — fixed: `cp` of the raw response body.
  - `low` `reject` Ctrl-C is reported as a step failure and the work dir is kept — rare in everyday use, the fix adds signal handlers, and keeping the work dir on abort is harmless.
  - `false` `reject` The download audit row is not checked — CAP-1 asks only to download and check the SHA-256; the audit is covered by the hub's own `tests/downloads.rs`.
  - `medium` `patch` (Edge) Stale binaries under a custom target dir — grouped with the `CARGO_TARGET_DIR` row above; same fix.
  - `low` `patch` (Edge) An interrupt between `CREATE DATABASE` and `DB_CREATED=1` leaks the database — fixed: the flag is set before the create (the drop is `IF EXISTS`).
  - `low` `reject` (Edge) Equal or non-numeric `HUB_PORT`/`STATION_PORT` — an unlikely misuse; the fix adds a guard, and a start-up failure is still reported through `FAILED:`.
  - `low` `patch` (Edge) Inherited `WIPESTATION_*`/`WIPE_HUB_*` env vars could make the unlicensed station load a licence, or stop the hub from starting — fixed: they are `unset` at the top. A run with hostile values set passed.
  - `low` `reject` (Edge) No `--max-time` on curl, so a peer that accepts but never answers hangs the run — all peers are local processes the script just started or compose services; unlikely in everyday use, and the fix touches every curl call.
  - `low` `reject` (Edge) Docker without the compose plugin gets a misleading "run docker compose up -d" — the compose file is already a documented prerequisite; unlikely.
  - `low` `patch` (Edge) No errtrace, so failures inside helpers were not named — fixed: `set -Eeuo pipefail`, with the ERR handler limited to the main shell. An injected failure inside `mail_token` reported the line and command.
  - `false` `reject` (Edge, claim) "Stale binaries are never exercised" — true after the `CARGO_TARGET_DIR` patch; grouped with that row.
  - `medium` `defer` (Verification gap) No automated check runs the script or pins the output lines it matches — filed disposition defer: there is no CI, and the SPEC excludes CI configuration. Recorded in frontmatter `deferred`.
  - `false` `reject` (Intent) Install by restart vs the ADR's "running station" — ADR-0005/0007 §5 define install as the offline file path; this is recorded as an autonomous decision, and the CONTEXT wording was patched (row 2).
  - `false` `reject` (Intent) The diff carries no evidence of a passing run — the runs were performed and are recorded in the Auto Run Result.
  - `false` `reject` (Intent) The release is published by CLI, not the admin HTTP routes — CAP-1 does not name the actor, AGENTS.md documents `release publish` as the path, and the contract allows it.
  - `false` `reject` (Intent) The station public key passed to verify-cert comes from the live station — the licence binds that key id and the chain is verified to the vendor root, so the root does vouch for it.
  - `false` `reject` (Intent) Mock erasure surface — the auditor flagged it as consistent with the intent, not a divergence.
  - `false` `reject` (Intent) Docs limited to README/CONTEXT — intentional; CAP-2/CAP-3 are story 2.

## Design Notes

The station is started twice. The first start mints the key at `--key-path` so the hub can register its `instance_key_id`; the second start loads the licence. Both starts use the same `--station-id demo-bench-1` and `--no-fleet`. The certificate is fetched by polling `GET /api/jobs/:id/certificate` until it returns 200, rather than by sleeping, because signing happens asynchronously after `erased`.

## Verification

**Commands:**
- `bash -n scripts/commerce-demo.sh` -- expected: no syntax errors (and run `shellcheck` on it if installed: no errors).
- `docker compose up -d && scripts/commerce-demo.sh` -- expected: exit 0 with `LICENSED` and the customer name.
- `scripts/commerce-demo.sh; scripts/commerce-demo.sh` -- expected: both exit 0.
- `HUB_PORT=55432 scripts/commerce-demo.sh` -- expected: non-zero exit with a `FAILED:` port-in-use message.
- `docker compose exec -T postgres psql -U wipehub -d wipehub -Atc "select datname from pg_database where datname like 'wipehub_demo_%'"` after runs -- expected: empty.

## Auto Run Result

Status: done

**Summary.** Added `scripts/commerce-demo.sh`, one command that proves the commerce journey on a laptop (ADR-0006 §3, CAP-1). It builds the binaries, then starts a hub with the Fake payment provider on a throwaway `wipehub_demo_<run>` database and a station on the mock backend. From there it creates a vendor admin and publishes an offering, signs up a customer and verifies the email through Mailpit, then buys the offering. It publishes a release and downloads it, checking the SHA-256. It registers the station's instance key, downloads the licence and installs it by restarting the station (the ADR-0005 offline path), then runs an erasure. It verifies the certificate offline against the vendor root: `LICENSED`, not evaluation, naming the customer. As a negative check, it confirms an unrelated root is rejected. Every exit cleans up processes and the database; a failed step prints `FAILED: <step>: <reason>` and exits 1.

**Files changed**
- `scripts/commerce-demo.sh` — new end-to-end commerce demo script.
- `README.md` — a run line next to `./scripts/demo.sh`.
- `CONTEXT.md` §10.2 — a row for the demo, with as-built gaps: restart install, mock backend only, no CI.

**Review findings:** 24 total.
- **Patches, 8 entries (2 medium, 6 low):** readiness retries; cargo target dir; strict wrong-root check; raw licence bytes; DB flag ordering; unset inherited env; errtrace; CONTEXT row reference and ADR wording.
- **Deferred, 1 (medium):** no automated run of the script.
- **Rejected:** 9 false and 4 low, with reasons in the triage log.

**Follow-up review recommended: true.** Two medium entries were patched (readiness retries and target-dir resolution). The specific unverified risk is the `wait_for` and `set -E` changes on Linux bash 4/5: they were verified only on macOS bash 3.2, and `BASH_SUBSHELL` and ERR-trap behaviour differ between those versions.

**Verification performed**
- `bash -n scripts/commerce-demo.sh`: OK. `shellcheck` is not installed, so it was not run.
- Happy path: about seven full runs across implementation, my own runs and the patch round, all exit 0. Two back-to-back runs passed after the patches, ending with `LICENSED`, `customer: <org>`, and the wrong root rejected with `attestation chain INVALID`.
- `HUB_PORT=55432`: exit 1 with `FAILED: check preconditions: port 55432 already in use …`.
- Mailpit stopped: exit 1 with `FAILED: check preconditions: Mailpit is not reachable … run docker compose up -d`. Postgres stopped (implementation agent): the matching message.
- Matrix rows checked by failure injection on temporary copies of the script (since deleted):
  - Checkout with a bad body: `FAILED: check out and pay …: POST /api/checkout returned 400 …`.
  - No `--license` on restart: `FAILED: install licence …`.
  - Registration with the wrong `station_id`: `FAILED: check the certificate …: certificate is marked evaluation`.
  - In each case the work dir was kept and the logs were tailed.
- After the runs: no `wipehub_demo_*` databases, no leftover `wipe-hub serve` or `wipestation serve` processes, and no work dirs.
- No Rust or TypeScript changed, so `cargo test` and pnpm tests were not run.

**Residual risks**
- Matrix coverage comes from manual and injected runs, not checked-in tests (deferred).
- The licence is installed by restart until the hot-install endpoint (`.scratch/licence-visibility`) lands.
- The script's output includes `verify-cert`'s mock "method" line. The closing summary states it was the mock backend and makes no hardware claim.
