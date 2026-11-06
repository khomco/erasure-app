---
id: SPEC-bench-throughput-inbox
companions: [../../docs/ROADMAP.md, ../../docs/adr/0002-configurable-bay-topology.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# A tray in one gesture, and nothing invisible

## Why

A pain to solve (ROADMAP O1.4, O2.1, O3.3, O1.5 in part). Operators start jobs one drive at a time, and things needing a human are scattered across per-card states and the Manifests page. On a real floor, a missed failure means a drive leaves undocumented.

## Capabilities

- **CAP-1**
  - **intent:** An operator selects several bays on the bay map and starts one job per drive with a shared spec.
  - **success:** Multi-select on the bay map; one confirm creates and starts N jobs via existing routes; partial failures are listed per drive; vitest over the selection module.
- **CAP-2**
  - **intent:** One inbox shows everything waiting on a human.
  - **success:** An Inbox page lists Jobs whose latest erasure failed, quarantined Jobs, PendingCoSign Jobs and pending manifests, each linking to where it is resolved; nav shows a count; vitest over the filter module.
- **CAP-3**
  - **intent:** Asset-tag fields accept a barcode scanner.
  - **success:** A scanner's typed input followed by Enter fills the field and advances without submitting the wrong form; covered by a component-level test.
- **CAP-4**
  - **intent:** The Jobs list shows the queue at a glance.
  - **success:** Jobs page groups active/queued/finished with counts; no ETA.

## Constraints

- Bay matching stays in Rust (ADR-0002): use the server-resolved topology, never re-derive bay↔device in TS.
- Only `JobRunner` mutates jobs; no new mutating route unless strictly needed, and if added, mirrored in `app.rs` and `client.ts`.
- Attribution uses the existing session operator; no auth (D2).

## Non-goals

- ETA, automatic failure routing (D10), RBAC (D2), gloved-hand/high-contrast mode.

## Success signal

- An operator loads eight drives, starts all eight in one action, and every failure appears in one place.
