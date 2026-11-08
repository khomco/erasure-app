---
title: 'Local platform docs and development banner'
type: 'feature'
created: '2026-11-08'
status: 'done'
baseline_revision: '990511ff672cd34f89ba060277eb5659384e8915'
review_loop_iteration: 0
followup_review_recommended: true
context: ['{project-root}/docs/adr/0006-web-presence-and-local-first-verification.md', '{project-root}/docs/adr/0007-hub-accounts-payments-persistence.md']
warnings: ['oversized']
deferred: []
---

<intent-contract>

## Intent

**Problem:** A new developer has no single guide to bringing up the commercial platform locally: compose, hub, portal, marketing site, optional Stripe test mode and the commerce demo are scattered across README and AGENTS.md (CAP-2). And nothing in the portal tells anyone that a hub is running the Fake payment provider or a file-held vendor root, so a local or staging stack can be mistaken for production (CAP-3).

**Approach:** Add `GET /api/deployment` to the hub. It reports the payment provider, the vendor-root custody and a `development` flag that the hub computes in Rust. The portal's root layout shows a persistent, non-dismissible banner whenever `development` is true and lists the reasons. Write `docs/agents/local-stack.md` as the step-by-step local guide, link it from README, and list the commands in AGENTS.md.

## Boundaries & Constraints

**Always:**
- The rule "development = provider is Fake OR vendor root is file-held" lives in the hub alone. The portal renders the hub's `development` flag and `reasons`; it never re-derives them.
- `GET /api/deployment` needs no session. It exposes configuration only (no account data, no paths, no key ids), and it is classified `Anonymous` in `tests/isolation.rs`.
- The banner sits in the root layout, so it is on every portal page, signed in or not. It has no close control.
- Docs change in the same commit as the code: README (`wipe-hub` and `apps/portal` rows, plus the quick start), CONTEXT.md §10.2 row, AGENTS.md.
- `GET /api/health` keeps its exact body (`tests/health.rs` pins it).

**Never:**
- Add deployment or CI configuration (SPEC non-goals).
- Make a hardware-sanitization claim in docs (ROADMAP §0.1). The guide says the station runs the mock backend.
- Change the Fake/Stripe or vendor-root behaviour itself, or the startup warnings.
- Put the banner on `sites/www` or `apps/desktop`: CAP-3 names the portal.

## I/O & Edge-Case Matrix

| Scenario | Hub config | `GET /api/deployment` body | Portal |
|----------|-----------|---------------------------|--------|
| Default local | fake, no seed | `{"development":true,"payment_provider":"fake","vendor_root":"none","reasons":["fake_payment_provider"]}` | Banner: payments simulated |
| Demo | fake, seed | `development:true`, `vendor_root:"file"`, reasons `["fake_payment_provider","file_vendor_root"]` | Banner with both reasons |
| Stripe test, file root | stripe, seed | `development:true`, reasons `["file_vendor_root"]` | Banner: file-held root |
| Stripe, no root | stripe, no seed | `{"development":false,…,"reasons":[]}` | No banner |
| Hub unreachable or bad body | — | fetch fails or does not parse | No banner (nothing claimed); other pages show their own errors |

## Autonomous decisions

- **How the portal learns the hub's mode.** The portal is static, so it cannot see the hub's flags. The options were: extend `GET /api/health` (its body is pinned by tests and scripts, and it answers "is it up", not "what is it"); bake the mode into the portal build (wrong, because one build is served by hubs configured differently); or add a small public endpoint. **Chose `GET /api/deployment`**, computed in Rust. This follows the repo's rule that logic lives in Rust and is mirrored, not re-derived, in TypeScript.
- **No vendor root at all.** CAP-3 names Fake and "file-held", so a hub with Stripe and no root shows no banner (it cannot issue licences, but that is not a mistakable-for-production stack). It is reported as `vendor_root: "none"`, so a future custody kind (HSM or KMS) adds a value and does not change the rule.
- **Unknown mode.** If `/api/deployment` fails, the portal shows no banner rather than a guessed one. A hub that cannot answer is already broken on every page, and a false "development" banner on a production hub would mislead customers.
- **Banner wording.** It appears only on non-production hubs, so it is developer-facing, not customer copy: "Development hub — not for production use." followed by one line per reason: "Payments are simulated by the test payment provider; no money moves." and "The vendor root is held in a file, so licences signed here are for testing only."

</intent-contract>

## Code Map

