---
id: SPEC-hardening
companions: [assessment.md, ../../docs/adr/0005-licensing-and-attestation.md, ../../docs/adr/0007-hub-accounts-payments-persistence.md, ../../docs/adr/0008-station-enrollment-and-licence-fetch.md, ../../docs/adr/0011-destruction-by-operator-attestation.md, ../../docs/adr/0013-licensing-controls-every-erasure.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate. `assessment.md` holds the evidence (file:line) for every item below; read the section for your story before planning.

# Hardening before real hardware

## Why

A whole-codebase review on 2026-11-16 (`assessment.md`) found the code in good shape but with four concentrated risks, all of which matter before a real drive backend ships (DECISIONS D12):
- the station API is reachable from any web page;
- station state is held only in memory and settlement can be lost under lag;
- the runner is not ready for real hardware;
- nothing enforces the checks: no CI, no browser tests, and wall-clock-dependent hub tests.

It also confirmed 24 still-open deferred review items. Each story below closes a coherent group. Every fix here is a standard practice chosen by default (AGENTS.md "Questions left for the human"). Anything that needed the owner is in DECISIONS D18–D20 and excluded here.

## Capabilities

- **CAP-1 CI and build hygiene**
  - **intent:** every push runs the same checks a person runs locally, and failures cannot hide.
  - **success:**
    - `.github/workflows/ci.yml` with these jobs:
      - `rust`: Postgres 16 and Mailpit as service containers on the compose ports, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast`, rust-cache;
      - `msrv`: `cargo +1.80 check --workspace`;
      - `web`: `pnpm install`, typecheck and test for desktop, portal and www, plus `pnpm --filter www build`;
      - `fmt`: `rustfmt --edition 2021 --check` on Rust files changed by the push only, since 13 tracked files are unformatted;
      - `demo`: `scripts/commerce-demo.sh`, on main pushes only.
    - Dependabot for cargo, npm and GitHub Actions, weekly.
    - Every crate sets `rust-version.workspace = true`.
    - The `deps.rs` guard also enforces that `wipe-common` has no sibling dependency and `wipe-engine` depends only on `wipe-common`.
    - The 4 `large_enum_variant` warnings are fixed by boxing.
    - The flaky `silent_smtp_and_database_time_out_within_bound` is investigated. If the probe really overruns `PROBE_TIMEOUT`, the probe is fixed, not the limit. The cause is recorded in the commit.
    - README and AGENTS.md describe CI.

- **CAP-2 Station API lockdown**
  - **intent:** only the station's own UI can drive the station API; another website, or a DNS-rebinding page, cannot.
  - **success:**
    - `CorsLayer::permissive()` is removed; no CORS headers are sent.
    - Every request's `Host` must be a loopback name or the bound address; anything else gets 421 (Misdirected Request).
    - State-changing requests (POST, PUT, PATCH, DELETE) and the `/api/events` WebSocket upgrade must carry an `Origin` equal to the station's own origin (or the Tauri origin) when an `Origin` is present. They must also carry the header `X-Wipestation-Client: 1`, which a cross-origin page cannot send without a preflight that the station refuses.
    - The desktop and CLI-served UI send the header from `client.ts`/`ws.ts`.
    - Licence install accepts only `application/json`.
    - Tauri's `csp` is set to a policy allowing only self and the station origin.
    - HTTP tests cover a foreign Origin, a foreign Host, a missing header, a text/plain POST, a cross-origin WebSocket, and that the real UI flows keep working.
    - CONTEXT records the protection and that operator identity (D2) is still the long-term answer.

- **CAP-3 Runner integrity and bounded job logs**
  - **intent:** a Job's state always tells the truth, and a long erasure cannot bloat its signed certificate.
  - **success:**
    - Progress updates set the event's latest `progress` and are broadcast, but are no longer appended to `ErasureEvent.events`. The log keeps state changes, command results and evidence. The v2 fixtures still verify, and a test runs a long mock erasure and asserts the event log stays bounded.
    - `create_job` refuses (409, naming the active Job) a second non-terminal Job for the same device, under the runner's write guard.
    - Any error inside the erasure run marks the ErasureEvent `Failed` with a recorded `reason`, keeps the evidence gathered so far, and logs at `warn`, so the Job reaches the Inbox.
    - Before `append_activity` and before `backend.issue`, the run re-checks under the write guard that the Job is still `InProgress`. The Erased transition is a single-guard compare-and-set.
    - Aborting a Job whose `manifest_id` is set (on a destruction run) is refused with 409 naming the run.
    - Verification honours the JobSpec (enabled and sample count) instead of the hard-coded values.
    - Tests cover each item through the mock backend, waiting on `JobBroadcast` rather than fixed sleeps.

- **CAP-4 Settlement survives lag**
  - **intent:** no Job goes uncertified and no credit stays reserved because a broadcast was missed.
  - **success:**
    - On `RecvError::Lagged`, and on a periodic tick (60 s), the certificate generator sweeps the runner. It certifies every `Erased` or `Destroyed` Job with no stored certificate, and releases reservations held by Jobs that are `Aborted` or no longer exist. The sweep is idempotent and never re-signs a stored certificate.
    - The generator is supervised: a panic is logged at error and the task restarts.
    - The WebSocket loop handles `Lagged`, telling the client to refetch, and a closed channel, ending that branch rather than spinning.
    - Tests force lag with a tiny channel and assert certification and release still happen.

- **CAP-5 Hub account and enrollment hardening**
  - **intent:** an account cannot be pre-claimed, and a seat cannot be spent on an enrollment the station can never collect.
  - **success:**
    - Verification is bound to the person who signed up:
      - **Same browser:** a link opened in the browser holding the sign-up session verifies at once, as today.
      - **Any other browser or device:** opening the link asks for the account's password.
        - The right password verifies the account and ends every other session.
        - "I didn't create this account / set a password" clears the password and every session, then continues as a password reset. A pre-registering attacker loses all access.

      Each path is audited. A regression test replays the pre-registration sequence, and portal tests cover the new prompt.
    - Failed verification uses are audited.
    - A test proves the dummy Argon2 verify runs for unknown emails, through a `#[cfg(test)]` counter.
    - Approval is refused with under 60 s left on the code; an approved enrollment stays collectable for 5 minutes past expiry.
    - Licences, grants and certificates use `verify_strict` everywhere.
    - The enrollment start signature covers a station-chosen nonce, and replays within the code lifetime are refused.

- **CAP-6 Hub production guards**
  - **intent:** what ADR-0007 says production must not do, the hub refuses to do.
  - **success:**
    - `--payment-provider stripe` with a file-held vendor root refuses to start unless `--allow-file-root` (env twin) is given, and that flag shows in `GET /api/deployment` and the development banner.
    - A seed file readable by group or other is refused.
    - The Fake provider refuses to start on a non-loopback listen address.
    - `--trusted-proxy <cidr>` (repeatable, env twin): only from those peers is `X-Forwarded-For`'s right-most untrusted address used as the client address for throttles, audit `peer` and enrollment `requested_from`. With no trusted proxy, nothing changes.
    - Unknown `/api/…/` paths with a trailing slash get the JSON 404.
    - Abandoned uploads remove their `.part` file (drop guard), plus a start-up sweep of stale `.part` files.
    - `Args` no longer derives a `Debug` that prints secrets.
    - Offering currencies are limited to an allow-list with one shared minor-unit table matching Stripe. Existing offerings in other currencies are reported, not changed.
    - A refunded subscription trims its stations' licences to the paid-through date like a cancellation (ADR-0013 §4; refund is the stronger case of cancel).
    - Each item has a test.

- **CAP-7 Station licence durability**
  - **intent:** a licensed station stays licensed across restarts and upgrades.
  - **success:**
    - `enrolled-licence.json` is loaded at start-up. Precedence against `--license`: the licence with the later `not_after` wins, and the choice is logged.
    - `Feature` deserialization tolerates unknown variants (kept as `Unknown(String)`, never granting anything), so a licence naming a newer feature still loads.
    - The desktop takes an exclusive OS lock on its app-data directory and shows a clear error if another copy holds it.
    - A signing seed readable by group or other is tightened to 0600 with a warning (or refused if that fails).
    - A corrupt bay-layout file is moved aside to a timestamped backup before being replaced.
    - Tests cover each.

- **CAP-8 Bench exception visibility and frontend resilience**
  - **intent:** the bench never shows a stuck or failed drive as "wiping", and a crash or server error never blanks the screen.
  - **success:**
    - `slotStatus` reports a verification-failed Job as failed and a Queued Job as queued.
    - Queued Jobs older than 5 minutes appear in the Inbox.
    - Both apps get a router-level error component.
    - `jsonFetch` merges caller headers instead of replacing them, and turns error bodies into a typed `ApiError` with the server's `error` message.
    - Happy-dom tests cover: Layout mounting the licence banner; the Sanitize wizard, batch wizard and record dialog scan groups; and the error component.
    - Inputs without labels get labels.

- **CAP-9 Deterministic hub tests**
  - **intent:** hub tests never depend on wall-clock time or fixed sleeps.
  - **success:**
    - The hub's `AppState` carries an injectable `Clock` (real by default), used for every `now` in token expiry, sessions, throttles, renewals, enrollment and grace.
    - Tests that slept to separate timestamps advance the test clock instead.
    - "No second email" checks use an explicit flush or barrier instead of sleep-then-absent.
    - `wipe-engine` gains runner unit tests that wait on broadcasts.
    - The suite's wall time drops measurably; the before and after times are recorded in the commit.

- **CAP-10 End-to-end and portal interaction tests**
  - **intent:** the joints between site, portal, hub and station are tested the way a customer crosses them.
  - **success:**
    - A Playwright smoke test against the compose stack and a Fake-provider hub covers: marketing lead form posts; portal sign-up with verification through Mailpit; buy; register a station; the licence shown. It runs in CI nightly (CAP-1).
    - The portal gains happy-dom, with interaction tests for sign-in, sign-up, accept invite, buy and the admin lead export/erase.
    - CLI tests pin the demo's output lines (`LICENSED`, `customer:`, `PUBLISH THIS`).
    - A bootstrap test proves a non-default `--concurrence-policy` reaches `AppState`.

- **CAP-11 Observability and shared contracts**
  - **intent:** every request can be traced, and the TypeScript types cannot drift from Rust.
  - **success:**
    - Both servers add `TraceLayer` with a request id (`X-Request-Id`, generated if absent, echoed in responses and errors).
    - The hub's `ApiError` moves to `error.rs` and logs the route instead of "auth request failed".
    - One `require_owner` replaces the four copies.
    - Rust types exposed to the desktop and portal are generated to TypeScript (ts-rs or specta, whichever builds on 1.80) into checked-in files, with a golden test that fails when generated output differs. The hand-written mirrors are replaced or re-exported from them, and AGENTS.md's "hand-mirrored" rule is updated.

- **CAP-12 A backend seam ready for real hardware**
  - **intent:** adding `wipe-engine-linux` (D12) means implementing a trait, not rewiring the station.
  - **success:**
    - The backend is chosen by configuration through a factory. `wipe-engine-mock` becomes an optional dependency behind a `mock` feature (on by default for now).
    - `WipeError::Backend(String)` becomes a typed `BackendError { kind: Retryable | Fatal | Unsupported, message, evidence }`.
    - `cancel` returns `CancelOutcome { Cancelled, StillRunning, NotSupported }`, and the runner records the truth: an event is never marked Aborted while the firmware is still sanitizing.
    - Poll errors retry with backoff before failing.
    - A `BlockingOp` helper runs long synchronous commands on a dedicated thread with a timeout, so `issue` may block.
    - `frozen` is stored once.
    - Device inventory is cached and refreshed on change rather than re-enumerated per create and run.
    - The mock implements the new contract, including a slow blocking command and a cancel that reports `StillRunning`.
    - Integration tests cover each.
  - **checkpoint:** this reshapes the engine's public contract; the owner reviews the plan before building.

- **CAP-13 Docs and copy truth-up**
  - **intent:** what the docs and the bench say matches the code.
  - **success:**
    - CONTEXT §12's Verification item matches ADR-0001 as built.
    - The tier-3 storage message and console line stop offering a control plane. The agent's proposed wording, for owner approval:
      - storage message: "This station cannot save its settings to {path}. Changes to the bay layout made now will be lost when it restarts.";
      - console: "the UI will ask the operator to accept that settings will not be saved".

      The control-plane variant (tier 2, a configured but unusable control plane) keeps naming it.
    - The honest limits recorded in `assessment.md` that remain after this epic are listed in CONTEXT §11.
    - ROADMAP is truthed up against the hub as built: A1.3's shipped parts and A1.4 (every hub change is audited, ADR-0007 §6) are struck with their shipping stories, leaving only what is really open (for example, the admin view of an organization's orders and entitlements).
  - **checkpoint:** customer-visible copy; the owner approves the finished story.

## Constraints

- ROADMAP §0.2 and ADR-0013 §1 hold throughout: no change here may refuse an erasure for any new reason, interrupt a running Job, or gate on connectivity.
- Certificates: never change the signed v3 shape incompatibly; v2 fixtures must still verify (AGENTS.md).
- Every hub behaviour change is audited where it changes state (ADR-0007 §6). Rust and TS shapes change together.
- Follow AGENTS.md formatting: rustfmt only touched files.

## Non-goals

- Anything waiting on DECISIONS D18 (returning credits on release), D19 (clock-watermark recovery), D20 (persisting certificates on the station), D2 (operator identity), D5 (vendor-root custody, including the desktop shell's missing root), D12 (the Linux backend itself).
- Publishing the privacy notice and the consent `notice_version` column (O-13), which go with the notice.

## Success signal

- A push to GitHub runs every check. A web page in the bench browser cannot touch the station. Forcing lag or a crash-free restart never leaves a drive uncertified or a credit stranded. Adding a real backend is a new crate implementing one trait.
