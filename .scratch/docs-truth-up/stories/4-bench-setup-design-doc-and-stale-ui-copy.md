---
title: 'Bench-setup design doc and stale UI copy'
type: 'chore'
created: '2026-11-08'
status: 'done'
baseline_revision: '2e856134cada59bd16bf1426a3182e999e554319'
review_loop_iteration: 1
followup_review_recommended: false
context: ['{project-root}/.scratch/audit-2026-11-05/shipped-claims.md']
warnings: ['oversized']
deferred:
  - summary: >-
      Copy outside the two UI strings still presents a control plane as a fix for a station that cannot save: the tier-3 banner's server-sent detail ("…and no control plane is configured" / "the configured control plane at {url} could not be used") and the `wipestation serve` console line ("the UI will ask the operator to point at a control plane or accept the loss").
    evidence: |-
      store.rs:362 and :374 build the EphemeralStore detail shown in the banner; ControlPlaneStore::probe always errors (store.rs:210-214). main.rs:480 prints the console line; after story 4 the UI no longer mentions a control plane. Customer-visible copy beyond the story's two strings.
    location: >-
      crates/wipe-server/src/store.rs:362, crates/wipe-server/src/store.rs:374, crates/wipe-cli/src/main.rs:480
    severity: medium
  - summary: >-
      The Devices page "Bench not configured" banner tells operators to restart with `--bay-profile` / `--bay-topology` and does not link to the Bench setup builder.
    evidence: |-
      DevicesPage.tsx:173-176. Pre-existing copy; the builder at /bench-setup is reached only from the sidebar.
    location: >-
      apps/desktop/src/pages/DevicesPage.tsx:173
    severity: low
  - summary: >-
      The tier-3 "cannot save configuration" banner no longer suggests any way to keep a layout; the lower-risk tier-4 banner says "Use Export to keep a copy." Adding that hint is new customer-visible copy.
    evidence: |-
      BenchSetupPage.tsx tier-3 paragraph now reads only "Continue without persistence — the bench will work normally until this station reboots." Pre-existing: the shipped paragraph never mentioned Export either.
    location: >-
      apps/desktop/src/pages/BenchSetupPage.tsx (StoreBanner, tier 3)
    severity: low
  - summary: >-
      docs/ARCHITECTURE.md says the /api/sim routes 404 "unless the backend simulates hot-plug"; the desktop shell's mock backend simulates hot-plug and still gets 404 because the shell never calls with_simulator.
    evidence: |-
      docs/ARCHITECTURE.md:33-34; apps/desktop/src-tauri/src/lib.rs:143 calls only AppState::with_static_dir. ARCHITECTURE belongs to docs-truth-up story 1.
    location: >-
      docs/ARCHITECTURE.md:33
    severity: low
  - summary: >-
      SES auto-detect, bulk custom labels and the Devices-page entry links into Bench setup are unbuilt but have no docs/ROADMAP.md entry.
    evidence: |-
      docs/design/bench-setup-builder.md as-built notes (2026-11-08) record them as not built; grep of docs/ROADMAP.md finds none of them.
    location: >-
      docs/ROADMAP.md
    severity: low
---

<intent-contract>

## Intent

**Problem:** `docs/design/bench-setup-builder.md` still lists four "Model changes this needs" that have shipped, promises an SES (SCSI Enclosure Services) auto-detect button, entry links and custom labels that were never built, presents the built-in templates as "the built-in presets", and says `/api/sim/*` is "mounted only when the backend implements it". Two code comments repeat the mounting claim. In the UI, the bay inspector says "Learn by hot-swap and SES auto-detect land next" (hot-swap shipped; SES is not coming soon), and the bench-setup "cannot save configuration" banner tells the operator to use `--control-plane-url`, whose store tier always fails to probe.

**Approach:** Correct the design doc in place with dated "As built (2026-11-08)" notes beside each affected section, fix every code comment that repeats the conditional-mounting claim, delete the "land next" line, and delete the `--control-plane-url` clause from the tier-3 banner, keeping only its already-shipped words.

## Boundaries & Constraints

