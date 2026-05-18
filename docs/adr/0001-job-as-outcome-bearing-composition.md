# Job as outcome-bearing composition of typed events

**Status:** accepted (2026-05-18)

## Context

In v0.1, `Job` means "one attempted erasure of one device" — `JobSpec`
takes a single `device_id` and a single `Method`, the `JobState` machine
runs that attempt to either `Completed`, `Failed`, or `Aborted`, and one
`Certificate` is signed per Job. Retries, fallback methods, pre-flight
diagnostics, and the worst-case path where erasure fails and the device
must be physically destroyed are all *unmodelled*: each becomes a
separate `Job` and the relationship between them lives only in the
operator's head (or, at best, a shared `asset_tag` string).

For a compliance-grade audit story this is materially weaker than it
needs to be. The cert says "this attempt happened"; it does not say
"here is the full processing history for this Asset to terminal
disposition." Auditors who want the latter currently have to compose it
themselves across multiple certs.

## Decision

`Job` is redefined as the **goal-oriented** unit: process this Asset to
a terminal disposition. A Job composes one or more typed events:

- `DiagnosticEvent`, `HealthCheckEvent` — pre-flight
- `ErasureEvent` — one wipe attempt (carries the existing inner state
  machine, the chosen Method, the captured CommandEvidence). A Job may
  contain several (retries, method fallback). Cryptographic
  instant-purge is a normal ErasureEvent that completes in milliseconds.
- `VerificationEvent` — post-erasure sampling
- `DestructionEvent` — chain-of-custody record for physical destruction
  when erasure is exhausted

The Job's outer state machine is `Queued → InProgress → (Erased |
Destroyed | Quarantined | Aborted)` — the Asset's terminal disposition,
not any single event's progress. One `Certificate` per Job aggregates
the full evidence chain.

Cross-station processing (Asset moves from one station to another mid-Job
due to interface compatibility) stays one Job; the station id is recorded
as metadata on each ErasureEvent, useful for analysis but not a Job
boundary.

The existing `Job` type in code becomes `ErasureEvent`; the existing
`JobEvent` low-level stream becomes `JobUpdate` to free the word "Event"
for the higher-level activity records.

## Considered and rejected

- **Keep `Job` = one attempt; add a separate `AssetHistory` view.** Audit
  still has to compose across records to read terminal disposition; the
  view papers over the missing first-class concept. Rejected: the audit
  cert is the artefact compliance care about, and the cert covers only
  one Job — so the view doesn't reach the auditor.
- **Job = one attempt; introduce a `JobChain` entity to link retries.**
  Strictly more entities for less semantic gain than re-scoping `Job`.
  Rejected on simplicity.

## Consequences

- Cert schema gains an outer wrapper and event composition; existing
  certs remain valid as single-ErasureEvent Jobs (migration is additive).
- Wizard/UX shifts from "create a Job → run it to Completed/Failed" to
  "create a Job → run events until terminal disposition." Operator
  workflow for retries and escalation to destruction becomes explicit.
- WebSocket payloads gain a Job-level state stream alongside the
  existing per-event stream.
- Code rename is broad: `Job` → `ErasureEvent`, `JobState` →
  `ErasureEventState`, `JobSpec` → `ErasureEventSpec` (or kept and
  re-scoped), `JobEvent` → `JobUpdate`. New `Job`, `JobState`, `JobSpec`
  introduced for the outer entity. Bundled in v0.2 item #2 (see §11).
