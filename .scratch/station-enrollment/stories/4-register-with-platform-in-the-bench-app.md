---
title: 'Register with platform in the bench app'
type: 'feature'
created: '2026-11-10'
status: 'done'
baseline_revision: '4a3a6d2690b56a4ed2fbea4913ee211514fdfc74'
review_loop_iteration: 1
followup_review_recommended: true
context:
  - '{project-root}/docs/adr/0008-station-enrollment-and-licence-fetch.md'
  - '{project-root}/.scratch/station-enrollment/SPEC.md'
  - '{project-root}/.scratch/station-enrollment/stories/3-wipestation-enroll-and-signed-station-requests.md'
warnings: [oversized]
deferred:
  - summary: >-
      The Tauri desktop shell trusts no vendor root, so its Licence page always shows Register with platform as unavailable.
    evidence: |-
      apps/desktop/src-tauri/src/lib.rs desktop_config passes no vendor_roots (its test asserts cfg.vendor_roots.is_empty()); CONTEXT already records "desktop shell is always unlicensed and refuses runtime install (no vendor root)". Giving the desktop a root depends on vendor-root custody (DECISIONS D5) and how a release bundles its root. Registering works under `wipestation serve --vendor-root-b64 --key-path`.
    location: >-
      apps/desktop/src-tauri/src/lib.rs
    severity: medium
  - summary: >-
      A licence installed by enrollment lasts until restart; nothing loads enrolled-licence.json at start-up.
    evidence: |-
      licence-visibility story 4's deferred item (start-up precedence against --license), already recorded by station-enrollment story 3. Matters more once ADR-0013's controlled licensing refuses unlicensed starts on real backends.
    location: >-
      crates/wipe-server/src/bootstrap.rs build_license_context
    severity: medium
  - summary: >-
      AGENTS.md test counts are stale (946 Rust, 385 vitest).
    evidence: |-
      The workspace now runs over 1040 Rust tests and 409+ desktop vitest tests; AGENTS.md is an agent-context file, so its edit is deferred.
    location: >-
      AGENTS.md
    severity: low
---

<intent-contract>

## Intent

**Problem:** Enrollment exists only on the command line (`wipestation enroll`, story 3). An operator at the bench app has no way to register the station with the platform (CAP-4), and the app has no licence screen at all, only the licence banner.

**Approach:** Station routes in `wipe-server` that run one enrollment inside the running station (start, background poll, verify, save, runtime install) by reusing `hub_client`, and a **Licence** page in the desktop app showing the licence status and a **Register with platform** panel: hub address, then code, link, QR code, countdown, and the outcome.

## Boundaries & Constraints

