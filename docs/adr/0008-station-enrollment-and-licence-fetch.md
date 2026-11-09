# Stations enroll and fetch their licences from the hub by device authorization

**Status:** accepted (2026-11-08). Decides the first part of ADR candidate 0008 (ROADMAP §6,
DECISIONS D6): how a station registers with a customer's organization from the bench, and how
a connected station collects its licence and its renewals. Usage reporting (A2.2), revocation
distribution (A2.3) and station inventory (A2.4) stay open and will amend this ADR.

## Context

Today a station is registered only from the customer portal: an owner types the station's
instance key id, the hub issues a licence file (ADR-0007 §5), and someone copies that file to
the station and restarts it with `--license`. That is the offline path ADR-0005 promises, and it
stays.

The owner raised two needs on 2026-11-08:

1. **Register from the station.** Some customers allow the bench a single firewall rule,
   outbound HTTPS to the platform, and nothing else. Copying a file across from an office
   machine is the step that goes wrong. Registering from the bench should be possible, but
   **only with the approval of a signed-in, authorized user of that organization: nobody may
   register a station with a tenant anonymously.**
2. **Connected stations renew by themselves.** Annual offerings renew as Stripe subscriptions
   (D15); a connected station should collect its renewed licence without anyone touching it.

A bench is a shared machine, often a kiosk or a PXE-booted station with no keyboard worth the
name. Typing an owner's portal password there would put a tenant credential on the machine
least able to protect it.

## Decision

### 1. Enrollment uses the OAuth device authorization grant (RFC 8628)

This is the flow TVs and command-line tools use to sign in: the device shows a short code, and
a person approves it on another device where they are already signed in.

1. The operator chooses **Register with platform** on the bench (or runs `wipestation enroll`).
   The station sends the hub its instance public key, station id and label, **signed with its
   instance key**. The hub checks that the key id matches the key and the signature matches,
   so nobody can start an enrollment for a key they do not hold.
2. The hub answers with a **user code** (8 characters from a 20-letter alphabet with no vowels
   or look-alikes, shown as `BCDF-GHJK`, about 34 bits), the address to enter it at, and a
   separate secret **device code** (256 bits, stored only as a hash) that the station polls with.
3. On any phone or computer, an **organization owner** signs in to the portal, opens the
   activation page and enters the code. The portal shows what is asking: the station id, label,
   key id, the network address the request came from and when. The owner checks it against the
   bench, picks the entitlement (and so the seat) to use, and approves or denies.
4. Approval registers the station through the same code path as portal registration
   (`stations::register`): the same seat check, the same refusals, the same audit, in one
   transaction with marking the enrollment approved.
5. The station, polling every 5 seconds with its device code (and signing each poll), receives
   the signed licence, verifies it **against the vendor root it already trusts**, and installs
   it without a restart.

Rules:

- **No anonymous registration.** A station can only *ask*. The organization is chosen by
  whoever approves, and only an owner of that organization can approve. Nothing on the bench
  holds or sends a portal credential.
- **Codes are short-lived and single-use.** An enrollment expires after 10 minutes and is
  consumed by its first approval, denial or licence collection. A key has at most one pending
  enrollment; starting again replaces it. Polling faster than the interval gets `slow_down`
  (RFC 8628 §3.5), which adds 5 seconds.
- **Throttled.** Starting enrollments is limited per network address (`/64` for IPv6), and
  code entry per signed-in user, through the existing restart-proof throttle
  (`crates/wipe-hub/src/throttle.rs`). A wrong code reveals nothing about other organizations.
- **Audited.** `enrollment.requested`, `enrollment.approved` (with the approver, station and
  licence) and `enrollment.denied` go to the append-only audit log (ADR-0007 §6). Expiry is not
  audited, to keep idle benches from flooding the log.
- **Who may approve** is the organization owner, the same as portal registration today. Letting
  members approve is a later, separate permission, not a default.

### 2. The station's instance key is its only credential

The station holds no token, password or session. Every station-to-hub request after
enrollment is signed with the instance key over a hub-issued, single-use challenge, the hub's
origin, the method, the path and a hash of the body. Using a hub challenge instead of a
timestamp keeps this working on stations whose clocks are wrong, which ADR-0005 already treats
as normal. A stolen licence file is useless without the key, as before.

### 3. A connected station fetches its own renewals

A station that enrolled online, or whose operator later points it at the hub, checks for a
newer licence when it starts and every 24 hours while running. If its entitlement has renewed,
or the organization's lapse policy changed (ADR-0012), and the station's licence is behind, the
hub **reissues automatically** (the same `stations::reissue` path the owner's button uses,
audited with the station as actor) and returns the new licence. The station verifies and
installs it. If the latest licence is revoked, the fetch returns nothing new in this slice:
telling stations about revocation is A2.3, still open.

### 4. Offline first is unchanged

- The connection is used to register and renew, **never to erase**. A station with a valid
  licence works with no network, forever. An unreachable hub never blocks a wipe, never raises
  a refusal under the `stop` lapse policy (ADR-0012 §5), and shows only as a status line.
- The file path stays for air-gapped sites: portal registration and `--license` (or the
  runtime install from `licence-visibility` story 4) keep working, and an air-gapped customer
  never needs this feature.
- **The station trusts only its configured vendor root.** The hub delivers licences; it cannot
  introduce a new root, so a compromised or impersonated hub cannot license a station.

### 5. Transport

- **HTTPS only**, except to a loopback host for development (the same rule as
  `--stripe-api-base`). The station refuses any other `http://` hub address.
- One outbound destination: the hub's origin. Nothing inbound to the station.
- The station honours `HTTPS_PROXY` and `NO_PROXY`, since sites that allow one firewall rule
  usually route it through a proxy.
- JSON over REST, as every other hub route.
- The hub address is station configuration, kept in the tiered config store (ADR-0003). Only the
  address is configuration. Licences stay licence state.

## Consequences

- One tenant credential fewer on the bench, and a setup with no file copied across.
- The hub must be reachable from customers' networks over real TLS before this ships beyond
  local development. Production hosting and the domain are still open (DECISIONS D1 notes).
- **Remote code phishing** (RFC 8628 §5.4) is the residual risk: someone could persuade an
  owner to approve a code from the wrong station, spending one of that customer's seats on it.
  The approval screen shows the station's id, key id, label and requesting address to make that
  visible, the seat can be released from the portal, and every approval is audited.
- `wipe-license`'s `HttpLicenseClient` seam (ADR-0005 §4) is not this protocol. Usage
  reporting will extend the signed-request scheme here when A2.2 is decided.
- Built by `.scratch/station-enrollment`, queued after `licence-visibility` because the
  station installs licences through that epic's runtime install (story 4).

## Not decided here

- Usage reporting and reconciliation (A2.2), revocation distribution (A2.3), station inventory
  (A2.4), and what a station does if it never reaches the hub again (nothing different from
  today: it keeps its licence until that licence's own expiry).
- Operator identity at the bench (D2, ADR candidate 0009). Enrollment registers a *station*,
  not an operator.
- Site-licence semantics (D4, ADR candidate 0010).
