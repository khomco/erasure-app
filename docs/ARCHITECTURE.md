# Wipestation Architecture

## Layering

Station side (one `wipestation` process per bench):

```
+----------------------------------------------------------------+
|  apps/desktop  (React/Vite bundle)                             |
|   • served by wipe-server at / to browsers and tablets         |
|   • also shown in the Tauri 2 window (see hosts below)         |
+----------------------------------------------------------------+
                              |
                              |  HTTP REST + WebSocket
                              v
+----------------------------------------------------------------+
|  hosts: wipe-cli (binary `wipestation`)                        |
|         wipestation-desktop (Tauri 2 shell, src-tauri)         |
|   each starts wipe-server in-process                           |
+----------------------------------------------------------------+
                              |
                              v
+----------------------------------------------------------------+
|  wipe-server  (Axum)                                           |
|   • /api/health, /api/station, /api/public_key                 |
|   • /api/fleet/peers, /api/fleet/lead                          |
|   • /api/devices, /api/devices/:id/capabilities                |
|   • /api/bay-topology (GET/PUT), …/config, …/resolve,          |
|     …/store, …/store/acknowledge; /api/enclosure-catalog       |
|   • /api/jobs[/:id], …/start, …/abort,                         |
|     …/record-for-destruction, …/certificate                    |
|   • /api/destruction/records, /api/destruction/statement       |
|   • /api/manifests[/:id] (destruction runs), …/completion,     |
|     …/concurrence, each with …/statement                       |
|   • /api/destruction/policy (read-only)                        |
|   • /api/sim/devices[/attach|/detach] — always routed, 404     |
|     unless the backend simulates hot-plug                      |
|   • /api/events (WS) — JobBroadcast + FleetEvent fan-out       |
|   • Auto-signs Certificate on Erased|Destroyed (schema v3);   |
|     certs held in RAM only                                     |
+----------------------------------------------------------------+
          |               |               |               |
          v               v               v               v
 +--------------+ +-----------------+ +--------------+ +-----------------+
 | wipe-engine  | | wipe-fleet      | | wipe-cert    | | wipe-license    |
 |  • Runner    | |  • mDNS adv+brw | |  • Schema    | |  • root→licence |
 |  • State mch | |  • Lead elect.  | |  • Canonical | |    →key→cert    |
 |  • Backend△  | |  • Peers        | |  • Ed25519   | |  • lease (RAM)  |
 +------△-------+ +-----------------+ +--------------+ +-----------------+
        │
        │ trait impl
 +------┴-----------+
 | wipe-engine-mock |  (today: simulated)
 | wipe-engine-linux|  (future: ioctl + raw I/O)
 +------------------+

 +------------------+
 | wipe-common      |  (types: Device, Method, Capabilities, Job,
 |  • shared types  |   ErasureEvent, StationInfo, Evidence);
 |  • method select |   depends on no sibling crate
 +------------------+
```

`wipe-server` is a library with two hosts:

- **`wipe-cli`**, the `wipestation` binary: `serve` (all station options),
  `bay-presets`, `verify-cert`, `inspect`, and the vendor-side
  `license new-root | issue | inspect`.
- **`apps/desktop/src-tauri`**, package `wipestation-desktop`: the Tauri 2
  shell, which starts `wipe-server` in-process on `127.0.0.1:7878` (or
  `WIPESTATION_API_ADDR`) with fixed defaults and none of `serve`'s
  options (see Operating modes).

Vendor side (separate binary, ADR-0007):

```
+----------------------------------------------------------------+
|  wipe-hub  — binary `wipe-hub` (serve | admin | release)       |
|   Axum + Postgres; accounts, payments, entitlements, licence   |
|   issuing, releases and downloads; serves apps/portal          |
|   depends on wipe-common, wipe-cert, wipe-license only         |
+----------------------------------------------------------------+
```

No station crate depends on `wipe-hub`, and no station talks to it: a
licence the hub issues reaches a station as a file passed to
`wipestation serve --license`.