**Always:**
- Routes (declared in `app.rs` `api_routes()`, called from `client.ts`, route golden regenerated):
  - `GET /api/enrollment` → `EnrollmentView`.
  - `POST /api/enrollment` `{hub_origin: string, label?: string|null}` → 200 `EnrollmentView`. Refusals before any hub request: hub address failing `HubOrigin::parse` → 400 `{error}` with the rule text; no vendor root, or an instance key that is not kept on disk → 409 `{error}`; label over 200 chars → 400. Otherwise it sends the hub start itself and answers with the attempt `pending`, or ended `unreachable`/`refused`. A start answer whose `verification_uri` or `verification_uri_complete` is not an absolute `http`/`https` URL ends `refused` (no pending attempt, no polling). A new start replaces (aborts) any pending attempt, as the hub replaces the key's pending enrollment.
  - `POST /api/enrollment/cancel` → 200 `EnrollmentView` with no attempt; stops polling (the hub's code simply expires).
- `EnrollmentView` (Rust serde, mirrored in `types.ts`): `{saved_hub_origin: string|null, can_enroll: boolean, cannot_enroll_reason: string|null, attempt: EnrollmentAttempt|null}`. `EnrollmentAttempt` tagged on `state`: `pending {hub_origin, user_code, verification_uri, verification_uri_complete, seconds_left, interval}`; `installed {hub_origin, licence_id, customer_name, not_after, operator_message, saved_to: string|null, message}`; `denied | expired | unreachable | refused | failed {hub_origin, message}`. `seconds_left` is computed at read time from a monotonic clock (never wall-clock), floored at 0. `saved_hub_origin` is the settings file's address if it parses, else null.
- Station id = the station's licensing station id (`build_station`'s resolved id; `standalone` default); key = `AppState::signing_key` (the instance key); roots = `AppState::vendor_roots` only. `can_enroll` is false with a reason when no vendor root is configured (as the runtime install already refuses), or when the instance key was generated for this run only (no key file: `serve` without `--key-path`), since a licence bound to it would not survive a restart.
- Polling runs as one spawned task per attempt using `hub_client` (start then the existing poll loop, split so the route gets the start answer synchronously); `enroll`'s behaviour for the CLI is unchanged. On a verified licence the task: saves it to `enrolled-licence.json` in the settings directory (when the station has one), saves the hub origin to `station-settings.json`, then installs it through `AppState::install_license` (in process, not HTTP). Installed → `installed` with the licence status message; the `message` says the station holds it until restart and, when saved, to start with `--license <saved path>`; when not saved, that it could not be saved. An install refusal → `failed` with its text. Denied/expired/unreachable/untrusted/other-key/poll or start refusal → the matching state (`untrusted`/other-key/poll and start refusals map to `refused`) with a message worded for the bench app: it says what happened, whether anything was saved or installed, and carries the hub's or verifier's reason where there is one, but names no `wipestation` command (the panel offers Start again); a flag is named only as the station's configuration fix. Share the saving helper and file name with the CLI (move them into `wipe-server`).
- The settings path reaches `AppState` from `build_station` (Detect: `station_settings_path`; InMemory: none, so nothing is saved and `saved_hub_origin` is null).
- Desktop: route `/licence`, nav item **Licence**. Page: the licence status (`GET /api/license` `operator_message`, customer, expiry) and **Register with platform**. Idle: hub address input prefilled with `saved_hub_origin`, optional label, Register button; when `can_enroll` is false, the reason and no form. Pending: user code large, `verification_uri`, the `verification_uri_complete` link and a QR code of it (rendered client-side with `qrcode-generator`, as inline SVG), countdown `m:ss` from `seconds_left`, Cancel; refetch every 2 s while pending. Installed: "Licensed" with the operator message and `message`, and **Register again** (back to the form). Denied / expired / unreachable / refused / failed: a heading naming what happened, the server message, and **Start again** (back to the form, address kept). The LicenceBanner is unchanged (it informs only).
- A pure view-model (`apps/desktop/src/api/enrollment.ts`: panel kind, heading, countdown text) unit-tested; the panel rendered with `renderToStaticMarkup` for every state.
- Docs in the same commit: CONTEXT §10.2 row (and §14 **Enrollment**: the bench app is a station side too), README (UI/routes, test counts), regenerated `docs/generated/http-routes.txt`.

