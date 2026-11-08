---
title: 'As-built addenda on ADR-0002, 0004 and 0005'
type: 'chore'
created: '2026-11-08'
status: 'done'
baseline_revision: 'dc9e6168956e93359f769421c9f1a7277ddef653'
review_loop_iteration: 0
followup_review_recommended: false
context: ['{project-root}/.scratch/audit-2026-11-05/shipped-claims.md']
warnings: ['oversized']
deferred: []
---

<intent-contract>

## Intent

**Problem:** ADR-0002, ADR-0004 and ADR-0005 state things the code does not do: ADR-0002 says no persistence layer is added; ADR-0004 shows `art` as an SVG-template object and a fail-closed, overlay-extensible catalog; ADR-0005 says the watermark persists, a `FileCounter` exists, `allowed_methods` gates methods, free-tier usage is counted, and an auditor needs only the vendor root key. Agents read ADRs as truth.

**Approach:** Append one dated "As built (2026-11-05)" section at the end of each of the three ADRs listing, per point, where the code differs from the decision and where it is, without touching any existing sentence.

## Boundaries & Constraints

**Always:** Edit only the three ADR files (plus this story file). Append-only: every existing line of each ADR stays byte-identical (`git diff` shows only added lines at the end of each file). Each addendum says the decision stands and names the decision section each item qualifies. Every item traceable to a Code Map anchor (file + symbol). Conforming points the audit checked may be stated in one line so the addendum is not read as a list of everything. ROADMAP §0.1: no claim a drive is really sanitized. "Licence" in prose, identifiers verbatim. CONTEXT §14 terms (evaluation certificate, attestation chain, licence certificate, instance key, lease / watermark, enclosure model / catalog, shell, store tier).

**Never:** Change code, tests, other ADRs, CONTEXT.md, README/ARCHITECTURE, `docs/design/*` or UI copy (story 4). Rewrite, strike or re-word decision text or the existing ADR-0004 "As built." paragraphs. Propose fixes or make product decisions in the addenda (other spec folders own the fixes — say "not built", not "will be").

</intent-contract>

## Code Map

ADR-0002 (`docs/adr/0002-configurable-bay-topology.md`, last Consequence "No persistence layer is added"):
- `crates/wipe-server/src/store.rs` + `AppState::save_bay_topology` (`crates/wipe-server/src/app.rs:~175`) -- ADR-0003 tiered store persists a saved layout; `--bay-topology` file and `--bay-profile` preset/catalog-model (`crates/wipe-cli/src/main.rs:305-335`) remain start-up sources.
- `crates/wipe-server/src/app.rs:297-311` -- `GET /api/bay-topology`, `POST /api/bay-topology/resolve` (dry run used by `BenchSetupPage.tsx:202`): device matching stays server-side (§3 conforms).
- `apps/desktop/src/bench/topologyEdit.ts:58` `generateBays` ("Mirrors `grid_bank`", `bay.rs:613`), `:163` `enclosureFromModel`, `:235` `validateLocal` (subset of `BayTopology::validate`; server re-validates on save) -- bay generation/numbering and part of validation re-implemented in TS; `topologyEdit.test.ts` tests TS alone, no cross-language parity test.
- `crates/wipe-common/src/bay.rs:769-780` presets `arma-4u-32`, `dock-2bay`, `nvme-carrier-8` vs `apps/desktop/src/pages/BenchSetupPage.tsx:476-497` UI templates Empty, Rackmount 24 (4×6), dock 2, NVMe 8 -- two separate lists; UI has no ARMA 32, Rust has no 24-bay rack.
- `ses_slot` binding: still populated by nothing (no `wipe-engine-linux`) -- conforms as written.

ADR-0004 (`docs/adr/0004-enclosure-model-catalog.md`):
- `crates/wipe-common/src/catalog.rs:182-205` -- `EnclosureModel.art: Option<String>`, a key into the frontend shell registry (`apps/desktop/src/bench/shells/`), not the §2 `{kind, shell, bay_slot, aspect}` object; no per-entry `schema_version`; `verified_by` provenance field exists and the picker shows "verified" (`BenchSetupPage.tsx:686`).
- `catalog.rs:274-320` -- only catalog-level `schema_version`; `Catalog::bundled()` parses without checking it and `Catalog::overlay` stamps `CATALOG_SCHEMA_VERSION` over whatever the overlay declared: §7 "fails closed" not built. Same-id overlay entries win (§7 conforms in the function).
- `crates/wipe-server/src/app.rs:141-148` `AppState::with_catalog_overlay` -- no caller in `wipe-cli` or `apps/desktop/src-tauri`; no overlay directory or control-plane catalog loader. Stations run on the bundled catalog only.
- `crates/wipe-engine/src/backend.rs:100-125` -- `EnclosureDiscovery`, `EnclosureControl` defined, no impl; `Catalog::match_identity`/`best_match` built and tested (`crates/wipe-common/tests/catalog.rs`) but no production caller.