**Always:** Every corrected sentence traceable to a Code Map anchor at HEAD. CONTEXT §14 terms (store tier, shell, enclosure model / catalog, binding). ROADMAP §0.1: no claim that a drive is really sanitized or that SES detection exists. Say "not built", never "will be" / "next". Keep the design's intent sentences (it is a design doc, not an ADR, so in-place correction is allowed, but rationale paragraphs stay). Format touched Rust files with `rustfmt --edition 2021 <file>` only.

**Never:** Change behaviour, routes, types or tests; change any copy other than the two UI strings (the `wipestation serve` console line at `main.rs:480` stays — see Design Notes); edit ADRs, CONTEXT.md, README or ARCHITECTURE (stories 1–3); build any of the missing features.

## Autonomous decisions

- **Which code comments to correct (review pass 1, 2026-11-08).** Developers reading the simulated hot-plug code are told in several places that `/api/sim/*` exists only on mock stations, but the routes are always mounted and answer 404 unless the host attaches a simulator, which the desktop shell never does. The story says "correct the … code comments"; planning listed only three, and review found three more (`wipe-cli` `main.rs` beside `.with_simulator`, `apps/desktop/src/api/client.ts` above `simAttach`, `BenchSetupPage.tsx` above the `detached` query). Options: fix the three listed; fix every comment repeating the claim. Chosen: every one — the story's wording covers all of them, they are comments (no behaviour or copy), and leaving some keeps the false claim alive.
- **Tier-3 banner wording (review pass 1).** Operators on a station with no writable storage see "This station cannot save configuration" followed by advice to use `--control-plane-url`, which cannot work. Options: (a) delete the control-plane clause and keep the rest of the shipped sentence verbatim ("Continue without persistence — the bench will work normally until this station reboots."); (b) write a new sentence that also mentions Export. Chosen (a): the story says remove, and AGENTS.md reserves new customer-visible copy for the owner; (a) adds no words that were not already shipped.

</intent-contract>

## Code Map

Design doc `docs/design/bench-setup-builder.md`:
- Status (l.3-4) -- "core builder and identify mode are built": true for `wipestation serve`; the Tauri shell (`apps/desktop/src-tauri/src/lib.rs:143`, only `AppState::with_static_dir`) never attaches a simulator, so `/api/sim/devices` answers 404 there. What the operator sees: the identify panel hides its simulated-bench controls and asks them to "Insert one at the bench" (`IdentifyPanel.tsx:129`), but the shell's mock backend never reports a new drive, so identify mode waits indefinitely.
- "Where it lives" (l.30-36) -- `/bench-setup` route (`apps/desktop/src/main.tsx:72`) is reached only from the sidebar item `Layout.tsx:175`. `DevicesPage.tsx:171-177` "Bench not configured" banner names `--bay-profile`/`--bay-topology`, no link; `:189-199` unplaced-devices warning says "Switch to Cards", no deep link; no "Configure bench" header button.
- Numbering run (l.87-96) -- `[Custom labels…]` not built (no textarea / custom-label code in `BenchSetupPage.tsx` or `topologyEdit.ts`).
- Binding mode 4 (l.132-135) -- no SES option, button or tooltip: inspector offers Auto / path / serial only (`BenchSetupPage.tsx:1140-1210`). `BayBinding::SesSlot` exists in `crates/wipe-common/src/bay.rs`.
- "As built" (l.152-154) -- `/api/sim/*` routes always mounted (`crates/wipe-server/src/app.rs:336-338`); `handlers::simulator` returns 404 when `AppState.simulator` is `None`; `wipe-cli` calls `.with_simulator(backend)` (`main.rs:498`), the Tauri shell does not.
- Templates (l.156-162) -- the 32-bay ARMA chassis is still reachable as a catalog enclosure model (`arma/industrial-4u-32`, `crates/wipe-common/data/catalog.json`) through the builder's model picker (`BenchSetupPage.tsx` "model picker") and as `--bay-profile arma-4u-32`; say so. UI `TemplateMenu` (`BenchSetupPage.tsx:471-497`: Empty, Rackmount 24 (4×6), dock 2, NVMe 8) is a separate list from Rust `preset_names()` (`bay.rs:778-780`: `arma-4u-32`, `dock-2bay`, `nvme-carrier-8`); the "starting points" copy exists (`:508`).
- Validation (l.164-171) -- builder runs `validateLocal` (`apps/desktop/src/bench/topologyEdit.ts:235`) plus server dry run; Rust `BayTopology::validate` (`bay.rs:450`). Implementer: compare the table to both and mark rows not implemented (e.g. device-attachment warnings, custom-label parse).
- Persistence (l.173-178) -- `PUT /api/bay-topology` (`app.rs:297-299`) → `AppState::save_bay_topology`; verify "atomically" against `store.rs` `LocalFileStore::save` before keeping it.
- Model changes (l.180-199) -- all four done: `Bank.numbering: Option<NumberingRun>` (`bay.rs:217,249`); opaque `Bay.id` `"{enc}.{bank}.r{row}c{col}"` from `grid_bank` (`bay.rs:~654`, doc `:181-184`; TS `topologyEdit.ts:82`); per-bay tray inset `traySize` (`apps/desktop/src/bench/BayMap.tsx:74-85`); `BayTopology.revision` (`bay.rs:312`). Enclosures flow and wrap (`BayMap.tsx:513` `flex-wrap`). `m2` label cramping: not re-checked — leave that bullet and say it was not re-checked. Under the retitled heading, put the original gap descriptions in the past tense or mark them as the gap as written, so no item reads as a live bug beside its **Done:** note; update the "Two rendering issues also want attention" lead-in so it does not call a done item open.

