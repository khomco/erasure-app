---
id: SPEC-destruction-cert-integrity
companions: [../audit-2026-11-05/shipped-claims.md, ../DECISIONS.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# Every destroyed drive is certified, once

## Why

A mandate to meet: the product's claim is that every disposition is attested. The audit found Jobs reaching `Destroyed` without a certificate (fixed as a refusal in c779361), a Job joinable to two manifests (server guard added in c779361; UI filter still wrong), `manifest_id` never stamped, and runner "not found" errors surfacing as 500s.

## Capabilities

- **CAP-1**
  - **intent:** A Job on a manifest records which manifest it is on, and the Destroy path links Job, DestructionEvent and manifest.
  - **success:** After `POST /api/manifests`, `GET /api/jobs/:id` shows `manifest_id` and the DestructionEvent's `manifest_ref` equal to the manifest id; an HTTP e2e test asserts it.
- **CAP-2**
  - **intent:** The Manifests page offers only Jobs that are not already on a live manifest.
  - **success:** With CAP-1 in place, a Job already on a pending manifest is absent from the assembly list; a vitest over the selection logic covers it.
- **CAP-3**
  - **intent:** API clients get the right status for runner errors.
  - **success:** Unknown Job ids return 404 and invalid transitions 409 from start/abort/escalate/cosign, not 500; tests cover each.
- **CAP-4**
  - **intent:** A drive destroyed without any erasure attempt is handled the way decision D9 resolves.
  - **success:** Defined by D9; until D9 is decided this story must halt at planning.

## Constraints

- Only `JobRunner` mutates Jobs (AGENTS.md); a manifest/Job link needs a runner method, not a handler-side copy.
- A signed certificate is immutable; co-sign signs the same bytes.
- Certificates are produced only via `JobBroadcast::JobStateChanged` into Erased/PendingCoSign.
- Keep c779361's guarantees: co-sign never finalises a Job without a certificate; one live manifest per Job.
- TS mirror (`types.ts`, `client.ts`) changes with the Rust shapes.

## Non-goals

- Per-person supervisor keys (needs D2).
- Changing what the PendingCoSign certificate says (D9b).

## Success signal

- No sequence of HTTP calls can produce a `destroyed` Job without a certificate, or a certificate co-signed twice.

## Open Questions

- D9 (in DECISIONS.md) for CAP-4.