**Never:**
- No hub change, migration, CLI flag change, or periodic licence fetch (story 5); no change to `POST /api/license`, `--license` or the banner.
- Nothing gates, delays or interrupts an erasure; no job code touched. No hardware-sanitization claim.
- No root from the hub or request; no portal credential on the station; the device code never leaves the server (not in any view).
- No auto-load of `enrolled-licence.json` at start-up (licence-visibility story 4's deferred item).
- No Tauri commands; the UI talks HTTP only.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected | Error |
|---|---|---|---|
| Approved | owner approves | `installed`; `GET /api/license` valid; licence + hub saved | — |
| Denied | hub `access_denied` | `denied`, start again offered | message |
| Expired | `expired_token` or past `expires_in` | `expired` | message |
| Hub unreachable | closed port | POST 200, `unreachable` | names hub |
| Hub refuses start | 429 | `refused` with retry hint | — |
| Untrusted licence | other root | `refused`; nothing saved or installed | "does not trust" |
| Bad address | `http://hub.example` | 400, no request | rule text |
| No vendor root | | GET `can_enroll:false`; POST 409 | reason |
| Key not kept | station built with no key path | GET `can_enroll:false`; POST 409, no hub request | reason names `--key-path` |
| Bad approval link | hub sends `javascript:…` as `verification_uri_complete` | `refused`; no pending attempt, no poll | message |
| Settings dir unwritable | settings path in a read-only dir | `installed`; `saved_to` null; message says not saved, no `--license <path>`, and that the hub address was not saved | — |
| Restart while pending | second POST | first task aborted; new codes | — |
| Cancel | pending | attempt null | — |
| In-memory station | InMemory persistence | installs; `saved_to` null; nothing written | message says not saved |

## Autonomous decisions

- **Where the screen lives.** *Problem:* the spec says "the licence surface", but the app has only a banner that by rule carries no button or link. *Options:* add a button to the banner; a panel on Bench setup; a new Licence page. *Choice:* a new **Licence** page in the nav. *Why:* keeps the banner informative-only (its documented rule) and gives the licence status a home.
- **QR code rendering.** *Options:* a Rust QR crate serving SVG, a client library, or our own encoder. *Choice:* `qrcode-generator` (MIT, no dependencies, bundled offline) in the desktop app. *Why:* the route stays plain JSON; a hand-written encoder is needless risk.
- **Hub outcomes are view states, not HTTP errors.** *Choice:* a start that reaches a refusing or absent hub answers 200 with an ended attempt; 4xx only for refusals made before contacting the hub. *Why:* the screen reads one shape, and `GET` shows the same outcome after a reload.
- **Persistence after install.** *Problem:* the runtime install is memory-only. *Choice:* save the licence file and tell the operator, as the CLI does; auto-loading is the already-deferred item. *Why:* consistent with story 3's recorded decision; changing start-up precedence is out of scope.
- **Countdown clock.** *Choice:* server-computed `seconds_left` from a monotonic clock. *Why:* bench clocks are often wrong (ADR-0005), and the browser may be on another machine.
- **Registering a station whose key will not last** (review pass 1). *Problem:* `serve` without `--key-path` makes a new instance key at every start; registering it spends one of the customer's seats on a licence the station loses at its next restart. *Options:* allow it; warn; refuse. *Choice:* refuse (`can_enroll` false, 409), with a reason naming `--key-path`. *Why:* the safe default for the customer's seats; `wipestation enroll` already requires an existing key file.
- **Bench-app wording for outcomes** (review pass 1). *Problem:* the hub client's errors are written for the command line ("run wipestation enroll again", `--vendor-root-b64`), shown beside a Start again button. *Choice:* the station maps each outcome to bench wording, keeping the reason. *Why:* the same facts, with instructions the operator can follow on that screen; the CLI text is unchanged.
- **Hub-supplied links** (review pass 1). *Problem:* the approval links come from the hub and are rendered as a link and QR code; a `javascript:` or other scheme would run in the station UI. *Choice:* accept only absolute `http`/`https` URLs, else end the attempt `refused`. *Why:* OWASP output-handling practice; the hub can deliver a licence but never act in the station's UI.

</intent-contract>

## Code Map

- `crates/wipe-server/src/hub_client.rs:485` `enroll` -- split into `client.start` (`:306`) + a pub poll-loop fn taking a `StartResponse`; `enroll` keeps its signature. `EnrollError` (`:169`) variants → attempt states; `check_licence` (`:538`); `SystemClock`; `HubOrigin::parse`, `HUB_ORIGIN_RULE`; `ProxySettings::from_env`.
- `crates/wipe-server/src/app.rs:40-120` `AppState` fields/constructor (add enrollment slot + settings path + station id, builder methods); `:194` `install_license` (returns `LicenseView`); `:427` `LicenseInstallError` messages; `:481` `api_routes()`.
- `crates/wipe-server/src/handlers.rs` -- handler style, `{error}` bodies (see `install_license` handler).
- `crates/wipe-server/src/bootstrap.rs:164,195-215,247-250` -- resolved station id, settings path; pass both into `AppState`.
- `crates/wipe-server/src/bootstrap.rs:469` `load_or_create_signing_key(None)` generates a run-only key: pass `key_path.is_some()` into `AppState` (e.g. `with_instance_key_kept(bool)`, default false; existing station-enrollment tests set it true). The Tauri shell's `key_path` is `None` when it has no app-data dir.
- `crates/wipe-server/src/store.rs:696-760` -- `StationSettings`, `station_settings_path`, `load/save_station_settings`; add `ENROLLED_LICENCE_FILE` + `save_enrolled_licence` here.
- `crates/wipe-cli/src/main.rs:693,892` -- remove local const/helper, use the moved ones.
- `crates/wipe-server/src/licensing.rs:428` `LicenseView` (`operator_message`, `customer`, `not_after`).
- `crates/wipe-server/tests/hub_client.rs:50-220` -- fake hub (axum, verifies signatures, scripted polls), `licence()`, `closed_port()`; copy into the new test file.
- `crates/wipe-server/tests/http_e2e.rs` -- how an `AppState` router is driven in tests; `license_chain`.
- `crates/wipe-server/tests/route_golden.rs`, `docs/generated/http-routes.txt`.
- `apps/desktop/src/api/client.ts:22` `jsonFetch` (error handling), `:239` licence calls; `types.ts:325-400` licence mirrors.
- `apps/desktop/src/main.tsx` routes; `components/Layout.tsx:148` `NavBar` items (lucide icon).
- `apps/desktop/src/components/LicenceBanner.test.ts` -- render-test style with seeded `QueryClient`.

## Tasks & Acceptance

**Execution:**
- `crates/wipe-server/src/hub_client.rs` -- split start / poll loop.
- `crates/wipe-server/src/store.rs`, `crates/wipe-cli/src/main.rs` -- move the enrolled-licence file helper.
- `crates/wipe-server/src/enrollment.rs` (new, `lib.rs`) -- `EnrollmentView`, `EnrollmentAttempt`, the slot (state + abort handle), start/cancel/view, the task.
- `crates/wipe-server/src/app.rs`, `handlers.rs`, `bootstrap.rs` -- state wiring and the three routes.
- `crates/wipe-server/tests/station_enrollment.rs` (new) -- every matrix row through the HTTP router with a fake hub.
- `apps/desktop/package.json` -- add `qrcode-generator`.
- `apps/desktop/src/api/types.ts`, `client.ts`, `enrollment.ts` (+ test) -- mirrors, calls, view-model.
- `apps/desktop/src/pages/LicencePage.tsx` (+ test), `main.tsx`, `components/Layout.tsx` -- page, route, nav.
- `docs/generated/http-routes.txt`, `CONTEXT.md`, `README.md` -- per Always. The CONTEXT row also states that the Tauri desktop shell trusts no vendor root, so its Licence page shows the panel unavailable (pre-existing; deferred), and that a station without a kept key cannot register. README inventory lists every new test, including unit tests.
- Review pass 1 hardening (also in `enrollment.rs` / `LicencePage.tsx`): give the poll loop the same `expires_in` cap (one day) the countdown uses; show a failed Cancel/Start again request's error under the panel; replace the panel with the "could not report" text only when there is no cached view (a failed refetch keeps the code on screen); the ended-state mapping unit test covers every `EnrollError` variant.
- `apps/desktop/src/pages/LicencePage.flow.test.ts` (new) -- mount `LicencePage` under a `QueryClientProvider` with stubbed `fetch` (pattern: `RecordForDestructionPage.test.ts`): pending re-read moves to Licensed and `GET /api/license` is fetched again; Start again sends `POST /api/enrollment/cancel` and shows the form with the address kept; a 400 from start shows the station's `error`.

**Acceptance Criteria:**
- Given a station with a vendor root and a fake hub that approves after one poll, when `POST /api/enrollment` is called, then `GET /api/enrollment` reaches `installed` and `GET /api/license` reports the licence valid.
- Given the Licence page with a pending attempt, when it renders, then it shows the user code, the link, an SVG QR code and a countdown.
- Given `cargo test --workspace` and `cd apps/desktop && pnpm typecheck && pnpm test`, then all pass.

## Spec Change Log

### Review pass 1 (2026-11-10)
- **Triggering findings:** enrollment allowed with an instance key generated for this run only (seat spent on a licence lost at restart); outcome messages worded for the CLI on the bench page; hub-supplied approval links rendered unchecked (`javascript:` href).
- **Amended:** intent contract (three autonomous decisions, the start/refusal and message rules, matrix rows for key not kept, bad approval link, unwritable settings dir); Tasks (CONTEXT/README wording, pass-1 hardening, a mounted-page flow test).
- **Known-bad state avoided:** a customer's seat spent on a station that forgets its key; "run wipestation enroll again" next to a Start again button; hub-controlled script in the station UI.
- **KEEP:** the pass-1 implementation is saved at `/tmp/story-4-review-pass-1.patch` (apply with `git apply` from the repo root at `baseline_revision`); start from it and change only what the amendments require. Keep: `poll_until_done` split with `enroll` unchanged; `EnrollmentSlot` generation scheme and abort-on-replace; `Record` with monotonic `expires_at`; the moved `ENROLLED_LICENCE_FILE` / `enrolled_licence_path` / `save_enrolled_licence` in `store.rs`; `AppState::with_station_id` / `with_settings_path` wired in `build_station`; `tests/station_enrollment.rs` fake hub and its 15 tests; the desktop view-model, `QrCode` and the static render tests; docs and route golden.

## Review Triage Log

### 2026-11-10 — Review pass
- verdicts: 37 findings — high 0, medium 10, low 25, false 2, maybe-false 0
- findings:
  - Blind Hunter
    - `[medium]` `[defer]` Tauri desktop shell has no vendor root, so the panel is always unavailable there — pre-existing documented limitation (desktop refuses runtime install); depends on D5. Deferred; the CONTEXT row now has to say so (folded into Tasks).
    - `[medium]` `[bad_spec]` Enrollment allowed with an instance key generated for this run only — confirmed: `load_or_create_signing_key(None)` generates a fresh key; a seat is spent on a licence lost at restart. Amended: autonomous decision, `can_enroll` false / 409, matrix row "Key not kept".
    - `[medium]` `[defer]` Restart loses the installed licence (advice not followable on desktop; ADR-0013 raises stakes) — the already-deferred start-up auto-load item; desktop cannot enroll at all (row above).
    - `[medium]` `[bad_spec]` Hub-supplied approval links rendered unchecked (`javascript:` href) — confirmed: `verification_uri_complete` goes straight into `<a href>` and React 18 renders it. Amended: decision + rule (http/https only, else `refused`) + matrix row.
    - `[low]` `[reject]` Over-long link makes `qr.make()` throw — needs a hostile hub sending a ~2 KB link; with links checked server-side and the hub operator-chosen, a try/catch guard is not worth it.
    - `[low]` `[bad_spec]` CLI wording ("run wipestation enroll again", `--vendor-root-b64`) on the bench page — confirmed, the spec required `EnrollError`'s text. Amended: decision + message rule.
    - `[low]` `[bad_spec]` Installed panel has no way back to the form — confirmed. Amended: **Register again** in the Installed rule.
    - `[low]` `[reject]` Files saved before the install is checked — the licence already passed the same `check_installable`; keeping a verified licence on disk is the spec's deliberate order (never lose an approved licence).
    - `[low]` `[reject]` Cancel racing a collecting poll — microsecond window after a deliberate operator action; the licence remains downloadable from the portal; a guard adds locking for an undemonstrated case.
    - `[low]` `[reject]` Unauthenticated routes reach out to an operator-given origin — same posture as every station route (CONTEXT records `POST /api/license` likewise); http only to loopback; station auth is D2.
    - `[low]` `[bad_spec]` Countdown capped at a day but the poll deadline is not — confirmed; folded into Tasks (same cap for the poll loop).
    - `[low]` `[bad_spec]` Error-mapping unit test misses variants — folded into Tasks.
    - `[medium]` `[bad_spec]` Page container logic untested — grouped with the verification-gap row; Tasks gain `LicencePage.flow.test.ts`.
    - `[low]` `[defer]` Doc counts stale (AGENTS.md; README misses unit-test rows) — AGENTS.md is agent-context, deferred; README inventory rows folded into Tasks.
    - `[low]` `[reject]` Corrupt settings file silently overwritten — rejected in story 3 for the same code; one field exists.
  - Edge Case Hunter
    - `[medium]` `[bad_spec]` `javascript:` link runs in the station UI — same as Blind's link row.
    - `[low]` `[reject]` QR overflow or non-Latin-1 link — URLs are ASCII; overflow as Blind's QR row.
    - `[low]` `[reject]` Cancel between poll end and `end()` still installs — as Blind's race row.
    - `[low]` `[reject]` Overlapping starts from two clients poll a replaced code — needs two near-simultaneous Register clicks; ends `refused` and Start again recovers; an async mutex is a new guard.
    - `[low]` `[bad_spec]` Expiry cap mismatch — same as Blind's row; folded.
    - `[low]` `[reject]` View `interval` stale after `slow_down` — the page re-reads on its own 2 s timer and never uses `interval`; fixing needs a callback into the poll loop.
    - `[low]` `[reject]` Polling task outlives a dropped `AppState` — the state lives for the process; tests end with the runtime.
    - `[low]` `[reject]` Install failure after saving leaves files — as Blind's save-order row.
    - `[low]` `[bad_spec]` Installed state offers no action — same as Blind's row; folded.
    - `[low]` `[bad_spec]` Failed Cancel/Start again is silent — confirmed (only `start.error` shown); folded into Tasks.
    - `[low]` `[bad_spec]` A failed refetch hides the pending code — confirmed (`isError` checked before data); folded into Tasks.
  - Verification Gap
    - `[medium]` `[bad_spec]` Save failure with a settings path is never tested — folded as matrix row "Settings dir unwritable".
    - `[medium]` `[bad_spec]` `LicencePage` wiring (refetch, licence invalidation, Start again, 400 text) never mounted in a test — folded as `LicencePage.flow.test.ts`.
    - `[low]` `[reject]` Files written before install (other finding) — as Blind's save-order row.
  - Intent Alignment
    - `[medium]` `[bad_spec]` Desktop-shell gap is not documented in the new CONTEXT row — folded into the docs task.
    - `[medium]` `[bad_spec]` Stateful page transitions untested — same as the verification-gap row.
    - `[low]` `[reject]` QR content not decoded in tests — produced by the library from the exact string; a decoder dependency for a test is not worth it.
    - `[low]` `[reject]` Countdown steps every 2 s — still a countdown; a local ticking timer adds state for cosmetic smoothness.
    - `[false]` `[reject]` "Licence surface" built as a new page — the recorded autonomous decision; the banner's rule forbids a button.
    - `[false]` `[reject]` Start again does not re-register in one step — the spec defines Start again as back to the form with the address kept; the diff matches.
    - `[low]` `[reject]` A hub vanishing mid-attempt ends `expired`, not `unreachable` — needs a 10-minute outage; transient retries are story 3's rule.
    - `[low]` `[reject]` Install refusal leaves saved files — as Blind's save-order row.

### 2026-11-10 — Review pass (after loopback 1)
- verdicts: 30 findings — high 0, medium 5, low 22, false 3, maybe-false 0
- findings:
  - Blind Hunter
    - `[low]` `[reject]` Cancel or a second start loses a collected licence; cancel cannot stop a running install — carried: pass-1 race row, code unchanged there.
    - `[low]` `[reject]` Register again on an already-registered key meets the hub's duplicate-key refusal and the bench just sees the code expire — the approving owner sees the refusal in the portal (story 2), which is where the fix (release the seat) happens; a bench warning adds copy and a licence lookup for a rare re-registration.
    - `[low]` `[reject]` Files saved before the install is checked, and the failure message does not say so — carried: pass-1 save-order row.
    - `[low]` `[reject]` Unauthenticated start/cancel; a bodiless cross-site form POST can cancel — carried posture row: every station route (job abort included) is unauthenticated with permissive CORS; station auth is D2.
    - `[low]` `[reject]` Loopback ports can be probed through the start's error text — carried posture row; http only to loopback, which the operator's own network peer can already reach.
    - `[medium]` `[defer]` Tauri shell can never register and the reason names a flag it cannot take — carried: pass-1 desktop deferral (D5).
    - `[low]` `[patch]` Panel says "platform" while the station's messages say "the hub at …" (CONTEXT §14 **Hub** is something else) — reworded the bench-app messages in `enrollment.rs` to "the platform at …"; CLI text and `HUB_ORIGIN_RULE` unchanged.
    - `[low]` `[reject]` A failed re-read freezes the countdown with no "not updating" note — the page is served by the same station, so a station that stops answering takes the whole UI with it; adding a note is a new branch.
    - `[low]` `[reject]` Pending `interval` stale after `slow_down` — carried: pass-1 row.
    - `[low]` `[defer]` AGENTS.md test counts stale — carried: deferred item.
    - `[false]` `[reject]` ROADMAP A2.1 / L-R4 not struck — they ship with story 5's signed licence fetch; neither is shipped yet, and both already name `station-enrollment`.
    - `[low]` `[patch]` CLI save-failure warning relies on the store error to name the path — grouped with the verification-gap path row; assertion added.
    - `[low]` `[reject]` `backToForm`/`onCancel` duplicate and `endedHeading` covers non-ended states — no user-facing harm; naming only.
  - Edge Case Hunter
    - `[low]` `[reject]` Cancel during synchronous `install()` — carried: pass-1 race row.
    - `[low]` `[reject]` Install refused after files written — carried: pass-1 save-order row.
    - `[low]` `[reject]` QR overflow on an over-long link — carried: pass-1 QR row.
    - `[low]` `[reject]` Approval link on a host other than the hub's — the hub is the operator-chosen, TLS-authenticated origin and the portal may be served from another origin; story 3 rejected the same for printed links.
    - `[low]` `[reject]` GET between reset and the hub's start answer shows the form — the tab that clicked Register keeps the button disabled while the start is in flight; another tab racing it is the carried overlapping-starts row.
    - `[low]` `[patch]` `startEnrollment` doc omits the run-only-key 409 — comment corrected.
  - Verification Gap
    - `[medium]` `[patch]` A successful Register is never shown to move to the pending code — added a flow test (200 pending start shows code and QR, re-reads, trims the label).
    - `[medium]` `[patch]` Nothing checks a failed licence save names the file — assertion added to the unwritable-dir test.
    - `[low]` `[reject]` Cancel racing install (other finding) — carried: pass-1 race row.
    - `[low]` `[patch]` `startEnrollment` doc comment incomplete (other finding) — same as Edge's row.
  - Intent Alignment
    - `[medium]` `[defer]` In the shipped Tauri shell the success state is unreachable — carried: desktop deferral.
    - `[medium]` `[defer]` Enrollment lasts only until restart — carried: auto-load deferral.
    - `[low]` `[reject]` No test pins the Rust serde shape against the TS mirror — repo-wide convention (AGENTS.md: mirrors change together, checked in review); a cross-language shape test is new infrastructure.
    - `[low]` `[reject]` QR content never decoded — carried: pass-1 row.
    - `[low]` `[reject]` Countdown only moves on re-read — carried: pass-1 row.
    - `[false]` `[reject]` "Licence surface" built as a new page — carried: pass-1 row.
    - `[false]` `[reject]` `refused` and `failed` states go beyond denied/expired/unreachable — the intent contract defines both; not a divergence.

## Verification

**Commands:**
- `cargo test -p wipe-server -p wipe-cli` then `cargo test --workspace` -- all pass (the known `health.rs` timing flake aside).
- `cargo clippy -p wipe-server -p wipe-cli --all-targets -- -D warnings` -- clean (pre-existing `wipe-engine` lint aside).
- `UPDATE_GOLDENS=1 cargo test -p wipe-server --test route_golden`, then without the variable -- pass.
- `cd apps/desktop && pnpm typecheck && pnpm test && pnpm build` -- pass.
- `rustfmt --edition 2021 --check <touched .rs files>` -- clean.

## Auto Run Result

Status: done

**Summary.** Operators can register a station with the platform from the bench app (ADR-0008, CAP-4).
- **Licence page.** A new **Licence** page (nav item, `/licence`) shows the station's licence status and a **Register with platform** panel. The operator enters the platform address (prefilled from `station-settings.json`) and an optional label. The panel then shows the user code, the address, the approval link and its QR code, and an `m:ss` countdown, plus Cancel.
- **Outcomes.** Once an owner approves, the panel reads "Licensed" and offers **Register again**. Denied, expired, unreachable, refused and failed outcomes each say what happened in bench wording and offer **Start again**.
- **Station routes.** `GET /api/enrollment`, `POST /api/enrollment` and `POST /api/enrollment/cancel` run one enrollment inside the station through `hub_client`, with the polling half split out as `poll_until_done`. On approval the station verifies the licence against its own vendor roots, saves it as `enrolled-licence.json`, saves the platform address, and installs it in process.
- **When registration is refused.** The station refuses to start an enrollment when it trusts no vendor root, or when its instance key was generated for this run only (no `--key-path`). A start whose approval links are not http(s) ends `refused`.

**Files changed**
- `crates/wipe-server/src/enrollment.rs` (new): view types, attempt slot, start/cancel/view, polling task, save and install, bench messages, link check.
- `crates/wipe-server/src/app.rs`, `handlers.rs`, `bootstrap.rs`, `lib.rs`: `AppState` station id, settings path, key-kept flag and enrollment slot, plus the three routes.
- `crates/wipe-server/src/hub_client.rs`: `poll_until_done` split out and `MAX_EXPIRES_IN_SECS` cap.
- `crates/wipe-server/src/store.rs`, `crates/wipe-cli/src/main.rs`: the enrolled-licence file helper moved into the server and shared with the CLI.
- `crates/wipe-server/tests/station_enrollment.rs` (new, 18), `tests/bootstrap.rs`, `tests/hub_client.rs`: route tests against a signature-checking fake hub, plus the wiring and expiry-cap tests.
- `apps/desktop/src/pages/LicencePage.tsx` (+ static and mounted flow tests), `api/enrollment.ts` (+ test), `api/types.ts`, `api/client.ts`, `main.tsx`, `components/Layout.tsx`, `package.json`/lockfile (`qrcode-generator`, MIT): page, view-model, mirrors, calls, route and nav.
- `docs/generated/http-routes.txt`, `CONTEXT.md` (§10.2 row; §14 **Enrollment**), `README.md`: route golden and docs.

**Review.** Two passes, with one loopback.
- **Pass 1:** 37 findings (10 medium, 25 low, 2 false). It looped back because three spec-level decisions were needed: refuse a station whose key does not last, word outcomes for the bench, and accept only http(s) approval links. The low and medium fixes were folded into the spec at the same time, and the code was re-derived from the saved pass-1 patch.
- **Pass 2:** 30 findings (5 medium, 22 low, 3 false). Patched: 2 medium entries (a mounted test for a successful Register, and an assertion that a failed save names the licence file) and 2 low (bench messages say "platform" rather than "hub", and the `startEnrollment` doc comment).
- **Deferred:** 3, listed in `deferred`: the Tauri shell has no vendor root, so it cannot register; nothing loads the licence at start-up; AGENTS.md test counts are stale.
- **Rejected:** each with its reason in the Review Triage Log. These are mainly races in the moment a deliberate Cancel lands, the unauthenticated station API (CONTEXT records this posture; station auth is D2), the save-before-install order, QR overflow from a hostile platform, and cosmetic countdown and interval items.

**Follow-up review recommended: true.** Pass 2 patched 2 medium entries. The unverified risk is the end-to-end path: no run has driven the Licence page in a browser against the real `wipe-hub` and the portal's `/activate` approval. Every station test uses a fake hub, and the commerce demo covers only the CLI.

**Verification**
- `cargo test --workspace --no-fail-fast`: everything passed except `wipe-hub` `tests/health.rs::silent_smtp_and_database_time_out_within_bound`. That is the pre-existing timing failure recorded by stories 1–3.
- After the pass-2 patches, `cargo test -p wipe-server -p wipe-cli`: 213 passed, 0 failed. This includes `station_enrollment` (18) and the route golden.
- `cargo clippy -p wipe-server -p wipe-cli --all-targets -- -D warnings -A clippy::large_enum_variant`: clean. The allowed lint fails only in untouched `wipe-engine`.
- `rustfmt --edition 2021 --check` on touched `.rs` files: clean. Only the untouched `ws.rs` is flagged, pulled in through `lib.rs`.
- `cd apps/desktop && pnpm typecheck && pnpm test && pnpm build`: pass, 416 tests.
- Matrix audit: all 14 rows are covered by passing tests in `tests/station_enrollment.rs`.

**Residual risks**
- In the Tauri desktop shell the panel always shows registration as unavailable, because it has no vendor root and depends on D5. Registering works under `wipestation serve --vendor-root-b64 … --key-path …`.
- An installed licence lasts until restart unless the station is started with `--license <saved file>`.
- A Cancel that lands exactly while the licence is being collected can leave the seat spent, or the licence installed while the panel shows nothing.
- The station API is unauthenticated, so any client on the network can start, replace or cancel an attempt.