Code comments:
- `crates/wipe-server/src/app.rs:44-46` -- `simulator` field doc "Present only when the backend can fake hot-plug. Gates the routes…": routes are always mounted; field is `Some` only when a host calls `with_simulator`, and handlers 404 otherwise.
- `crates/wipe-server/src/app.rs:165` -- "enabling the `/api/sim/*` routes" → makes them answer instead of 404.
- `crates/wipe-server/src/handlers.rs:525-529` -- "Only mounted when the backend implements `DeviceSimulator`…" → always mounted, 404 unless `with_simulator` was called (the mock in the desktop shell included).
- `crates/wipe-cli/src/main.rs:495-497` -- "A real backend will not implement DeviceSimulator and the /api/sim/* routes stay unavailable." → routes are always mounted; without `with_simulator` they answer 404.
- `apps/desktop/src/api/client.ts:82-84` -- "Only present when the station runs the mock backend; a real-hardware station returns 404…" → 404 whenever no simulator is attached, including the mock-backed desktop shell.
- `apps/desktop/src/pages/BenchSetupPage.tsx:136-137` -- "Only the mock backend simulates hot-plug; a real station 404s here…" → same correction; `simAvailable = !detached.isError` (`:144`) is what hides the simulated controls.

UI copy:
- `apps/desktop/src/pages/BenchSetupPage.tsx:1213-1216` -- div "Learn by hot-swap and SES auto-detect land next — see ADR-0002."; `HardDrive` import (`:10`) has no other use.
- `apps/desktop/src/pages/BenchSetupPage.tsx:581-588` -- tier-3 banner `--control-plane-url` sentence. `ControlPlaneStore::probe` always errors (`crates/wipe-server/src/store.rs:210-214`), and `detect_store` then falls back to an ephemeral store (`store.rs:355-366`), so a control-plane store is never active.

## Tasks & Acceptance

**Execution:**
- `docs/design/bench-setup-builder.md` -- update Status line; add short dated "As built (2026-11-08)" notes to Where it lives, Numbering run, SES mode, Templates, Validation, Persistence (if needed); correct the `/api/sim/*` sentence; retitle "Model changes this needs" to say done and mark each of the four with its symbol; mark enclosure wrapping done -- the doc stops promising unbuilt features and listing shipped ones as needed.
- `crates/wipe-server/src/app.rs`, `crates/wipe-server/src/handlers.rs`, `crates/wipe-cli/src/main.rs`, `apps/desktop/src/api/client.ts` -- rewrite the simulator comments per Code Map; `rustfmt --edition 2021` the three Rust files (check the diff stays comment-only).
- `apps/desktop/src/pages/BenchSetupPage.tsx` -- delete the "land next" div and the unused `HardDrive` import; fix the `detached` query comment; in the tier-3 paragraph delete the control-plane clause and the `<code>--control-plane-url</code>` element so the paragraph reads exactly "Continue without persistence — the bench will work normally until this station reboots." (no other words)