## Why Rust + Tauri (not Bun + Hono)

The hybrid Bun+Rust design from earlier in scoping collapsed to all-Rust once we acknowledged:

1. **The engine code wants to live in Rust** anyway (ioctl, raw block I/O, audit story).
2. **Tauri is a Rust app** with an OS-native webview — adding Bun back in would mean two processes and a network hop for what should be a single binary.
3. **Everything Bun was buying us (HTTP server, SQLite, cert generation) has perfectly good Rust equivalents** that don't add audit surface: Axum, rusqlite (added when the Enterprise data model lands; station *configuration* uses a plain JSON store — ADR-0003), ed25519-dalek.
4. **The team scaling concern** flips around: if the team needs to maintain Rust for the engine, doubling down on one language is easier than maintaining a polyglot codebase.

The frontend stays TypeScript because that's where TS earns its keep — the React + Tailwind ecosystem is genuinely faster than any native UI toolkit for the table-heavy, wizard-driven, dashboard-style UI this product needs.

## Trait seam: `DeviceBackend`

`wipe-engine::DeviceBackend` is the single seam between the orchestrator and
hardware-touching code. It hands the orchestrator a small async surface:

- `enumerate()` — read-only catalog
- `capabilities(id)` — probe a device
- `unfreeze(id)` — recover from ATA frozen state
- `issue(id, method)` — start the sanitize command, returns a `BackendHandle`
- `poll(handle)` — non-blocking progress checkpoint, returns `InProgress`/`Completed`/`Failed` with `CommandEvidence`
- `cancel(handle)` — best-effort abort
- `verify(id, method, samples)` — post-erase sampled reads

`wipe-engine-mock` simulates all of this against an in-memory catalog. A
future `wipe-engine-linux` will issue `NVMe_IOCTL_ADMIN_CMD` / `SG_IO`
ioctls and read `/dev/sdX` with `O_DIRECT`. The orchestrator does not
change.

## Job lifecycle (ADR-0001)

Two nested state machines. The **outer `Job`** tracks the Asset's
disposition; the **inner `ErasureEvent`** tracks one wipe attempt. Today a
Job holds at most one `ErasureEvent` (none if it is recorded for
destruction or aborted from `Queued`, or created from a scanned group).

Outer — `JobState`:

```
Queued ─▶ InProgress ─┬─▶ Erased
                      │
                      ├─▶ PendingDestruction ─▶ Destroyed
                      │     (recorded for destruction; completing
                      │      its destruction run, plus any required
                      │      concurrence, finalises it)
                      ├─▶ Quarantined                 (declared; nothing
                      │                                  transitions into it)
                      └─▶ Aborted                       (operator)

Queued also moves straight to PendingDestruction when recorded for
destruction, and a scanned drive's Job is created in PendingDestruction.
Aborted is reachable from any non-terminal state, including Queued and
PendingDestruction (no certificate is signed then). PendingDestruction
serializes as pending_destruction and still accepts pending_co_sign.
```

Inner — `ErasureEventState`, one per attempt:

```
Queued ─▶ Probing ─▶ (Unfreezing?) ─▶ Confirming ─▶ Running ─▶ Completed
                                       │
                                       ├─▶ Failed   (a command poll
                                       │             reports failure)
                                       └─▶ Aborted  (operator)

A backend error from the capabilities probe, unfreeze or issue leaves
the event in its last state rather than Failed.
```

The inner machine ends at `Completed`. Post-erase verification is not an
inner state: the runner then runs the sampled reads and appends a sibling
`JobActivity::Verification` to the Job; if the sampled reads fail, the
event stays `Completed`, the failing Verification is recorded and the Job
stays `InProgress`. Certificate signing is not an inner state either;
it is done by `wipe-server` (below) when the outer Job reaches `Erased`
or `Destroyed`.

