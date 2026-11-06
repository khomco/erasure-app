---
id: SPEC-docs-truth-up
companions: [../audit-2026-11-05/shipped-claims.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# Docs say what the code does

## Why

A pain to solve. AGENTS.md names docs trailing code as this repo's repeated failure, and the 2026-11-05 audit found README, ARCHITECTURE, three ADRs and the bench-setup design doc describing states, signatures and flags that do not exist. Unattended agents read these docs as truth, so every false sentence becomes a wrong implementation later.

## Capabilities

- **CAP-1**
  - **intent:** A reader of README.md and docs/ARCHITECTURE.md learns the system as built: crates (incl. `wipe-license`), CLI subcommands, inner ErasureEvent states, routes, test inventory, desktop-shell limits.
  - **success:** Every correction listed for README.md and ARCHITECTURE.md in the companion §(e) source list is applied; no sentence claims `Verifying`/`GeneratingCert`/`Signing` states, an "independent" co-signature, `--hub-url`, or "token embedded in the binary".
- **CAP-2**
  - **intent:** CONTEXT.md §5, §7, §11 and §14 match the code.
  - **success:** No inner `Verifying` state; PXE items describe what happens today (certs are not streamed; signing keys are on the station); §14 defines evaluation certificate, attestation chain, licence certificate, instance key, lease/watermark, enclosure model/catalog, shell, store tier.
- **CAP-3**
  - **intent:** ADRs carry as-built addenda where code deviates, without rewriting the decision.
  - **success:** ADR-0002 notes ADR-0003 superseded "no persistence"; ADR-0004 notes the unwired overlay and the `art` key-vs-object difference; ADR-0005 gains an "As built (2026-11-05)" addendum listing the non-persisted lease, missing FileCounter, unenforced `allowed_methods`, root-only verification not yet possible.
- **CAP-4**
  - **intent:** docs/design/bench-setup-builder.md and user-facing UI copy stop promising things that shipped or never did.
  - **success:** Design doc marks the four "Model changes" done, notes the unbuilt SES button and template/preset mismatch, and corrects the `/api/sim/*` mounting claim (also `app.rs:44`, `handlers.rs:468` comments); the stale "lands next" copy at `BenchSetupPage.tsx:1216` is removed; the tier-3 banner no longer recommends `--control-plane-url` as a working fix.

## Constraints

- Docs and comments only, plus the two UI copy strings in CAP-4. No behaviour change.
- An ADR's decision text is never rewritten; deviations go in a dated addendum (AGENTS.md: contradict an ADR explicitly).
- Name concepts with CONTEXT §14 terms.
- `pnpm typecheck && pnpm test` and `cargo test --workspace` still pass.

## Non-goals

- Fixing the code gaps the addenda describe (other spec folders own those).
- CONTEXT §10.2, ROADMAP §0.4/T0 and AGENTS.md counts — already corrected 2026-11-05.

## Success signal

- A reviewer can pick any sentence in README/ARCHITECTURE/ADR addenda and find the code it describes.