**Acceptance Criteria:**
- Given HEAD after the change, when grepping `apps/desktop/src` for `land next` and `control-plane-url`, then there are no matches.
- Given the design doc, when read, then no sentence says `/api/sim/*` is conditionally mounted, the four model changes are marked done with their symbols, SES auto-detect, the three entry links and custom labels are marked not built, and the template list difference from Rust presets is stated.
- Given `grep -rn "Only mounted\|Gates the\|stay unavailable\|Only present when\|Only the mock backend" crates apps/desktop/src`, then no comment claims `/api/sim/*` is conditionally mounted.
- Given the tier-3 banner source, then its advice paragraph is exactly the shipped clause "Continue without persistence — the bench will work normally until this station reboots."
- Given `git diff 2e85613 --stat`, then only the six files above, this story file and `_bmad-output/implementation-artifacts/docs-truth-up-4-attempt-1.patch` change.
- Given `cargo test --workspace` (compose up) and `cd apps/desktop && pnpm typecheck && pnpm test`, then both pass.

## Spec Change Log

### 2026-11-08 — Review pass 1 loopback (review_loop_iteration 1)
- **Trigger:** review found three more comments repeating the conditional-mounting claim (`wipe-cli/src/main.rs:495-497`, `apps/desktop/src/api/client.ts:82-84`, `BenchSetupPage.tsx:136-137`), and that the prescribed tier-3 sentence was partly new customer-visible copy while the Design Notes called it reused wording. Lower-grade doc findings: the Templates note implied the 32-bay chassis is unreachable; the Status note did not say what the operator sees in the desktop shell; the Model-changes section kept present-tense bug text beside **Done:** and an unflagged, not re-checked `m2` bullet.
- **Amended:** intent-contract Approach + `## Autonomous decisions` (comment scope, tier-3 wording); Code Map (three comments, ARMA catalog/profile reachability, desktop-shell identify behaviour, control-plane fallback, Model-changes tense); Tasks (two more files, exact tier-3 text); ACs and Verification (wider grep, exact banner text, file list); Design Notes tier-3 entry.
- **Known-bad state avoided:** a stale "routes only on mock stations" claim surviving in three places; new banner copy shipped without owner sign-off.
- **KEEP:** attempt 1 (`_bmad-output/implementation-artifacts/docs-truth-up-4-attempt-1.patch`) is otherwise correct and verified — reuse it: the Status, Where-it-lives, Custom-labels, SES, `/api/sim/*`, Templates, Validation and Persistence notes; the retitled "Model changes this needed (all four done)" with each **Done:** symbol; enclosure wrapping marked done; the `app.rs` field/`with_simulator` and `handlers.rs` section comments; deleting the "land next" div and the `HardDrive` import.

## Review Triage Log

