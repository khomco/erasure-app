# Station configuration persists to a local file; evidence still does not

**Status:** proposed (2026-08-06) — pending review. Do not implement until accepted.

## Context

ADR-0002 made the bay topology configuration. It is currently supplied by
`--bay-topology <file>` or `--bay-profile <name>` at startup, both of which
assume *we* author the document. For a product sold to ITAD shops that is the
wrong shape: every customer's bench is different hardware we have never seen,
and asking them to hand-edit JSON — or file a support ticket for a layout — is
not a product. They need to build the layout in the app and have the station
remember it.

That collides with a stated architectural property. CONTEXT §7 and the README
both describe Wipestation as **PXE-ephemeral by design: "no persistent local
state for cert content; certs are signed in-RAM and shipped."** The repo has no
persistence layer at all — no SQLite, no state directory. rusqlite is named in
CONTEXT as something that arrives with the Enterprise data model (v0.2 #4), not
before.

So: where does a customer-authored bay topology live, and does storing it break
the ephemerality claim?

## Decision

**Persist station *configuration* to a local JSON file. Keep the ephemerality
guarantee, but state it precisely: it is about evidence, not about config.**

### The distinction being drawn

| Kind | Examples | Persisted locally? | Why |
| --- | --- | --- | --- |
| **Evidence** | Certificates, activity chains, signatures, command evidence | **No** | Signed in RAM and shipped. A station that dies loses nothing an auditor needed, and a seized station yields no customer data. This is the property the compliance story rests on. |
| **Configuration** | Bay topology, station label, future station-scoped settings | **Yes** | Describes the *bench*, not any customer's data. Losing it on reboot is merely annoying; it contains nothing an auditor or an attacker wants. |

A bay topology says "this chassis has 24 trays and tray 7 is blanked off". It
contains no customer identity, no asset, no serial of a drive that was
processed — only bindings that *may* reference a drive serial or a `/dev` path
of hardware currently attached to the operator's own bench. It is not evidence
and it is not customer data.

**Consequently the ephemerality claim in CONTEXT §7 and the README should be
reworded from "no persistent local state" to "no persistent local state for
evidence".** Leaving it as-is while shipping a config file would be exactly the
silent drift the repo's own rules forbid.

### Storage

A single JSON document — the same `BayTopology` shape `--bay-topology` already
reads, so a file written by the UI and a file written by hand are the same
thing, and export/import is a copy.

Path resolution, first match wins:

1. `--bay-topology <path>` — explicit. This file is now read **and written**.
2. `$WIPESTATION_CONFIG_DIR/bay-topology.json`
3. `~/.config/wipestation/bay-topology.json` (platform config dir)

Writes are atomic: temp file in the same directory, then `rename`. A
half-written topology is worse than none.

### API

| Endpoint | Purpose |
| --- | --- |
| `GET /api/bay-topology` | *(exists)* geometry + resolved occupancy, for rendering |
| `GET /api/bay-topology/config` | the raw stored document, for the editor |
| `PUT /api/bay-topology` | validate, persist atomically, hot-reload in memory |

`PUT` hot-reloads — no restart. A bench being configured is a bench with an
operator standing at it.

### Concurrency

The document carries a `revision: u32`. `PUT` with a stale revision is rejected
with 409 and the editor re-reads. Two tablets pointed at the same station is a
normal ITAD situation and last-write-wins would silently discard someone's work.

### Read-only stations stay first-class

On a PXE or read-only-root station the config path is not writable. `PUT` then
returns a clear 409 explaining so, and the UI falls back to **Export JSON** —
the operator bakes the file into the PXE image or drops it on removable media.

This is deliberate: the station must never *require* writable local state to
function. Writable config is an affordance, not a dependency.

## Considered and rejected

- **SQLite now.** One small human-readable document does not need a database,
  and making rusqlite's first job "store a config blob" would set an awkward
  precedent for the Enterprise data model that actually needs it (v0.2 #4). A
  JSON file is diffable, hand-editable, trivially exportable for fleet rollout,
  and already the format we read. If topology later needs to live beside
  Customer/WorkOrder/Asset rows, moving it is a migration, not a redesign.
- **Keep it startup-only; make the UI emit a file the operator installs.** Honest
  but hostile: it makes every layout tweak a filesystem errand, and on a Tauri
  station the operator may have no shell. Export stays available for the
  read-only case; it should not be the only path.
- **Store topology on the fleet Lead and sync it down.** Attractive for a
  50-station floor, but the Lead currently has no differentiated behaviour at
  all (CONTEXT §12 lists that as open), and each station's bench is physically
  its own. Per-station local config first; fleet push is a later feature whose
  seam is the same `PUT` endpoint on each station.
- **Put topology in the certificate.** No. A bay is where a drive sat on a
  bench, not a fact about the erasure. Deliberately restated from ADR-0002
  because a config store makes it newly tempting.

## Consequences

- First persistent local state in the product. CONTEXT §7, the README's
  PXE-ephemeral bullet, and ARCHITECTURE's "no persistent local state" line all
  need the evidence/config wording.
- Anyone who can reach the API can rewrite the bench layout. There is no
  authn/authz yet (v0.2 #6/#7), so this inherits whatever those land with;
  topology editing should be a `supervisor` capability when RBAC exists. Worth
  stating plainly now rather than discovering it during a security review.
- A corrupt or unparseable config file must not brick the station: fall back to
  the generated bench, log loudly, and surface it in the UI. Same reasoning as
  ADR-0002's honest default.
