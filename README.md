# Wipestation — v0.1

A NIST SP 800-88 Rev. 2 / IEEE 2883-2022 data-sanitization tool with a
local web UI, an HTTP API, and per-station mDNS fleet discovery. Single
binary, single executable target, ships with command-level evidentiary
certificates that anyone can verify offline.

> **Status:** v0.1 vertical slice with a mock device backend, plus the
> ADR-0001 outer-Job model landed in v0.2. No real hardware ioctl yet —
> replace `wipe-engine-mock` with `wipe-engine-linux` (planned) when a
> Linux box with real drives is available. **Nothing in this repo has
> ever erased a real drive**; the mock backend synthesises the command
> evidence it reports.

## Domain model (ADR-0001)

A **Job** is the outcome-bearing unit: process one Asset to a terminal
disposition. It composes a `Vec<JobActivity>` — `Diagnostic`,
`HealthCheck`, `Erasure`, `Verification`, `Destruction` — and signs one
`Certificate` covering the whole evidence chain.

| Layer | State machine |
| --- | --- |
| Outer `Job` | `Queued → InProgress → (Erased \| PendingCoSign → Destroyed \| Quarantined \| Aborted)` |
| Inner `ErasureEvent` | `Queued → Probing → (Unfreezing) → Confirming → Running → Verifying → GeneratingCert → Signing → Completed`, with `Failed` / `Aborted` escapes |

One Job may hold several `ErasureEvent`s (retries, method fallback). When
erasure is exhausted, the Job escalates to `PendingCoSign`, is rolled
into a `DestructionManifest`, and reaches `Destroyed` on supervisor
co-sign. See [ADR-0001](docs/adr/0001-job-as-outcome-bearing-composition.md).

> Pre-ADR-0001 the type called `Job` meant "one attempted erasure". That
> type is now `ErasureEvent`, and the old `JobEvent` stream is now
> `JobUpdate`. Both renames have shipped — the code and the CONTEXT.md
> glossary agree.

## What's in the box

| Crate | Purpose |
| --- | --- |
| `wipe-common` | Shared domain types (`Device`, `Method`, `Capabilities`, `Job`, `JobActivity`, `ErasureEvent`, `DestructionManifest`, `StationInfo`, …) and the NIST R2 method selector |
| `wipe-engine` | `DeviceBackend` trait + `JobRunner`; runs the outer-Job and inner-ErasureEvent state machines and broadcasts `JobUpdate` records |
| `wipe-engine-mock` | Fake fleet of 4 drives (NVMe x2, SATA SSD, HDD) with simulated progress, failure injection, synthesised evidence |
| `wipe-cert` | JSON-LD certificate schema (v2 — carries the activity chain) + canonical serialization + detached Ed25519 sign / verify + supervisor co-signature |
| `wipe-fleet` | mDNS service advertise / browse + deterministic lead election |
| `wipe-server` | Axum REST API + WebSocket event stream; auto-signs certs when a Job reaches `Erased` or `PendingCoSign`; destruction-manifest assembly and co-sign |
| `wipe-cli` | `wipestation serve / inspect / verify-cert` binary |
| `apps/desktop` | Vite + React + TanStack Router + Tailwind frontend, plus Tauri 2 shell |

## Quick start

```bash
# Prereqs: rustup-installed stable toolchain, pnpm, jq
# (the CI/demo scripts source ~/.cargo/env automatically).

# Run all Rust tests (83 across 6 crates):
cargo test --workspace -- --test-threads=1

# Build + typecheck + test the frontend:
cd apps/desktop && pnpm install && pnpm build && pnpm typecheck && pnpm test && cd ../..

# Run the two-station demo end-to-end (mDNS discovery + erase + cert verify):
./scripts/demo.sh
```

## Operating modes (one binary, one origin)

The same Axum server serves **both** `/api/*` and the React UI at `/`. That
means a browser, a tablet, or the Tauri window can all point at the same
URL — there's no "API port" and "UI port" split.

| Mode | Invocation | What you get |
| --- | --- | --- |
| **Standalone (browser)** | `wipestation serve` then open `http://127.0.0.1:7878` | UI + API on one port; mDNS-advertised so tablets discover this station |
| **Headless / air-gap** | `wipestation serve --no-fleet` | Same as above, mDNS off |
| **Remote tablet console** | open `http://<station-ip>:7878` from another device | Same React UI, talks to the remote station's API |
| **Fast demo mode** | `wipestation serve --fast` | Erasures complete in ~1s for live demos |
| **Native desktop window** | `cargo run -p wipestation-desktop` | Tauri window; in-process API; same UI as the browser path |
| **Inspect catalog** | `wipestation inspect` | Prints the mock device catalog as JSON |
| **Offline cert verify** | `wipestation verify-cert path.json --public-key-b64 <key>` | Validates a signed cert; no network needed |

### Important: build the frontend first

The `wipestation` binary auto-detects the frontend bundle at one of:

* `--static-dir <path>` (explicit override)
* `$WIPESTATION_STATIC_DIR`
* `apps/desktop/dist` relative to the CWD
* `apps/desktop/dist` relative to the binary

If none of these are present you'll see an "API only" landing page that
tells you exactly how to fix it. To build the frontend:

```bash
cd apps/desktop && pnpm install && pnpm build && cd ../..
cargo build -p wipe-cli
./target/debug/wipestation serve --fast
# open http://127.0.0.1:7878
```

The Tauri window uses the same in-process API and serves the same UI — no
Vite dev server required for production use. If you want hot reload while
hacking on the frontend, run `pnpm dev` separately at `apps/desktop` (Vite
on :5173, which proxies `/api/*` to :7878).

## End-to-end flow

### Erase path

1. `POST /api/jobs` with a `JobSpec` (device, classification, intent, operator).
2. `POST /api/jobs/:id/start` — the Job moves `Queued → InProgress` and the
   runner appends an `ErasureEvent`, driving it through Probing →
   (Unfreezing) → Confirming → Running → Verifying → GeneratingCert →
   Signing → Completed and emitting a `JobUpdate` at every step. On
   success it appends a `VerificationEvent` and the Job reaches `Erased`.
3. `GET /api/jobs/:id/certificate` — returns a `SignedCertificate` containing:
   * The job spec, the operator (with email — required by R2), and asset/ticket linkage
   * The full `activities` chain, and within each `ErasureEvent` the captured
     `CommandEvidence` (interface, opcode, action, raw CDB, log pages) for
     every command issued
   * The `VerificationReport` (sampled reads, SHA-256, entropy)
   * The resolved `AssetDisposition`, stated explicitly so an auditor need
     not re-derive it from the chain
   * A detached Ed25519 signature over the canonical (sorted-keys) JSON
4. Anyone with the public key can `wipestation verify-cert` it offline — no vendor lookup, no online check.

### Destroy path

When erasure is exhausted, `POST /api/jobs/:id/escalate-to-destroy` appends a
`DestructionEvent` and moves the Job to `PendingCoSign` — at which point its
certificate is generated, with `media_status.operational = false`. The Job is
then rolled into a manifest via `POST /api/manifests`, and
`POST /api/manifests/:id/cosign` records the supervisor, attaches a **second,
independent signature** to each member Job's certificate, and moves those Jobs
to `Destroyed`.

| Endpoint | Purpose |
| --- | --- |
| `GET /api/manifests` | List destruction manifests |
| `POST /api/manifests` | Assemble a manifest from N `PendingCoSign` Jobs |
| `GET /api/manifests/:id` | Fetch one manifest |
| `POST /api/manifests/:id/cosign` | Supervisor co-sign → member Jobs become `Destroyed` |

## Design highlights

- **NIST 800-88 Rev. 2 aware** — method selector in [`wipe-common/src/method.rs`](crates/wipe-common/src/method.rs) follows the R2 decision flow (intent → classification → media → capability), prefers NVMe Sanitize Crypto Erase on SED-provisioned NVMe, ATA Secure Erase Enhanced on SATA SSDs, single-pass overwrite on HDDs.
- **Command-level evidence** — every command the backend issues produces a [`CommandEvidence`](crates/wipe-common/src/evidence.rs) record (opcode, action, status, log page bytes). Auditors read the actual proof, not a marketing summary.
- **Offline-verifiable certs** — JSON-LD payload, canonicalized via deterministic key-sorting, signed with Ed25519. Embedded `canonical_sha256_hex` defends against silent payload modification.
- **mDNS fleet discovery** — every station advertises `_wipestation._tcp.local.` with TXT metadata. Tablets/operators discover and select stations from the LAN. Lead election is deterministic by `(started_at, id)` — no Raft round needed for small fleets.
- **PXE-ephemeral by design** — **no persistent local state for evidence**: certs are signed in-RAM and shipped to lead/hub/cloud or downloaded, never written to local storage. *Configuration* (the bay topology) is a different matter and does persist where the station has somewhere to put it — see [ADR-0003](docs/adr/0003-station-config-persistence.md) for the tiered store and why the distinction is drawn.
- **Single binary, three frontends** — the Tauri window, the Axum HTTP API (for tablets and automation), and a future Ratatui TUI all wrap the same engine.

## Test inventory (83 Rust + 29 TypeScript, all passing)