- `crates/wipe-hub/src/config.rs:49-60,207,213` -- `PaymentProvider` (`as_str()` gives `fake`/`stripe`); `Config.vendor_root: Option<FileVendorRoot>` (the only custody kind today). `startup_warnings` (:416) already words both conditions; leave it unchanged.
- `crates/wipe-hub/src/health.rs` -- the pattern for a small public handler module (`State<AppState>`, serde enums `rename_all = "lowercase"`/`snake_case`).
- `crates/wipe-hub/src/lib.rs:101-104` -- the router: add `.route("/api/deployment", get(deployment::deployment))` beside `/api/health`; declare `mod deployment` (pub, like `health`).
- `crates/wipe-hub/tests/isolation.rs:40-45` -- the `ROUTES` table. An unclassified `/api` route fails the test, so add `("GET", "/api/deployment", Anonymous("hub configuration only; no account data"))`.
- `crates/wipe-hub/tests/support/hub.rs:50,109` -- `spawn_hub_args` (Fake by default, no seed) and `spawn_hub_with_root`; `tests/support/stripe_mock.rs:162` `spawn_stripe_hub(&StripeMock)` (Stripe, no seed; to add a seed, use `spawn_hub_args` with the mock's base and the Stripe secrets the way :165 does).
- `apps/portal/src/api/health.ts` + `health.test.ts` -- the pattern for a typed fetch, a guard and its vitest (node env, `src/**/*.test.ts` only, so test logic, not rendering).
- `apps/portal/src/main.tsx:67-80` -- `rootRoute` layout: render `<DevelopmentBanner />` above `<header>`.
- `README.md:48,50,53-88` -- the `wipe-hub` and `apps/portal` rows, and the quick start (add a pointer to the guide).
- `AGENTS.md` "Running and verifying" -- add the commerce demo command and a pointer to `docs/agents/local-stack.md`.
- `CONTEXT.md:711` -- the commerce demo row in §10.2; add a row for this story after it. The glossary term is **Payment provider** (`Fake`/`Stripe`); "vendor root" and "file-held" are ADR-0007's words.
- `vite.config.ts` portal dev on :5174 proxying `/api` to :7900 (`VITE_API_TARGET`); the site dev server is `pnpm --filter www dev` on :4321; README "Stripe test mode" (:91) is the Stripe walkthrough to link, not copy.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-hub/src/deployment.rs` -- new: `DeploymentReport { development, payment_provider, vendor_root, reasons }` (serde; `VendorRootCustody` `file`|`none`; `Reason` `fake_payment_provider`|`file_vendor_root`), a pure `fn report(&Config) -> DeploymentReport`, and the handler. Add unit tests over a `Config` for the four matrix rows. `Cache-Control: no-store` is not needed (it is static per process).
- `crates/wipe-hub/src/lib.rs` -- `pub mod deployment;` and the route.
- `crates/wipe-hub/tests/deployment.rs` -- new HTTP tests: default hub (fake, no seed), `spawn_hub_with_root` (fake, file), and Stripe with no seed (`development:false`). Assert exact JSON bodies and status 200 with no session.
- `crates/wipe-hub/tests/isolation.rs` -- classify the route.
- `apps/portal/src/api/deployment.ts` + `deployment.test.ts` -- `fetchDeployment` with a guard; `bannerLines(report): string[]` mapping each reason to its fixed sentence (unknown reasons are ignored; `development:false` gives `[]`). Tests: each matrix body, a non-JSON body, an unexpected body.
- `apps/portal/src/DevelopmentBanner.tsx` -- `useQuery(["deployment"], staleTime: Infinity)`; renders nothing unless `data.development`; otherwise an amber full-width `role="status"` bar with the heading line and `bannerLines`.
- `apps/portal/src/main.tsx` -- render the banner in `rootRoute`.
- `docs/agents/local-stack.md` -- new guide: prerequisites; `docker compose up -d` (ports, Mailpit UI); hub (`cargo run -p wipe-hub -- serve`, the key flags and env twins, a vendor-root seed via `wipestation license new-root`, health check, `wipe-hub admin create`); portal (build-and-serve vs `pnpm --filter portal dev` on :5174); marketing site (`pnpm --filter www dev`, `PUBLIC_HUB_ORIGIN`/`PRICING_HUB_ORIGIN`); optional Stripe test mode (link README §Stripe test mode); `scripts/commerce-demo.sh`; the development banner and when it shows; teardown. The commands in it must be real (check flags against `config.rs`).
- `README.md` -- quick-start pointer to the guide; `wipe-hub` row mentions `GET /api/deployment`; `apps/portal` row mentions the banner.
- `AGENTS.md` -- under "Running and verifying", one bullet: the full local stack is in `docs/agents/local-stack.md`; `scripts/commerce-demo.sh` (needs compose) proves the customer journey; `GET /api/deployment` drives the portal's development banner.
- `CONTEXT.md` §10.2 -- new row for this story (commerce-end-to-end story 2, CAP-2/CAP-3, ADR-0006 §3, ADR-0007), with as-built gaps: the banner was checked by unit tests and a build, not a browser (unless one is run), and it is not shown on the marketing site.

**Acceptance Criteria:**
- Given a hub started with the default (Fake) provider, when anyone requests `GET /api/deployment` without a session, then it answers 200 with `development: true` and the reason `fake_payment_provider`.
- Given the portal is built and served by such a hub, when any portal page loads (signed in or not), then a banner saying it is a development hub is shown above the header and has no dismiss control.
- Given a Stripe hub without a vendor root seed, when the portal loads, then no banner is shown.
- Given a fresh clone, when a developer follows `docs/agents/local-stack.md`, then every command it gives exists with the flags shown, and it reaches the running hub, portal and commerce demo.
- Given the change, then `cargo test -p wipe-hub` (with compose up) and `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` all pass.

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 20 findings — high 0, medium 4, low 9, false 7, maybe-false 0
- findings:
  - `false` `reject` (Blind) A Stripe test-mode hub with no vendor root shows no banner — CAP-3's success line names exactly two conditions (Fake, file-held root); this is a recorded autonomous decision, and changing the rule would edit this build's spec.
  - `low` `patch` (Blind) The portal guard rejects any `vendor_root` other than `file`/`none`, so a future custody value would hide the banner — fixed: `vendor_root` is any string; a test parses `"hsm"`.
  - `medium` `patch` (Blind) `retry: false` + `staleTime: Infinity` hide the banner for the whole tab after one failed first fetch, and a hub restart is never noticed — fixed: default retries, `staleTime` 30 s, refetch on focus and every 60 s.
  - `medium` `patch` (Blind) Nothing renders the banner, so a broken gate or a missing mount passes every test — fixed: `DevelopmentBanner.test.ts` (development report renders `role="status"`, the heading, both lines and no button; `development:false` and no data render nothing).
  - `low` `reject` (Blind) No HTTP test for the Stripe + file-root row — the handler is one line over `report()`, whose unit test covers that row; the HTTP path is the same for every row.
  - `low` `patch` (Blind) "Development hub" was used with no §14 glossary entry, next to the station-side "Hub" — fixed: a CONTEXT.md §14 entry.
  - `low` `patch` (Blind) Another full `Args { … }` literal in deployment.rs tests — fixed: reuses `config::tests::args()` (now `pub(crate)`).
  - `low` `patch` (Blind) `.gitignore` described `.wipe-hub/` as artifacts only, though the guide puts the private seed there — fixed: comment names the seed. (The teardown part is already covered: the guide's `rm -rf .wipe-hub` line names the seed.)
  - `false` `reject` (Blind) Loose TS `payment_provider: string` and unused `Deserialize` derives — no named harm; the string is deliberate forward tolerance (the same fix applied to `vendor_root`), and the derives match `health.rs`.
  - `false` `reject` (Blind) `role="status"` is wrong for a permanent banner — its content is static after mount, so it is announced once when it appears, which is the wanted effect; the trivial heading-constant test is superseded by the render test.
  - `medium` `patch` (Edge) A failed first fetch hides the banner for the session — grouped with the Blind retry/staleTime row; same fix.
  - `medium` `patch` (Edge) A hub restarted with another provider/root is not noticed by an open tab — grouped with the same row; same fix.
  - `low` `patch` (Edge) The guard rejects future vendor-root values — grouped with the Blind guard row; same fix.
  - `low` `patch` (Edge) Re-running `license new-root --out` silently overwrites the seed (`fs::write`, crates/wipe-cli/src/main.rs:770) — fixed: the guide says run it once and why.
  - `medium` `patch` (Verification gap) `DevelopmentBanner` and its gate are untested — grouped with the Blind render-test row; same fix. Checking the mount in `main.tsx` stays with the build (entry point with a `createRoot` side effect).
  - `low` `reject` (Intent) The documented steps are not run by any test (reading 2-C) — the implementer checked every command and flag against `--help` and ran the hub in each configuration; automating a docs walkthrough would need CI, a SPEC non-goal.
  - `false` `reject` (Intent) The README carries a pointer rather than the full walkthrough — CAP-2 says "README and `docs/agents/`"; the README already held the core commands, and it links the guide.
  - `false` `reject` (Intent) AGENTS.md points to the guide rather than listing commands — AGENTS.md already lists compose, hub serve, portal build/dev and www dev; the new bullet adds the demo and the guide.
  - `false` `reject` (Intent) Fail-open (no banner when the hub cannot answer) vs a fail-safe reading — a recorded autonomous decision; with retries a transient failure now recovers.
  - `false` `reject` (Intent) Broader reading 3-C (Stripe test hubs, other surfaces) — the same as the first row; the SPEC names the portal and two conditions.

## Design Notes

```json
GET /api/deployment → 200
{"development":true,"payment_provider":"fake","vendor_root":"file",
 "reasons":["fake_payment_provider","file_vendor_root"]}
```

`reasons` are machine codes so that the wording stays in one portal file and the rule stays in one Rust function. Order: provider first, then root.

## Verification

**Commands:**
- `docker compose up -d && cargo test -p wipe-hub` -- expected: all pass, including `tests/deployment.rs` and `tests/isolation.rs`.
- `pnpm --filter portal typecheck && pnpm --filter portal test && pnpm --filter portal build` -- expected: pass.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: unchanged pass (sanity).
- `cargo run -p wipe-hub -- serve --addr 127.0.0.1:7920 …` then `curl 127.0.0.1:7920/api/deployment` -- expected: the matrix body.
- `scripts/commerce-demo.sh` -- expected: still exits 0.

## Auto Run Result

Status: done

**Summary.** CAP-2 and CAP-3 of the commerce end-to-end spec.
- **Hub.** It now answers `GET /api/deployment` (no session, configuration only) with `{development, payment_provider, vendor_root, reasons}`. A hub is a development hub when it uses the Fake payment provider or a file-held vendor root; the rule is decided in `wipe_hub::deployment::report` alone.
- **Portal.** Every page shows a persistent amber banner with no close control while `development` is true: "Development hub — not for production use." plus one sentence per reason. If the hub cannot answer, no banner is shown; the portal retries and re-reads the report every 60 s and on window focus.
- **Docs.** `docs/agents/local-stack.md` walks a new developer through compose, the hub (flags, env twins, vendor root, first admin), the portal, the marketing site, Stripe test mode, the commerce demo, the banner and teardown. README and AGENTS.md point to it.

**Files changed**
- `crates/wipe-hub/src/deployment.rs` — new: report, rule, handler and unit tests.
- `crates/wipe-hub/src/lib.rs` — module and route.
- `crates/wipe-hub/src/config.rs` — test defaults helper made `pub(crate)` for reuse.
- `crates/wipe-hub/tests/deployment.rs` — new HTTP tests (default, file root, Stripe without root).
- `crates/wipe-hub/tests/isolation.rs` — route classified `Anonymous`.
- `apps/portal/src/api/deployment.ts`, `deployment.test.ts` — fetch, guard, reason sentences, tests.
- `apps/portal/src/DevelopmentBanner.tsx`, `DevelopmentBanner.test.ts` — the banner and its render tests.
- `apps/portal/src/main.tsx` — banner mounted in the root layout.
- `docs/agents/local-stack.md` — new local platform guide.
- `README.md`, `AGENTS.md`, `CONTEXT.md` (§10.2 row, §14 "Development hub"), `.gitignore` (comment).

**Review findings:** 20 total.
- **Patches, 7 entries (2 medium, 5 low):** banner query retries and refresh; banner render test; tolerant `vendor_root`; glossary entry; shared test `Args`; `.gitignore` comment; `new-root` overwrite warning.
- **Deferred:** none.
- **Rejected:** 7 false and 2 low, with reasons in the triage log.

**Follow-up review recommended: true.** Two medium entries were patched. The specific unverified risk is the banner's refetch behaviour (retries, 60 s interval, focus refetch). It is covered by no test, and nobody has seen the banner in a browser, so its look and its recovery after a hub restart were never observed.

**Verification performed**
- `cargo test -p wipe-hub --no-fail-fast` with compose up: all 26 test binaries pass, including `tests/deployment.rs` and `tests/isolation.rs`.
  - One earlier run failed `tests/health.rs::silent_smtp_and_database_time_out_within_bound` (a timing bound, 4.2 s, while the machine was loaded). That test does not depend on this change, and it passed on the re-run. The implementer saw the same flake once.
- Portal typecheck, test (240) and build pass. Desktop typecheck and test (200) pass.
- `rustfmt --check` is clean on the new Rust files.
- The implementer ran a live `wipe-hub serve` in all four configurations, and `curl /api/deployment` returned the matrix bodies.
- `scripts/commerce-demo.sh` exits 0 (LICENSED, verified offline).

**Residual risks**
- The banner has not been seen in a real browser.
- A default `wipe-hub serve` against an existing local `wipehub` database refused to start: "migration 8 was previously applied but has been modified". The implementer saw this before making any change, and `0008` has only one commit, so the developer's local database was probably migrated from an uncommitted copy. `docker compose down -v` clears it. This is not caused by this story, but a developer following the new guide may hit it.