A `Failed` ErasureEvent does **not** fail the Job — the Job stays
`InProgress` awaiting an operator decision: abort, or record for
destruction. There is no retry or method-fallback route yet.
A Job keeps its attempt and results in `activities: Vec<JobActivity>`;
after the attempt completes the runner appends a sibling
`JobActivity::Verification`, and when the Job is recorded for
destruction a `JobActivity::Destruction` carrying the operator's
`DestructionAttestation` (ADR-0011 §1). The runner's
`record_for_destruction` refuses an event with no attestation, and
`record_scanned_for_destruction` creates one Job per scanned drive,
all-or-nothing, refusing a drive that matches an open Job's asset tag or
serial. Only these two enter `PendingDestruction`, and each broadcasts
`ActivityAdded` then `JobStateChanged`. `Diagnostic` and `HealthCheck` variants
exist in the type but the runner never emits them.

Every transition emits a `JobUpdate` of kind `StateChanged` (renamed
from `JobEvent` in v0.2). Each command issued emits `CommandIssued` and
`CommandResult`; progress emits `Progress`; verification emits
`Verification`. The runner's broadcast channel carries three envelope
kinds — `JobBroadcast::JobStateChanged` (outer transitions),
`JobBroadcast::ActivityAdded` (a typed activity was appended), and
`JobBroadcast::ErasureUpdate` (a `JobUpdate` from inside a running
attempt) — which flow channel → WebSocket → frontend.

## Certificate flow

1. Job reaches `JobState::Erased` **or** `JobState::Destroyed` — once, so
   a certificate is signed once. (There is no `JobState::Completed` — that
   name belongs to the inner `ErasureEventState`.) Entering
   `PendingDestruction` signs nothing.
2. `wipe-server::AppState::spawn_cert_generator()` (a broadcast subscriber)
   wakes up.
3. For `Erased` it calls `Certificate::from_job(...)`, which flattens the
   job's `activities` chain into a JSON-LD payload (schema v3, no
   `destruction` key). For `Destroyed` it reads the Job's destruction run
   from `AppState::manifests` and calls `Certificate::from_destroyed_job(job,
   run, ...)`: `disposition: destroyed`, `media_status` `operational: false,
   damaged: true`, `sanitization` describing the destruction technique
   (category `destroy` only for the five NIST techniques), and a
   `destruction` block (`DestructionRecord`: run id, the attested
   Destruction activity's id, technique, honest label, `nist_destroy`,
   caution, the run's completion, `required_concurrence` and concurrence).
   A Job never erased has no `device` / `capabilities_snapshot`, its
   validation `media_class` is `unknown`, and its `started_at` is its
   `created_at`; its identifiers are on the attestation.
4. `wipe_cert::sign(cert, signing_key)` canonicalizes the JSON (BTreeMap key
   sorting at every depth, finite-number check), hashes it with SHA-256,
   signs with Ed25519, and emits a `SignedCertificate { certificate,
   signature }`.
5. The signed cert is stashed in `AppState::certs` and served by
   `GET /api/jobs/:id/certificate`.
6. **Destroy path only.** Assembling a destruction run
   (`DestructionManifest`) stamps its id on each member Job
   (`manifest_id`) and that Job's latest DestructionEvent
   (`manifest_ref`), and signs nothing; a Job already on a run cannot be
   recorded for destruction again, so a new DestructionEvent cannot
   replace the record after assembly (aborting a linked Job is still
   allowed). Confirming the run's completion (and, when the station's
   concurrence policy requires it, a concurrence from a different signer)
   first builds every member's `DestructionRecord` against the run as it
   will be stored (`check_certifiable` in `advance_run`); any failure is a
   409 naming the Jobs and nothing changes, so no Job reaches `Destroyed`
   uncertifiable. Then the runner moves every member Job to `Destroyed` in
   one call, all or nothing, with a `JobStateChanged` per Job, and the
   generator certifies each. The run is stored under the same lock held
   while the Jobs move, so the generator reads exactly the run the
   preflight checked. The certificate follows the response asynchronously.
   `wipe_cert::co_sign` is gone; `CoSignatureBlock` / `co_signatures` /
   `verify_co_signatures` stay so pre-ADR-0011 (v2) certificates still
   verify.

