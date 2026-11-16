# Codebase assessment — 2026-11-16

Taken on `main` at `ac138ad` (after `controlled-licensing` story 5). Four read-only reviews ran in
parallel (security, architecture, testing and tooling, and a triage of the 43 medium deferred review
items). The headline findings were re-checked against the code. This file is the evidence behind
`SPEC.md`. Each finding names the story that fixes it, or the decision it waits on.

## Overall

The codebase is in good shape for its age: clean layering, a strong hub, careful licence handling,
1232 Rust tests and about 1000 frontend tests, all passing apart from one flaky test. The risks are
concentrated in four places, all of which matter before a real drive backend ships:

1. **The station's HTTP API accepts requests from any website.** A page open in the bench's browser
   can start Jobs, install licences or re-enroll the station. It is harmless while only the
   simulated backend exists; once a real backend ships, any web page could start a drive wipe.
2. **Station state is held only in memory.** Certificates and Jobs live in RAM, while credits are
   spent and saved to disk. A broadcast skipped under lag, or a restart, can lose a certificate
   whose credit was already spent, or strand a reservation.
3. **The runner is not ready for real hardware.** The backend is hard-wired to the mock, errors are
   strings, cancel and abort semantics are optimistic, every progress tick is appended to the
   signed certificate, and an early error leaves a Job silently stuck.
4. **Nothing enforces the checks.** There is no CI, linting or formatting gate, and no browser-level
   test, and the hub's tests depend on wall-clock time.

## Scorecard

| Dimension | State | Main gaps | Story |
|---|---|---|---|
| Security: hub | Strong. Argon2id with equalised timing, hashed single-use tokens, constant-time double-submit CSRF, tenant isolation, all queries bound, verified idempotent Stripe webhooks, signed one-use station challenges, HMAC download links | Account pre-registration hijack; a file-held vendor root accepted with live Stripe; throttles see only the proxy behind TLS termination; non-strict Ed25519 in places | 4, 5 |
| Security: station | Weak at the edge | Permissive CORS, no Host/Origin check, plain-text POSTs accepted, WebSocket readable from any page; existing 0644 seed read silently | 2, 6 |
| Correctness: runner and state | Good core, fragile edges | Duplicate Jobs per device, Jobs stuck after early errors, abort racing probe/issue, unbounded progress log in signed certs, lag-dropped certification and credit release | 3, 4 (station) |
| Licensing durability | Careful decision logic | Enrolled licence not loaded at start-up; closed `Feature` enum would refuse a newer licence; two desktops can share one data dir; future-dated watermark unrecoverable (decision) | 7 |
| Extensibility: real hardware | Trait seam exists | Backend not injectable, string errors, no cancel outcome, blocking-command model, verification hard-coded, SCSI capabilities missing | 12 |
| Maintainability | Good. Hub files ≤ 1650 lines, 4 clippy warnings, few non-test unwraps | Duplicate `require_owner` ×4, shared `ApiError` lives in `auth.rs` and logs "auth request failed" for all modules, hand-mirrored TS types | 11 |
| Testability | Strong hub harness, golden drift tests, injectable clock on the station | No hub clock (~99 `now_utc` calls) so tests sleep; one flaky health test; no Playwright; portal tests are static markup only; the biggest desktop pages untested; `wipe-engine` has no tests of its own | 9, 10 |
| Tooling | Manual | No CI, no lint/format gate, toolchain pinned to `stable` not 1.80, `rust-version` inherited by 1 of 10 crates, no dependency update bot | 1 |
| Observability | Logging only | No request tracing or request ids on either server; failures in the runner logged at debug | 3, 11 |
| Frontend resilience | Clean, no XSS sinks | No error boundary in either app; `jsonFetch` shows raw 500 bodies and drops caller headers; polling despite a WebSocket | 8 |

## Findings by story

Severity: **H** high, **M** medium, **L** low. "Verified" means re-checked against the code by hand
after the review.

**Story 1, CI and build hygiene**
- H: no `.github/workflows`, hooks, eslint/prettier, rustfmt/clippy config, dependabot or `cargo-deny` (ROADMAP §0.5).
- M: `rust-toolchain.toml` says `stable`; only `wipe-hub` sets `rust-version.workspace`, so MSRV 1.80 is never checked.
- M: `crates/wipe-hub/tests/deps.rs` guards only the hub; the common and engine layering rules (AGENTS.md) are unchecked.
- M: `silent_smtp_and_database_time_out_within_bound` (`crates/wipe-hub/tests/health.rs:167`) fails intermittently (5.36 s against a 3.5 s limit, with a 2 s `PROBE_TIMEOUT`). This may be a real probe overrun, not just a loose limit.
- L: 4 clippy `large_enum_variant` warnings (`runner.rs:47`, `ws.rs:18`, `hub_client.rs:310`, `station_link.rs:384`).
- M: `scripts/commerce-demo.sh` is run by nothing (deferred O-6).

