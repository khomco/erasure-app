# Station configuration persists through a tiered, self-detecting backend; evidence never persists

**Status:** accepted (2026-08-06)

Supersedes the proposed version of this ADR, which assumed a single local
file. The tiering below is the substantive change.

## Context

ADR-0002 made the bay topology configuration, but every route to authoring one
went through us — a JSON file, a CLI flag, or a preset compiled into the binary.
Customers have benches we have never seen. The layout has to be theirs to build
in the app, which means the station has to be able to *save* it.

That collides with a stated property. CONTEXT §7, the README and ARCHITECTURE
all describe Wipestation as **PXE-ephemeral: "no persistent local state"**. The
repo has no persistence layer at all — no SQLite, no state directory.

And the collision is not hypothetical. A PXE-booted station on a read-only root
genuinely *cannot* write a config file. A benchtop station with a normal disk
trivially can. The same binary must handle both, and an operator should not have
to know which one they are standing in front of.

## Decision

### 1. The ephemerality guarantee is about evidence, not configuration

| Kind | Examples | Persisted? | Why |
| --- | --- | --- | --- |
| **Evidence** | Certificates, activity chains, signatures, command evidence | **Never** | Signed in RAM and shipped. A station that dies loses nothing an auditor needed; a station that is seized yields no customer data. This is what the compliance story rests on. |
| **Configuration** | Bay topology, station label, future station-scoped settings | **Yes, where possible** | Describes the *bench*, not any customer's data. Losing it is annoying, not a breach. |

A bay topology says "this chassis has 24 trays and tray 7 is blanked off". It
carries no customer identity, no asset, no record of anything processed.

**The wording in CONTEXT §7, README and ARCHITECTURE therefore changes from "no
persistent local state" to "no persistent local state for evidence".** Shipping
a config store while leaving the old claim standing would be exactly the silent
drift the repo forbids.

### 2. Persistence is a pluggable backend, chosen by tiered auto-detection

A `TopologyStore` trait with three implementations. The station **detects** which
applies at startup and degrades down the tiers on its own — the operator is asked
only when a real decision needs a human.

```
                    ┌─ Tier 1: LocalFile ─────────────────────────────┐
  config path       │  Path is writable (probe-and-remove, not a      │
  writable? ──yes──▶│  permissions guess). Atomic write: temp file    │
                    │  in the same dir + rename. Survives reboot.     │
       │            └─────────────────────────────────────────────────┘
       no
       │            ┌─ Tier 2: ControlPlane ──────────────────────────┐
  control plane     │  Read-only root / PXE. Push topology to a hub   │
  configured &  ────▶  keyed by station id; pull it back on next      │
  reachable?  yes   │  boot. Survives reboot centrally.               │
                    └─────────────────────────────────────────────────┘
       no
       │            ┌─ Tier 3: needs operator ────────────────────────┐
                    │  Nowhere to persist. Ask: point at a control    │
       ────────────▶│  plane, or acknowledge there is none.           │
                    └─────────────────────────────────────────────────┘
                                        │
                                   acknowledged
                                        │
                    ┌─ Tier 4: Ephemeral ─────────────────────────────┐
                    │  Fully functional this boot. Config held in     │
                    │  RAM. UI states plainly that it is lost on      │
                    │  reboot, and offers Export.                     │
                    └─────────────────────────────────────────────────┘
```

Detection is a **write probe**, not an inference. Guessing from mount flags or
`uid == 0` gets read-only NFS, overlayfs and full disks wrong; actually creating
and removing a temp file in the target directory does not.

### 3. `ControlPlane` is a seam now, not a server now

The hub does not exist (CONTEXT §11 v0.3 #7) and the fleet Lead still has no
differentiated behaviour (CONTEXT §12). Building a control-plane server to hold
one JSON document would be building the hub by accident.

So `ControlPlane` ships as a **client-side seam with real runtime behaviour**:

- Endpoint comes from configuration first (`--control-plane-url`), because
  config always works and discovery may not. mDNS and "the Lead is the control
  plane" are the obvious later options, deliberately left TBD.
- Wire contract is deliberately boring: `GET /api/stations/{station_id}/bay-topology`
  and `PUT /api/stations/{station_id}/bay-topology`, the same document the local
  file holds. Keyed by station id, which already exists and is already
  mDNS-advertised.
- With no endpoint configured, the backend reports itself unavailable and the
  station falls to Tier 3. The tier logic, the operator prompt and the ephemeral
  fallback are all **real and exercised today**; only the far end is future work.

This is the difference between a stub and a seam: nothing pretends to succeed.

### 4. Concurrency and safety

- The document carries `revision: u32`. A `PUT` with a stale revision is
  rejected with 409 and the editor re-reads. Two tablets pointed at one station
  is normal on an ITAD floor, and last-write-wins would silently eat someone's
  afternoon.
- A corrupt or unparseable stored config must never brick a station: fall back to
  the generated bench, log loudly, surface it in the UI.
- Export/import stays first-class at every tier — it is the fleet-rollout path
  and the escape hatch on an ephemeral station.

## Considered and rejected

- **SQLite now.** One small human-readable document does not need a database,
  and making rusqlite's first job "hold a config blob" sets an awkward precedent
  for the Enterprise data model that genuinely needs it (v0.2 #4). A JSON file
  is diffable, hand-editable and already the format we read.
- **Make the operator declare the tier.** They should not have to know whether
  their root is read-only. The station can find out in a millisecond.
- **Silently fall back to ephemeral when nothing is writable.** The tempting
  option and the wrong one: the operator spends twenty minutes mapping a 24-bay
  chassis and loses it at the next reboot, having been told nothing. Tier 3
  exists precisely so that outcome is a decision rather than a surprise.
- **Block the station until persistence is configured.** Worse. A station that
  refuses to wipe drives because it cannot save a bay map has its priorities
  backwards. Ephemeral is fully functional.
- **Store topology on the fleet Lead right now.** Attractive for a 50-station
  floor, but the Lead has no differentiated responsibilities yet, and each
  station's bench is physically its own. `ControlPlane` is where that lands
  when it lands.
- **Put topology in the certificate.** No. A bay is where a drive sat on a
  bench, not a fact about the erasure. Restated from ADR-0002 because having a
  config store makes it newly tempting.

## Consequences

- First persistent local state in the product. The three ephemerality claims
  need the evidence/config wording, or the drift is ours.
- Anyone who can reach the API can rewrite the bench layout. There is no
  authn/authz yet (v0.2 #6/#7); topology editing should be a `supervisor`
  capability once RBAC exists. Stated now rather than discovered in a security
  review.
- A station's persistence tier is operational state an operator may need to
  explain ("why did my layout vanish?"), so it is exposed at
  `GET /api/bay-topology/store` and shown in the UI rather than buried in logs.
- `wipe-server` gains a filesystem dependency it did not have. Confined behind
  the `TopologyStore` trait so the Tauri and future TUI hosts can substitute
  their own.
