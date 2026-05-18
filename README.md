# Wipestation — v0.1

A NIST SP 800-88 Rev. 2 / IEEE 2883-2022 data-sanitization tool with a
local web UI, an HTTP API, and per-station mDNS fleet discovery. Single
binary, single executable target, ships with command-level evidentiary
certificates that anyone can verify offline.

> **Status:** v0.1 vertical slice with a mock device backend. No real
> hardware ioctl yet — replace `wipe-engine-mock` with `wipe-engine-linux`
> (planned) when a Linux box with real drives is available.

## What's in the box

| Crate | Purpose |
| --- | --- |
| `wipe-common` | Shared domain types (`Device`, `Method`, `Capabilities`, `Job`, `StationInfo`, …) and the NIST R2 method selector |
| `wipe-engine` | `DeviceBackend` trait + `JobRunner` state machine; broadcasts `JobUpdate` events |
| `wipe-engine-mock` | Fake fleet of 4 drives (NVMe x2, SATA SSD, HDD) with simulated progress, failure injection, evidence capture |
| `wipe-cert` | JSON-LD certificate schema + canonical serialization + detached Ed25519 sign / verify |
| `wipe-fleet` | mDNS service advertise / browse + deterministic lead election |
| `wipe-server` | Axum REST API + WebSocket event stream; auto-signs certs on `Completed` |
| `wipe-cli` | `wipestation serve / inspect / verify-cert` binary |
| `apps/desktop` | Vite + React + TanStack Router + Tailwind frontend, plus Tauri 2 shell |

## Quick start

```bash
# Prereqs: rustup-installed stable toolchain, pnpm, jq
# (the CI/demo scripts source ~/.cargo/env automatically).

# Run all Rust tests (25 across 6 crates):
cargo test --workspace -- --test-threads=1

# Build + typecheck the frontend:
cd apps/desktop && pnpm install && pnpm build && pnpm typecheck && cd ../..

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

1. `POST /api/jobs` with a `JobSpec` (device, classification, intent, operator).
2. `POST /api/jobs/:id/start` — runner transitions Queued → Probing → Confirming → (Unfreezing) → Running → Verifying → GeneratingCert → Signing → Completed, emitting events at every step.
3. `GET /api/jobs/:id/certificate` — returns a `SignedCertificate` containing:
   * The job spec, the operator (with email — required by R2), and asset/ticket linkage
   * The captured `CommandEvidence` (interface, opcode, action, raw CDB, log pages) for every command issued
   * The `VerificationReport` (sampled reads, SHA-256, entropy)
   * A detached Ed25519 signature over the canonical (sorted-keys) JSON
4. Anyone with the public key can `wipestation verify-cert` it offline — no vendor lookup, no online check.

## Design highlights

- **NIST 800-88 Rev. 2 aware** — method selector in [`wipe-common/src/method.rs`](crates/wipe-common/src/method.rs) follows the R2 decision flow (intent → classification → media → capability), prefers NVMe Sanitize Crypto Erase on SED-provisioned NVMe, ATA Secure Erase Enhanced on SATA SSDs, single-pass overwrite on HDDs.
- **Command-level evidence** — every command the backend issues produces a [`CommandEvidence`](crates/wipe-common/src/evidence.rs) record (opcode, action, status, log page bytes). Auditors read the actual proof, not a marketing summary.
- **Offline-verifiable certs** — JSON-LD payload, canonicalized via deterministic key-sorting, signed with Ed25519. Embedded `canonical_sha256_hex` defends against silent payload modification.
- **mDNS fleet discovery** — every station advertises `_wipestation._tcp.local.` with TXT metadata. Tablets/operators discover and select stations from the LAN. Lead election is deterministic by `(started_at, id)` — no Raft round needed for small fleets.
- **PXE-ephemeral by design** — no persistent local state for cert content; certs are signed in-RAM and shipped to lead/hub/cloud or downloaded. (Persistence layers will land alongside the real hardware backend.)
- **Single binary, three frontends** — the Tauri window, the Axum HTTP API (for tablets and automation), and a future Ratatui TUI all wrap the same engine.

## Test inventory (25 tests, all passing)

| Crate / file | Tests | Covers |
| --- | --- | --- |
| `wipe-common/tests/method_selection.rs` | 7 | R2 decision flow across NVMe/SATA/HDD; destroy intent; frozen device handling; evidence serde round-trip |
| `wipe-cert/src/canonical.rs` (inline) | 4 | Canonical JSON: sorted keys, nested sort, finite-only numbers, integer preservation |
| `wipe-cert/tests/sign_verify.rs` | 6 | sign+verify happy path, unknown-key rejection, tamper detection, JSON round-trip, deterministic public-key-id, verifying-key round-trip |
| `wipe-engine-mock/tests/end_to_end.rs` | 4 | NVMe crypto-erase happy path, SATA failure propagation, enumerate, event-stream lifecycle observation |
| `wipe-fleet/tests/two_instance.rs` | 2 | Solo lead election, two-instance mutual discovery + cross-station election agreement |
| `wipe-server/tests/http_e2e.rs` | 2 | Full HTTP flow → signed cert → offline verify; aborted job emits no cert |

Plus the `scripts/demo.sh` end-to-end shell drives two real station processes, mDNS discovery, a real cert issuance, and three negative tests (correct key passes, wrong key rejected, tampered cert rejected).

## What's deliberately not in v0.1

- Real Linux ioctl backend (`wipe-engine-linux` — drop in alongside the mock).
- Cloud / Hub mode (the protocol seam exists; the server just needs an HTTP client to register).
- License enforcement, RBAC, SAML/OIDC, customer cert portal.
- PDF/A-3 cert rendering (JSON-LD only for now; PDF wraps the JSON later).
- Bootable ISO + UEFI Secure Boot signing chain.
- Mobile / iOS-Android erasure (separate product surface).