| Crate / file | Tests | Covers |
| --- | --- | --- |
| `wipe-common/tests/method_selection.rs` | 7 | R2 decision flow across NVMe/SATA/HDD; destroy intent; frozen device handling; evidence serde round-trip |
| `wipe-common/tests/bay_topology.rs` | 36 | Bay grid construction and vendor numbering runs (row/column-major, four origins, label offsets); bay→device resolution across every binding kind; declared bindings beating enumeration fill; no device placed twice; disabled bays skipped; overflow reported; generated-fallback flagging; serde round-trip and a hand-written config |
| `wipe-cert/src/canonical.rs` (inline) | 4 | Canonical JSON: sorted keys, nested sort, finite-only numbers, integer preservation |
| `wipe-cert/tests/sign_verify.rs` | 8 | sign+verify happy path, unknown-key rejection, tamper detection, JSON round-trip, activity chain carries erasure + verification, supervisor co-signature verifies independently, deterministic public-key-id, verifying-key round-trip |
| `wipe-engine-mock/tests/end_to_end.rs` | 8 | NVMe crypto-erase happy path reaches `Erased`; SATA failure keeps the outer Job `InProgress` for an operator decision; enumerate; broadcast stream observes outer *and* inner transitions |
| `wipe-fleet/tests/two_instance.rs` | 2 | Solo lead election, two-instance mutual discovery + cross-station election agreement |
| `wipe-server/tests/http_e2e.rs` | 3 | Full HTTP flow → signed cert → offline verify; aborted job emits no cert; destroy-via-manifest-cosign produces two signatures |

| `wipe-server/tests/topology_store.rs` | 15 | All four persistence tiers (ADR-0003); the write probe against a read-only directory; corrupt-config recovery; save-survives-restart; and that a path binding learned by identify mode keeps its bay when a *different* drive is swapped into that port |
| `apps/desktop` (`pnpm test`) | 29 | Numbering runs across every order/origin/offset; grid rebuilds preserving operator edits by position; hot-plug diffing (a reused port is a *different* drive); binding rules; validation |

Plus the `scripts/demo.sh` end-to-end shell drives two real station processes, mDNS discovery, a real cert issuance, and three negative tests (correct key passes, wrong key rejected, tampered cert rejected).

## Frontend surfaces

| Page | What it shows |
| --- | --- |
| **Devices** | Bench view, in two modes — a **bay map** that mirrors the station's physical drive bays (see below), and a **card grid** with one card per attached device. Both join `/api/devices` against `/api/jobs` by `device_id` and colour-code by slot status (empty / idle / wiping / erased / failed / pending co-sign / destroyed / quarantined / aborted), with a "safe to disconnect" affordance on Erased and a "needs attention" affordance on Failed |
| **Jobs** | All Jobs with outer state and activity counts |
| **Job Detail** | Live activity timeline over the `JobUpdate` WebSocket stream |
| **Certificate** | Signed-cert viewer, including the activity chain and both signatures on the destroy path |
| **Manifests** | Destruction-manifest assembly and supervisor co-sign |
| **Fleet** | mDNS-discovered peers and the elected lead |

## Bay map — mirroring the physical bench

A station can declare the drive bays it physically has, so the Devices page
renders a layout the operator recognises instead of a grid of cards in
enumeration order. The hierarchy is `BayTopology → Enclosure → Bank → Bay`;
everything is parameterised (grid shape, tray orientation, form factor,
numbering run, bay→device binding) rather than templated per chassis. See
[ADR-0002](docs/adr/0002-configurable-bay-topology.md).

```bash
# List the built-in presets
wipestation bay-presets

# Run against one
wipestation serve --fast --bay-profile arma-4u-32

# Or dump a preset as a starting point for your own bench and edit it
wipestation bay-presets --dump arma-4u-32 > bench.json
wipestation serve --bay-topology bench.json
```

| Preset | Shape |
| --- | --- |
| `arma-4u-32` | 4U rackmount: two banks of 2×8 front-loading trays either side of a ventilation column |
| `dock-2bay` | Two-bay top-loading hot-swap dock |
| `nvme-carrier-8` | Eight-socket M.2 NVMe carrier |

`GET /api/bay-topology` returns the geometry with each bay's `device_id`
already resolved — the frontend renders, it never re-implements the binding
rules. Bindings resolve `by` SES device slot, `/dev` path, serial, WWN or
explicit device id; unbound bays fall back to enumeration order unless
`auto_fill_unbound` is off. Devices that no bay claimed are reported in
`unplaced_devices` and surfaced in the UI rather than silently dropped.

**A station with no bay config does not get a plausible-looking chassis.** It
gets a single auto-sized bank flagged `generated: true`, and the UI says the
positions are enumeration order, not physical bays.

## Not implemented yet

- Real Linux ioctl backend (`wipe-engine-linux` — drop in alongside the mock).
- `DiagnosticEvent` and `HealthCheckEvent` are **schema-only**: the types and
  the `JobActivity` variants exist and serialise into the cert, but the runner
  never emits them. `Erasure`, `Verification` and `Destruction` are live.
- `DestructionEvent.photo_refs` is a schema slot; there is no photo-capture UX.
- Cloud / Hub mode (the protocol seam exists; the server just needs an HTTP client to register).
- License enforcement, RBAC, SAML/OIDC, customer cert portal.
- PDF/A-3 cert rendering (JSON-LD only for now; PDF wraps the JSON later).
- Bootable ISO + UEFI Secure Boot signing chain.
- Mobile / iOS-Android erasure (separate product surface).