**Story 2, station API lockdown** (verified)
- H: `CorsLayer::permissive()` at `crates/wipe-server/src/app.rs:874`; no Host or Origin check; `install_license` (`handlers.rs:992`) takes raw bytes, so a no-preflight text POST works. DNS rebinding reaches the same routes.
- M: `/api/events` WebSocket upgrades without an Origin check (`ws.rs:30-35`).
- L: Tauri `"csp": null`.

**Story 3, runner integrity and bounded job logs** (verified)
- H: every 250 ms progress tick is pushed onto `ErasureEvent.events` (`runner.rs:538`, emitted at `:745`) and copied into the signed certificate; a 12-hour overwrite gives about 170k entries.
- M: `create_job` allows a second Job for a device with an active one (`runner.rs:111-123`; deferred O-1).
- M: early errors (`?` at `runner.rs:646-802`) leave the erasure Probing or Running and the Job InProgress, invisible to the Inbox (O-3); failures are logged at debug (`:187`, `:627`).
- M: abort or escalation while probing still issues the wipe (`runner.rs:667`, `:707`), and the Erased transition is check-then-act (`:605-608`) (O-10).
- M: aborting a Job on a live destruction run strands the run (`runner.rs:201-216`) (O-4b).
- M: verification is hard-coded on with 8 samples whatever the JobSpec says (`runner.rs:589-590`).

**Story 4, settlement survives lag**
- H: on `RecvError::Lagged` the certificate generator skips the lost broadcasts (`app.rs:533-539`, verified), so an `Erased` Job is never certified and an `Aborted` Job's reserved credit is never released (O-9).
- M: the generator is one unsupervised `tokio::spawn` (`app.rs:526`); a panic stops certification silently.
- M: the WebSocket loop drops lagging clients and spins hot on a closed fleet channel (`ws.rs:69`, `:76`).

**Story 5, hub account and enrollment hardening**
- M: account pre-registration hijack, verified: `verify_email` (`crates/wipe-hub/src/auth.rs:458`) calls `consume_verification_token` (`accounts.rs:592`), which only marks the address verified, leaving the pre-registering attacker's password and 14-day session valid (O-19). Wiping the password on every verification would force every honest user verifying on their phone through a reset, so CAP-5 binds verification to the sign-up session instead.
- M: approving an enrollment just before expiry spends a seat the station can never collect (`enrollments.rs:421`) (O-20).
- L: non-strict Ed25519 `verify` for licences and certificates (`wipe-license/src/chain.rs:273`, `wipe-cert/src/sign.rs:203`); the enrollment start signature has no nonce (`enrollment.rs:79`).
- L: the unknown-email dummy Argon2 has no test (O-18); failed verification uses are not audited.

**Story 6, hub production guards**
- M: a file-held vendor root only warns with `--payment-provider stripe` (`config.rs:418-430`), against ADR-0007; seed file permissions unchecked (`config.rs:540`).
- M: throttles and the enrollment `requested_from` key on the direct peer (`auth.rs:516`, `enrollment_routes.rs:50`, `station_link_routes.rs:137`); behind a TLS proxy every client shares one key.
- L: the Fake provider is gated only by base URL (`payments.rs:450`) with a public webhook secret (`:51`).
- L: `/api/checkout/` with a trailing slash serves portal HTML, not a JSON 404 (`lib.rs:123`); an abandoned upload leaves a `.part` file (`artifacts.rs:210-227`); `Args` derives `Debug` over Stripe secrets (`config.rs:60-95`).
- M: currencies whose Stripe minor unit differs from ISO (ISK, UGX, HUF, TWD, MGA) are accepted (`offering_admin.rs:229`, `stripe.rs:189`) (O-24).
- L: a refunded subscription keeps the 30-day renewal grace (`stations.rs` `LICENCE_PAST_END`).

**Story 7, station licence durability**
- M: nothing loads `enrolled-licence.json` at start-up (`licence_fetch.rs:27`); an offline restart comes up unlicensed and, under ADR-0013, refuses real erasure (O-23).
- M: `Feature` is a closed enum (`wipe-license/src/entitlement.rs:60-64`); a licence naming a newer feature fails to parse on an older station, which now means refused erasure (O-8).
- M: two desktop copies can share one app-data directory (`apps/desktop/src-tauri/src/lib.rs:303`) (O-7).
- L: an existing 0644 signing seed is read without warning or tightening; a corrupt bay layout is overwritten with no backup (`store.rs` `LocalFileStore`).

