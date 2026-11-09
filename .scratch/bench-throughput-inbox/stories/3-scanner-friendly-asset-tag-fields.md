---
title: 'Scanner-friendly asset-tag fields'
type: 'feature'
created: '2026-11-09'
baseline_revision: 'f37b2f7ca0b72e993f254ed075c937c654e84a0f'
status: 'done'
review_loop_iteration: 0
followup_review_recommended: true
context: []
warnings: [oversized]
deferred:
  - summary: >-
      No test renders the Sanitize wizard, batch wizard or Job-page record-for-destruction dialog, so their scan-group wiring is unchecked.
    evidence: |-
      ScanInput.test.ts uses stand-in groups; EraseWizard and BatchWizard are private to DevicesPage and need query, operator and api mocks to render. Reverting the EraseWizard ScanGroup to a div, or TicketField to a plain input, leaves all 362 tests green. Same missing page harness as story 1's deferred interaction tests.
    location: >-
      apps/desktop/src/pages/DevicesPage.tsx (EraseWizard, BatchWizard); apps/desktop/src/pages/JobDetailPage.tsx (RecordForDestructionButton)
    severity: medium
---

<intent-contract>

## Intent

**Problem:** On a real floor asset tags are entered with a HID-wedge barcode scanner (a scanner that "types" the barcode as keystrokes, then presses Enter). Only the Record-for-destruction page handles that Enter; in the Sanitize wizard, the batch wizard and the Job page's record-for-destruction dialog the Enter does nothing, so the operator must reach for the mouse between every scan (ROADMAP O-R5, O3.3).