### 2026-11-08 — Review pass
- verdicts: 21 findings — high 0, medium 3, low 15, false 3, maybe-false 0
- findings:
  - `[low]` `[bad_spec]` (blind) `wipe-cli/src/main.rs:495-497` comment still says the sim routes "stay unavailable" on real backends — confirmed; triggered the comment-scope amendment (Code Map/Tasks).
  - `[low]` `[bad_spec]` (blind) Tier-3 replacement sentence is new customer-visible copy though Design Notes call it reuse — confirmed (the `:607` banner says only "{detail} Use Export to keep a copy."); triggered the delete-only amendment, shipped words verbatim.
  - `[medium]` `[defer]` (blind) Tier-3 banner still shows server `detail` "…and no control plane is configured" (`store.rs:374`), still hinting a control plane would help — confirmed; server copy is outside the intent's two UI strings; deferred.
  - `[low]` `[defer]` (blind) Devices page "Bench not configured" banner points at CLI flags, not the builder — confirmed (`DevicesPage.tsx:173-176`); pre-existing copy outside the two strings; deferred.
  - `[low]` `[reject]` (blind) Empty frontmatter `deferred` list contradicts the Design Notes' deferred console line — fix edits this build's spec; the console line is now in `deferred` with the detail-text item.
  - `[low]` `[bad_spec]` (blind) Templates note implies the 32-bay chassis cannot be built — confirmed: `arma/industrial-4u-32` is in the bundled catalog and the builder's model picker, and `--bay-profile arma-4u-32` exists; Code Map amended.
  - `[low]` `[bad_spec]` (blind) Status note does not say what the desktop-shell operator sees — partly confirmed: the panel hides simulator controls (`simAvailable = !detached.isError`) and asks for a drive the shell's mock never reports, so it waits indefinitely (not "silently an empty list"); Code Map amended.
  - `[false]` `[reject]` (blind) Persistence note's "holds until restart" is wrong for the control-plane tier whose save fails — `detect_store` never returns a `ControlPlaneStore`: `probe` always errors and it falls back to `EphemeralStore` (`store.rs:355-366`); omitting the revision conflict is not a false statement.
  - `[low]` `[bad_spec]` (blind) Model-changes item 3 keeps present-tense bug text beside **Done:**, lead-in calls a done item open, `m2` not flagged as not re-checked — confirmed; Code Map amended.
  - `[low]` `[reject]` (blind) No render test pins the banner copy or inspector binding options — static copy in a docs-and-copy story with no behaviour change; verification-gap layer found no gap; adding UI tests is new surface.
  - `[low]` `[bad_spec]` (edge) same `main.rs:495-498` comment — grouped with comment scope.
  - `[low]` `[bad_spec]` (edge) `client.ts:82-84` "Only present when the station runs the mock backend" — confirmed false for the desktop shell; grouped, amended.
  - `[low]` `[bad_spec]` (edge) `BenchSetupPage.tsx:136-137` "Only the mock backend simulates hot-plug; a real station 404s here" — confirmed; grouped, amended.
  - `[low]` `[bad_spec]` (edge) spec claim "Two code comments repeat the mounting claim" — confirmed at least five; grouped (comment-scope autonomous decision).
  - `[low]` `[bad_spec]` (edge) option (b) "reuses existing wording" claim — same root as the tier-3 copy entry; grouped.
  - `[medium]` `[defer]` (verification-gap, other) Tier-3 `{status.detail}` still suggests a control plane (`store.rs:362`, `:374`) — grouped with the blind detail-text entry; deferred.
  - `[medium]` `[defer]` (intent) Banner promise lives partly in server `detail` text the diff cannot reach — same root as the detail-text entry; deferred.
  - `[low]` `[reject]` (intent) Checks are source greps, never the rendered banner — same as the blind test entry; static copy, no behaviour change.
  - `[low]` `[bad_spec]` (intent) Banner reworded rather than removed — same root as the tier-3 copy entry; amended to deletion.
  - `[false]` `[reject]` (intent) New doc claims (Tauri status, validation split, atomic save) are untested — each checked at HEAD in triage: `lib.rs:143` only `with_static_dir`; `validateLocal` vs `BayTopology::validate`; `LocalFileStore::save` temp file + rename.
  - `[false]` `[reject]` (intent) SPEC's `handlers.rs:468` vs edited `:523-529` — the auditor itself says it is the same comment after line drift.
- verification-gap layer: no gap findings, 1 other finding. Total 21 = blind 10 + edge 5 + verification-gap 1 + intent 5.