**Story 8, bench exception visibility and frontend resilience**
- M: verification-failed Jobs and Queued Jobs are drawn as "wiping" (`apps/desktop/src/bench/slotStatus.ts:39-48`) (O-2); Queued Jobs never reach the Inbox (O-4).
- M: no error boundary in either app; `jsonFetch` (`apps/desktop/src/api/client.ts:23-37`) shows raw 500 text and its `...init` drops `Content-Type` when a caller passes headers.
- M: no test that Layout mounts the licence banner (O-16); no render tests for the Sanitize wizard, batch wizard or record dialog (O-5).
- L: about 49 labels for 70 inputs; polling every 4 s alongside the WebSocket (`Layout.tsx:16-17`).

**Story 9, deterministic hub tests**
- H: no injectable clock in the hub (~99 direct `now_utc` calls); `orders.rs:510` sleeps 1.1 s, and "no second email" checks sleep 500 ms then assert absence (`renewal_grace.rs:638,707`, `auth.rs:2376,2442`).
- L: `wipe-engine` has no tests of its own; mock end-to-end tests use fixed 25–100 ms sleeps.

**Story 10, end-to-end and portal interaction tests**
- H: no browser crosses the site → hub joint (credentialed CORS, `SameSite=Lax` CSRF cookie) (O-14); no Playwright.
- M: portal tests are `renderToStaticMarkup` only; SignIn, SignUp, AcceptInvite, Order, Organization and most admin pages are untested (O-15).
- M: CLI output lines the demo greps (`LICENSED`, `customer:`, `PUBLISH THIS`) are not pinned (O-6); a non-default `--concurrence-policy` is untested (O-21).

**Story 11, observability and shared contracts**
- M: no `TraceLayer` or request ids on either server although the feature is enabled (`Cargo.toml:56`).
- M: `ApiError` lives in `auth.rs:108` and logs "auth request failed" for every module (`:124`); `require_owner` is written four times.
- M: `apps/desktop/src/api/types.ts` (833 lines) and portal API types are hand-mirrored with no contract test (`route_golden.rs` checks routes, not shapes).

**Story 12, a real-hardware-ready backend seam**
- H: `bootstrap.rs:156` hard-codes `MockBackend`; `wipe-server` depends on the mock crate as a normal dependency.
- H: `WipeError::Backend(String)` loses sense data and retryability; one transient poll error abandons tracking (`runner.rs:729`); a failed `cancel` is ignored but the event is marked Aborted (`:721-726`); `issue` is assumed non-blocking (`backend.rs:54-56`), which an ATA SECURITY ERASE through `SG_IO` breaks.
- M: no SCSI SANITIZE or FORMAT UNIT; `frozen` stored twice (`device.rs:93`, `:102`); full enumerate on every create and run.

**Story 13, docs and copy truth-up**
- M: `CONTEXT.md:1251` says the inner `Verifying` state runs the sampled reads (O-11).
- M: station banner and console text still offer a control plane as the fix for a station that cannot save (`store.rs:379,391`, `bootstrap.rs:229`) (O-12). This is customer-visible copy, so the owner approves the story.

## Waiting on owner decisions (DECISIONS.md)

- **D18, returning credits on release.** A released station's self-reported balance is returned to the pool (`credits.rs:346`), and the station keeps its grants, so the customer can mint credits.
- **D19, recovering from a future-dated clock watermark** (O-17). Under ADR-0013 the station refuses every real start, and deleting the lease file also loses the credit reserve.
- **D20, persisting certificates on the station.** CONTEXT §6 #2 keeps evidence in RAM. Story 4's sweep closes the lag gap, but a crash still loses signed certificates whose credits are spent.
- **D5 / D2.** The desktop shell trusts no vendor root, so Register is unavailable there (O-22). Concurrence by "a second person" is self-asserted until operator identity exists.

## Still unsettled (needs a manual check, not code reading)

- Honeypot autofill on the lead form (`LeadForm.astro:160`).
- Delayed-payment (ACH/SEPA) subscription events answered 503 beyond Stripe's retry window (`payment_events.rs:157-182`): needs a recorded test-mode sequence.

## Strengths worth keeping

- Layering is clean and enforced for the hub; the station and hub share signed-request and licence types rather than copying them.
- The hub harness (database per test, mail isolation, recording Stripe mock, signed webhook fixtures, concurrency tests) is better than most production codebases.
- The licence start decision is taken under one lock, the method checked is the method issued, and certificates are marked from the recorded decision. A new backend is gated unless it says it is simulated.
- Golden tests guard CLI help, routes against the client, the entitlement matrix and the v2 certificate fixtures.
- Fast frontend suites with well-tested pure modules; no XSS sinks; no tokens in browser storage.
