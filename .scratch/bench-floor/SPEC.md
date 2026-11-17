---
id: SPEC-bench-floor
companions: [../../docs/ROADMAP.md, ../../docs/adr/0002-configurable-bay-topology.md, ../../docs/adr/0003-station-config-persistence.md, ../../docs/adr/0004-enclosure-model-catalog.md, ../../CONTEXT.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# The bench as a physical place

## Why

ROADMAP O2 and O3 aim for "nothing needing a human sits invisible" and a station that "works on a loud, badly-lit floor at arm's length". Four pieces are still missing and none waits on a decision:
- an operator learns nothing when a station restarts mid-erasure (O-R7, O2.3);
- drives leave the bench with no label tying them to their certificate (O-R6, O2.4);
- there is no view legible from across the room (O3.1);
- the UI is sized for a mouse and an office monitor, not gloves and warehouse light (O3.2, O3.4).

Decisions taken by default, recorded here:
- **The restart record holds no evidence and no customer identifiers.** CONTEXT §7 #5 says a station pulled off the rack should hold "nothing that matters". The record of in-flight Jobs therefore keeps only the Job id, bay label, start time and method, never the serial, asset tag, operator or evidence. The bay is enough, because the drive is still physically in it. The record is kept only where the station already persists configuration (ADR-0003 tier 1). A RAM-only station (PXE, `--ephemeral-config`) cannot know it restarted, and the bench says so.
- **The label QR carries the certificate's identity, not a URL.** Its payload is `WSC1:<certificate UUID>:<canonical SHA-256 hex>` in upper case, so QR alphanumeric mode keeps it small. It matches a label to its certificate file offline, forever. A URL would die with a domain (ROADMAP §0.3), and the domain is not chosen yet.
- **Labels print through the operating system's print dialog** from a print-styled page. Any printer with a driver works, including the common thermal label printers. Label size is a per-browser preference (local storage) with presets: 50×25 mm (default), 2×1 in and 4×2 in.
- **Bench mode and high contrast are per-browser preferences**, like the operator session. High contrast also turns on with the operating system's `prefers-contrast: more`.
- **Accessibility target: WCAG 2.1 AA** (Web Content Accessibility Guidelines, the standard accessibility checklist), checked by axe (an automated accessibility checker) in the Playwright suite that hardening story 10 adds.

## Capabilities

- **CAP-1 Restart recovery notice**
  - **intent:** After a station restarts, the operator sees which bays had an erasure running, that those drives were not certified, and what to do.
  - **success:**
    - The station keeps `interrupted-jobs.json` beside `lease-state.json`, written atomically through the same persistence choice as the lease (tier 1 only). It holds one entry per Job between its first `InProgress` and any terminal or Pending state: `{job_id, bay_label, started_at, method_human}`, and nothing else.
    - At start-up, entries left from the previous run become **interruptions**, and the file is cleared. `GET /api/interruptions` lists them. `POST /api/interruptions/{job_id}/acknowledge` removes one, recording the session operator in the station log. Both routes are declared in `app.rs`, called from `client.ts`, and the golden is regenerated.
    - The Inbox shows each interruption first, counted in the nav. The bay's card on the bay map shows an "interrupted" attention state until acknowledged. On a station with no persistence, the Inbox shows a one-line note instead: "This station keeps nothing across restarts, so it cannot list erasures a restart interrupted."
    - Proposed operator wording (each interruption): "**Bay {bay}: erasure interrupted by a restart.** The erasure started {time} was not finished and has no certificate. Start a new Job for this drive. A drive may still be completing an erasure on its own, so check the bay's status before starting." Acknowledge button: "Done".
    - A restart caused by the station itself (a crash) and a normal stop are treated the same: a normal stop with Jobs running records them too.
    - Tests:
      - a station started over a leftover file lists the interruptions;
      - acknowledging one removes it from the list and the file;
      - a Job finishing normally leaves no entry;
      - a RAM-only station writes nothing and reports the note;
      - nothing in the file beyond the four fields (a schema test);
      - desktop vitest for the Inbox item and the bay attention state.
  - **checkpoint:** the operator wording is drafted by the agent, not yet owner-approved; the owner approves the finished story before it counts as done.
- **CAP-2 Certificate labels with an offline QR**
  - **intent:** An operator prints a label for each sanitized drive, one at a time or a whole tray, whose QR ties the drive to its certificate without any website.
  - **success:**
    - The Certificate page and each finished Job in the Jobs list have **Print label**. The Jobs list's Finished group has select-and-print for a tray. Only Jobs whose certificate exists (Erased, Destroyed) can be printed.
    - The label shows:
      - the QR (`qrcode-generator`, already a dependency, rendered as SVG);
      - the disposition ("Erased" / "Destroyed") with the NIST method;
      - the drive model and serial;
      - the asset tag when present;
      - the date;
      - the first 8 characters of the certificate UUID;
      - "EVALUATION" when the certificate is an evaluation one.

      The layout is one `@page`-sized print stylesheet per preset, with no clipping at the smallest size; a test asserts it per preset.
    - The QR payload is exactly `WSC1:<UUID>:<SHA256>` from the certificate's `id` (without `urn:uuid:`) and `signature.canonical_sha256_hex`, both upper case. A shared function and unit test define it.
    - The `/verify/` page (`cert-verification` story 3) gains "Check a label". The operator pastes or types the QR text and the page says whether it matches the loaded certificate (UUID and hash) or which part differs. The parsing is in the shared Rust core (`wipe-verify`, `parse_label`), not TypeScript.
    - The Tauri desktop shell is checked to print. If its webview cannot open the print dialog on a platform, the label view offers "Save as SVG" there instead, and the story's result records which platforms were checked.
    - Tests: payload function; label component per preset; tray selection prints only certificated Jobs; the `/verify/` label check for match, wrong hash, wrong id and malformed text.
- **CAP-3 Wall-board view**
  - **intent:** A supervisor across the room can see the bench's state at a glance.
  - **success:**
    - `/wallboard` in the desktop app is a full-screen, read-only view with no navigation chrome and no actions. It shows:
      - the bay grid from the server-resolved topology (ADR-0002, never re-derived in TypeScript) with each bay's state;
      - counts: erasing, finished since start-up, needing attention (the Inbox count);
      - the station name and licence state.
    - It updates live over `/api/events`, reconnects by itself, and shows "Not connected — last update {time}" when the connection drops.
    - Body text is at least 32 px, bay states are distinguishable without colour (a label or icon per state), and it meets AA contrast in both themes.
    - Linked from the nav and openable directly by URL (for a kiosk browser).
    - Tests: the board renders each bay state with its text label; the counts derive from the same functions as the Inbox and Jobs groups (no second copy); the disconnected state.
- **CAP-4 Gloved hands and high contrast**
  - **intent:** An operator in gloves under poor light can use every bench screen.
  - **success:**
    - A **Bench mode** toggle (in the operator menu, remembered per browser) makes every interactive target at least 56×56 px with 8 px spacing, and enlarges body text one step.
    - A **High contrast** toggle (also on with `prefers-contrast: more`) gives text and controls at least 7:1 contrast. The bay-status palette keeps ADR-0004's luminance ordering in high contrast.
    - Both are Tailwind theme variants, not per-component overrides.
    - Tests:
      - a vitest walks every page's rendered interactive elements in bench mode and asserts the minimum size class is applied;
      - a palette unit test checks every token pair's contrast ratio in normal (≥ 4.5:1 text) and high contrast (≥ 7:1);
      - the toggles persist and fail safe (storage errors leave defaults).
- **CAP-5 Accessibility audit and fixes**
  - **intent:** Every bench page can be used with a keyboard alone and meets WCAG 2.1 AA.
  - **success:**
    - The Playwright suite (hardening story 10) runs axe on every desktop route, with the bay map populated and Jobs in every group, in normal and high-contrast themes. The test fails on any serious or critical violation; the story fixes what it finds.
    - Keyboard-only Playwright flows: start a Job from the bay map, batch start, record a destruction, acknowledge an interruption, print a label (up to the dialog). Every step is reachable by Tab/Enter/Space with a visible focus ring and a logical focus order, and dialogs trap and return focus.
    - Findings and fixes are listed in the story result. Anything not fixed is recorded with its reason in CONTEXT §11.

## Constraints

- Bay matching stays in Rust (ADR-0002). Every bay ↔ device mapping comes from the server.
- Only `JobRunner` mutates Jobs. The interruption record is written from runner state changes and never mutates a Job.
- New routes are declared once in `app.rs` `api_routes()` and called from `client.ts`; the route golden is regenerated.
- No evidence, serials, asset tags or operator identities on disk (CONTEXT §7 #5). Labels and the wall-board are rendered from data the station already serves.
- Customer-visible wording is the approved text verbatim once approved.

## Non-goals

- Keeping certificates across a restart (DECISIONS D20) and resuming an interrupted erasure (needs a real backend, D12).
- Operator sign-in and roles (D2); throughput and ETA (O1.5 remainder).
- Direct printer drivers (ZPL and similar); label templates beyond the three presets.
- Accessibility of the portal and marketing site (the site already runs Lighthouse).

## Success signal

- An operator stops a station mid-tray, starts it again and finds each interrupted bay in the Inbox. They print a tray of labels and scan one into `/verify/` next to its certificate, which matches. A supervisor reads the wall-board from three metres away, and the axe run is clean.