`wipe_cert::verify(signed, &[trusted_keys])`, the one verifier (the CLI and
the tests call it):
- Re-canonicalizes the embedded certificate to compute the actual
  SHA-256.
- Compares against the signature's `canonical_sha256_hex` (fails closed on
  any payload mutation).
- Looks up a matching trusted key by `public_key_id`.
- Verifies the Ed25519 signature.
- Then checks content by `cert_format_version` (`wipe_cert::check`): v2 is
  signature-only, as it always was; v3 refuses any co-signature, a
  `destruction` block on a certificate that is not destroyed (or a
  destroyed one without it), a record whose event link, label, flag,
  caution, `sanitization` method or category disagrees with its technique,
  a statement that does not re-render from its fields through
  `wipe_common::{attestation_statement, completion_statement,
  concurrence_statement}`, a completion not verified, and a concurrence
  that is missing, unrequired, of the wrong kind or by the completion's
  signer; any other version is refused as `unsupported cert_format_version
  N`. Every field v3 added or relaxed is omitted when absent, so v2
  certificates re-canonicalize to their signed bytes; committed fixtures
  in `crates/wipe-cert/tests/fixtures/` (generated by the pre-v3 code) pin
  it.

This is the offline-verification path. The CLI command
`wipestation verify-cert <cert> --public-key-b64 <station key>` exposes
it; with `--vendor-root-b64` it also checks the ADR-0005 attestation chain
(who the station key was licensed to), and `--require-licensed` fails an
evaluation cert; it takes effect only together with `--vendor-root-b64`.
It checks the primary signature and the content, never v2 co-signatures,
and prints the format version, the device (or "not read from the device"
for a drive never erased) and, for a destroyed drive, the technique and
its label, the caution, who recorded it for destruction (name, title,
identity assurance, time, identifiers and their source), the completion
(time, equipment, criterion, video) and the concurrence or "none
required". The signed
cert lives only in the station's RAM, so it must be downloaded before the
station restarts.

## Station configuration store

Evidence never touches local storage. *Configuration* — today just the bay
topology — does, through a `TopologyStore` seam whose tier the station
detects for itself at startup (ADR-0003):

```
  writable config path?  ──yes──▶  LocalFile     atomic write, survives reboot
        │ no
  control plane set?     ──yes──▶  ControlPlane  stub: probe always fails,
        │ no                                     so detection falls through
        ▼
  Tier 3: ask the operator ──acknowledged──▶ Ephemeral   RAM only, stated in the UI
```

Tier 2 is a stub: `--control-plane-url` is accepted, but no HTTP client
is wired in and `ControlPlaneStore::probe` always returns an error, so a
station with no writable path always ends up on the ephemeral tier. Nothing
serves the control-plane side; `wipe-hub` does not.

Detection is a write probe — create a temp file in the target directory and
remove it — rather than an inference from mount flags or uid, which get
read-only NFS, overlayfs and full disks wrong.

The current tier is operational state an operator may have to explain, so it
is served at `GET /api/bay-topology/store` and shown in the UI rather than
buried in a log line.

## Fleet discovery

`wipe-fleet::FleetService::start(StationInfo)` performs:

1. Spawn `mdns_sd::ServiceDaemon`.
2. Register `_wipestation._tcp.local.` with TXT records: `id`, `role`,
   `version`, `port`, `started` (unix ts), `active` (job count).
3. Browse the same service type; on `ServiceResolved`, decode peers and
   stash in a registry.
4. Recompute lead deterministically: `min_by_key(|s| (s.started_at, s.id))`.

This is intentionally simple. Raft / SWIM gossip is the wrong amount of
machinery for a 5–50-station LAN; cross-LAN fleets are left to a future hub/cloud tier.
The seam to add full gossip later is the `FleetService` API — peers and
lead are read-side; the discovery transport is pluggable.

