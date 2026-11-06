---
id: SPEC-licensing-as-built
companions: [../audit-2026-11-05/shipped-claims.md, ../../docs/adr/0005-licensing-and-attestation.md, ../../docs/adr/0003-station-config-persistence.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# Make ADR-0005's lease claims true

## Why

A pain to solve. ADR-0005 says the lease watermark and quota counter persist, a `FileCounter` exists, and free-tier use is counted. None is true: every restart builds a fresh `LeaseState`, so restarting defeats clock-rollback detection and the `Count` quota. Licensing that resets on reboot cannot be sold.

## Capabilities

- **CAP-1**
  - **intent:** A station's lease state (watermark, erasure count) survives restart wherever the station can persist anything.
  - **success:** Integration test: record N erasures, rebuild the station from the same store, observe count N and the same watermark; rolling the clock back below the persisted watermark after restart is detected.
- **CAP-2**
  - **intent:** On a station that cannot persist (read-only root / ephemeral tier), lease state is RAM-only and the station says so.
  - **success:** The lease status reports the persistence tier; an ephemeral station's status text states that quota and rollback protection reset on reboot; a test covers it.
- **CAP-3**
  - **intent:** A `FileCounter` monotonic counter exists and is honestly labelled as not hardware-backed.
  - **success:** `FileCounter` implements the counter trait, `is_hardware_backed()` is false, and `TpmCounter::is_hardware_backed()` no longer returns true while unimplemented.
- **CAP-4**
  - **intent:** Free-tier (evaluation) erasures are counted as ADR-0005 §5 says.
  - **success:** An unlicensed station's lease records evaluation erasures separately from licensed ones; a test shows both counts.

## Constraints

- Never block, delay or fail an erasure because of lease state or a storage error (ROADMAP §0.2); on any lease I/O error, sign as evaluation and log.
- The topology store holds configuration only; lease state gets its own key/file through the same tier detection, never inside the topology document.
- Bay-topology writes still go only through `AppState::save_bay_topology`.
- Engine crates gain no licensing dependency.

## Non-goals

- TPM, hub reporting, `allowed_methods` enforcement (D13), site scope (D4).

## Success signal

- Restarting a licensed station with a `Count` quota no longer refills the quota.