**Approach:** One shared `ScanInput` text field: typed characters fill it as usual; Enter (a scanner's terminator) is swallowed and moves focus to the next scan field in the same scan group, or stays put when there is none, or runs a caller-supplied action. Every asset-tag field in the desktop UI uses it. A component-level vitest test renders it in a DOM (`happy-dom`, as `sites/www` already does).

## Boundaries & Constraints

**Always:**
- Enter in a scan field never starts, records, adds or submits anything except where the caller passes an explicit `onScanEnter` (only the Record-for-destruction page's serial field, which keeps today's "Enter adds the drive").
- Enter is swallowed (`preventDefault`, `stopPropagation`) so no enclosing form or global key handler sees it.
- Enter while an IME (input method editor, used for composing e.g. CJK text) is composing is ignored, left to the IME.
- The field's value is never altered on Enter; trimming stays where requests are built today.
- Fields covered: Sanitize wizard asset tag → Ticket/RMA; each batch-wizard row's asset tag → next row's asset tag (last row: focus stays); Job page record-for-destruction "Scanned" asset tag → serial (serial Enter: focus stays); Record-for-destruction page asset tag → serial → add drive (behaviour unchanged, now via `ScanInput`).
- No change to Rust, routes, request shapes or `api/types.ts`.

**Never:**
- Never move focus to a button (Begin, Record, Add): a second scan's Enter would then press it.
- No scan-speed detection, auto-submit, sound, or asset-tag lookup (CONTEXT "Asset-tag lookup" is a separate WorkOrder feature).
- No hardware-sanitization claim in any new copy (ROADMAP §0.1).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Scan then Enter, next field exists | "AT-0042" typed + Enter in field 1 of a group of 2 | Field 1 holds "AT-0042"; focus in field 2; no button clicked | — |
| Enter in last field of group | Enter in last scan field | Focus stays; value kept; nothing submitted | — |
| Caller action | `onScanEnter` given, Enter | Action runs once; focus does not auto-advance | — |
| IME composing | Enter with `isComposing` true | Not handled (no advance, no preventDefault) | — |
| Other keys / Tab | Any key but Enter | Default behaviour | — |
| Batch, 8 rows | Scan 8 tags in a row | Each row gets its tag; focus ends on row 8; Begin not pressed | — |

</intent-contract>

## Code Map

- `apps/desktop/src/pages/RecordForDestructionPage.tsx:74-87,136-151` -- existing scanner handling (`onTagEnter`/`onSerialEnter`, refs `tagInput`/`serialInput`); the pattern to generalise; keep `tagInput.current?.focus()` after add.
- `apps/desktop/src/pages/DevicesPage.tsx:648-760` -- `EraseWizard`: asset tag input (~L713), `TicketField` (~L847, shared with batch).
- `apps/desktop/src/pages/DevicesPage.tsx:~905-1080` -- `BatchWizard`: per-row asset tag input in table body (`aria-label="Asset tag for bay …"`), rows in bay-map order.
- `apps/desktop/src/pages/JobDetailPage.tsx:286-302` -- `RecordForDestructionButton` "Scanned" asset tag / serial inputs.
- `apps/desktop/vite.config.ts` -- vitest `environment: "node"`, `include: ["src/**/*.test.ts"]`; comment says a DOM harness can be added when component tests arrive.
- `sites/www/test/lead-form-dom.test.ts` + `sites/www/package.json` -- precedent: `// @vitest-environment happy-dom` per file, `happy-dom ^21.0.4`.
- `apps/desktop/src/bench/BayMap.test.ts` -- style precedent: `createElement` in `.test.ts`, no JSX.
- No `<form>` wraps these fields today (only `operator/SignInModal.tsx`), so Enter currently is a no-op there.

## Tasks & Acceptance

**Execution:**
- `apps/desktop/package.json` (+ `pnpm-lock.yaml` via `pnpm add -D happy-dom@^21.0.4`) -- add DOM test environment -- required by CAP-3's component-level test.
- `apps/desktop/src/components/ScanInput.tsx` -- new: `forwardRef` input passing through input props, `data-scan-field`; exported `ScanGroup` (or a `data-scan-group` container) and pure helper deciding whether a key event is a scan Enter; Enter → `onScanEnter` if given, else focus next enabled `[data-scan-field]` after this one inside the nearest `[data-scan-group]`, else stay -- one behaviour for every asset-tag field.
- `apps/desktop/src/components/ScanInput.test.ts` -- `// @vitest-environment happy-dom`; render with `react-dom/client` + `act`; cover every I/O-matrix row including a button after the last field never clicked and value preserved; also the pure helper -- CAP-3 success.
- `apps/desktop/src/pages/DevicesPage.tsx` -- `EraseWizard` asset tag and ticket in one scan group (ticket as `ScanInput`); `BatchWizard` table body as a scan group, each row's asset tag a `ScanInput`.
- `apps/desktop/src/pages/JobDetailPage.tsx` -- scanned asset tag and serial as `ScanInput`s in one group.
- `apps/desktop/src/pages/RecordForDestructionPage.tsx` -- replace `onTagEnter`/`onSerialEnter` with `ScanInput` (tag advances to serial; serial `onScanEnter={addDrive}`).
- `apps/desktop/vite.config.ts` -- update the "Node environment" comment to name per-file happy-dom.
- `CONTEXT.md` (§8 paragraph, §10.2 row with limitations), `README.md` if it lists desktop features, `docs/ROADMAP.md` (strike O3.3, note O-R5), `AGENTS.md` (frontend test count) -- docs ship with code.

**Acceptance Criteria:**
- Given the Sanitize wizard is open, when a scanner types a tag and Enter into Asset tag, then the tag is in Asset tag, focus is in Ticket/RMA, and no Job is created.
- Given the batch wizard with N rows, when N tags are scanned in sequence starting at row 1, then each row holds its own tag and Begin has not been pressed.
- Given `cd apps/desktop && pnpm typecheck && pnpm test`, then both pass with the new test file included.

## Spec Change Log

## Review Triage Log

### 2026-11-09 — Review pass
- verdicts: 21 findings — high 0, medium 5, low 15, false 0, maybe-false 0 (one finding, IA5, shares V1's root cause and verdict)
- findings:
  - `medium` `patch` [blind] A scan after Enter lands in a pre-filled field is appended ("AT-4AT-9") — fixed: `focusNextScanField` now selects the next field's text; test "advancing into a pre-filled field selects it" added. Staying in the last field still appends, visibly, rather than silently replacing.
  - `low` `patch` [blind] CONTEXT §8 claimed "without the mouse" though nothing focuses the first scan field on open — fixed: reworded "without the mouse between scans", limitation added to §10.2.
  - `low` `patch` [blind] Batch wizard Ticket/RMA was a ScanInput outside any group, a dead end — fixed: Ticket/RMA and rows wrapped in one `ScanGroup` (ticket → row 1 → … → row N).
  - `low` `reject` [blind] Modifier+Enter is also swallowed — a single-line input does nothing with Shift/Ctrl+Enter and the app has no such shortcut; the fix adds guard conditions for a case not met in daily use.
  - `low` `reject` [blind] Nested scan groups would be crossed — no nested group exists anywhere; the fix adds a filter for unreachable structure.
  - `low` `patch` [blind] Bare `data-scan-group=""` string in DevicesPage — fixed by the same `ScanGroup` wrap (tbody attribute removed).
  - `medium` `defer` [blind] Page wiring untested; ACs cover two of four places — wizard and Job dialog part deferred (see V1); Record-for-destruction part patched (see V2).
  - `low` `reject` [blind] keyCode 229 not exercised through the component — helper is tested and the component passes `e.keyCode` straight through; extra test buys little.
  - `low` `reject` [blind] "No enclosing key handler sees it" overstated for capture-phase listeners — the desktop app registers no capture-phase or native keydown listeners; wording in the intent contract is read-only.
  - `low` `reject` [edge] Modified Enter swallowed — duplicate of blind row 4.
  - `low` `reject` [edge] readOnly/hidden next field — no scan field is ever readOnly or hidden while focusable; disabled is already filtered.
  - `low` `patch` [edge] Batch Ticket/RMA dead end — duplicate of blind row 3, same fix.
  - `low` `reject` [edge] Held Enter (`e.repeat`) repeats `addDrive` — a repeat after the first add hits the empty-scan refusal message; identical to the pre-change handler; a guard adds complexity for a rare case.
  - `low` `reject` [edge] stopPropagation claim — duplicate of blind row 9.
  - `medium` `defer` [verification-gap] V1: Sanitize wizard, batch wizard and Job dialog scan groups have no rendered test — deferred: needs a page harness (query, operator, api mocks, exports of private wizards), pre-existing gap also deferred in story 1.
  - `medium` `patch` [verification-gap] V2: Record-for-destruction page's rewritten scan flow untested — fixed: `RecordForDestructionPage.test.ts` renders the real page, tag Enter → serial, serial Enter adds one drive, clears both, focus back on tag, no fetch; shown to fail without `onScanEnter`, `ref` or `select()`.
  - `medium` `defer` [intent-alignment] Tests reach only the component, not the pages the ACs name — same root as V1, deferred.
  - `low` `patch` [intent-alignment] Batch Ticket/RMA outside the field list and group — same as blind row 3.
  - `low` `reject` [intent-alignment] "Advances" read narrowly; Enter never presses Begin — recorded autonomous decision (safe default), not a defect.
  - `low` `reject` [intent-alignment] Record-for-destruction page now ignores Enter during IME composition, so not "strictly unchanged" — intended per the intent contract's IME rule; an improvement.
  - `medium` `defer` [intent-alignment] Docs describe page-level behaviour while evidence is component-level — same root as V1, deferred.

## Design Notes

Advancing by DOM order within a group avoids threading refs through dynamic batch rows. Sketch:

```ts
const group = el.closest("[data-scan-group]");
const fields = [...(group?.querySelectorAll<HTMLInputElement>("[data-scan-field]") ?? [])]
  .filter((f) => !f.disabled);
fields[fields.indexOf(el) + 1]?.focus();
```

## Autonomous decisions

- **Where Enter goes from the last field.** A scanner sends Enter after every scan; if the last asset-tag field moved focus to Begin/Record, the next accidental scan would start a wipe or record a destruction. Options: (a) focus the action button; (b) leave focus in the last field. Chosen: (b) — the safe default; starting stays a deliberate click (CAP-3: "without submitting the wrong form").
- **Which fields count as asset-tag fields.** ROADMAP O3.3 says "every asset-tag field". Chosen: all four asset-tag inputs in the desktop app, plus the serial and ticket fields that sit directly after them, so the scan chain has somewhere to land. Hub portal and marketing site have no asset-tag fields.
- **Test DOM.** Options: jsdom + Testing Library, or happy-dom alone. Chosen: happy-dom with React's own `act`, the repo's existing precedent (`sites/www`), one dev dependency.

## Verification

**Commands:**
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: clean typecheck; all vitest pass including `ScanInput.test.ts`.
- `cd apps/desktop && pnpm build` -- expected: success.

**Manual checks (if no CLI):**
- `cargo run -p wipe-cli -- serve --static-dir apps/desktop/dist` (mock backend); open the Sanitize wizard and batch wizard, type a tag + Enter in each asset-tag field and see focus advance without starting anything.

## Auto Run Result

Status: done

**Summary.** Every asset-tag field in the desktop UI now takes a HID-wedge barcode scanner (one that types the barcode, then presses Enter). A shared `ScanInput` swallows Enter and moves focus to the next scan field in its `ScanGroup`, selecting that field's text so a scan replaces rather than appends; in the last field focus stays, so Enter never reaches Begin, Record or any button. Chains: Sanitize wizard asset tag → Ticket/RMA; batch wizard Ticket/RMA → row 1 → … → row N; Job-page record-for-destruction (Scanned) asset tag → serial; Record-for-destruction page asset tag → serial → add drive (as before). Enter during IME composition is left alone. No Rust, route or type change.

**Files changed.**
- `apps/desktop/src/components/ScanInput.tsx` — new `ScanInput`, `ScanGroup`, `isScanEnter`, `focusNextScanField`.
- `apps/desktop/src/components/ScanInput.test.ts` — 10 happy-dom component tests covering every I/O-matrix row.
- `apps/desktop/src/pages/RecordForDestructionPage.test.ts` — page-level test of the scan-then-add flow.
- `apps/desktop/src/pages/DevicesPage.tsx` — Sanitize and batch wizard scan groups; `TicketField` is a `ScanInput`.
- `apps/desktop/src/pages/JobDetailPage.tsx` — Scanned asset tag/serial group.
- `apps/desktop/src/pages/RecordForDestructionPage.tsx` — inline Enter handlers replaced by `ScanInput`.
- `apps/desktop/package.json`, `pnpm-lock.yaml` — `happy-dom ^21.0.4` dev dependency; `vite.config.ts` comment.
- `CONTEXT.md` (§8, §10.2), `README.md`, `docs/ROADMAP.md` (O3.3 struck, O-R5 noted), `AGENTS.md` (362 tests).

**Review.** 21 findings: 4 patch entries applied (select text on advance; batch Ticket/RMA joined to the rows' group, removing the bare attribute string; CONTEXT "between scans" wording; Record-for-destruction page test), 1 entry deferred (wizards and Job dialog wiring have no rendered test), 10 rejected low with reasons in the triage log (modifier Enter, nested groups, keyCode 229 component test, capture-phase wording ×2, readOnly/hidden fields, held Enter, narrow "advances", IME on the old page, duplicate modifier row).

**Follow-up review recommended:** true — two medium entries were patched (patched by verdict: high 0, medium 2, low 2). Named unverified risk: select-on-advance and the batch wizard's new wrapper (layout and scroll) have only been checked in happy-dom and by type-checking, never in a browser or against a real scanner.

**Verification.** `pnpm typecheck` clean; `pnpm test` 362/362 passed (17 files); `pnpm build` succeeded. The implementer showed the tests fail when `onScanEnter={addDrive}`, `ref={tagInput}` or `next.select()` is removed, and when Enter jumps to the button or is not swallowed.

**Residual risks.**
- Nobody has scanned into these fields in a browser or the Tauri shell; the manual check was not run.
- Wizard and Job-dialog scan-group wiring is untested (deferred).
- No field is focused when a wizard opens, so the operator clicks once before the first scan.
- A scanner configured to send Tab instead of Enter follows the browser's Tab order, which reaches buttons.