### 2026-11-08 — Review pass
- verdicts: 17 findings — high 0, medium 1, low 11, false 5, maybe-false 0
- findings:
  - `[false]` `[reject]` (blind) Tier-3 remaining sentence ("until this station reboots") is false in the desktop shell, where layouts are lost on quit — the shell never shows the tier-3 banner: `AppState::with_static_dir` builds `EphemeralStore::new(.., false)` (`app.rs:93-96`), so `needs_operator_decision` is false and the tier-4 "Configuration is not saved" banner shows instead.
  - `[low]` `[defer]` (blind) Tier-3 banner gives no Export hint while the lower-risk tier-4 banner does — pre-existing (the shipped paragraph never mentioned Export); adding it is new customer-visible copy for the owner; deferred.
  - `[low]` `[defer]` (blind) `docs/ARCHITECTURE.md:33-34` says sim routes 404 "unless the backend simulates hot-plug", false for the mock-backed desktop shell — confirmed; ARCHITECTURE is story 1's file and outside this story's intent; deferred.
  - `[low]` `[defer]` (blind) Unbuilt SES auto-detect, custom labels and Devices-page entry links have no `docs/ROADMAP.md` entry — confirmed; pre-existing, roadmap entries are prioritisation outside this story; the SES dependency on `wipe-engine-linux` is still stated in the doc's binding mode 4 text; deferred.
  - `[low]` `[patch]` (blind) Design doc does not say the desktop shell wires no store, so saved layouts are lost on quit — confirmed (`with_static_dir` → `EphemeralStore`); added one sentence to the Persistence note.
  - `[low]` `[reject]` (blind) Intent-contract Problem still says "Two code comments" — fix edits this build's spec; the Autonomous decisions entry records the wider scope.
  - `[low]` `[patch]` (blind) wipe-server comment hard-codes the desktop shell's current wiring and will go stale when it calls `with_simulator` — confirmed for `handlers.rs`; deleted the shell clause so it states the contract only (desktop-side comments keep it, as they describe their own host).
  - `[low]` `[reject]` (blind) Removing the inspector hint leaves no pointer to identify mode — the removal is the intent's instruction; a replacement pointer is new customer-visible copy, more than a direct correction.
  - `[low]` `[reject]` (blind) `m2` cramping bullet left "not re-checked" — the doc now says so plainly rather than asserting either way; settling it needs a visual check of `BayMap.tsx` at `m2` size, more than a direct correction, and readers are not misled.
  - `[low]` `[patch]` (blind) Templates note paraphrases the menu labels and says the "starting points" copy is "shown as designed" though the shipped wording differs — confirmed (`BenchSetupPage.tsx:478-497`, `:508-509`); note now quotes shipped labels and sentence.
  - `[low]` `[patch]` (edge) `BenchSetupPage.tsx` `detached` comment says only a 404 hides the simulated controls; `simAvailable = !detached.isError` hides them on any error — confirmed; comment corrected.
  - `[low]` `[patch]` (edge) `crates/wipe-engine/src/backend.rs:88-89` `DeviceSimulator` doc says the server "only mounts the `/api/sim/*` routes when something does" — confirmed; missed by the Code Map's list though the autonomous decision covers every such comment; comment corrected.
  - `[medium]` `[defer]` (intent) Tier-3 banner still renders server `detail` implying a control plane would help — carried: same location and claim as pass-1 row; already in `deferred`.
  - `[false]` `[reject]` (intent) `--control-plane-url` CLI help (`main.rs:70-73`) still points at a control plane — the help says the hub "is future work" and that without a reachable control plane the station runs ephemeral and says so; it is accurate. The console line is carried in `deferred`.
  - `[low]` `[reject]` (intent) Verification is source greps, never the rendered screen — carried: same claim as pass-1 rows; static copy, no behaviour change.
  - `[false]` `[reject]` (intent) "Identify mode waits indefinitely in the shell" is code-read only — the mock adds or removes devices only through `DeviceSimulator::attach`/`detach` (`wipe-engine-mock`), which the shell never attaches, so no new drive can appear.
  - `[false]` `[reject]` (intent) SPEC `handlers.rs:468` vs edited `:523` — carried: same comment after line drift.
- verification-gap layer: no findings. Total 17 = blind 10 + edge 2 + verification-gap 0 + intent 5.

## Design Notes

Autonomous decisions:
- **Tier-3 banner wording.** Superseded by the intent-contract autonomous decision (review pass 1): shipped words only.
- **Console line `main.rs:480`** ("the UI will ask the operator to point at a control plane or accept the loss") becomes slightly stale too. The SPEC limits copy changes to two UI strings, so it is deferred, not edited.
- **In-place correction of a design doc.** ADRs take addenda only; design docs describe intended UI and are corrected in place (AGENTS.md: update `docs/design/*` with the code), keeping the rationale and marking status per section.