## Operating modes

| Mode | Binary | Frontend |
| --- | --- | --- |
| Standalone, browser | `wipestation serve` | Same React bundle at `http://127.0.0.1:7878` |
| Standalone, native window | `wipestation-desktop` (`cargo run -p wipestation-desktop`, or `pnpm tauri dev` / `pnpm tauri build`) | Tauri 2 window over an in-process `wipe-server` on `127.0.0.1:7878`. Only `WIPESTATION_API_ADDR` is honoured (it moves the API; the window URL stays `127.0.0.1:7878`); none of `serve`'s options apply: no topology store, licence or simulator, default mock timing, and a fresh signing key and random station id on every launch. It advertises on mDNS, so each launch appears as a new peer. If the port is taken the bind error is only logged and the window shows whatever answers there |
| Headless, no mDNS | `wipestation serve --no-fleet` | Same bundle; tablets must be given the address |
| Headless wipestation (PXE / rack) | `wipestation serve --no-ui` (future flag) | none on device; operator tablet connects to it |
| Roaming operator tablet | Same React bundle | Pointed at any station's `/api` |
| Hub multi-site fleet view | `wipestation serve --hub` (future) | Same React bundle in "fleet" mode |

The vendor hub is not a station mode: it is the separate `wipe-hub` binary
(ADR-0007) serving `apps/portal`.

## Pricing-model implications

Two SKUs share one station binary:

- **Tier 1 — Station / annual unlimited.** No cloud. Local signing with a
  station key (`--key-path` keeps it across restarts; without it a new key
  is generated on every start). YubiKey/PIV signing is future work. LAN
  discovery + lead election. Air-gap clean. The KillDisk-like pricing
  model, without the legacy crapness.
- **Tier 2 — Cloud / per-station + cloud features** (future). Cloud-side
  cert archive, multi-site fleet view, customer-facing cert portal, ITAM
  integrations, SAML/OIDC, cloud KMS signing. None of this exists, and
  there is no station flag that points a station at a cloud or hub.

What exists today: a station is licensed by a licence file passed with
`wipestation serve --license <file> --vendor-root-b64 <root>` (ADR-0005);
without one it runs free-tier and marks every certificate `evaluation`.
The vendor hub (`wipe-hub`) sells entitlements and issues those licence
files for customers to download from the portal; stations never contact
the hub, so air-gapped fleets need nothing extra. Licensing never stops an
erasure.

## Where the code will need to grow

Replace, add, or extend:

1. `wipe-engine-linux` crate — real `nix::ioctl_*` calls, `SG_IO`,
   `NVME_IOCTL_ADMIN_CMD`, `/dev/sdX` O_DIRECT reads.
2. `wipe-license` (exists) — the attestation chain, entitlements and the
   in-payload `evaluation` marker are built; licences verify against vendor
   root keys passed at run time. Remaining: persist lease state (the
   anti-rollback watermark and quota count reset on every restart), wire
   in allowed-method and feature checks, and the online reconciliation
   client (`client.rs` is a seam with no server).
3. `wipe-hub` (exists) — accounts, payments, entitlements, licence issuing,
   releases and downloads (ADR-0007). Remaining: any station ↔ hub
   protocol (cert upload, licence or config sync, control-plane tier),
   an immutable cert archive and a cert verification API for customer
   portals.
4. `wipe-pdf` — render the signed JSON-LD into a PDF/A-3 with the JSON
   embedded as an attachment, for audit-friendly delivery.
5. Operator RBAC inside `wipe-server` — SSO via OIDC, audit attribution.
   Destruction runs on ADR-0011's operator attestation (recording,
   completion and a startup-set concurrence policy are built; role-gated
   policy editing waits for RBAC; the certificate is signed once at
   `Destroyed`, schema v3).
6. `wipe-iso` — bootable image build (Alpine + signed shim + GRUB +
   the binary).
