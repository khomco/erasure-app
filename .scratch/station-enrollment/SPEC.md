---
id: SPEC-station-enrollment
companions: [../../docs/adr/0008-station-enrollment-and-licence-fetch.md, ../../docs/adr/0005-licensing-and-attestation.md, ../../docs/adr/0007-hub-accounts-payments-persistence.md, ../../docs/adr/0013-licensing-controls-every-erasure.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# A station registers from the bench, with an owner's approval, and renews itself

## Why

Owner request, 2026-11-08 (ADR-0008). Some customers allow the bench one outbound HTTPS rule to the platform and nothing else. Today registering a station means typing its key id into the portal, downloading a licence file, copying it to the bench and restarting. Registering from the bench removes the copy step. Nobody may register a station with a tenant anonymously: an owner of that organization, signed in to the portal, approves every enrollment. A connected station then collects its renewed licences itself (D15 renewals, charged and delivered 30 days early under ADR-0013 §4).

## Capabilities

- **CAP-1**
  - **intent:** A station can ask the hub to enroll it, proving it holds its instance key, and poll for the outcome.
  - **success:**
    - A new migration adds an enrollments table: key id, public key, station id, label, requesting peer, user code, device-code hash (never the code), status (pending, approved, denied, collected), expiry, approver, organization, station and licence once approved.
    - `POST /api/enrollments` (no session) takes the instance public key, station id, label and a signature by that key; refuses a key id that does not match the key, a bad signature, or bad text with 400; answers `{device_code, user_code, verification_uri, verification_uri_complete, expires_in: 600, interval: 5}`. A new request for the same key replaces its pending one.
    - The user code is 8 characters from `BCDFGHJKLMNPQRSTVWXZ`, shown `XXXX-XXXX`, matched case-insensitively with or without the dash. The device code is 256 random bits, stored as a SHA-256 hash.
    - `POST /api/enrollments/poll {device_code}` (signed by the same key) answers the RFC 8628 §3.5 errors `authorization_pending`, `slow_down` (interval +5 s), `expired_token`, `access_denied`, or 200 with `{station, licence}` once approved. Collecting the licence consumes the enrollment.
    - Starts are throttled per peer (`enroll:ip:<peer>`, IPv6 by `/64`) through `throttle.rs`; expired rows are purged.
    - `enrollment.requested` is audited with the peer; expiry is not audited.
    - Compose-backed tests cover every refusal, replacement, expiry, slow_down, throttle and the consumed code.
- **CAP-2**
  - **intent:** An organization owner approves or denies an enrollment from the portal, and approval registers the station with the same rules as portal registration.
  - **success:**
    - The portal has an activation page at `/activate` (and `/activate?code=…`), reachable signed out (it sends the user to sign in and back).
    - An owner who enters a pending code sees the station id, label, key id, requesting address and time, and the entitlements with free seats; they pick one and approve, or deny. Members and vendor admins without the organization see a refusal that names who can approve.
    - Approval calls `stations::register` in the same transaction that marks the enrollment approved, so seat, scope and duplicate-key refusals are identical to `POST /api/stations` and leave the enrollment pending.
    - Code lookups are throttled per user (`enroll:user:<id>`); a wrong, expired or used code gets one message that reveals nothing about other organizations.
    - `enrollment.approved` (approver, station, licence) and `enrollment.denied` are audited.
    - Hub tests cover approve, deny, every register refusal, a member, another organization's owner, and the throttle; portal vitest covers the page states.
- **CAP-3**
  - **intent:** An operator can enroll a station from the command line and the station installs its licence without a restart.
  - **success:**
    - `wipestation enroll --hub <origin>` prints the code and address, polls, and on approval verifies the licence against the station's configured vendor root and installs it through the runtime install (`licence-visibility` story 4); with `serve` running, it goes through that server's install route.
    - The hub address is saved in the tiered config store (ADR-0003) as station configuration.
    - The station refuses an `http://` hub except on a loopback host, honours `HTTPS_PROXY`/`NO_PROXY`, and refuses a licence signed by any root it does not trust, with a message saying so.
    - Every station request is signed per ADR-0008 §2 (hub challenge, hub origin, method, path, body hash); the hub's verifier and the station's signer share one implementation in a crate both depend on.
    - `scripts/commerce-demo.sh` enrolls a second station this way, approving through the hub API as the owner, and verifies a licensed certificate.
- **CAP-4**
  - **intent:** An operator can enroll a station from the bench app.
  - **success:**
    - The desktop app's licence surface has **Register with platform**: enter or confirm the hub address, then see the code, the address (and a QR code of `verification_uri_complete`), a countdown, and the outcome, all driven by station routes that wrap CAP-3. Denied, expired and unreachable outcomes each say what happened and offer to start again.
    - Rust route shapes and the TS mirrors (`types.ts`, `client.ts`) change together; vitest covers the screen's states.
- **CAP-5**
  - **intent:** A connected station collects its renewed or policy-updated licence by itself.
  - **success:**
    - `GET /api/station/challenge` issues a single-use challenge (expires in 60 s); `POST /api/station/licence`, signed over it, returns the station's latest licence, or 204 when the station's copy is current.
    - When the station's `renewal_available` is true, the hub reissues through `stations::reissue` (audited with the station as actor) and returns the new licence. Once `controlled-licensing` issues next-term licences early (ADR-0013 §4), the response also carries the station's next-term licence, which the station holds until it starts. A released station gets 404; a revoked latest licence returns nothing new.
    - A station with a hub address checks at start and every 24 hours, verifies against its trusted root and installs. A failure is logged and shown in the licence status, and never blocks or delays a job: an unreachable hub is never a reason to refuse (ADR-0013 §1).
    - Tests cover renewal, policy change, current, released, revoked, a replayed challenge and an unreachable hub.
  - **checkpoint:** an automatic reissue spends no seat but issues a licence without an owner's click; confirm the audit trail reads correctly in the portal before building.

## Constraints

- No anonymous registration: only an owner of the organization approves (ADR-0008 §1).
- No portal credential is ever entered on, stored on or sent from a station.
- The station trusts only its configured vendor root; the hub cannot introduce one.
- Nothing here gates, delays or interrupts an erasure, and an unreachable hub is never a reason to refuse a start (ROADMAP §0.2, ADR-0013 §1).
- The file-based registration and install paths keep working unchanged.
- Every hub change is audited (ADR-0007 §6). Rust serde shapes and TS mirrors change in the same commit.

## Non-goals

- Usage reporting, revocation distribution and station inventory (A2.2–A2.4, the rest of D6).
- Operator sign-in at the bench (D2), site-licence semantics (D4).
- Members approving enrollments; production hosting and TLS for the hub.

## Success signal

- On a bench whose only network access is HTTPS to the hub, an operator clicks Register, an owner approves on their phone, and the bench shows "Licensed" within seconds, with no file copied. A year later, after the subscription renews, the bench shows the new expiry without anyone touching it.