## Verification

**Commands:**
- `grep -rn "land next\|control-plane-url" apps/desktop/src` -- expected: no output.
- `git diff --stat 2e85613` -- expected: six files plus this story file and the attempt-1 patch.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: pass, 200 tests.
- `docker compose up -d && cargo test --workspace` -- expected: pass.

## Auto Run Result

Status: done

**Summary.** The bench-setup design doc, the simulated hot-plug code comments and two UI strings now describe what the code does.
- `docs/design/bench-setup-builder.md`: dated "As built (2026-11-08)" notes. Unbuilt: SES (SCSI Enclosure Services) auto-detect (not even shown disabled), bulk custom labels, the three Devices-page entry links, two validation warnings. Template menu vs Rust presets quoted exactly; the 32-bay chassis remains reachable via the catalog and `--bay-profile`. Persistence is atomic, and the desktop shell wires no store (lost on quit). `/api/sim/*` is always mounted and 404s without `with_simulator`, so identify mode waits indefinitely in the desktop shell. "Model changes this needed (all four done)", each with the symbol that closed it; enclosure wrapping done; `m2` label cramping marked not re-checked.
- Comments: seven simulator comments now state the contract (always mounted; 404 unless the host calls `AppState::with_simulator`).
- UI: removed "Learn by hot-swap and SES auto-detect land next"; the tier-3 "cannot save configuration" banner drops the `--control-plane-url` advice and keeps only its shipped words: "Continue without persistence — the bench will work normally until this station reboots."

**Files changed**
- `docs/design/bench-setup-builder.md`: as-built notes and status corrections.
- `apps/desktop/src/pages/BenchSetupPage.tsx`: two copy edits, one comment, unused `HardDrive` import removed.
- `apps/desktop/src/api/client.ts`, `crates/wipe-cli/src/main.rs`, `crates/wipe-server/src/app.rs`, `crates/wipe-server/src/handlers.rs`, `crates/wipe-engine/src/backend.rs`: comment-only.
- `_bmad-output/implementation-artifacts/docs-truth-up-4-attempt-1.patch`: attempt 1, kept for the loopback record.
- This story spec.

Note: `crates/wipe-engine/src/backend.rs` is outside the six-file AC list. It is a seventh comment that the review found, covered by the intent-contract autonomous decision to correct every comment that repeats the claim. The file-list AC is an undercount, not a scope breach.

**Review.** Two passes, one bad_spec loopback (review_loop_iteration 1).
- Pass 1, 21 findings (medium 3, low 15, false 3). bad_spec: comment inventory incomplete (three more comments), and the prescribed banner sentence was partly new copy. Both resolved by autonomous decisions: correct every comment, and use shipped words only. Doc precision items were folded in. Deferred: server-sent banner detail and the console line still imply a control plane, and the Devices banner points at CLI flags. Rejected: the spec-edit item, the two render-test items, and three false findings.
- Pass 2, 17 findings (medium 1, low 11, false 5). Patched (5, all low): `backend.rs` comment, desktop-shell clause removed from the `wipe-server` comment, the `detached` comment covers any error, the shell-has-no-store sentence, exact template quotes. Deferred (3, low): no Export hint in tier 3, ARCHITECTURE.md sim-route sentence, no ROADMAP entries for the unbuilt features. Rejected: false tier-3-in-shell claim, spec-edit, the inspector pointer (new copy), the `m2` re-check, CLI help (accurate), and carried rows.

**Follow-up review recommended:** false. Pass 2 patched 0 high, 0 medium, 5 low.

**Verification.**
- Greps for `land next`/`control-plane-url` in `apps/desktop/src` and for the conditional-mounting phrases in `crates` and `apps/desktop/src`: no output.
- Rust diff is comment-only (rustfmt on touched files only).
- `apps/desktop`: `pnpm typecheck` ok, `pnpm test` 200 passed.
- `cargo test --workspace` (compose up): exit 0, 696 passed, 0 failed.

**Residual risks.**
- The tier-3 banner's server-sent detail still says "no control plane is configured" (deferred, medium). It needs an owner wording decision.
- Doc notes cite files and symbols, so they survive line moves but not renames.
