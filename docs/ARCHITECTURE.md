# Wipestation Architecture

## Layering

```
+----------------------------------------------------------------+
|  apps/desktop  (Tauri 2 + React/Vite)                          |
|   • WebKit/WebView2 native window                              |
|   • Same React bundle is also served by the HTTP API for       |
|     remote tablet operators                                    |
+----------------------------------------------------------------+
                              |
                              |  HTTP REST + WebSocket
                              v
+----------------------------------------------------------------+
|  wipe-server  (Axum)                                           |
|   • /api/devices, /api/jobs, /api/jobs/:id/certificate         |
|   • /api/jobs/:id/escalate-to-destroy, /api/manifests[/:id]    |
|   • /api/events (WS) — JobBroadcast + FleetEvent fan-out       |
|   • Auto-signs Certificate on JobState::Erased|PendingCoSign   |
+----------------------------------------------------------------+
                |               |               |
                v               v               v
       +--------------+ +-----------------+ +--------------+
       | wipe-engine  | | wipe-fleet      | | wipe-cert    |
       |  • Runner    | |  • mDNS adv+brw | |  • Schema    |
       |  • State mch | |  • Lead elect.  | |  • Canonical |
       |  • Backend△  | |  • Peers        | |  • Ed25519   |
       +------△-------+ +-----------------+ +--------------+
              │                                       
              │ trait impl                            
       +------┴-----------+                           
       | wipe-engine-mock |  (today: simulated)       
       | wipe-engine-linux|  (future: ioctl + raw I/O)
       +------------------+                           
                                                      
       +------------------+                           
       | wipe-common      |  (types: Device, Method,  
       |  • shared types  |   Capabilities, Job,      
       |  • method select |   StationInfo, Evidence)  
       +------------------+                           
```

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
disposition; the **inner `ErasureEvent`** tracks one wipe attempt.

Outer — `JobState`:

```
Queued ─▶ InProgress ─┬─▶ Erased
                      │
                      ├─▶ PendingCoSign ─▶ Destroyed   (supervisor co-sign
                      │                                  on a manifest)
                      ├─▶ Quarantined
                      └─▶ Aborted                       (operator)
```

Inner — `ErasureEventState`, one per attempt:

```
Queued ─▶ Probing ─▶ (Unfreezing?) ─▶ Confirming ─▶ Running ─▶ Verifying
                                                                  │
                                                                  ▼
                                              GeneratingCert ─▶ Signing
                                                                  │
                                                                  ▼
                                                              Completed
                                       │
                                       ├─▶ Failed   (any stage)
                                       └─▶ Aborted  (operator)
```

A `Failed` ErasureEvent does **not** fail the Job — the Job stays
`InProgress` awaiting an operator decision (retry, method fallback, or
`escalate-to-destroy`). A Job accumulates its attempts and results in
`activities: Vec<JobActivity>`; on a successful attempt the runner
appends a sibling `JobActivity::Verification`, and on escalation a
`JobActivity::Destruction`. `Diagnostic` and `HealthCheck` variants
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

1. Job reaches `JobState::Erased` **or** `JobState::PendingCoSign`.
   (There is no `JobState::Completed` — that name belongs to the inner
   `ErasureEventState`.)
2. `wipe-server::AppState::spawn_cert_generator()` (a broadcast subscriber)
   wakes up.
3. It calls `Certificate::from_job(...)` which flattens the job's
   `activities` chain into a JSON-LD payload (schema v2), stamping
   `media_status.operational = false` on the `PendingCoSign` path and
   recording the resolved `AssetDisposition`.
4. `wipe_cert::sign(cert, signing_key)` canonicalizes the JSON (BTreeMap key
   sorting at every depth, finite-number check), hashes it with SHA-256,
   signs with Ed25519, and emits a `SignedCertificate { certificate,
   signature }`.
5. The signed cert is stashed in `AppState::certs` and served by
   `GET /api/jobs/:id/certificate`.
6. **Destroy path only.** When the linked `DestructionManifest` is
   co-signed, `wipe_cert::co_sign(...)` appends a `CoSignatureBlock`
   over the *same* canonical bytes the primary signer used, carrying the
   co-signer's role, identity, `manifest_ref` and timestamp. Because the
   payload is unchanged, a verifier holding only the supervisor's public
   key can independently confirm "this party attested to this exact
   cert" without trusting the station key. `co_signatures` is empty on
   Erased certs.

`wipe_cert::verify(signed, &[trusted_keys])`:
- Re-canonicalizes the embedded certificate to compute the actual
  SHA-256.
- Compares against the signature's `canonical_sha256_hex` (fails closed on
  any payload mutation).
- Looks up a matching trusted key by `public_key_id`.
- Verifies the Ed25519 signature.

This is the offline-verification path. The CLI command
`wipestation verify-cert` exposes it; auditors only need the published
public key.

## Station configuration store

Evidence never touches local storage. *Configuration* — today just the bay
topology — does, through a `TopologyStore` seam whose tier the station
detects for itself at startup (ADR-0003):

```
  writable config path?  ──yes──▶  LocalFile     atomic write, survives reboot
        │ no
  control plane set?     ──yes──▶  ControlPlane  PUT/GET keyed by station id
        │ no                                     (client seam; hub is future work)
        ▼
  Tier 3: ask the operator ──acknowledged──▶ Ephemeral   RAM only, stated in the UI
```

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
machinery for a 5–50-station LAN; the hub/cloud tier handles cross-LAN.
The seam to add full gossip later is the `FleetService` API — peers and
lead are read-side; the discovery transport is pluggable.

## Operating modes

| Mode | Binary | Frontend |
| --- | --- | --- |
| Standalone with monitor | `wipestation serve` | Tauri 2 window (`pnpm tauri dev` / `pnpm tauri build`) |
| Headless wipestation (PXE / rack) | `wipestation serve --no-ui` (future flag) | none on device; operator tablet connects to it |
| Roaming operator tablet | Same React bundle | Pointed at any station's `/api` |
| Hub (multi-site) | `wipestation serve --hub` (future) | Same React bundle in "fleet" mode |

## Pricing-model implications

Two SKUs share one binary:

- **Tier 1 — Station / annual unlimited.** No cloud. Local signing via
  embedded key (or YubiKey/PIV at the operator desk in v0.2). LAN
  discovery + lead election. Air-gap clean. The KillDisk-like pricing
  model, without the legacy crapness.
- **Tier 2 — Cloud / per-station + cloud features.** Same binary, with a
  `--hub-url` pointing at the cloud. Cloud-side cert archive, multi-site
  fleet view, customer-facing cert portal, ITAM integrations, SAML/OIDC.
  Cloud KMS signing as an alternative to YubiKey.

A single customer can run both — `--hub-url` is per-station configuration.
Air-gapped fleets simply omit it.

## Where the code will need to grow

Replace, add, or extend:

1. `wipe-engine-linux` crate — real `nix::ioctl_*` calls, `SG_IO`,
   `NVME_IOCTL_ADMIN_CMD`, `/dev/sdX` O_DIRECT reads.
2. `wipe-license` crate — token verification (license signed by the
   vendor's key, embedded in the binary), per-success usage accounting.
3. `wipe-hub` crate — same protocol the stations already speak, but
   tenant-aware; immutable cert archive; cert verification API for
   customer portals.
4. `wipe-pdf` — render the signed JSON-LD into a PDF/A-3 with the JSON
   embedded as an attachment, for audit-friendly delivery.
5. Operator RBAC inside `wipe-server` — SSO via OIDC, audit attribution.
6. `wipe-iso` — bootable image build (Alpine + signed shim + GRUB +
   the binary).