ADR-0005 (`docs/adr/0005-licensing-and-attestation.md`):
- `crates/wipe-license/src/lease.rs:19-35` -- `LeaseState` doc claims persistence through the ADR-0003 store; `crates/wipe-cli/src/main.rs:752` and `crates/wipe-server/src/licensing.rs:36` build `LeaseState::new(now)` every start: watermark and `erasures_used` reset on restart, defeating §3a rollback detection and `Count` quota.
- `lease.rs:62-95` -- `MonotonicCounter` trait; `FileCounter` named in a comment, absent; `TpmCounter` returns `Unsupported` but `is_hardware_backed()` returns `true`.
- `crates/wipe-license/src/entitlement.rs:105-112` `AllowedMethods::permits` -- tested (`tests/attestation.rs:225`), never called by server/engine/CLI: §2 `allowed_methods` unenforced.
- `crates/wipe-license/src/chain.rs:87,326` -- licence carries `instance_public_key_id` only (compared to the cert's key id), not the instance public key; `wipestation verify-cert` requires `--public-key-b64` (`main.rs:98-104`): §1 root-only verification not possible.
- `crates/wipe-server/src/app.rs:246-276` -- `record_erasure` only when `!decision.evaluation`: §5 free-tier count is not tracked; a PendingCoSign (destroy) certificate signed under a licence counts against quota like an erasure.
- `crates/wipe-license/src/lib.rs:63-69` `machine_fingerprint` = hash of station id; `main.rs:431` station id defaults to `standalone`, so stations without `--station-id` share a fingerprint.
- `crates/wipe-license/src/client.rs:62-100` `HttpLicenseClient` always `Unreachable`, constructed nowhere in production (§4 seam conforms as "seam only"). `anchor.rs:31-50` `CertAnchor` + `UnconfiguredAnchor` (§6 conforms).
- Conforms: licensing never gates erasure (`licensing.rs:10-15`, `app.rs:246-248`); `evaluation` inside the signed payload; `verify-cert` prints EVALUATION/Unlicensed (`main.rs:654`). ADR-0012 `lapse_policy` carried in the licence; no station code reads it (`grep LapsePolicy crates/wipe-server crates/wipe-cli crates/wipe-engine` empty).

Style precedent: `docs/adr/0001-job-as-outcome-bearing-composition.md:75-97` (dated addendum, numbered items, "recorded here rather than edited above").

## Tasks & Acceptance

**Execution:**
- `docs/adr/0002-configurable-bay-topology.md` -- append `## As built (2026-11-05)`: (1) ADR-0003 superseded the "No persistence layer is added" consequence (tiered store; file/preset start-up sources remain); (2) matching stays server-side, but bay generation/numbering and a subset of validation are mirrored in TS with no parity test; (3) Rust presets and UI templates are separate, differing lists; one line that `ses_slot` and the honest fallback conform.
- `docs/adr/0004-enclosure-model-catalog.md` -- append `## As built (2026-11-05)`: (1) `art` is a shell-registry key string, not an SVG-template object; (2) no per-entry `schema_version`, catalog-level version unchecked, overlay overwrites it — not fail-closed; (3) overlay function exists but is unwired, no overlay directory or control-plane catalog; (4) matcher built with no caller, seams defined with no implementation (already in Status).
- `docs/adr/0005-licensing-and-attestation.md` -- append `## As built (2026-11-05)`: non-persisted lease (watermark + count reset each start, `LeaseState` doc wrong); `FileCounter` missing, `TpmCounter::is_hardware_backed` true; `allowed_methods` unenforced; root-only verification not yet possible; free-tier count untracked and destroy certificates consume quota; shared `standalone` fingerprint; `HttpLicenseClient` never constructed; ADR-0012 lapse policy not read by the station; conforming points in one line.

**Acceptance Criteria:**
- Given `git diff dc9e616 -- docs/adr/`, when inspected, then only the three files change and every hunk is pure additions at end of file.
- Given each addendum, when a reviewer picks any item, then the cited file/symbol exists at HEAD and shows the stated behaviour.
- Given ADR-0005's addendum, when read, then it covers non-persisted lease, missing `FileCounter`, unenforced `allowed_methods`, and root-only verification not yet possible (SPEC CAP-3); ADR-0002's notes ADR-0003 superseded "no persistence"; ADR-0004's notes the unwired overlay and the `art` key-vs-object difference.
- Given `cargo test --workspace` and `cd apps/desktop && pnpm typecheck && pnpm test`, then both pass (no code changed).

## Spec Change Log

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 19 findings — high 0, medium 2, low 15, false 2, maybe-false 0
- findings:
  - `[low]` `[reject]` (blind) Verification grep `'^-[^-]'` misses removed bullet lines (`-- foo`) — real weakness in the check, but `git diff --numstat dc9e616` shows 0 deletions in all three ADRs, and the fix edits this build's spec.
  - `[medium]` `[patch]` (blind) ADR-0005 omits that site scope is never checked — confirmed: `evaluate` (lease.rs) checks only `required_fingerprint()`, `None` for `Scope::Site`; nothing compares `site_id`. Added to item 6.
  - `[medium]` `[patch]` (blind) ADR-0005 omits that `features` is unenforced — confirmed: `feature_available` called only from `tests/attestation.rs`. Added to item 3.
  - `[low]` `[patch]` (blind) ADR-0005 item 8 (ADR-0012, 2026-11-08) credited to the 2026-11-05 audit — item now says it was added at the 2026-11-08 re-check.
  - `[low]` `[patch]` (blind) Destroy certificates consuming quota filed under §5 — now cites §2 `quota: Count` and states that under ADR-0011 a destruction is an operator attestation, not an erasure.
  - `[low]` `[patch]` (blind) ADR-0004 item 4 lists seams as a difference though Consequences already say they are unimplemented — item now covers only the matcher (built and tested, no production caller, beyond what the ADR says); seams moved to a "Conforms as written" line.
  - `[low]` `[patch]` (blind) ADR-0004 item 1 treats the "verified" marker as meeting "make 'unverified' visible" — confirmed `BenchSetupPage.tsx:686` marks only verified entries; now recorded as a difference.
  - `[low]` `[patch]` (blind) ADR-0002 item 3 is drift between two lists, not a §1 breach — recast: §1 conforms, the lists have drifted.
  - `[low]` `[reject]` (blind) No pointer from Status/decision text to the addenda — the intent is "Dated addenda only; never rewrite the decision text"; it excludes in-place edits.
  - `[low]` `[patch]` (blind) ADR-0005 item 2 does not say why `is_hardware_backed() == true` matters — added: an unsupported counter reports itself hardware-backed, against §3b "nothing pretends to succeed".
  - `[low]` `[reject]` (blind) Acceptance criteria weaker than the Always rule (no automated term/citation check) — fix edits this build's spec; every cited symbol was spot-checked by hand.
  - `[low]` `[patch]` (edge) ADR-0002 item 1 omits that `--bay-topology` is also the local-file store's write target and that file/profile only seed when no saved layout exists — confirmed (`StoreConfig.explicit_path`, main.rs:469,500); added.
  - `[low]` `[patch]` (edge) ADR-0005 item 6 implies `--station-id` avoids the shared fingerprint — confirmed `AppState::with_static_dir` (app.rs:101-104), used by the desktop shell, builds `LicenseContext::unlicensed("standalone", ..)`; added.
  - `[low]` `[patch]` (edge) ADR-0002 item 2 omits `rebuildBank` mirroring `renumber_bank` — confirmed (topologyEdit.ts:97, bay.rs:689); added.
  - `[low]` `[reject]` (intent) Readers meet the false sentence before the end-of-file addendum — same as the blind pointer finding; the intent allows dated addenda only.
  - `[low]` `[reject]` (intent) ADR-0004 now has corrections in two places (existing §3/§4 "As built." paragraphs plus the new section) — moving the existing paragraphs would rewrite the ADR, which the intent forbids; the new section names them.
  - `[low]` `[reject]` (intent) Checks do not test prose claims — fix edits this build's spec; claims were hand-verified at HEAD and in triage.
  - `[false]` `[reject]` (intent) Citation by file+symbol instead of line numbers — the spec's Always rule asks for file + symbol; not a defect.
  - `[false]` `[reject]` (intent) Diff also adds the story file — boundaries allow it; not a defect.
- verification-gap layer: no findings. Total 19 = blind 11 + edge 3 + intent 5.

## Design Notes

Autonomous decisions:
- **Addendum date.** The audit is dated 2026-11-05; this story re-checked every item at dc9e616 on 2026-11-08. Options: date by audit, by check, or both. Chosen: heading "As built (2026-11-05)" (the SPEC's wording, matching the audit it cites) with one sentence saying it was re-checked against the code on 2026-11-08.
- **Scope beyond SPEC's minimum list.** CAP-3's success list is a floor; its intent is "addenda where code deviates". Chosen: include every audit §(b) deviation for these three ADRs, verified at HEAD, plus ADR-0012 lapse policy not read by the station (it amends ADR-0005 §5). Fixes stay out.

## Verification

**Commands:**
- `git diff --stat dc9e616 -- docs/ && git diff dc9e616 -- docs/adr/ | grep '^-[^-]'` -- expected: three ADR files, no removed lines.
- `cargo test --workspace` (compose up) and `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: pass.

## Auto Run Result

Status: done

**Summary.** ADR-0002, ADR-0004 and ADR-0005 each end with a dated `## As built (2026-11-05)` section, re-checked against the code on 2026-11-08. No existing ADR line changed. Each addendum says the decision stands, names the section each item qualifies, cites file + symbol, and proposes no fix.
- ADR-0002: ADR-0003 superseded "No persistence layer is added". The `--bay-topology` path is also the store's write target, and a file or profile only seeds the bench when no layout is saved. Bay generation, renumbering, model expansion and part of validation are mirrored in TypeScript with no parity test. The Rust presets and UI templates are two lists that have drifted. `ses_slot`, the honest fallback and §1 conform.
- ADR-0004: `art` is a shell-registry key, not an SVG-template object. No entry is marked unverified. There is no per-entry `schema_version`, and the catalog version is never checked (not fail-closed). The overlay is unwired. The matcher is built but has no production caller. The seams conform.
- ADR-0005: the lease is not persisted. `FileCounter` is missing and `TpmCounter` claims to be hardware-backed. `allowed_methods` and `features` are not enforced. Verification with only the vendor root is not possible. The free-tier count is not tracked, and destroy certificates consume a `Count` quota. Stations share the `standalone` fingerprint, including the desktop shell, and site scope is unchecked. `HttpLicenseClient` is never constructed. The station does not read ADR-0012's lapse policy.

**Files changed**
- `docs/adr/0002-configurable-bay-topology.md`: as-built addendum, +50 lines.
- `docs/adr/0004-enclosure-model-catalog.md`: as-built addendum, +43 lines.
- `docs/adr/0005-licensing-and-attestation.md`: as-built addendum, +78 lines.
- `.scratch/docs-truth-up/stories/3-as-built-addenda-on-adr-0002-0004-and-0005.md`: this story spec.

**Review.** 19 findings: high 0, medium 2, low 15, false 2. Verification-gap layer: none.
- Patched: 11 entries (2 medium, 9 low), all wording changes checked against the code.
- Deferred: none.
- Rejected:
  - weak append-only grep: numstat shows 0 deletions, and the fix edits the spec;
  - no pointer from the decision text, raised twice: the intent allows dated addenda only;
  - two "As built" layers in ADR-0004: merging them would rewrite the ADR;
  - ACs and checks do not test prose, raised twice: the fix edits the spec, and every claim was checked by hand;
  - citation style and the story file in the diff (both false).

**Follow-up review recommended:** false. Two medium entries were patched, but both are additions to the docs, checked directly against code (`evaluate` and `feature_available` call sites), and no unverified risk can be named.

**Verification.**
- `git diff --numstat dc9e616 -- docs/`: three ADR files, 171 added, 0 removed.
- `cargo test --workspace` (compose up): exit 0, 696 passed, 0 failed, both before and after the patches.
- `apps/desktop`: `pnpm typecheck` ok, `pnpm test` 200 passed.

**Residual risks.**
- Addenda cite symbols, not line numbers, so they survive line moves but not renames.
- Readers who stop at a superseded decision sentence won't see the addendum. The intent ("addenda only") rules out in-place pointers.
- ADR-0003's own deviations (the audit's §(b) item) are not in this story's scope and remain unannotated.
