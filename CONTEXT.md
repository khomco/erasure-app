# Wipestation — Project Context

> The canonical product, domain, and technical vision document. Read this
> before grilling, designing, or shipping. Use `/grill-with-docs` to
> challenge any claim here against the codebase and update inline.

---

## 1. Problem & opportunity

Organizations that retire, redeploy, or destroy storage media need
defensible proof that the data is gone. The market for that proof — data
sanitization software — is dominated by tools (Blancco, BitRaser,
WhiteCanyon WipeDrive, Certus, Active@ KillDisk, EPS XErase) that share a
few persistent weaknesses:

- **Pricing models punish QA.** Blancco bills a license credit the
  moment an erase *starts*, regardless of outcome; a 30-day "no
  duplicate" rule means re-running a drive for verification re-bills.
  ITAD shops budget 10–15% extra license headroom for failures.
- **NVMe support is shallower than the marketing.** Many products
  advertise NVMe Sanitize but in practice fall back to overwrite, fail
  to detect supported actions, or mangle namespaces. The gap between
  *"supports NVMe"* and *"honestly invokes Sanitize action 0x02/0x04
  and captures `Get Log Page 0x81` evidence"* is wide.
- **Certificates lack command-level evidence.** Auditors want to see
  the opcode that was issued, the return status, and the post-op log
  page. Most certs are marketing-grade prose attached to a serial.
- **Verification of authenticity is vendor-dependent.** Certs are
  validated by lookup against the vendor's DB. There is no
  offline-verifiable signature chain that a downstream auditor or end
  customer can verify without phoning home.
- **APIs are an afterthought below Blancco.** Mid-tier products expose
  CSV export at best. Webhook-driven ITAD automation is hard.
- **HPA/DCO/reallocated-sector handling is invisible to the operator.**
  Tools claim NIST Purge without proving they reached non-addressable
  regions.

The regulatory tailwind in 2026 is significant:

- **NIST SP 800-88 Rev. 2** went final 2025-09-26. Defers to
  **IEEE 2883-2022**, adds NVMe-aware mappings, adds a programmatic
  **Validate** step, requires operator email on certificates, calls
  out single-pass-is-sufficient on modern flash.
- **R2v3** (ITAD certification) mandates NIST 800-88 conformance,
  which in 2026 effectively means R2 conformance.
- **EU Right to Repair Directive (2024/1799)** — transposition deadline
  **2026-07-31** — includes "easy deletion of personal data"
  requirements; pulls sanitization tooling down-market into refurb.
- **GDPR Art 17** continues to drive proof-of-deletion demand.
- **~20 US states** have deletion-on-request mandates with no federal
  preemption.
- **IEEE 2883-2022** is now the technical anchor NIST defers to;
  vendors who can't honestly claim alignment will struggle in RFPs.

## 2. Target customers

In rough decreasing order of fit:

1. **Mid-to-large ITAD operators** — process thousands of drives per
   week, sell certificates of destruction to their own customers, hate
   per-event pricing, need fleet visibility, want a customer-facing
   verification portal.
2. **Government / defense / classified environments** — need NIST 800-88
   R2 + IEEE 2883 conformance, air-gap operation, ADISA AL5 +
   Common Criteria EAL 2 minimum, NATO NIAPC listing preferred.
3. **Enterprise IT refresh teams** — internal disk wipe for fleet
   refreshes; want simple operator UX, asset/ticket linkage, integration
   with their ITAM.
4. **Refurbishers / repair shops** — pulled in by EU Right to Repair;
   need consumer-grade simplicity, mobile + storage erasure, low
   per-device cost.

Explicit non-target for v1: end-user "wipe my hard drive" consumer tool.

## 3. Product positioning

> **Tagline frame:** *"The proof is in the certificate."*

Wipestation is a NIST 800-88 Rev. 2 / IEEE 2883-2022 sanitization tool
distributed as a single signed binary with a native desktop UI, an HTTP
API, and mDNS-based fleet coordination. It is designed to be honestly
defensible at the command level, fairly priced per success, and
verifiable offline by anyone with the public key.

### Differentiator stack (ranked by defensibility)

1. **Command-level evidentiary certificate** — every issued opcode,
   action byte, status, and log-page read is captured in the cert.
   Auditors see proof, not prose.
2. **Outcome-bearing Job model** — one Job per Asset, composing every
   attempt (Diagnostic, ErasureEvents — possibly several, Verification,
   DestructionEvent if it comes to that) into one signed Certificate
   with the full evidence chain. Retries and *"had to shred after
   erasure failed"* are first-class parts of the audit story, not
   separate records to be joined later. See ADR-0001.
3. **Offline-verifiable signature chain** — Ed25519 detached signature
   over canonical JSON. Anyone with the public key validates a cert
   without contacting the vendor.
4. **ITAD-ERP integration first-class** — REST in/out, webhooks on
   Job state transitions and cert signing, asset-id lookup callbacks.
   Wipestation is not a CRM; the ITAD's ERP stays the system of
   record. WorkOrder identifiers shared across systems so reconciliation
   is a join, not an export.
5. **Per-success licensing** — credit consumed only on a verified NIST
   Clear/Purge success. Failures, aborts, and re-runs are free.
   Anti-Blancco by design.
6. **Honest NVMe** — `NVMe Sanitize` first (Crypto Erase when SED
   provisioned, Block Erase otherwise), explicit fallback order, full
   `Get Log Page 0x81` capture.
7. **HPA/DCO detection + clear before Purge** — refuse to claim Purge
   if non-addressable regions weren't reached.
8. **Programmatic Validate workflow** — built-in registry for the R2
   Validate step (per-media-class assurance), distinct from per-event
   verification.
9. **Open cert format** — publish the JSON-LD spec; third parties can
   build verifiers; lock-in by quality, not opacity.
10. **UEFI Secure Boot baseline** — signed shim + GRUB on the bootable
    image. No "disable Secure Boot" step in any workflow.
11. **Bundled mobile (phase 2)** — undercuts double-SKU competitors.

### What we are not

- We are not a Blancco clone. We deliberately remove per-event billing
  and add offline cert verification.
- We are not a hobbyist tool. We target compliance procurement; ADISA
  AL5 + Common Criteria EAL 2 is the certification floor.
- We are not a destruction-only product. We model the Destroy path
  (chain-of-custody, supervisor co-sign) but our primary value is in
  Clear/Purge.

## 4. Standards posture

| Standard | Role | Status |
| --- | --- | --- |
| **NIST SP 800-88 Rev. 2** | Decision framework: Clear / Purge / Destroy; Validate step; cert fields | Final 2025-09-26 |
| **IEEE 2883-2022** | Technical "how" for every method NIST defers on | Active |
| **ISO/IEC 19790** | Cryptographic module zeroization, referenced by R2 for crypto-erase | Active |
| **NIST SP 800-57 Pt 1 Rev. 5** | Key-management criteria for crypto-erase eligibility | Active |
| **FIPS 199** | Confidentiality impact level (Low/Moderate/High) — input to R2 flow | Active (but **not exposed to floor operators** — see §7) |
| **ADISA Assurance Level 5** | Independent product certification — our floor | Year-1 target |
| **Common Criteria EAL 2 / EAL 2+** | Procurement requirement for gov/defense | Year-1/2 target |
| **R2v3, e-Stewards** | ITAD operator certifications — drives Wipestation adoption | Customer-side |
| **NATO NIAPC** | Defense procurement listing | Year-2 target |

## 5. Core domain concepts

The ubiquitous language of this project. If you find yourself using a
synonym, push back or update this list.

### Hardware / media

- **Device** — a single addressable storage unit (HDD, SATA SSD, NVMe
  SSD, eMMC, UFS, USB flash). Always has a stable `DeviceId`, vendor,
  model, serial, capacity, media type, bus, and a path on the host.
- **Capabilities** — what a device supports: ATA Security, NVMe
  Sanitize actions, TRIM, crypto-erase, SED status, HPA/DCO presence,
  frozen state.
- **Media class** — coarse grouping used for the *Validate* registry
  (e.g. `ssd-nvme`, `ssd-sata`, `magnetic-hdd`, `emmc`).

### Bench topology

The operator's physical workspace, modelled so the screen can mirror
the hardware in front of them. See ADR-0002.

- **Enclosure** — one physical housing that presents Bays to the
  operator: a rackmount chassis, a benchtop duplicator, a hot-swap
  dock, an NVMe carrier, a single-drive USB caddy. A station has one
  or more. *Avoid: "chassis" (that's one kind of Enclosure), "machine",
  "box".*
- **Bank** — a contiguous grid of Bays within an Enclosure that share a
  form factor, orientation and numbering run. The chassis on the
  reference bench has two Banks separated by a ventilation column;
  modelling them as one 4-column grid would misplace every bay on
  screen. *Avoid: "column", "group", "cage".*
- **Bay** — one physical slot that holds at most one Device. Has a
  stable `BayId`, an operator-facing `label` (what is silkscreened on
  the hardware, not our index), a grid position within its Bank, and a
  **BayBinding**. A Bay with no Device resolved is **empty**, which is
  a distinct and useful state from *idle* — empty means "a drive can
  go here", idle means "a drive is here and no Job is running". *Avoid:
  "slot" as a noun for this — `slot` is reserved for the SES device
  slot number.*
- **BayBinding** — the rule resolving which Device occupies a Bay:
  by SES device slot number, by `/dev` path, by serial, by WWN, by
  explicit `DeviceId`, or unbound. Real hardware will bind by SES slot
  (SAS-3 expanders report a 0–255 device slot number, 255 meaning "no
  slot"); the mock backend binds by `DeviceId`; an unconfigured bench
  falls back to enumeration order.
- **BayTopology** — a station's declared description of its Enclosures,
  Banks and Bays. Station-scoped configuration, served over the API so
  a roaming operator tablet renders the *station's* hardware, not the
  tablet's. *Avoid: "layout" (too vague), "rack map".*
- **Bay map** — the on-screen vector rendering of a BayTopology with
  each Bay carrying its live **slot status**. The screen-to-hardware
  glance is the whole point: an operator standing at the bench should
  be able to look at a red bay on screen and reach for the right
  physical tray without counting.
- **Identify mode** — the bench-setup flow that learns which Bay is
  which by watching the operator's hands. The station watches its own
  device list; when a drive appears the UI asks *"which bay did you put
  it in?"*, the operator clicks that Bay, and a **BayBinding** by path
  is written. Removal works the same way. This is how an unknown
  chassis gets mapped with no vendor data and no SES, and it is the
  reason bindings are declared rather than positional. *Avoid:
  "discovery" — that's mDNS; "auto-detect" — that's the future SES
  path, which needs no operator at all.*

### Sanitization

- **Category** (NIST R2) — `Clear` / `Purge` / `Destroy`.
- **Method** — concrete technique (`nvme_sanitize_crypto_erase`,
  `ata_secure_erase` enhanced/basic, `block_overwrite`, `opal_revert`,
  `destroy`). Each method maps to a Category.
- **Classification** (FIPS 199) — `Low` / `Moderate` / `High`. Input
  to method selection.
- **Intent** — `Reuse` / `Recycle` / `Destroy`. Input to method
  selection.
- **Verification** — post-erase sampled reads (entropy or pattern
  check) producing a `VerificationReport`.
- **Validation** (capital-V, R2-specific) — a *programmatic, one-time
  per-media-class* assertion that a Method works on that class.
  Distinct from per-event Verification.

### Evidence & cert

- **CommandEvidence** — record of one command issued to one device:
  interface (`nvme-admin`, `ata-passthrough`, etc.), opcode, action,
  raw CDB, status, sense, log page, duration, optional note.
- **Certificate of Sanitization** — JSON-LD document containing the
  device snapshot, capabilities, spec, resolved method, all command
  evidence, verification report, validation reference, operator,
  media status, timestamps.
- **SignedCertificate** — Certificate + detached signature
  (algorithm, public-key id, canonical SHA-256, base64 signature).
- **Public key id** — `ed25519:<base64(SHA256(verify_key)[..16])>`.

### Job orchestration

> **Glossary and code agree.** The ADR-0001 model below shipped in
> v0.2 (§11 #2): `Job` → `ErasureEvent`, a new outer `Job`,
> `JobEvent` → `JobUpdate`. `JobEvent` no longer exists anywhere in
> the codebase. Use these terms in code, docs and design discussion
> without qualification.

- **Job** — the goal-oriented unit of work: process this **Asset**
  (or freeform `Device`+`asset_tag` in Simple mode) to a terminal
  disposition. Composes one or more typed events and signs one
  **Certificate** covering the full evidence chain. *Avoid: erasure,
  wipe, "the run".*
- **JobState** — outer state machine: `Queued → InProgress →
  (Erased | PendingCoSign → Destroyed | Quarantined | Aborted)`.
  Reflects the Asset's terminal disposition, not the progress of any
  single attempt.
- **PendingCoSign** — the audit-honest waypoint on the Destroy path: the
  Asset has been physically destroyed (or scheduled for it), a
  **DestructionEvent** has captured the evidence, and the Job is waiting
  for supervisor co-sign on its **DestructionManifest**. The certificate
  is generated *at* PendingCoSign with `media_status.operational =
  false`; co-sign attaches the second signature and moves the Job to
  `Destroyed`. Not a terminal state. *Avoid: "pending", "awaiting
  approval" — the co-signature is the specific thing being waited on.*
- **AssetDisposition** — the resolved terminal outcome
  (`Erased | Destroyed | Quarantined`) stated explicitly on the
  certificate so an auditor never re-derives it from the activity chain.
- **ErasureEvent** — one attempted wipe within a Job. Carries the
  inner state machine, the chosen `Method`, the captured
  `CommandEvidence`, the resulting `VerificationReport`, and the
  station id (so cross-station retries can be analysed). A Job may
  contain several (retries; method fallback). A cryptographic
  instant-purge counts as one ErasureEvent that completes in
  milliseconds. *Avoid: erasure, wipe-attempt, "the job" — that's
  the outer unit.*
- **ErasureEventState** — inner state machine: `Queued → Probing →
  (Unfreezing) → Confirming → Running → Verifying → GeneratingCert →
  Signing → Completed`, with `Failed` and `Aborted` as terminal
  escapes. Note that a `Failed` ErasureEvent does **not** fail the
  Job — the Job stays `InProgress` awaiting an operator decision
  (retry, fall back to another Method, or escalate to Destroy).
- **JobActivity** — the sum type of everything a Job composes:
  `Diagnostic | HealthCheck | Erasure | Verification | Destruction`.
  A Job carries `activities: Vec<JobActivity>` in event order and the
  certificate serialises the same list. *Avoid: "job event" — that
  collides with JobUpdate.*
- **VerificationEvent** — post-erasure sampled-read check, appended by
  the runner as a sibling activity naming the ErasureEvent it ran
  against. **Live.** (This settles one of the §12 open questions in
  favour of sibling-event over embedded-in-ErasureEvent; the inner
  `Verifying` state still exists and is what produces the report.)
- **DestructionEvent** — chain-of-custody record for physical
  destruction: method, operator, optional supervisor, manifest
  reference, optional photo references, notes. **Live** — appended by
  `escalate_to_destroy`; it is what permits a Job to reach `Destroyed`
  once erasure attempts are exhausted. `photo_refs` is a schema slot
  with no capture UX (v0.3 #12).
- **DiagnosticEvent / HealthCheckEvent** — pre-flight typed events.
  **Schema-only**: the types and `JobActivity` variants exist and
  serialise into the cert, but the runner never emits them. Whether
  they run always or only when a SanitizationProfile asks is still
  open (§12).
- **DestructionManifest** — an auditor-facing grouping of N
  `PendingCoSign` Jobs assembled for a single supervisor co-sign
  action, matching paper-shredder convention. Carries
  `assembled_by`, `job_ids`, `state`
  (`Pending | Signed | Rejected`), the co-signing `supervisor` and
  `signed_at`. Distinct from **Batch**: a Batch is an ad-hoc
  operator-UX selection, a Manifest is a persisted evidentiary
  record that a supervisor signs. Tier 1 ships local-sync co-sign at
  the lead station; async remote co-sign is a Tier-2 cloud feature on
  the same schema. *Avoid: "destruction batch", "shred list".*
- **JobSpec** — what was asked for at Job creation: target Asset (or
  Device + asset_tag in Simple mode), classification, intent, optional
  method override, verify config, operator, WorkOrder/ticket/site
  references.
- **JobUpdate** — low-level streamed record from a running event
  (StateChanged, Progress, CommandIssued, CommandResult, Warning,
  Failed). The thing fanned out over the WebSocket. Renamed from
  `JobEvent` to free "Event" for the higher-level activity records
  above; that collision was the *only* reason for the rename —
  JobUpdate is not a new concept.
- **JobRunner** — owns a `DeviceBackend` and runs Jobs to terminal
  disposition. Orchestrates Erasure, Verification and Destruction
  activities; will grow to orchestrate Diagnostic and HealthCheck when
  those stop being schema-only.

### Operator & work

- **Operator** — identified by id, display name, email. Email is
  required (NIST R2). Session-scoped today; YubiKey/PIV-backed in v0.2.
- **Batch** — a lighter, operator-UX concept: an ad-hoc selection of
  Devices/Assets the operator processes together with shared settings
  (think Active@ KillDisk's batch mode). **Not** a persisted
  engagement entity, **not** a substitute for `WorkOrder`. *Avoid:
  using "Batch" interchangeably with "WorkOrder" — they're scoped
  differently and the collision is the first thing to confuse a new
  contributor.*

### Enterprise data model *(deferred to v0.2 — Enterprise mode only)*

The product ships in two modes (see §10/§11): **Simple** (just Jobs,
Certs, freeform `asset_tag`/`ticket_ref`/`site_label`) and
**Enterprise** (adds the entities below). The Enterprise schema is a
*strict superset* of the Simple schema — there is no fork; upgrading
is additive.

- **Customer** — the end party whose data was on the drive. Master
  record lives in the ITAD's ERP/CRM. Wipestation holds the minimum
  reference (`customer_ref` opaque id + display name) needed to render
  certs and group work; it is **not** trying to be a CRM. *Avoid:
  Account, Client, Tenant — those are different concepts.*
- **Contract** — long-standing agreement covering **how this
  Customer's data-bearing devices are erased**. Carries policy
  defaults: required Category floor, preferred Methods, verify config,
  supervisor co-sign rules. Deliberately narrower than the full ITAD
  contract (which covers transport, environmental, R2 reporting,
  billing); Wipestation only models the erasure-relevant slice.
  *Avoid: SLA, MSA, Agreement — those describe the broader contract.*
- **WorkOrder** — the shared identifier that flows through the
  ITAD's logistics → intake → erasure pipeline. The ITAD's ERP is
  authoritative; Wipestation references it by the same id the
  logistics team uses. Stores the **erasure-relevant slice only**
  (customer ref, classification policy or Contract ref, ticket id,
  due date) — *not* logistics fields (driver, truck, route).
  *Avoid: PickupRequest, Engagement, Visit — those are upstream
  logistics concepts and not how the ERP keys this work.*
- **Asset** — one specific device-as-customer-property. Persists
  across multiple Jobs (Asset #A-4421 might be processed once a
  quarter for years). Carries asset tag, intake notes, condition,
  expected vs actual specs, history of Jobs. Distinct from `Device`,
  which is hardware metadata; an Asset *has* a Device snapshot at
  intake but the Device may be re-snapshotted on subsequent intakes.
  *Avoid: InventoryItem, Item, Asset Tag (the tag is a field on the
  Asset, not the Asset itself).*
- **SanitizationProfile** — pre-configured combination of
  classification, intent, verify config, allowed methods, supervisor
  co-sign rules. **Owned globally by the ITAD** (not per-Customer);
  selected by a Contract or per-WorkOrder. Decoupling from Customer
  is deliberate — profiles are reusable shorthand, not customer
  records.

### Relationships (Enterprise mode)

- A **Customer** has many **Contracts** (typically one active) and
  many **WorkOrders**.
- A **Contract** belongs to one **Customer** and references zero or
  one default **SanitizationProfile**.
- A **WorkOrder** belongs to one **Customer**, may reference one
  **Contract**, and contains one or more **Assets**.
- An **Asset** belongs to one **Customer**, was received in one
  **WorkOrder**, and has zero or more **Jobs** over its lifetime.
- A **Job** processes one **Asset** (Enterprise) or one **Device**
  +freeform `asset_tag` (Simple) and produces one **Certificate**.

### Fleet

- **Station** — any peer on the `_wipestation._tcp.local.` mDNS bus.
  Most Stations are full wipestation instances (binary + UI + API)
  with `StationRole::Lead` or `Member`; a few are non-erasing
  participants — operator consoles (tablets) or multi-site Hubs —
  that join the bus to browse and coordinate. All Stations have a
  `StationId`, hostname, role, API port, `started_at`.
- **StationRole** — `Member` / `Lead` / `Console` / `Hub`. Only
  `Lead` and `Member` are wipestation instances; `Console` and `Hub`
  are non-erasing participants on the same bus.
- **Lead** — the station within a LAN that holds canonical config,
  audit aggregation, license state. Elected deterministically by
  `min(started_at, id)`.
- **Hub** *(deferred)* — cross-site or cross-LAN coordinator. Same
  protocol as the lead, multi-tenant aware.
- **Cloud** *(deferred)* — multi-tenant SaaS hub run by us; tier 2
  premium features attach here.

## 6. Technical architecture

### Stack (decided)

- **Engine, server, cert, fleet:** **Rust**.
- **Desktop shell:** **Tauri 2** (OS-native WebView).
- **HTTP server:** **Axum** (REST + WebSocket).
- **Frontend:** **Vite + React + TanStack Router + Tailwind**.
- **mDNS:** `mdns-sd` (pure-Rust).
- **Signing:** Ed25519 via `ed25519-dalek`.
- **Local DB (when persistence lands):** SQLite via `rusqlite`.
- **Hub (`wipe-hub`, vendor side):** Axum + **Postgres via `sqlx`**
  (runtime queries, no compile-time macros); Postgres and Mailpit for local
  development from the root `compose.yaml` (ADR-0007).
- **Mock backend:** `wipe-engine-mock` until Linux ioctl backend is ready.

### Single binary, one origin

The Axum server serves **both** `/api/*` and the React UI at `/`. The
browser, the operator tablet on the LAN, and the Tauri window all hit
the same URL. No "API port" vs "UI port" split. SPA fallback to
`index.html` for client-side routing.

### Crate seam: `DeviceBackend`

`wipe-engine::DeviceBackend` is the single trait between the
orchestrator and the hardware. Implementations:

- `wipe-engine-mock` (today) — simulated fleet of NVMe x2, SATA SSD,
  HDD with deterministic timing and failure injection.
- `wipe-engine-linux` (deferred) — direct `NVME_IOCTL_ADMIN_CMD`,
  `SG_IO` passthrough, `/dev/sdX` `O_DIRECT` reads. **No shelling out
  to `nvme-cli` / `hdparm`** — the audit story and evidence quality
  are materially worse if we orchestrate CLIs.

### Three frontends, one engine

- **Tauri window** — native shell pointed at the in-process Axum API,
  initially hidden until `/api/health` responds.
- **Axum HTTP + WebSocket** — same Axum, also serves the React UI;
  used by tablets and automation.
- **Ratatui TUI** *(deferred)* — for the PXE/bootable scenario where
  no display server exists.

### Why Rust + Tauri, not Bun + Hono

Decided early. The reasoning is captured in
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). The short version:

- The engine wants Rust anyway (ioctl, raw I/O, audit story).
- Tauri *is* a Rust app; adding Bun back means two processes for a
  single binary.
- The non-engine work (HTTP, SQLite, cert gen, signing) has perfectly
  good Rust crates and adding TypeScript here would only grow audit
  surface.
- TypeScript lives where it earns its keep: the React frontend.

### Why mDNS + simple lead election, not Raft/SWIM

For LANs of ≤50 stations (the 90% case), `min((started_at, id))`
election is deterministic, correct, and needs no coordination round.
Above ~50 nodes the customer almost certainly has a Hub or Cloud
already, so we'd never run flat-mesh-at-scale. The `FleetService` API
is the seam; we can swap the transport later without changing
consumers.

## 7. Operating modes

| Mode | Where it runs | Who uses it | Status |
| --- | --- | --- | --- |
| **Standalone with monitor** | Workstation with attached display | Floor operator at the station | v0.1 ✅ |
| **Headless wipestation** | PXE-booted rack or kiosk; no display | Roaming operator on tablet | v0.1 ✅ (no PXE image yet) |
| **Roaming operator tablet** | Tablet/laptop on LAN | Operator managing many stations | v0.1 ✅ (same UI bundle) |
| **Bootable ISO** | Self-sanitization of the host's OS drive | ITAD field tech | Deferred — UEFI Secure Boot signing chain required |
| **Hub on-prem** | Customer-controlled server, multi-LAN | IT supervisor | Deferred |
| **Cloud** | Multi-tenant SaaS | Tier 2 customers | Deferred |

### PXE-ephemeral discipline

> **The guarantee is about evidence, not configuration** (ADR-0003).
> **Evidence** — certs, activity chains, signatures, command evidence —
> is never written to local storage. **Configuration** — bay topology,
> station label — describes the bench rather than anyone's data, and
> persists where the station has somewhere to put it. The audit answer
> below is unchanged: a station pulled off the rack yields no evidence
> and no customer data.

For PXE-booted stations:

1. Read-only OS image; no writable storage that survives reboot.
2. All *evidence* in RAM; certs streamed to lead/hub/cloud immediately
   on signing.
3. License token fetched from lead/hub at boot, RAM-resident, expires
   if check-in stops.
4. Signing keys never on the station. Signing happens at the lead
   (YubiKey/PIV at supervisor desk) or in the cloud (KMS).
5. Audit answer to *"what's on this station if pulled off the rack?"*
   is *"nothing that matters"* — no certificates, no asset records, no
   customer identifiers. Possibly a bay map describing our own bench.
6. Station configuration resolves through the tiered store in ADR-0003:
   local file where writable, else a control plane keyed by station id,
   else the operator is asked, else per-session ephemeral with the loss
   stated plainly in the UI. A PXE station typically lands on tier 2 or
   4, and stays fully functional either way.

## 8. Operator UX model

### Session-scoped operator identity (v0.1, shipped)

Operator signs in once per session via a blocking modal on first launch.
Identity lives in `localStorage` (`wipestation.operator.v1`), shown as
a chip in the app header with *Switch operator…* / *Sign out*. Every
job uses the session operator; the wizard never asks for name/email.

In v0.2 the seam in `OperatorProvider` swaps `localStorage` for an
auth call; the chip swaps for a YubiKey/PIV/OIDC prompt.

### Classification is **not** an operator decision (v0.2 work)

> **Identified gap, not yet implemented.** The current wizard shows a
> FIPS 199 Low/Moderate/High picker. A factory-floor operator cannot
> know the FIPS 199 impact level of unknown-customer data. In real
> ITAD workflows the classification is set upstream and **inherited
> through the WorkOrder/Contract chain**:

- **Contract default** — the Customer's active Contract supplies
  the default classification (and the preferred SanitizationProfile).
  Per-Customer policy lives in the Contract, not in a separate
  "Customer profile" — Customer is a reference; Contract is where
  policy lives.
- **WorkOrder override** — the supervisor or the ITAD's ERP sets
  classification per WorkOrder when it differs from the Contract
  default (e.g. an unusually sensitive batch from an otherwise
  Moderate-default customer). Operator inherits by scanning the
  WorkOrder.
- **Asset-tag lookup** — scanning the Asset barcode calls the ITAD's
  ITAM (via the integration callback) to fetch classification
  recorded against that Asset, overriding the WorkOrder default if
  set there.
- **Default policy** — anything unclassified at all falls to a
  configurable fallback, in practice **High** (over-sanitization is
  cheap; the alternative is a breached contract). Configured per
  Wipestation install.
- **Destruction order** — Intent=Destroy sets a minimum rigor floor
  on the chosen Method regardless of Classification.

The operator's job is *identification, not classification*: scan a
barcode, confirm the device matches the WorkOrder, load it. The
inherited policy decides the method. The cert still records the
FIPS 199 level (R2 requirement) — the operator just isn't the one
picking it.

In Simple mode (no WorkOrder/Contract), the classification falls
through to the Default policy unless the operator overrides — the
picker stays, but Default policy minimises how often it must be used.

This is the largest known UX gap in v0.1. Replacing the picker is a
v0.2 priority (see §11).

### Wizard flow (current)

1. Operator clicks a device card on the Devices page.
2. A modal shows the device, the active operator (chip), and a small
   form: classification (to be removed), intent, asset tag, ticket,
   verification sample count.
3. Detected capabilities are surfaced inline (NVMe Sanitize actions,
   ATA Security, SED, frozen).
4. *Begin sanitization* creates a `JobSpec`, starts the job, navigates
   to the Job Detail page where the WebSocket stream drives live
   progress, an event log, and (on completion) a *View certificate*
   button.

## 9. Pricing & packaging

### Tier 1 — Station (annual unlimited, local)

- Annual subscription per licensed wipestation.
- **Unlimited** erasures; per-success accounting on the honor system
  (the audit log is the truth-of-volume artifact).
- LAN discovery + lead election + air-gap clean.
- Local signing via YubiKey/PIV at the operator/supervisor desk
  (v0.2) or a locally-managed key seed (v0.1).
- Self-hosted Hub optional for multi-site (included over N stations
  or sold as separate SKU).
- **Target buyer:** gov / defense / classified, ITADs with air-gap
  mandates, paranoid enterprises.

Pricing posture: **fair, mid-range vs Blancco per-event TCO.** Do not
gouge — Tier 1 is the high-volume long-tail.

### Tier 2 — Cloud (per station + cloud features)

Everything in Tier 1 plus features only the cloud can credibly deliver:

- **Customer-facing cert verification portal** *(headline feature)* —
  the ITAD's customer gets a branded link that proves destruction
  without contacting either party.
- **Multi-site fleet visibility** — single pane across LANs.
- **Immutable cert archive** with 7/10/perpetual retention SLAs.
- **Cert verification API** — public, third-party callable.
- **ITAM / ServiceNow / Jira integration** — webhooks + outbound API.
- **SOC 2 + GDPR Data Processing Agreement** for the archive.
- **Cloud KMS signing** alternative to YubiKey/PIV.
- **Analytics** — drive-model success rates, throughput benchmarks,
  anonymized industry comparison.
- **SAML/OIDC SSO** for operator auth.
- **Optional per-drive consumption pricing** for customers who hate
  fixed subscriptions.

Strategic incentive: **free cloud tier up to N stations / N certs/yr**
drives small-shop adoption; the customer-facing portal is the
upsell that ITADs happily pay for because they pass cost through.

Single binary, identical code; tier is a `--hub-url` configuration.

### How the tiers are enforced

The mechanism is ADR-0005's attestation chain, not a licence server: a
station verifies its own entitlements offline and marks certificates
`evaluation` when it cannot. The three purchasable scopes
(free/evaluation, per-machine, site-wide), what each one actually
enforces, and where offline enforcement is honestly impossible are
specified in [ROADMAP](docs/ROADMAP.md) §4.

One constraint overrides every commercial consideration here: **an
unlicensed, expired, revoked or clock-rolled-back station still erases
drives.** What changes is the certificate marking, never the ability to
sanitize. A licence server having a bad day must not be able to stop a
drive being wiped.

## 10. What has shipped

### 10.1 v0.1 baseline

Historical record of the v0.1 vertical slice, kept as-shipped. Where
v0.2 has since moved something, §10.2 says so — do not edit this table
to match current code.

| Surface | State |
| --- | --- |
| Rust workspace, 7 library crates + 1 binary + Tauri app | ✅ |
| `DeviceBackend` trait + mock backend (4 simulated drives) | ✅ |
| Job state machine + event broadcast | ✅ |
| JSON-LD certificate schema + canonical serialization + Ed25519 sign/verify | ✅ |
| mDNS service advertise + browse + deterministic lead election | ✅ |
| Axum REST + WebSocket + auto-sign-on-complete | ✅ |
| Static file serving + SPA fallback so the browser path works | ✅ |
| `wipestation` CLI (`serve` / `inspect` / `verify-cert`) | ✅ |
| Tauri 2 shell with in-process API, hidden-until-ready window, mDNS-tolerant | ✅ |
| React UI: Devices, Jobs, Job Detail (live progress), Certificate viewer, Fleet | ✅ |
| Session-scoped operator identity via `OperatorProvider` + localStorage | ✅ |
| Two-station E2E demo (mDNS + erase + sign + offline verify + negative tests) | ✅ |
| 25 tests across 6 crates passing | ✅ |
| README + ARCHITECTURE | ✅ |

### 10.2 v0.2 shipped so far

All of this is on `main` (feature branches merged 2026-11-05, c9a9214). The
**As-built gaps** column is from the 2026-11-05 audit
(`.scratch/audit-2026-11-05/`); each gap is tracked in a `.scratch/` spec
folder or in `.scratch/DECISIONS.md`.

| Surface | Ref | State | As-built gaps |
| --- | --- | --- | --- |
| Outer-Job composition — `Job` → `ErasureEvent`, new outer `Job` + `JobState`, `JobActivity` sum type, `JobEvent` → `JobUpdate` | §11 #2 / ADR-0001 | ✅ partial | One ErasureEvent per Job (no retries/fallback); `Quarantined` never entered; `station_id` never recorded |
| Destroy path — `DestructionEvent`, `PendingCoSign`, `DestructionManifest`, supervisor co-sign | ADR-0001 | ✅ partial | Co-sign uses the **station** key, and signer/role sit outside the signed bytes — it is a second signature, not an independent one; `verify-cert` ignores co-signatures; `manifest_id` never stamped. A never-erased Job can no longer be finalised uncertified (c779361), but how to certify it is open |
| Certificate schema v2 — `activities` chain and explicit `AssetDisposition` | ADR-0001 | ✅ partial | v1 certs do **not** verify under v2 (see the ADR-0001 addendum) |
| REST: escalate-to-destroy, manifests, co-sign | ADR-0001 | ✅ | |
| React UI: activity timeline on Job Detail + Manifests page | ADR-0001 | ✅ | |
| At-a-glance bench status overlay on the Devices page | §11 #3 | ✅ | |
| Configurable bay topology + generated SVG bay map | §11 #10 / ADR-0002 | ✅ | UI templates differ from the Rust presets |
| Tiered, self-detecting station config store | ADR-0003 | ✅ partial | Control-plane tier always fails (no hub); corrupt config only logged, not surfaced |
| Bench-setup builder + learn-by-hot-swap identify mode | ADR-0002/0003 | ✅ CLI only | The desktop (Tauri) shell wires no store or simulator: layouts are lost on quit and identify mode is dead |
| Model-aware enclosure catalog + shell artwork with machine-enforced legibility rules | ADR-0004 | ✅ partial | Site-local overlay exists in code but no host wires it |
| Licensing by attestation — vendor root → licence → instance key → cert; entitlements; offline lease; `evaluation` marking inside the signed payload | ADR-0005 | ✅ partial | Lease/watermark not persisted (resets on restart); no `FileCounter`; `allowed_methods` unenforced; free-tier count not tracked; no API route or UI; desktop shell is always unlicensed |
| Vitest suite over the frontend's pure domain modules | §12 | ✅ | |
| `DiagnosticEvent` / `HealthCheckEvent` | ADR-0001 | ⛔ schema-only; runner never emits | |
| Marketing-site scaffold `sites/www` (static Astro + Tailwind) + capability list with `available`/`in-development`/`planned` badges + build-time honest-claims/meta/third-party check; landing page (problem, offline-verifiable certificate, differentiators, "what we are not", one early-access CTA); product, standards posture (certification roadmap labelled future), segment (ITAD, enterprise IT, government), pricing ("early-access pricing on request"), contact, privacy and terms pages; early-access and contact forms | ADR-0006 / ROADMAP W1.1–W1.4 | ✅ partial | The forms stay closed (no script, submit disabled, "not open yet") unless the build is given `PUBLIC_HUB_ORIGIN`, and `astro build` refuses that until the privacy notice is published (`site.privacyNoticePublished`), so production builds cannot take sales leads yet; pricing has no amounts unless the build is given `PRICING_HUB_ORIGIN` (`offerings-and-payments` story 5, below); privacy and terms are placeholders to publish before launch; brand and origin are placeholders; no CI runs the build (ROADMAP §0.5); copy and statuses await vendor sign-off |
| Vendor hub foundation — `crates/wipe-hub` (`wipe-hub serve`), flag/`WIPE_HUB_*` env config validated at start-up (`fake`\|`stripe` payment provider; `stripe` refuses to start without a secret key and a webhook secret; optional 32-byte vendor root seed file, warned as file-held on every start), lazy Postgres pool, embedded sqlx migrations applied before listening (start-up exits within ~5 s if Postgres is unreachable), append-only `audit_log` table (ADR-0007 §6; written only through `audit::record`, which takes any executor so an action and its audit row share a transaction; UPDATE/DELETE/TRUNCATE rejected by triggers), `GET /api/health` probing Postgres and SMTP (≤2 s each; 503 `degraded` when either is down), root `compose.yaml` with Postgres and Mailpit, integration-test harness (`tests/support`: per-test `TestDb` databases dropped with their guard, `Mailpit` API client), portal serving (`apps/portal/dist` under the working directory or `--portal-dir`/`WIPE_HUB_PORTAL_DIR`; SPA fallback to `index.html`, `/assets/*` with no fallback, unknown `/api/*` paths JSON 404s; an explicit `--portal-dir` without `index.html` refuses to start, an unbuilt default is warned and non-API paths 404 naming the build) | ADR-0007 / ADR-0006 §1 / ROADMAP A1.1, A1.4 | ✅ partial | Only auth events call `audit::record` so far (row below); append-only does not stop the table owner (the hub's own role) or a superuser disabling the triggers (a separate unprivileged runtime role is a deployment decision, ROADMAP §0.5); payments are the orders and Stripe rows below, but there is no licence issuance; vendor root custody is a file (production must not launch on it, ROADMAP A1.5); `cargo test --workspace` now needs `docker compose up -d` |
| Hub accounts: sign-up, email verification and password sign-in — migration `0003_accounts` (`organizations`; `users` with role `owner`\|`member`\|`vendor_admin`, unique on `lower(email)`; `auth_tokens` and `sessions` storing only the SHA-256 of 32-byte OS-RNG tokens), Argon2id password hashes (12–1024 chars, hashed off the async runtime), `/api/auth/{sign-up,verify-email,sign-in,sign-out,me,verification-email}` (sign-up creates an organization and its `owner` and signs them in; verification links `{public_base_url}/verify-email?token=…` are single-use and expire in 24 h; one generic 401 for a wrong password or unknown email, which still runs one Argon2 verification), server-side session per sign-up/sign-in in the `wipehub_session` cookie (`HttpOnly; SameSite=Lax; Path=/`, 14 days; `Secure` policy in the story-2 row below), plain-SMTP mailer with `--mail-from`/`WIPE_HUB_MAIL_FROM` sent after commit (SMTP down at sign-up: account still created, `verification_email_sent:false`, resend route recovers), every auth event audited in its transaction (`auth.sign_up`, `auth.email_verified`, `auth.sign_in`, `auth.sign_in_failed`, `auth.sign_out`), customer reads scoped by organization id in `accounts`; portal `/sign-up`, `/sign-in`, `/verify-email` pages and an account panel on home (verify notice with resend, sign-out) | ADR-0007 §3 / SPEC portal-accounts CAP-1, CAP-2 | ✅ partial | Nothing is purchase-gated yet (no purchase route; `email_verified` in `/api/auth/me` is the field to gate on); plain SMTP only, no STARTTLS/auth for a production relay (ROADMAP §0.5); expired tokens are purged per user when that user is next issued one (a user's expired sessions are purged when they next sign in); sign-up's 409 for a taken email reveals whether that email is registered |
| Hub sessions hardened: CSRF, sign-in throttles, rotation, sign out everywhere — double-submit CSRF token (`GET /api/auth/csrf` returns `{csrf_token}` with `Cache-Control: no-store`, reusing a well-formed `wipehub_csrf` cookie or minting 32 OS-RNG bytes; every non-GET/HEAD/OPTIONS `/api/*` request, unknown paths included, needs an `X-CSRF-Token` header equal to the cookie, compared in constant time, else 403 `missing or invalid CSRF token` before any handler or audit write; middleware `csrf::require`, no exemptions); migration `0004_auth_throttle` and `throttle::{charge, confirm_failure, refund, clear, purge_stale}` (reusable by story 3): every sign-in attempt, known or unknown email, is charged against `email:<address>` (trimmed, Postgres `lower()`, ≤254 chars) and `ip:<peer>` (IPv4 full address; IPv6 its /64) in one atomic statement per key before any lookup or Argon2 work, in a fixed 15-min window from the first charge; an attempt is refused while the key is locked or once charges in flight plus failures reach 5 per email or 20 per IP, so at most 5 (20) password checks run per window however many arrive at once; a refusal answers 429 `too many failed sign-in attempts; try again later` with `Retry-After` (time left on the lock, or 1 while only in-flight checks fill the window), even for the right password, and never extends a lock; a failure keeps its charge and the one that reaches the limit locks the key for 15 min; a success deletes the email key and refunds its IP charge, so the right password on the 5th try signs in unlocked; audit volume is bounded: `auth.sign_in_locked` once per lock (`{until}`), refusals counted in the row's `refused` and audited once as `auth.sign_in_lock_summary` (`{until, refused}`) when the row is cleared, restarted or purged, every sign-in audit row carrying `peer` except summaries of purged rows, which belong to other keys; the IP key is charged before the email key, so a locked IP never creates `email:` rows; the work after a charge runs in its own task, so a client disconnecting mid-check still has its outcome recorded; stale rows purged on failures with `SKIP LOCKED`; sign-in and sign-up delete the incoming session in the new session's transaction (`auth.sign_in` detail `{rotated}`); `POST /api/auth/sign-out-everywhere` ends every session of the signed-in user (`auth.sign_out_everywhere`, `{sessions: n}`); session and CSRF cookies are `Secure` unless `--public-base-url` is http on `localhost`, `*.localhost` or a loopback IP (http on any other host gets `Secure` and a start-up warning that browsers will drop the cookies); the portal client fetches and caches the token, sends it on every POST and refetches/retries once on a CSRF 403; the sign-in page turns a 429's `Retry-After` into "try again in N minutes"; "Sign out everywhere" on home | ADR-0007 §3 / SPEC portal-accounts CAP-2, CAP-3 | ✅ partial | The IP throttle keys on the TCP peer only (`X-Forwarded-For` is never trusted, ROADMAP §0.5), so behind a reverse proxy or a shared office NAT everyone shares one IP key and 20 failures among them lock all of them out (an IPv6 /64 is likewise one key); anyone who knows an email can lock that account out of password sign-in for 15 min at a time; because in-flight attempts count toward the limit, one failure while 19 other sign-ins from the same IP are mid-check can lock that IP; double-submit CSRF is open to cookie-tossing from a sibling subdomain until hosting is decided; no Origin/`Sec-Fetch-Site` checks; no idle timeout (sessions last 14 days unless signed out) |
| Hub magic links, password reset and the email-send throttle — migration `0005_auth_token_purposes` widens `auth_tokens.purpose` to `verify_email`\|`magic_link`\|`password_reset`; every emailed token is 32 OS-RNG bytes with only its SHA-256 stored, consumed by one atomic `UPDATE … WHERE used_at IS NULL AND expires_at > now()` (concurrent uses: exactly one wins), and issuing one deletes that user's expired tokens; `POST /api/auth/magic-link` and `POST /api/auth/password-reset` take `{email}` and answer 202 `{}` for known and unknown emails alike, SMTP down included: the handler only validates and charges the email-send throttle, and the account lookup, the token, the audit row (`auth.magic_link_requested`/`auth.password_reset_requested`) and the send of `{public_base_url}/magic-link?token=…` (15 min) or `/reset-password?token=…` (1 h) all run in a spawned task after the answer is decided, so neither the response nor its timing reveals whether the account exists (a failure there is logged at `warn`, never returned); an unknown email writes no token, mail or audit row; `POST /api/auth/magic-link/sign-in {token}` signs in like a password sign-in (rotating the incoming session), retires the user's other magic links, marks the email verified, clears that email's sign-in lock and is not subject to the sign-in throttle; if this sign-in is what verifies the email (a possibly pre-registered account, whose password was set by someone who never proved they own the mailbox), the same transaction also ends every other session of the user and replaces the password hash with the unusable marker `!unusable` until a reset (`auth.magic_link_sign_in` `{rotated, email_verified, sessions_ended, password_cleared}`); `POST /api/auth/password-reset/complete {token, password}` checks the password first (an invalid one never burns the token), peeks the token before Argon2, then in one transaction consumes it, sets the hash, retires outstanding reset and magic links, ends every session of the user and the browser's incoming session (whoever it belongs to), verifies the email and clears the sign-in lock, and answers `{"password_reset": true}` with the session cookie cleared, without signing in (`auth.password_reset` `{sessions, email_verified}`); a used or expired link is audited as `auth.magic_link_failed`/`auth.password_reset_failed` `{reason: "used"|"expired"}` once per token while the token is still stored (issuing a new link purges the user's expired ones), while an unknown or malformed token writes nothing, so these rows are bounded by the links issued (migration 0005 adds `auth_tokens.failure_audited_at` to mark them); every route that sends email (sign-up, resend verification, magic link, reset) is charged against `mail:to:<address>` (5 per hour) and `mail:ip:<peer>` (20 per hour) before any lookup, Argon2 or send, each charge final whether or not mail goes out, the one reaching a limit locking the key for 1 h (`auth.mail_locked` once; refusals summarised as `auth.mail_lock_summary`), a refusal being 429 `too many emails requested; try again later` with `Retry-After` and writing nothing but its throttle counters (stale throttle rows are purged only on a charged send); portal `/sign-in-link`, `/forgot-password` (same confirmation whether or not the account exists), `/magic-link` (signs in only on a button press, never on load, so a mail scanner cannot use the link) and `/reset-password`, linked from `/sign-in`; email-sending forms show "Too many emails requested. Try again in N minutes." | ADR-0007 §3 / SPEC portal-accounts CAP-2 | ✅ partial | Anyone who knows an address can lock its owner out of every route at once: 5 wrong passwords lock password sign-in for 15 min, and 5 link requests use up the address's emails for an hour, blocking magic link, reset, sign-up and resend (the limit counts unknown addresses too, so it reveals nothing; a sign-up that 409s still counts); when SMTP is down the two request routes still answer 202 and the user is never told the link did not go out; a user who signed up, never verified and then signs in by magic link loses their password and must reset it before password sign-in works again (the pre-registered-account mitigation); story 1's verification link still verifies a pre-registered account without ending its creator's session or password (deferred: on that path the clicker cannot be told apart from the person who just set the password); a send in flight is lost if the hub stops between answering and sending (the user asks again); no notice email when a password is reset or cleared; the per-IP mail limit has the same shared-NAT/proxy caveat as sign-in; expired tokens are only purged per user, with no global sweeper; the reset and magic-link token routes have no throttle of their own (a 256-bit token cannot be guessed); no MFA |
| Hub account and team management — `POST /api/account/profile {name}` (own name only; the email is read-only; `account.profile_updated` `{}`, the name kept out of the log); `POST /api/auth/password {current_password, new_password}`: the new password is validated first (nothing charged), then the attempt is charged against the sign-in throttle (`email:<the user's address>`, `ip:<peer>`) exactly like a sign-in, so a stolen session cannot guess the current password unthrottled; a wrong one is 400 `current password is incorrect` (`auth.password_change_failed`, and the 5th locks the email for password sign-in too), the right one sets the new hash, retires outstanding reset and magic links, ends every session of the user and starts a new one for this browser (`auth.password_changed` `{sessions_ended}`); `CurrentUser` carries the role, read in the session query, so a demotion or removal applies to the very next request; organization-scoped data layer `team` (every query takes the session's organization id first, so another organization's user or invitation id is a 404 that changes and audits nothing, and a `vendor_admin` is never a target); `GET /api/organization/members` (any role, ordered by name then email); owner-only routes answer a `member` or `vendor_admin` 403 `only an organization owner can manage the team` before reading the body, charging a throttle or writing: `POST /api/organization {name}` (`organization.renamed` `{from, to}`), `POST /api/organization/members/{id}/role {role}` (`owner`\|`member` only; same role is a no-op; `organization.member_role_changed`), `POST /api/organization/members/{id}/remove` (deletes the user, so sessions and tokens cascade and the address is free again; `organization.member_removed` `{organization_id, email, role, sessions_ended}`; removing yourself is 409); every owner write (rename, invite, revoke, role change, removal) locks the organization row (`SELECT … FOR UPDATE`) and re-checks that the actor is still an owner, so a just-demoted owner cannot act, and an organization always keeps an owner: owners are counted under that lock, so two owners demoting each other at once leave one (409 `an organization must keep at least one owner`); removing or demoting an owner revokes, in the same transaction, every pending invitation they sent, so a departing owner cannot rejoin through one; migration `0006_invitations` and `POST /api/organization/invitations {email, role}` (a verified owner only; charged against the email-send limit plus `mail:org:<organization id>`, 20 invitations per organization per hour, whose refusal gives back the request's IP and address charges; 409 for an address already in the organization, decided under the lock, the charge standing; revokes that address's other pending invitations, so at most one is pending even for concurrent invites, and purges expired ones; `organization.member_invited`; SMTP down still creates it with `invitation_email_sent:false`), `GET …/invitations` (pending only), `POST …/invitations/{id}/revoke`; every revocation is audited as `organization.invitation_revoked` `{invitation_id, email, reason}` with `reason` `revoked`, `replaced`, `sender_demoted` or `sender_removed`; the emailed `{public_base_url}/accept-invite?token=…` link is single-use and expires in 7 days; `POST /api/invitations/lookup {token}` previews it without consuming, `POST /api/invitations/accept {token, name, password}` validates first, peeks before Argon2, then atomically consumes the invitation and creates the user in the inviting organization with its role and a verified email, rotates the incoming session and signs in (`organization.invitation_accepted`); names reject control characters; portal `/account` (name, password change), `/organization` (rename, member list with role select and Remove for owners, both confirmed before acting on yourself; invite form, replaced by the verify-and-resend notice for an unverified owner; pending invitations with Revoke) and `/accept-invite` (looks up on load, joins only on submit, then drops every query cached for the browser's previous user), linked from home | ADR-0007 §3 / SPEC portal-accounts CAP-4 | ✅ partial | No email-address change (needs a confirmation link to the new address and a notice to the old one); one organization per person, so an invitation to an address that already has an account anywhere fails only at acceptance (409, told to the mailbox owner, never to the inviter); removal deletes the user row (the audit log keeps id, email and role; revisit when orders reference users); organization and inviter names appear in invitation emails sent to third parties; the inviter learns only `invitation_email_sent` (handed to SMTP), not delivery; no ownership-transfer flow beyond role changes, no leaving or deleting an organization, no organization fields beyond its name; no notice email when a password is changed or a member removed; invitee and removed-member email addresses, and organization names (`organization.renamed` `{from, to}`), stay in the append-only audit log for good (the log is the record of who was invited and removed; the user's display name is kept out), pending a data-retention decision; a locked email (5 wrong passwords by anyone who knows the address) also blocks the signed-in user's password change for 15 min, since both share the `email:` throttle key; an organization that genuinely invites more than 20 people in an hour waits for the window to pass |
| Hub vendor admin area and tenant isolation — migration `0007_vendor_organization` adds `organizations.is_vendor` with a partial unique index (`organizations_one_vendor`), so at most one organization is the **vendor organization** (`Vendor staff`), which sign-up and invitations never create or join; `wipe-hub admin create --email <address> [--name <name>]` (with `serve`'s `--database-url`, `--smtp-*`, `--mail-from`, `--public-base-url` and their `WIPE_HUB_*` twins) validates first, applies migrations, then in one transaction gets or creates the vendor organization (race-free: `INSERT … ON CONFLICT ((true)) WHERE is_vendor DO NOTHING`), inserts a `vendor_admin` with an unusable password and an unverified email, issues a 1-hour password-reset token and audits `admin.created` (actor `cli:admin-create`, `{email, organization_id, vendor_organization_created, os_user}`), then emails the existing reset link; it prints `created vendor admin <email> (<id>)`, exits 0 even when SMTP is down (warning that the admin should use Forgot password) and refuses, writing nothing, an address that is already a vendor admin or belongs to a customer (it never converts a customer user); a session is a vendor admin only when the user's role is `vendor_admin`, their organization is the vendor organization and their email is verified, read live per request, else every `/api/admin/*` route is 403 `only vendor admins can use the admin area` (401 without a session) before the query is read; the admin data layer `admin` is the only code that reads across organizations, excludes the vendor organization and every `vendor_admin` from results and counts, and takes a `VendorAdmin` (built only by the extractor) plus a transaction in which it writes its own audit row before returning data; `GET /api/admin/organizations?q=` (name, or any user's email or name; `{organizations: [{id, name, created_at, user_count}], truncated}`; `admin.organizations_searched` `{query, organization_ids}`), `GET /api/admin/users?q=` (email or name, with organization; `admin.users_searched` `{query, user_ids}`), `GET /api/admin/organizations/{id}` (organization and users; `admin.organization_viewed` `{user_ids}`; unknown, non-UUID or vendor id 404 `no such organization` with no audit row); `q` is trimmed, case-insensitive, LIKE wildcards literal, empty matches everything, over 200 characters or any control character 400; `tests/isolation.rs` calls every customer and admin route as one organization's owner and member with another organization's ids and addresses and asserts no response carries the other's identifiers and its records are byte-for-byte unchanged, and a guard fails on any `/api` path literal in `src/` missing from its classification table; portal `/admin` (organization and user search forms, run only on submit, results as tables linking to the organization) and `/admin/organizations/$id`, linked from home for `vendor_admin` | ADR-0007 §3, §6 / SPEC portal-accounts CAP-5, CAP-6 / ROADMAP A1.3 (customers part) | ✅ partial | Views only for customer accounts (sales leads can be worked: see the row below): no admin actions on customer accounts (sending a reset, resending verification, changing roles, disabling); no CLI to list or remove admins (deleting the user row by SQL ends their sessions by cascade); admins have the same 14-day session lifetime as customers and no MFA; search queries and the ids they returned stay in the append-only audit log for good; results are capped at 100 with no paging; substring search scans the tables (no trigram index); the set-password email is the password-reset email and reads as a reset; offerings, orders and licences are not in the admin area yet |
| Sales leads from the marketing site — migration `0008_leads`: `leads` (one per `lower(email)`, unique index; `segment` `itad`\|`enterprise-it`\|`government`\|`other`; `status` `new`\|`contacted`\|`qualified`\|`closed`; `unsubscribed_at`) and `lead_submissions` (kind, message, the exact consent text and `consented_at`, the SHA-256 of a 32-byte OS-RNG unsubscribe token; cascade on lead delete); `POST /api/leads {kind, name, email, company, role?, segment?, message?, consent, consent_text, website?}` keeps the double-submit CSRF check (no exemption) and validates (400; contact needs a message and takes no role or segment; `consent` must be true and `consent_text` must equal the hub's text for the form, pinned to `sites/www/src/data/consent.ts`), then charges `lead-form:ip:<peer>` (10 an hour, final; the 10th locks the key for an hour, `lead.submit_locked`, subject the throttle key; a later restart audits `lead.submit_lock_summary` with `peer`; refusal 429 `too many submissions; try again later` with `Retry-After`), answers a filled honeypot `website` 202 `{}` doing nothing else (it was charged, so it can engage that lock), charges the email-send throttle (`mail:` keys, shared with sign-in links), then in one transaction upserts the lead (a known address updates email as typed, name and company, role and segment only when given, and never changes `status` or clears `unsubscribed_at`: an unsubscribe is a suppression that only a future confirmed opt-in may lift), inserts the submission and audits `lead.submitted` `{kind, created, suppressed}`; 202 `{}` either way; the confirmation email (per-form subject, no submitted text echoed, a `{public_base_url}/unsubscribe?token=…` link and a "not you?" line) is sent from a spawned task, failures logged at `warn`, and is never sent to an unsubscribed (`suppressed`) lead, whose submission is still stored and still charged to the email-send throttle so neither the answer nor a 429 reveals the unsubscribe. `POST /api/leads/unsubscribe {token}`: any of the lead's tokens sets `unsubscribed_at` once (`lead.unsubscribed` `{}` only on change), 200 `{"unsubscribed": true}` also when already unsubscribed, 404 `this unsubscribe link is not valid` otherwise; the portal's `/unsubscribe` page acts only on a button press. Audit rows about a sales lead (`lead.submitted`, `lead.unsubscribed`, subject `lead:<id>`) hold ids and flags only (no name, email, company, message or IP); the per-IP throttle rows use the `lead-form:` prefix, never `lead:`, and are never linked to a lead id. Credentialed CORS (`cors::site_cors`, outside the CSRF middleware) answers only `--site-origin` / `WIPE_HUB_SITE_ORIGIN` (default `http://localhost:4321`; anything but a bare http(s) origin refuses to start), only on `GET /api/auth/csrf` and `POST /api/leads` and their preflights. The site's forms post with `src/lib/lead-submit.ts` only in builds given `PUBLIC_HUB_ORIGIN` | lead-capture story 1 (CAP-1, CAP-2, CAP-4 storage and unsubscribe) / ADR-0007 §3 | ✅ partial | The admin list, notes, export and erasure are in the row below (story 2); the site and hub must be the same *site* (registrable domain) because the CSRF cookie is `SameSite=Lax`: `localhost:4321` with `localhost:7900` works, `localhost` with `127.0.0.1` does not, and production hosting must keep them same-site; no RFC 8058 one-click `List-Unsubscribe` header (it would need a CSRF exemption); no double opt-in, so anyone can put any address on the list and its owner gets a confirmation email (at most 5 an hour per address, shared with sign-in links) until they unsubscribe; re-subscribing after an unsubscribe needs a confirmed opt-in (an emailed confirm link), which is not built, so an unsubscribed sales lead stays suppressed whatever they submit; consent rows do not record which privacy-notice version was agreed to (add it before the notice is published); the email-send throttle's own audit rows (`auth.mail_locked`, `auth.mail_lock_summary`) name the address and peer in their subject, and the per-IP throttle lock rows keep the peer IP (`lead.submit_locked` in its subject `lead-form:ip:<peer>`, `lead.submit_lock_summary` in subject and `peer`), so erasing a sales lead (story 2) cannot remove those audit rows (it deletes the address's `mail:to:` throttle row, so none is written after it, but rows written before stay in the append-only log); the form script has been exercised in tests and against a running hub with a scripted client, not in a real browser; the privacy placeholder says the site "sets no cookies", but an open form makes the hub set the `wipehub_csrf` cookie and sends the visitor's details to the hub's origin, so the published notice must disclose both before `privacyNoticePublished` is set; only one `--site-origin` can be configured, so an apex domain and its `www.` host cannot both host the forms |
| Sales-lead admin: list, notes, CSV export and erasure — migration `0009_lead_notes` (`lead_notes`: append-only, `author_id` → users `ON DELETE SET NULL`, cascade on lead delete, non-blank body); the data layer `lead_admin` (every function takes a `VendorAdmin` and a transaction and writes its own audit row; each mutation first locks the lead `FOR UPDATE`, so one racing an erasure is a 404, never a 500) behind six vendor-admin routes whose `VendorAdmin` extractor runs before any path, query or body: `GET /api/admin/leads?segment=&status=&from=&to=` (`segment` one of the four segments or `none` for contact-only leads, `status` `new`\|`contacted`\|`qualified`\|`closed`, `from`/`to` inclusive UTC days `YYYY-MM-DD` matching a lead with **any** submission in the range; empty means any; a bad value or an unknown key (a misspelt `Segment` would otherwise list everything) 400 with no audit row; latest submission first, then id; at most 100 plus `truncated`; `admin.leads_listed`, subject `search:leads`, `{filter, lead_ids}`), `GET /api/admin/leads/{id}` (`{lead, submissions, notes}`, submissions with their exact consent text and notes with their author, newest first; `admin.lead_viewed` `{}`), `POST /api/admin/leads/{id}/status {status}` (`{lead}`; `admin.lead_status_changed {from, to}` and an `updated_at` bump only on a change; setting the current status again still returns the lead, so it is audited as `admin.lead_viewed`: every answer carrying lead data is audited), `POST /api/admin/leads/{id}/notes {body}` (201 `{note}`, trimmed, same rules as a lead message; `admin.lead_note_added {note_id}`), `POST /api/admin/leads/{id}/erase` (deletes the lead, its submissions and notes, and the address's `mail:to:` email-send throttle row, in one transaction; `admin.lead_erased {submissions, notes}`; no suppression record is kept, so the same address submitting again is a new lead), `POST /api/admin/leads/export {segment?, status?, from?, to?}` (a CSRF-protected POST so no other site can start a download; the list's filters and order with no row cap; a body that is not a JSON object of optional strings with known keys is the same 400, never an unfiltered export; a UTF-8 byte-order mark so Excel reads non-ASCII names, then RFC 4180 `text/csv; charset=utf-8`, `Content-Disposition: attachment; filename="sales-leads-<UTC date>.csv"`, `Cache-Control: no-store`; cells starting `=`, `+`, `-`, `@`, TAB or CR prefixed with `'` against spreadsheet formula injection; the lead row only, never messages, consent text or notes; `admin.leads_exported` `{filter, lead_ids}` in the read's transaction); an unknown, non-UUID or erased lead id is 404 `no such sales lead` with no audit row. These audit rows hold ids, filter values, counts and status names only, never personal data, which is why there is no free-text search. Portal `/admin/leads` (segment, status and date filters run only on Show leads; Export CSV posts the last *submitted* filters and saves the file; unsubscribed leads marked with their date) and `/admin/leads/$id` (submissions with consent text, status, notes, erase behind a second confirming press, then back to the list; changes update the page from their answer, never by refetching, since a refetch is another audited view), linked from `/admin` | lead-capture story 2 (CAP-3, CAP-4 erasure) / ADR-0007 §3, §6 | ✅ partial | No guidance on whether sales may answer, by hand, a direct message from an unsubscribed sales lead (customer-visible policy, waiting on the owner); no editing or deleting single notes or submissions, no bulk status change or bulk erase, no paging past 100 in the list (the export is uncapped), no free-text search by design; the filter values and returned lead ids of every list and export stay in the append-only audit log for good (ids only, not personal data); an erasure cannot reach a CSV already downloaded, the email-send throttle's audit rows written before it that name the address (`auth.mail_locked`, `auth.mail_lock_summary`; the throttle row itself is deleted, so no later purge audits the address), or backups; that `mail:to:` row is shared with any portal account at the same address (sign-in links, password resets, invitations), so erasing the lead also resets that account's email-send count and lifts any active email-send lock on it; the portal pages were checked by typecheck, unit tests of the API client and a production build, not in a real browser |
| Offerings admin — migration `0010_offerings` (`offerings`: `price_minor bigint` 1–99 999 999, `currency` `^[A-Z]{3}$`, `billing_interval` `one_off`\|`annual`, `status` `draft`\|`published`\|`retired`, `scope` `machine`\|`site`, `seats` 1–10 000, `quota_erasures` null (unlimited) or 1–10⁹, `features text[]`, `term_months` 1–120 and exactly 12 when annual, `created_by` → users `ON DELETE SET NULL`, `published_at`/`retired_at` tied to status by CHECKs; a partial unique index on `lower(name)` among offerings not retired; a trigger refusing deletes, any lifecycle step but draft→published→retired, term changes once published and any change once retired); the data layer `offering_admin` (each function takes a `VendorAdmin` and a transaction, writes its own audit row, and locks the row `FOR UPDATE` before a change) behind six vendor-admin routes: `GET /api/admin/offerings` (every status: drafts, published, retired, then lower-cased name, id; `admin.offerings_listed`, subject `search:offerings`, `{offering_ids}`), `POST /api/admin/offerings` (201 `{offering}`, a draft; currency upper-cased, features validated against `wipe_license::Feature` and stored sorted, de-duplicated; `admin.offering_created {offering}`), `GET /api/admin/offerings/{id}` (`admin.offering_viewed {}`), `POST /api/admin/offerings/{id}` (a full body; `admin.offering_updated {changes: {field: {from, to}}}` with `grants` as one field; a body changing nothing is audited as `admin.offering_viewed`), `POST /api/admin/offerings/{id}/publish` (`admin.offering_published {}`), `POST /api/admin/offerings/{id}/retire` (`admin.offering_retired {}`). An invalid body (unknown keys included; a missing `quota_erasures` is refused, never read as unlimited) is 400 naming the field; a name in use by an offering not retired (case-insensitive) is 409 `an offering with this name is already in draft or for sale`; changing a published offering's price, currency, interval or grants is 409 (`… retire it and create a new one`), any edit of a retired one 409, publishing a non-draft or retiring a non-published one 409; an unknown or non-UUID id 404 `no such offering`; none of these writes anything. Portal `/admin/offerings` (the list with status, price and grants; a new-draft form) and `/admin/offerings/$id` (edit with the terms disabled once published; publish and retire each behind a second confirming press; changes update the page from their answer, never by refetching), linked from `/admin`; prices are typed and shown by string arithmetic over Intl's minor-unit digits (`toMinorUnits`/`formatPrice`), never floats | offerings-and-payments story 1 (CAP-1) / ADR-0007 §4, §6 | ✅ partial | Published offerings are read through the catalog, public since story 5 (below), and checkout; currency is checked for format only, not against the ISO 4217 list, and the portal falls back to 2 minor digits for a code Intl does not know; no deletion, un-retire or re-publish, by design; a name freed by retirement can be reused, so two offerings (one retired) may share a name; the feature vocabulary is mirrored by hand in `apps/portal/src/api/adminOfferings.ts`; the trigger's refusals surface as a 500 if the data layer's own checks are ever bypassed; the portal pages were checked by typecheck, unit tests of the API client and a production build, not in a real browser |
| Orders, the `PaymentProvider` seam and checkout — migration `0011_orders` (`orders`: `organization_id`, `offering_id`, `created_by` → users `ON DELETE SET NULL`; a snapshot of the offering's name, price, currency, interval and grants under offerings' CHECKs; `status` `pending`\|`paid`\|`expired`; `provider` `fake`\|`stripe` and `provider_session_id`, unique together; `paid_at`/`expired_at` tied to status by CHECKs; index `(organization_id, created_at DESC)`; a trigger refusing deletes, any step but pending→paid or pending→expired, any change once terminal, and any change to the snapshot, organization, offering, provider, session or `created_at`); `payments::PaymentProvider` (`create_checkout`, `billing_portal`, `verify_event`) chosen by `--payment-provider`: `Fake` (session ids `fake_cs_<uuid>`, checkout `{public_base_url}/checkout/fake?session=…`, billing `{public_base_url}/billing/fake`, events `{id, type, session_id}` JSON signed HMAC-SHA256 under the public `FAKE_WEBHOOK_SECRET` in a `fake-signature` hex header, verified in constant time; refuses checkout, billing and verification with 503 `the fake payment provider works only on a local hub` when `--public-base-url` is not localhost/loopback, and warns at every start) and `Stripe` (story 3, row below); `VerifiedEvent` has private fields and only `verify_event` makes one, and `orders::apply_event` (re-exported as `payments::apply_event`; locks the order by provider and session `FOR UPDATE`, changes only a `pending` order, audits `order.paid`/`order.expired` `{event_id}` as `provider:<name>`) is the only `UPDATE orders SET status` (a test greps for it). Customer routes: `GET /api/offerings` (public since story 5, no session; published only, by lower-cased name, id; `{offerings: [{id, name, description, price_minor, currency, interval, grants}]}`), `POST /api/checkout {offering_id}` (an owner with a verified email, else 403 `only an organization owner can buy` / `verify your email address before buying`, checked before the body; the offering read `FOR SHARE` so a concurrent retire waits, unknown/draft/retired 404 `this offering is not for sale`; the provider is called first, then the order and `order.created {offering_id, price_minor, currency, provider}` (actor `user:<id>`, subject `order:<id>`) commit together; 201 `{order, checkout_url}`), `GET /api/orders` and `GET /api/orders/{id}` (any role, own organization, newest first; another organization's or unknown id 404 `no such order`), `POST /api/billing/portal` (owner, else 403 `only an organization owner can manage billing`; `{url}`), and the Fake hosted-page stand-in (owner; 404 `no such checkout` under another provider or for another organization's session): `GET /api/payments/fake/sessions/{session}`, and `POST …/pay` / `…/abandon`, which sign a `checkout.completed` / `checkout.expired` event, run it through `verify_event` and (since story 3) `payment_events::receive` under the order's row lock (a non-pending order is 409 `this checkout has already ended`, so concurrent pays give one 200 and one 409). Portal `/buy` (Buy button for verified owners, following `checkout_url` only if http(s)), `/orders` (with Billing for owners), `/orders/$id` (re-read every 3 s while pending), `/checkout/fake` ("Test payment: no money moves", Pay and Abandon, both ending on the order) and `/billing/fake`, linked from home | offerings-and-payments story 2 (CAP-2, CAP-5 locally) / ADR-0007 §4, §6 | ✅ partial | Stripe, the webhook and the event store are the row below (story 3); fulfilment, entitlements, renewal and refunds are story 4's row (`refunded` orders since); the catalog was signed-in only until story 5; only owners buy or open billing (story 2 decision 1), every role sees orders; checkout holds the offering's share lock across the provider call; a Fake order whose buyer leaves without pressing Pay or Abandon stays `pending` for good (a Stripe order expires through Stripe's `checkout.session.expired`, row below); the portal pages were checked by typecheck, unit tests of the API client and a production build, not in a real browser |
| Stripe provider and verified, idempotent payment events — `--stripe-secret-key`/`WIPE_HUB_STRIPE_SECRET_KEY` (required with `--payment-provider stripe`, as is the webhook secret; both trimmed, env values hidden, redacted in `Config`'s `Debug`) and `--stripe-api-base`/`WIPE_HUB_STRIPE_API_BASE` (default `https://api.stripe.com`; only https, or http on a local host, else it refuses to start). `stripe::Stripe` calls the Stripe REST API with `Authorization: Bearer`, `Stripe-Version: 2024-06-20` and form bodies over one `reqwest` client (5 s connect, 15 s total): checkout `POST /v1/checkout/sessions` (`mode=payment`; since story 4 an annual offering is `mode=subscription`, row below; `client_reference_id` and `metadata[wipehub_order_id]` = the order id, `metadata[wipehub_organization_id]`, `metadata[wipehub_hub]` = this hub's public origin (`scheme://host[:port]`), `success_url`/`cancel_url` `{public_base_url}/orders/<id>`, one `price_data` line item from the snapshot, the organization's **Billing customer** as `customer`, or `customer_creation=always`, `Idempotency-Key: wipehub-checkout-<order id>`; no email or other personal data; when Stripe answers 400 `resource_missing` on `customer` to a request that sent the billing customer, it is sent once more with `customer_creation=always` and the key `wipehub-checkout-<order id>-new-customer`, so a customer deleted at Stripe never locks the organization out); billing `POST /v1/billing_portal/sessions` (`customer`, `return_url={public_base_url}/orders`), and 409 `billing opens after your first payment` while the organization has no billing customer or Stripe answers `resource_missing` on it. A non-2xx answer, timeout or reply without a usable id/http(s) url is 502 `the payment provider could not be reached; try again later`, logged without the key, with nothing written. `Stripe-Signature` is verified per Stripe's scheme (`t=` and any `v1=` hex HMAC-SHA256 over `"{t}.{body}"` keyed with the whole `whsec_…` secret, constant-time, `|now − t|` ≤ 300 s; anything else 400 `{error}`); `stripe::signature_header` signs for tests. Event mapping: `checkout.session.completed` with `payment_status=paid` or `checkout.session.async_payment_succeeded` → completed; `checkout.session.expired` or `checkout.session.async_payment_failed` → expired; anything else (including completed but unpaid) ignored; a `checkout.session.*` event without `data.object.id` is rejected. `VerifiedEvent` also carries the customer id, the subtotal and upper-cased currency, and the order reference (`metadata[wipehub_order_id]` only, and only when `metadata[wipehub_hub]` is this hub's origin; `client_reference_id` is sent but never read back, so neither another app's UUID there nor another hub's order on a shared Stripe account is mistaken for this hub's order). Migration `0012_payment_events`: `payment_events` (primary key `(provider, event_id)`, `event_type`, `session_id`, `payload_sha256`, `outcome` `applied`\|`unchanged`\|`ignored`\|`unmatched`\|`mismatch`, `received_at`; a trigger refuses DELETE and every UPDATE but setting `outcome` once from NULL) and `billing_customers` (`(organization_id, provider)` primary key, `(provider, customer_id)` unique, `created_at`/`updated_at`; never deleted, and the only update allowed replaces `customer_id` with `updated_at`). `payment_events::receive` is the only caller of `orders::apply_event` (a test greps for it): it inserts the event `ON CONFLICT DO NOTHING` (a conflict is a duplicate that writes nothing; concurrent duplicates serialise on the key), applies it and sets the outcome. `apply_event` now returns `Applied`/`Unchanged`/`Ignored`/`Unmatched`/`Mismatch`: a paid event whose subtotal, currency or order reference differs from the order leaves it `pending` and is logged at error; a paid transition stores the event's customer as the organization's billing customer (`billing_customers`): inserted, or replacing a different one (logged at info with both ids); the same customer writes nothing, and a customer already stored for another organization is left alone (logged at error). `POST /api/payments/webhook` (the one route `csrf::require` exempts; the provider's signature is the credential) verifies with the configured provider (a Fake event works only on a local hub, else 503), then stores and applies in one transaction: 200 `{"received":true}`, or, for a paid event naming a hub order whose session no order has yet, a rollback and 503 `this payment event cannot be applied yet; retry later` (logged at error) so Stripe retries. The Fake pay and abandon routes go through `receive` too. Tests: `tests/stripe.rs` against a local Stripe API mock (`tests/support/stripe_mock.rs`) with hand-built Stripe-shaped fixtures in `tests/fixtures/stripe/` signed in the test; README documents `stripe listen` test mode | offerings-and-payments story 3 (CAP-3, CAP-5, CAP-7) / ADR-0007 §4, §6 | ✅ partial | Not exercised against real Stripe (no account in the repo; fixtures are hand-built from Stripe's documented shapes, not recorded); fulfilment, entitlements, subscriptions, renewal and refunds are story 4's row below; invoices come only with subscriptions (story 4, D16): a one-off payment keeps Stripe's receipts; a paid event with no `amount_subtotal`/`currency` is not amount-checked; checkout holds the offering's share lock and a pool connection across up to two sequential Stripe calls (about 30 s at worst with the stale-customer fallback, each bounded by the 15 s timeout); live Stripe keys should wait for the open sales-tax/VAT decision (`.scratch/DECISIONS.md` D14, ADR-0007 "Not decided here") |
| Fulfilment into entitlements, renewal and cancellation — migration `0013_entitlements`: `entitlements` (one per order (`order_id` unique), `organization_id`, `provider`, `subscription_id` (unique with `provider`), `payment_ref` (the first payment's provider reference: Stripe's payment intent, or a subscription's first invoice), `term_start`/`term_end` (`term_end > term_start`), `renews`, `status` `active`\|`cancelled`\|`refunded`, `renewal_changed_at`, `cancelled_at`/`refunded_at` tied to status by CHECKs, `renews` only while active with a subscription; seats, scope, quota, features and term length are read from the order's snapshot, never copied; a trigger refuses deletes, any change to the order, organization, provider references, `term_start` or `created_at`, a decreasing `term_end`, a growing `term_end` unless active, any status step but active→cancelled, active→refunded, cancelled→refunded, and any change to a refunded entitlement), `entitlement_renewals` (`(provider, invoice_id)` primary key, `entitlement_id`, `previous_term_end`, `term_end`; append-only), and orders gain `refunded` (paid→refunded only, terminal, `refunded_at`; `paid_at` kept). The paid transition in `orders::apply_event` creates the order's **Entitlement** in the same transaction (`entitlements::grant`, the only insert: `term_start = paid_at`, `term_end = paid_at + term_months`, renewing when the payment started a subscription; audited `entitlement.granted {order_id, term_end}` as `provider:<name>`, subject `entitlement:<id>`). New **Payment event** kinds: `RenewalPaid` (Stripe `invoice.paid` with `billing_reason=subscription_cycle`; any other `invoice.paid`, including the first invoice, is ignored) extends the term by 12 months from its current end once per invoice (`entitlement.renewed {event_id, invoice_id, previous_term_end, term_end}`); an already-applied invoice is `unchanged`, a renewal for a cancelled or refunded entitlement is `unchanged` and logged at error (refund it at Stripe), one whose subtotal or currency differs from the order is `mismatch`, and one matching no entitlement is logged at error; `RenewalChanged` (`customer.subscription.updated`: renewing unless `cancel_at_period_end` or `cancel_at` is set) sets `renews`, ordered by the event's `created` so an older change arriving late is `unchanged` (`entitlement.renewal_stopped`/`renewal_resumed`; a change equal to the current state is `unchanged` and unaudited but still advances `renewal_changed_at`, so an older opposite change delivered after it stays stale); `SubscriptionEnded` (`customer.subscription.deleted`, after a cancellation at period end or after Stripe gives up on failed renewal charges) marks it `cancelled` at the event's time, its term running to its end (`entitlement.cancelled`); `Refunded` (`charge.refunded` with `refunded=true`; a partial refund is ignored, and a full refund naming neither a payment intent nor an invoice, such as another app's legacy charge on a shared account, is ignored with a warn log rather than refused) finds the entitlement by its first payment or a renewal invoice and marks it and its order `refunded` (`entitlement.refunded`, `order.refunded`, both `{event_id, payment_ref}` naming the refunded payment), withdrawing its seats; refs matching nothing are `unmatched` (warn). All go through `verify_event → payment_events::receive → orders::apply_event`; entitlement writes live only in `entitlements.rs` (a test greps for `UPDATE entitlements`). A renewal, subscription change or end that matches no entitlement but names this hub's order (subscription metadata `wipehub_order_id` with this hub's `wipehub_hub`) is 503 and rolled back, so Stripe retries until the paid checkout arrives, but only while that order can still be paid (it does not exist yet, or is `pending` with no `mismatch` event for its session); for an order that expired, was refunded or stays pending after a mismatch it is stored `unmatched` with 200, so the endpoint is never retried into being disabled. Renewal invoices are read in API version 2024-06-20's shape (`subscription`, `subscription_details.metadata`) and newer versions' (`parent.subscription_details`); a renewal invoice with neither subscription is ignored and logged at error. None of it revokes, shortens or reissues a licence (ROADMAP §0.2). Stripe checkout for an annual offering: `mode=subscription`, `line_items[0][price_data][recurring][interval]=year`, `subscription_data[metadata]` (the session's three values), the auto-renewal notice in `custom_text[submit][message]`, and never `customer_creation` (the stale-customer fallback drops `customer` only). The owner-approved auto-renewal notice (`entitlements::renewal_notice`: price like the portal's `formatPrice` with ISO 4217 minor-unit digits, date = today (UTC) + 12 months as `8 November 2027`) is carried by annual catalog offerings (`renewal_notice: {headline, detail}`, null for one-off) and the Fake session read (`{order, renewal_notice}`); the portal renders it verbatim with the headline in bold on `/buy` and `/checkout/fake`. `GET /api/entitlements` (any role, own organization): `{entitlements: [{id, order_id, offering_name, interval, grants, term_start, term_end, status, renews, current, cancelled_at, refunded_at}], available_seats}`, `current` = not refunded and `term_end > now()`, `available_seats` the seats of current entitlements. Fake: paying an annual order starts a `fake_sub_<uuid>` subscription; `POST /api/payments/fake/entitlements/{id}/renew` and `/cancel` (owner; 404 `no such entitlement` under another provider or for another organization's; 503 off a local hub; 409 `this subscription is not renewing` unless active, renewing and subscribed) sign a `subscription.renewed` (new `fake_in_<uuid>` invoice) or `subscription.cancelled` event and run it through `receive`. Portal: `/seats` ("Licence seats", linked from home and orders), `/billing/fake` lists renewing subscriptions with "Renew now (test)" and "Cancel (test)" (owners only; members see the list), orders show `Refunded`. Tests: `tests/entitlements.rs` (Fake), `tests/stripe.rs` (Stripe rows, new fixtures), trigger refusals in `tests/migrations.rs`, the new routes and tables in `tests/isolation.rs` | offerings-and-payments story 4 (CAP-4, CAP-5 invoices) / ADR-0007 §4, §6, D15, D16 | ✅ partial | Not exercised against real Stripe (renewals need a test clock; fixtures are hand-built); seats are consumed by station registration since `licence-delivery` story 1 (row below), and a renewal reissues no licence by itself: the station shows a renewal available and an owner issues the renewed licence (`licence-delivery` story 2, row below; a connected station fetching its renewed licence waits on D6); a renewal paid for a cancelled or refunded entitlement is only logged, so a vendor who refunds a subscription payment must also cancel the subscription at Stripe (README); refunding any one payment of a subscription refunds the whole entitlement and its order (a partial refund keeps the seats); a lost dispute (chargeback) is not mapped, so the vendor refunds or cancels in Stripe after losing one; a renewing entitlement is not `current` between `term_end` and Stripe's renewal charge (about an hour after the period starts, longer while Stripe retries a failed charge); orders paid before migration 0013 have no entitlement (no backfill; no hub took live payments before it); refund matching of subscription payments needs `charge.invoice`, absent in API versions newer than 2024-06-20, so the webhook endpoint and `stripe listen` must render 2024-06-20 (README); Stripe's "Manage failed payments" must cancel the subscription once retries fail, or a lapsed subscription stays `past_due` and keeps showing as renewing (README); a mismatched subscription checkout leaves a live Stripe subscription the vendor must cancel (README); a refund that arrives before its checkout event is stored `unmatched` and not retried; two subscription changes within the same second are ordered by arrival; the lapse policy (ADR-0012) is signed into licences since `licence-delivery` story 4 and enforced by stations only from `licence-visibility` story 5; the portal pages were checked by typecheck, unit tests and a production build, not in a real browser |
| Public offerings catalog and live pricing page — `GET /api/offerings` needs no session and ignores any cookie (a stale one is not a 401): the same `{offerings}` body, published only, by lower-cased name then id, to anyone, with no audit row and no CORS (the site reads it at build time in Node); the isolation sweep classifies it `Anonymous`. `sites/www` gains `PRICING_HUB_ORIGIN`, a build-time-only, non-`PUBLIC_` variable (environment or `.env`) validated like `PUBLIC_HUB_ORIGIN`: when set, `/pricing/` fetches `<origin>/api/offerings` once (10 s timeout), checks the body strictly (`src/lib/offerings.ts` `parseCatalog`: string id/name/description, positive safe-integer `price_minor`, `^[A-Z]{3}$` currency, `one_off`\|`annual`, grants, a `renewal_notice` headline exactly on annual offerings), and fails `astro build` naming `PRICING_HUB_ORIGIN`, the URL and the cause on a network error, timeout, non-2xx status or bad body. With offerings, `src/components/PriceList.astro` lists each one (escaped name and description with line breaks kept, price formatted like the portal's `formatPrice` plus "per year"/"one-off payment", seats, scope, quota, term, and on annual offerings the hub's renewal-notice headline verbatim) in place of the "Early-access pricing on request" block, with the meta description "Published prices for Wipestation offerings."; without the variable, or with an empty catalog, the page is byte-for-byte unchanged. The priced page ships no JavaScript and still runs through the site check, so an overclaim in a vendor-entered name or description fails the build (fix it in the admin area) | offerings-and-payments story 5 / CAP-6, ADR-0006 §1 | ✅ | Prices are as of the last build (the page is a static snapshot): rebuild the site after publishing, editing or retiring an offering, or a retired or edited offering keeps showing; a build never publishes prices it could not read. Licence features (`grants.features`) are not listed: the tier→feature matrix is ROADMAP W3.3 (depends on L1) and the feature names have no capability statuses; the page has no buy link (the site does not know the portal's public URL); copy awaits vendor sign-off |
| Station registration and licence download — migration `0014_stations`: `stations` (`organization_id`, `entitlement_id` (a composite foreign key with the organization, so a station can only hold its own organization's entitlement), `instance_key_id` `^ed25519:[A-Za-z0-9+/]{22}$`, the bench's `station_id`, `label`, `site` (each blank-free, ≤ 200 chars), `registered_by`/`registered_at`, `released_by`/`released_at` set together; a partial unique index on `(organization_id, instance_key_id)` while not released; a trigger refusing deletes, any change to the registration, and any change once released) and `licences` (`id` = the signed `license_id`, organization, station and entitlement by composite keys, `instance_key_id`, `root_key_id`, `not_after` > `issued_at`, `document text` (the exact `serde_json::to_string_pretty` that `wipe_license::save_license` writes), `status` `active`\|`superseded` with `superseded_at`; at most one active licence per station; a trigger refusing deletes, any change to the signed content, and any step but active→superseded). `stations::register` (owner; the only writer of both tables) locks the entitlement `FOR UPDATE`, refuses one not current (409 `this entitlement has ended or was refunded, so it has no seats to register`; another organization's or unknown is 404 `no such entitlement`), a machine entitlement without a station id (400 `a machine licence needs the station's id: …`), a key already registered and not released in the organization (409; not checked across organizations, which would leak another customer's key), and a full entitlement (409 `every seat of this entitlement is in use; …`; seats used = its stations not released), then signs with `VendorRoot::issue` from the hub's file-held root: `customer_id`/`customer_name` the organization's id and name, `quota` from `quota_erasures` (null → `Unlimited`), `features` from the order's names, `allowed_methods: All`, `not_before` = `issued_at` = now (microsecond precision so the stored time equals the signed one), `not_after` = the entitlement's `term_end`; machine scope `Scope::Machine { fingerprint: machine_fingerprint(station_id) }` with no `machine_binding`, site scope `Scope::Site` with the entered site or the organization id. Station, licence and audit rows (`station.registered`, `licence.issued {station_id, entitlement_id, instance_key_id, root_key_id, not_after}`, actor `user:<id>`) commit in one transaction. Routes: `GET /api/stations` (any role; released last, each with its latest licence), `POST /api/stations` (owner, else 403 `only an organization owner can register or release stations` (since story 2: `…register, renew or release…`), before the body; 503 `licence issuance is unavailable: this hub has no vendor root configured` with nothing written; 400 for a key id not `ed25519:` + 22 base64 chars encoding 16 bytes; 201 `{station, licence}`), `POST /api/stations/{id}/release` (owner; seat freed, licence `superseded`, `station.released` and `licence.superseded {station_id, reason: "station released"}`; 409 `this station has already been released`), `GET /api/licences/{id}/download` (any role; the stored bytes as `application/json`, `Content-Disposition: attachment; filename="licence-<id>.json"`, `Cache-Control: no-store`, audited `licence.downloaded {station_id}`), `GET /api/licensing/root` (any role; `{root: {public_key_id, public_key_b64} | null}`); another organization's station or licence is 404 like an unknown one. Portal `/stations` (linked from home and `/seats`): the station list with Download licence and, for owners, Release (behind a confirm that says the file keeps working) and a register form (entitlement select showing free seats, preselected when only one has a free seat; key id, station id required for machine scope, label, site for site scope), a plain note that a released station's licence file keeps working until its own expiry because the hub does not distribute revocation or supersession, how to read a station's instance key id (`GET /api/public_key` → `public_key_id`, the `signing key public id:` startup line) and the vendor root public key with the `wipestation serve --license … --vendor-root-b64 …` line. Tests: `tests/stations.rs` (every I/O row through HTTP; the downloaded file goes through `load_license`, `check_installable` and `evaluate` with `machine_fingerprint("bench-1")` to `Valid`; eight concurrent registrations on a one-seat entitlement give one 201; trigger refusals), the new routes and tables in `tests/isolation.rs` | licence-delivery story 1 (CAP-1, CAP-2, CAP-5) / ADR-0005, ADR-0007 §5, §6 | ✅ partial | The root is file-held (ADR-0007: production must not launch on it; A1.5 open); releasing never revokes anything a station can see, so a released station's file stays valid offline until `not_after` (no revocation distribution, ADR candidates 0008/0010); a machine licence binds the customer-controlled `--station-id`, not hardware (`machine_fingerprint` is a hash of it); a station registered under one id and started under another signs `evaluation` certificates, which the form explains but cannot check; renewal reissue shipped in story 2 (row below), vendor issue/inspect/revoke in story 3 (row below), the lapse policy in story 4 (row below); duplicate keys are checked per organization only; a superseded licence can still be downloaded through the API (the portal offers only active ones); registering and downloading were checked through HTTP tests and a manual run with a real `wipestation serve`, the portal page by typecheck, unit tests and a production build, not in a real browser |
| Renewal reissue — no migration. Every station in `GET /api/stations` carries `renewal_available`, computed in SQL and never stored: not released, entitlement current (not refunded, `term_end > now()`), and its latest licence `active` with `not_after < term_end` (a paid renewal moved the term on). `POST /api/stations/{id}/reissue` (owner, else 403 `only an organization owner can register, renew or release stations`; 503 `licence issuance is unavailable: …` with no vendor root, before the id is read; a non-UUID, unknown or another organization's id 404 `no such station`) locks the station and its entitlement `FOR UPDATE` in one statement scoped to the session's organization, then refuses, in order and writing nothing, a released station (409 `this station has already been released`), an entitlement not current (409 `this entitlement has ended or was refunded, …`) and a licence that already runs to `term_end` (409 `this station's licence already runs to the end of its entitlement's term`, read in a statement after the lock so a concurrent reissue that committed meanwhile is seen: eight parallel reissues give one 201). Otherwise it marks the old licence `superseded` and signs a new one through the same helper `register` uses (`stations::issue_licence`, since story 3 a wrapper over `stations::sign_and_store`: the `Entitlements` build, `VendorRoot::issue`, the `to_string_pretty` file, microsecond `now`, the insert and the `licence.issued` audit row): same instance key, scope (`machine_fingerprint` of the stored `--station-id`, or the stored site or organization id), quota and features, `not_after` = the current `term_end`; answers 201 `{station, licence}`. The station keeps its seat and registration. Audited (actor `user:<id>`) as `licence.issued` with story 1's detail plus `reissue_of`, and `licence.superseded {station_id, reason: "renewed", superseded_by}`. The payment path never signs: a renewal only moves `term_end`, so a missing or broken vendor root can never stop a renewal payment applying. Portal `/stations`: the state reads "Renewal available (current licence to …)"; owners get "Issue renewed licence", then a link to download the renewed file; members read "Renewal available: an owner can issue it."; the note says the station uses the renewed licence only once it is installed and the old file keeps working until its own expiry. Tests: `tests/stations.rs` (every I/O row through HTTP: a Fake renewal flips `renewal_available`; the reissued file installs and `evaluate`s `Valid` 30 days after the old term end while the old file is `Expired`; site scope keeps the entered site or the organization id; the old file stays downloadable byte for byte; eight concurrent reissues give one 201 and one active licence), the route in `tests/isolation.rs` | licence-delivery story 2 (CAP-3) / ADR-0005, ADR-0007 §5, §6 | ✅ partial | The station keeps using the old file until someone installs the renewed one by hand (no push or online fetch, D6/ADR candidate 0008); an owner reissues each station separately, and nothing notifies anyone that a renewal is waiting: until an owner opens `/stations` and reissues, each bench keeps its old file and marks certificates `evaluation` after the old `not_after`, although the customer has paid; the old licence is superseded at the hub only, and its file stays valid offline until its own `not_after`; since story 4 reissue also issues a licence carrying a changed lapse policy (row below); the portal was checked by typecheck, unit tests and a production build, not in a real browser |
| Vendor-admin licence issue, inspection and revocation — migration `0015_licence_admin`: `licences` gain `source` `station`\|`vendor` (station: `station_id` and `entitlement_id` set, no `issue_reason`; vendor: both null and a non-blank `issue_reason` ≤ 500 chars), `status` adds `revoked` with `revoked_at` and `revocation_reason` (non-blank, ≤ 500) set exactly when revoked, `superseded_at` kept when a superseded licence is revoked; `licences_lifecycle()` is replaced so signed content, `source` and `issue_reason` never change and the only steps are active→superseded, active→revoked, superseded→revoked (revoked is terminal). `stations.rs` stays the only writer: `register`, `reissue` and the new `vendor_issue` all sign and store through `sign_and_store`. Routes (vendor admins only, `VendorAdmin` first: no session 401, anyone else 403 `only vendor admins can use the admin area`; a non-UUID, unknown or vendor organization 404 `no such organization`, a non-UUID or unknown licence 404 `no such licence`, none audited): `POST /api/admin/organizations/{id}/licences {instance_key_id, scope, station_id?, site?, quota_erasures (null = unlimited, else 1–10⁹), features, days (1–1095), reason (non-blank, ≤ 500, no control characters)}` (unknown fields refused; 503 without a vendor root; 201 `{licence}`) signs for the organization's id and current name, `not_before` = `issued_at` = now, `not_after` = now + `days`, `allowed_methods: All`, no machine binding, machine scope `machine_fingerprint(station_id)` (required), site scope the entered site or the organization id; no station, entitlement or seat is touched; audited `licence.issued {organization_id, instance_key_id, root_key_id, not_after, source: "vendor", scope, quota_erasures, features, reason}`. `GET /api/admin/organizations/{id}/licences` → `{licences}` (station and vendor, newest first; `admin.organization_licences_viewed {licence_ids}`). `GET /api/admin/licences/{id}` → `{licence, body, verification}`: the record (organization, `source`, station `{id, station_id, label}` or null, status, `issue_reason`, `revocation_reason`), the stored file's `LicenseBody`, and `{status: verified\|failed\|no_root, root_key_id, detail}` against the hub's root only (`admin.licence_viewed`). `POST /api/admin/licences/inspect {document}` (≤ 64 KiB; anything else 400 `this is not a licence file: …`, unaudited) → `{body, verification, licence}` with the hub's record of that `license_id` or null (`admin.licence_file_inspected {license_id, canonical_sha256_hex, verified, known}`, subject `inspect:licence-file`; `license_id` only when it is a UUID, else null, and the hash computed by the hub over the pasted body, not the one the file claims). `GET /api/admin/licences/{id}/download` → the stored bytes with the customer download's headers, revoked or not (`licence.downloaded {organization_id, by: "vendor"}`). `POST /api/admin/licences/{id}/revoke {reason}` locks the licence's station `FOR UPDATE` before the licence row (the order reissue and release use) and marks an active or superseded licence revoked (200 `{licence}`; 409 `this licence is already revoked`; audited `licence.revoked {organization_id, station_id, previous_status, reason}`). Customer side: `Licence` gains `revoked_at` (never the reason); a station whose latest licence is revoked reads "Revoked on <date>" on `/stations` with no download link and `renewal_available: false`; reissue is 409 `this licence was revoked by the vendor; contact support` (checked after the station lock, so a revocation racing a reissue gives 200 + 409 or 201 + 200, never a 500), the customer download of a revoked licence is the same 409, the station keeps its seat until an owner releases it, which still works; the customer download serves station licences only (a vendor-issued one is 404 there). Portal: the admin organization page lists its licences and has an issue form stating it uses no seat or entitlement; `/admin/licences/$id` shows the record, the decoded file, its verification, a download, and a revoke form behind a reason and a confirming press, with a note that revocation is recorded in the hub only and the file keeps verifying offline until its expiry; `/admin/licences/inspect` (linked from `/admin`) decodes a pasted file. Tests: `tests/licence_admin.rs` (every I/O row through HTTP, incl. installing a vendor-issued file to `Valid` on `bench-1`, a tampered paste, a hub without a root, four rounds of revoke racing reissue plus a revoke held behind the station lock while the licence is superseded, the customer-side effects), the routes in `tests/isolation.rs`, 0015's refusals in `tests/migrations.rs` | licence-delivery story 3 (CAP-4) / ADR-0005, ADR-0007 §5, §6, ROADMAP A1.2, A1.3, A-R1, A-R7 | ✅ partial | Revocation is recorded in the hub only: no revocation list, push or online check (A2.3, ADR candidates 0008/0010), so a revoked file keeps verifying offline and a station keeps using it until its `not_after`; a vendor-issued licence is not listed on the customer's Stations page (it belongs to no station), so the vendor admin downloads it and sends it; revoking a station's licence does not stop its owner releasing the station and registering the same key again on a free seat, which signs a new licence (the vendor's lever on seats is refunding or cancelling the entitlement, and the admin licence page says so); the root is still file-held (A1.5); a pasted file is verified against this hub's root only; the portal pages were checked by typecheck, unit tests and a production build, not in a real browser |
| Customer lapse policy in issued licences — migration `0016_lapse_policy`: `organizations.lapse_policy` `evaluation`\|`stop` (default `evaluation`, CHECKed). `wipe_license::LapsePolicy` (`snake_case`, `Default` = `Evaluation`) is a field of the signed `LicenseBody`, `#[serde(default, skip_serializing_if = "LapsePolicy::is_evaluation")]`: an `evaluation` licence serializes exactly as before the field existed, so every earlier file keeps its canonical bytes, hash and signature (a committed pre-change fixture, `crates/wipe-license/tests/fixtures/licence-before-lapse-policy.json`, verifies and re-serializes byte for byte), and removing `stop` from a signed body is `Tampered`; `license_format_version` stays 1. `VendorRoot::issue` keeps its signature and delegates to the new `issue_with_lapse_policy`. The hub signs only through `stations::sign_and_store`, which reads the organization's policy `FOR SHARE` in the issuing transaction and signs it into every licence it issues (register, reissue and vendor issue alike: a vendor trial follows the customer's choice), adding `lapse_policy` to the `licence.issued` detail. A licence's policy is never stored beside it: it is read from the stored document in SQL (`COALESCE((document::jsonb)->'body'->>'lapse_policy', 'evaluation')`), and `Licence`, `AdminLicence` and `AdminOrganizationDetail` carry `lapse_policy`. Routes: `GET /api/organization/lapse-policy` (any role, `{lapse_policy}`) and `POST /api/organization/lapse-policy {lapse_policy}` (owner, else 403 `only an organization owner can change what stations do when a licence lapses`, before the body; `deny_unknown_fields`, a bad value, missing field or non-JSON is 400): under the organization row lock (`team::lock_organization`) the owner is re-checked, and a change audits `org.lapse_policy_changed {old, new}` (actor `user:<id>`, subject `organization:<id>`); posting the current value is 200 with no write and no audit row. Stations gain `lapse_policy_update_available` (computed in SQL: not released, entitlement current, latest licence active and signed with another policy than the organization's). `POST /api/stations/{id}/reissue` now also issues such a station a licence with the current policy (same key, scope, quota, features, to `term_end`), superseding the old one with reason `"lapse policy changed"` (`"renewed"` when the licence was also behind the term); otherwise 409 `this station's licence already runs to the end of its entitlement's term and carries your organization's lapse policy`. Portal `/stations`: a "When a licence lapses" section for every member (the current setting, what each policy does and never does, that a change applies only to files issued afterwards, and a removable sentence that station enforcement arrives in a later station release); owners switch it, confirming before `stop`, members read "Only an organization owner can change this."; a station row reads "Updated licence available: lapse policy changed (…)" with "Issue updated licence". The admin organization and licence pages show the policy read-only. Tests: `crates/wipe-license/tests/lapse_policy.rs` (old fixture, omitted default, stop round trip, tamper), `tests/stations.rs` (every I/O row through HTTP, including a stop file installed with `check_installable` and `evaluate` to `Valid` and eight concurrent reissues after a change giving one 201), `tests/licence_admin.rs` (vendor issue under stop, admin views), the routes in `tests/isolation.rs`, the CHECK in `tests/migrations.rs` | licence-delivery story 4 (CAP-6) / ADR-0012, ADR-0005 | ✅ partial | The hub only writes the policy into licences: no station enforces it, warns about it or refuses a job yet (`licence-visibility` story 5), and nothing in the hub can stop a station erasing (ROADMAP §0.2); a change reissues nothing by itself, so each station keeps its old policy until an owner issues and someone installs its updated file; a `stop` licence fails verification on a station or verifier built before this change (it carries a field those builds re-serialize away), and no such build is deployed; a superseded file, such as the earlier `evaluation` one, stays valid offline until its own `not_after` (supersession is not distributed, ADR candidates 0008/0010), so someone at the bench can reinstall it and the `stop` choice holds only while the newest file stays installed; a vendor-issued licence is never flagged or reissued after a policy change (the vendor admin issues a new one); the vendor has no route to change a customer's policy; the portal was checked by typecheck, unit tests and a production build, not in a real browser |
| Releases, the artifact store and `wipe-hub release publish` — migration `0017_releases`: `releases` (`version` SemVer 2.0.0 without build metadata (`+…` is refused: SemVer ignores it when ranking, so `1.0.0` and `1.0.0+build.5` could not both be the latest) checked by the hub with the `semver` crate, ≤ 64 chars, unique across every release including withdrawn ones (`releases_version_key`); `notes` trimmed, non-blank, ≤ 10 000 chars, no control characters but newline and tab; `channel` `stable`\|`preview`; `status` `draft`\|`published`\|`withdrawn`; `created_by` (null for the CLI), `created_at`, `published_at`, `withdrawn_at`) and `release_artifacts` (`id` = the store key, `platform` `^[a-z0-9][a-z0-9._-]{0,63}$` unique per release, `filename` `^[A-Za-z0-9_+-][A-Za-z0-9._+-]{0,127}$`, `size_bytes` > 0, `sha256` lowercase hex). Triggers: a release is inserted only as a draft, never deleted, its version, channel, notes and creation fields never change, the only status steps are draft → published (needs an artifact), draft → withdrawn and published → withdrawn, each stamping its time exactly once; artifacts are never updated and are inserted or deleted only while their release is a draft. `wipe_hub::artifacts`: the async `ArtifactStore` trait (`put`, `open`, `delete`; `Arc<dyn ArtifactStore>` in `AppState`), `LocalDirStore` (keys must be canonical UUIDs; writes a dot-file in the same directory, fsyncs, renames; deleting a missing key is fine) and `store_artifact`, which computes size and SHA-256 while the bytes stream into any store and keeps nothing for an empty file or one over `ARTIFACT_MAX_BYTES` (1 GiB, `Config::artifact_max_bytes`, not a flag). `serve` and the CLI take `--artifact-dir` / `WIPE_HUB_ARTIFACT_DIR` (default `.wipe-hub/artifacts` under the working directory, git-ignored), created at start-up; a path that is a file or cannot be created refuses to start. Routes (vendor admins only, `VendorAdmin` first: 401/403 before anything is read): `GET\|POST /api/admin/releases`, `GET /api/admin/releases/:id`, `POST …/:id/artifacts?platform=&filename=` (the raw `application/octet-stream` body, else 415; streamed to the store before any transaction, then one transaction locks the release `FOR UPDATE`, re-checks draft and platform, inserts and audits; any refusal deletes the stored file; 413 over the limit, 400 empty, 409 platform taken or not a draft, 503 on a storage failure with the cause logged), `POST …/:id/artifacts/:artifact_id/remove`, `POST …/:id/publish` (409 without an artifact), `POST …/:id/withdraw`. Audit (subject `release:<id>`): `release.created {version, channel}`, `release.artifact_added`/`release.artifact_removed {artifact_id, platform, filename, size_bytes, sha256}`, `release.published {artifact_ids}`, `release.withdrawn {previous_status}`, and reads `admin.releases_listed {release_ids}` / `admin.release_viewed {}`. `wipe-hub release publish --version --notes [--channel] --file <platform>=<path>…` validates every flag and file first, applies migrations, stores each file, then creates, adds and publishes in one transaction as `cli:release-publish` (`os_user` in each detail), printing the release id and each artifact's size and SHA-256; any failure exits non-zero naming the problem, commits nothing and deletes what it stored. Portal: `/admin/releases` (every release with status, channel and file count; a create form that opens the new draft) and `/admin/releases/$id` (notes, files with the full SHA-256, upload (platform, file and an editable file name prefilled from it, the file sent raw) and remove while draft, with remove, publish and withdraw each behind a confirming press). Tests: `tests/releases.rs` (every I/O row through HTTP and the built binary, including stored bytes hashing to the recorded checksum and no leftover temp files), store unit tests in `src/artifacts.rs`, the routes in `tests/isolation.rs`, the triggers in `tests/migrations.rs`, the artifact directory in `tests/cli.rs` | software-downloads story 1 (CAP-1, CAP-4, CAP-5) / ADR-0007 §5 | ✅ partial | Customer downloads shipped in story 2 (next row); admins cannot download a file through the hub; no code signing, auto-update or CDN; only the local-directory store exists; a file stored by an upload whose transaction then fails is deleted best-effort (a failed delete is logged and leaves an orphan file); withdrawal takes no reason; the portal was checked by typecheck, unit tests and a production build, not in a real browser |
| Entitled downloads with audit — migration `0018_release_downloads`: `release_downloads` (`release_id` → releases, `artifact_id` → release_artifacts, `organization_id` → organizations, `user_id` with no foreign key (removing a team member deletes the user, and the download stays recorded), `downloaded_at`; no email or other personal data is copied in, since the table can never erase it; index `(release_id, downloaded_at DESC)`); triggers refuse UPDATE, DELETE and TRUNCATE, and an INSERT whose artifact is not in its release or whose release is not `published`. `wipe_hub::downloads`: an organization is **entitled** when it has a current entitlement (not refunded, `term_end > now()`, the `current` rule of `GET /api/entitlements`, from the one shared SQL fragment `entitlements::CURRENT_SQL` that station seats also use; the vendor organization never is). `GET /api/downloads` (session, any role, entitled, else 403 `your organization has no current entitlement, so it cannot download releases`): `{releases}`, published only, both channels, newest `published_at` first, each with `id, version, notes, channel, published_at, artifacts: [{id, platform, filename, size_bytes, sha256}]`. `POST /api/downloads/:artifact_id/link` (session, any role, entitled, checked before the id): `{url, expires_at}`, a **download link** good for `Config::download_link_ttl` (300 s, not a flag); a draft, withdrawn, unknown or non-UUID artifact is 404 `no such download`. `GET /api/downloads/:artifact_id/file?user=&expires=&sig=` needs no session (the isolation sweep classifies it `Anonymous`): the signature is HMAC-SHA256 over `wipe-hub/download/v1\n<artifact>\n<user>\n<expires>`, base64url without padding, keyed by 32 OS-RNG bytes drawn when the hub starts (`AppState::download_signer`) and compared with `Mac::verify_slice`; `GET` only: `HEAD` (which axum would route to the GET handler) and every other method are 405 and write nothing, so a link-preview bot's probe records no download; a missing, malformed, tampered or expired link, or one whose user no longer exists, is 403 `this download link is invalid or has expired`, an organization no longer entitled 403, a release no longer published 404, and a stored file the store cannot open 503 `file storage is unavailable; try again later` (cause logged), none writing anything. On success it opens the stored file, then one transaction (the release row share-locked) inserts the `release_downloads` row and the audit row `release.downloaded` (actor `user:<id>`, subject `release:<id>`, detail `{artifact_id, platform, filename, sha256, organization_id}`) and commits, then streams the bytes as `application/octet-stream` with `Content-Disposition: attachment`, `Content-Length`, `Cache-Control: no-store`, `X-Content-Type-Options: nosniff` and `Referrer-Policy: no-referrer`. Listing and link issuing are not audited. Admin: `GET /api/admin/releases/:id/downloads` (`VendorAdmin` first): `{downloads: [{id, artifact_id, platform, filename, user_id, user_email, organization_id, organization_name, downloaded_at}]}` (`user_email` is the user's current email, joined from `users`, `null` once they have been removed from their team), newest first, any release status, audited as `admin.release_downloads_viewed {}`; unknown or non-UUID id 404 `no such release` with no audit row. Portal: `/downloads` (home-page link) lists each release with channel, publication date and notes, and each file with platform, human size and full SHA-256 and a Download button that requests a link and navigates to it; an unentitled organization sees that it has no current entitlement and a link to `/buy` (matched on the hub's not-entitled message, so another 403 such as a CSRF refusal shows as an error). `/admin/releases/$id` gains a Downloads section (when, user email or, for a removed user, their id, organization, platform), loaded on a press because the read is audited. Tests: `tests/downloads.rs` (every I/O row through HTTP, including the fetched body's SHA-256 against the listed checksum and the CLI-published file, header values, exact row and audit counts, a withdrawal racing a fetch, a `HEAD` probe, a removed user's `null` email), sign/verify unit tests in `src/downloads.rs`, the routes in `tests/isolation.rs`, the triggers in `tests/migrations.rs` | software-downloads story 2 (CAP-2, CAP-3) / ADR-0007 §5, §6 | ✅ partial | A link is a bearer credential for five minutes: anyone holding it can fetch the file as its user (re-checked against that user and their organization's entitlement, and recorded as them); the signing key is per hub process, so a restart voids outstanding links and two hub instances would refuse each other's links (§11 v0.3 #14); no `Range`/resume, so an interrupted download starts again and records another row; an interrupted transfer still counts as a download; vendor admins cannot download release files through the hub; no code signing, auto-update or CDN; the portal was checked by typecheck, unit tests and a production build, not in a real browser |
| Commerce end-to-end demo — `scripts/commerce-demo.sh` (bash, curl, jq): builds `wipe-hub` and `wipestation`, creates a throwaway `wipehub_demo_<run>` database (dropped on exit, through `docker compose exec postgres psql`), starts a hub with the Fake payment provider and a run-scoped vendor root on :7910 and a station on :7890 (`HUB_PORT`/`STATION_PORT`), then over the hub's public HTTP API and its `admin create`/`release publish` CLIs only: vendor admin, published offering, customer sign-up, email verification via Mailpit, Fake checkout, release download with a SHA-256 check against the built binary, station registration, licence download, licensed erasure, and `verify-cert --require-licensed` against the vendor root (plus a wrong-root negative check); the first failed step prints `FAILED: <step>: <reason>`, exits 1 and keeps the work dir | commerce-end-to-end story 1 (CAP-1) / ADR-0006 §3 | ✅ partial | ADR-0006 §3's "install it on a running station" is met by restarting the station with the licence (`--license`, the ADR-0005 offline path), pending the hot install planned in `.scratch/licence-visibility`. Mock backend only: it proves the commercial flow, not sanitization of real drives. Not run by any CI (there is none) |

## 11. Deferred — known and committed

In rough priority order for v0.2 and beyond. Item numbering is stable —
shipped items keep their number and are struck through rather than
removed, so that §10.2, ADRs and commit messages that cite "v0.2 #N"
keep resolving.

### v0.2 candidates

1. **Real Linux ioctl backend** (`wipe-engine-linux`) — the trait seam
   exists; consumers don't change. Needs hardware. **Now the single
   largest gap: everything downstream of the `DeviceBackend` seam is
   built and tested, but the only implementation is the mock, which
   synthesises the very command evidence the product's value rests on.**
2. ~~**Job as outcome-bearing composition** (ADR-0001)~~ — **SHIPPED**
   (see §10.2). Renamed `Job` → `ErasureEvent`, introduced the outer
   `Job` + `JobState` with `PendingCoSign` on the Destroy path,
   introduced `JobActivity` composition, renamed `JobEvent` →
   `JobUpdate`, moved the cert schema to v2, and added the
   `DestructionManifest` supervisor co-sign flow. Diagnostic and
   HealthCheck landed as schema only — see #11 below for the remainder.
3. ~~**At-a-glance bench status on Devices page**~~ — **SHIPPED** (see
   §10.2). Device cards are joined against `/api/jobs` by `device_id`
   and colour-coded by slot status, with a "safe to disconnect"
   affordance on Erased, a "needs attention" affordance on Failed, and
   a "Start new" affordance for re-plug-after-wipe. Pure frontend
   wiring; no schema or backend changes.
4. **Enterprise data model — Customer + Contract + WorkOrder + Asset
   + SanitizationProfile** — closes the FIPS 199 gap in §8. Backed by
   SQLite (Enterprise mode only; Simple mode stays schemaless beyond
   Jobs/Certs). WorkOrder identifier shared with the ITAD's ERP via
   the integration API. Wizard becomes "scan Asset / pick from
   WorkOrder" not "pick classification." Policy inherited via
   WorkOrder → Contract → Default chain. Profiles decoupled from
   Customer (globally owned by the ITAD).
5. **ITAD-ERP integration tier-zero** — REST in (ERP pushes
   Customer/Contract/WorkOrder/Asset records), REST out (ERP queries
   Jobs/Certs by id), webhooks on Job state transitions and cert
   signing, asset-id lookup callbacks for inline classification fetch.
   Differentiator §3 #4; not a Tier-2 cloud feature.
6. **Operator authentication** — replace localStorage with a real
   auth call. YubiKey/PIV at the supervisor desk is the headline
   path; OIDC/SAML for cloud tier.
7. **RBAC** — roles: `loader`, `operator`, `supervisor`, `auditor`.
   Every action attributed; some actions require supervisor co-sign.
8. ~~**License token verification**~~ — **DECIDED AND PARTLY SHIPPED**
   by ADR-0005, which chose an
   attestation *chain* over a standalone token: vendor root → licence
   certificate → station instance key → erasure certificate, with
   entitlements and an offline lease. The remaining product work — the
   three purchasable tiers, activation, expiry, renewal, revocation —
   is #15.
9. **PDF/A-3 cert wrapping** — JSON-LD inside, human-readable PDF
   outside, single attestation artifact.
10. ~~**Configurable bay topology + bay map**~~ — **SHIPPED** (see §10.2). A station declares its
    Enclosures/Banks/Bays; the server resolves each Bay to a Device and
    the UI renders a vector bay map with live per-Bay status. Grew
    beyond the original scope into three ADRs: the topology model
    (ADR-0002), the tiered config store that lets a customer *save* one
    (ADR-0003), and model-aware enclosure artwork (ADR-0004), plus the
    bench-setup builder and learn-by-hot-swap identify mode.
11. **Diagnostic / HealthCheck runtime** — the remainder of ADR-0001.
    `DiagnosticEvent` and `HealthCheckEvent` shipped as schema-only
    types and `JobActivity` variants; the runner never emits them and
    `HealthCheckEvent.attributes` is still an untyped
    `serde_json::Value`. Blocked on the §12 question of whether they
    run always or only when a SanitizationProfile requests them, and
    on what a Critical finding should do to the Job.

12. **Marketing + documentation website** — one in-repo static site
    doing both jobs, with the code-derived reference material generated
    and CI-gated so the docs cannot drift from the binary. Requirements,
    architecture and phases: [ROADMAP](docs/ROADMAP.md) §1 (**W**).
    *The marketing half is gated on #1 — we cannot publicly claim
    hardware sanitization while the only backend is the mock.*
13. **Admin / control plane** — vendor console (licence issuance,
    entitlements, revocation, metering) and fleet console (station
    inventory, cert search, seat allocation) over one multi-tenant
    `wipe-hub`. Stands up the seams ADR-0003 (`ControlPlaneStore`) and
    ADR-0005 (`HttpLicenseClient`) already declare and leave
    unimplemented. [ROADMAP](docs/ROADMAP.md) §2 (**A**).
14. **First-class operator portal** — the existing React/Tauri app is
    the most mature surface; what it still needs is identity and RBAC
    (#6/#7), batch work, an exception inbox, bench ergonomics for
    gloves and warehouse lighting, offline-verifiable label QR, and
    reboot recovery. [ROADMAP](docs/ROADMAP.md) §3 (**O**).
15. **Licensing tiers end to end** — free/evaluation, per-machine and
    site-wide, joined up from purchase through activation, expiry,
    renewal and revocation. The attestation chain and entitlement model
    exist (ADR-0005); the product around them does not. Supersedes #8,
    which described the token verification ADR-0005 has since decided.
    [ROADMAP](docs/ROADMAP.md) §4 (**L**).

### v0.3 / beyond

> Numbering below starts at 7 for historical reasons and is **not**
> continuous with the v0.2 list above. It is left as-is because
> existing references (e.g. "v0.3 #12" in §5) resolve against it.

7. **Hub mode** — same binary, `--hub` flag; tenant-aware; cert
   archive; cross-LAN fleet view.
8. **Cloud SaaS** — multi-tenant Hub run by us; customer-facing cert
   portal; verification API; SOC 2.
9. **Bootable ISO** — Alpine + signed shim + GRUB + binary; Microsoft
   3rd-party UEFI signing path.
10. **Mobile (iOS / Android) erasure** — separate product surface but
    same SKU/contract.
11. **TUI mode** (Ratatui) — for SSH / serial-console scenarios.
12. **Destroy chain-of-custody workflow** — formal photo/video evidence,
    handoff records, supervisor sign-off.
13. **Failure routing** — drives that fail repeatedly automatically
    routed to Destroy with linked cert history.
14. **More than one hub instance** — the hub keeps one process-local
    secret: the key that signs **download links** is drawn at start-up
    (software-downloads story 2), so links issued by one instance fail on
    another and a restart voids outstanding links (each lives five
    minutes). Running hubs behind a load balancer needs a shared key
    (a configured secret or one stored in Postgres) first.

## 12. Open questions for grilling

These are the questions `/grill-with-docs` should help us pressure-test
and resolve. None of these have a decided answer.

### Domain & UX

- Should an operator be allowed to **override** a Contract/WorkOrder
  classification per-Job, or is that a supervisor-only privilege?
- For PXE stations, how does the operator **authenticate** at first
  boot when there's no display + no local key? (Tablet handoff?
  Pre-shared site token? Both?)
- When erasure attempts on the same Asset are **exhausted** (N
  consecutive ErasureEvent failures), do we auto-route the Job to a
  pending-Destroy state for supervisor sign-off, or always require an
  explicit supervisor action to escalate? *Code today takes the
  explicit path: a failed ErasureEvent leaves the Job `InProgress` and
  escalation happens only via `POST /api/jobs/:id/escalate-to-destroy`.
  There is no auto-routing and no N-failure counter — v0.3 #13.*
- ~~**Verification** — first-class `VerificationEvent`, or embedded in
  each ErasureEvent's state machine?~~ **RESOLVED in code (ADR-0001):
  both, deliberately.** The inner `Verifying` state runs the sampled
  reads and produces the `VerificationReport`; the runner then appends
  a sibling `JobActivity::Verification` naming the ErasureEvent it ran
  against, so audit prose reads "Verification passed against
  ErasureEvent 2". If this dual shape proves confusing, that is an ADR,
  not a bug fix.
- **DiagnosticEvent / HealthCheckEvent timing** — always run
  pre-erasure, or only when a SanitizationProfile requests them?
  What happens if Diagnostics finds a condition the operator wasn't
  warned about (drive degraded, SED locked)?
- Offsite multi-station WorkOrder propagation: pre-loaded "trip
  file" exported from the Hub before the visit, PIN-shared on arrival,
  or fleet-Lead-creates-and-syncs? *Architectural; ADR candidate
  once decided.*
- Offsite deferred sync: each station pushes its own Jobs/Certs to
  the Hub on reconnect, or one station aggregates and pushes for the
  whole WorkOrder? *Architectural; ADR candidate once decided.*

### Technical

- For raw block reads on Linux, do we use `O_DIRECT` from Rust
  natively, or is there a perf/correctness argument for a small
  C-helper crate?
- Cert format version — **v2 was cut by ADR-0001, but no
  multi-version support was built with it.** `CERT_FORMAT_VERSION` is
  `2`, nothing reads the field, and `disposition` / `activities` are
  required, so a v1 cert fails to deserialize with `missing field
  'disposition'` before signature checking. Harmless today (no v1
  certs exist outside development) and a real problem the first time a
  cert outlives a schema bump — an auditor's whole value proposition
  is that an old cert stays verifiable. Decide before v1.0: embedded
  migration shims, verifier-side multi-version support, or an explicit
  "certs are verifiable only by their own major version" stance that
  we publish. *Whatever we pick needs a test that verifies a
  previous-version cert fixture.*
- Should the **Validate** registry live on the station, the lead, or
  only the hub/cloud? Implications for air-gap customers.
- Lead election — do we need to add quorum / Raft when fleets grow,
  or always-promote-to-Hub at that point?
- Hub ↔ Station protocol — same REST surface (current) or
  gRPC/protobuf for the cross-LAN tier?
- License token format — signed JWT, custom Ed25519-signed JSON, or
  something OAuth-shaped?

### Product surfaces (raised by the roadmap, none decided)

- **Site licences cannot be enforced offline.** A site licence that
  installs on station A installs on station B, and neither can count the
  other without a network. Detect-and-report (LAN quorum + hub
  reconciliation), or accept unbounded offline use and make it a
  commercial conversation? *ROADMAP §4.3 recommends detect-never-prevent;
  ADR candidate 0010.*
- **What does the free tier actually withhold?** Time limit, volume
  limit, feature limit, or nothing but the `evaluation` marking on the
  certificate? **Resolved 2026-11-08 (ADR-0012):** nothing but the
  marking, with no time bomb. A customer may opt in to a `stop` lapse
  policy, under which a lapsed station refuses to start new jobs instead of
  issuing `evaluation` certificates.
- **Does the control plane share a store with the Enterprise data
  model?** v0.2 #4 proposes SQLite on the station; the hub needs
  Postgres. Two stores for two jobs is defensible and must be stated,
  not stumbled into. *ADR candidate 0007.*
- **PXE first-boot authentication** — already listed under Domain & UX
  above, and now blocking: operator RBAC (#7), certificate attribution
  and the whole of ROADMAP O1 depend on an answer.
- **Does the public verification API return the same verdict as offline
  `verify-cert`?** It must, or we have rebuilt the vendor-dependent
  verification we attack in §1. Anything that makes them diverge is a
  product bug, but nothing enforces that today.

### Compliance & packaging

- Does our **single-pass overwrite** stance fully satisfy procurement
  teams whose policy still says "DoD 3-pass" even though R2 deprecates
  it? Do we expose a "compatibility" multi-pass mode purely for
  procurement appeasement?
- Microsoft's **3rd-party UEFI signing** for the bootable image — do
  we commit to that or ship MOK + customer enrollment?
- Pricing: is **annual unlimited per station** the right unit, or
  should we also offer per-site / per-rack tiers for high-density
  ITAD?
- For Tier 2 cloud, does the **free tier** include the customer-facing
  portal, or is that always paid?

### Engineering process

- Do we **vendor the frontend dist** into the binary via `rust-embed`
  for true single-file distribution, or keep runtime resolution
  (current — `--static-dir` flag with auto-detect)?
- ~~TS/UI tests — do we add Vitest + React Testing Library now, or rely on
  the Rust integration tests until the UI surface grows?~~ **RESOLVED
  (2026-08-06): Vitest added.** The trigger was the bench-setup builder and
  identify mode — the frontend now carries real domain logic (numbering
  runs, grid rebuilds, hot-plug diffing, binding rules) that the Rust
  tests cannot reach. `pnpm test` runs a node-environment suite over the
  pure modules in `src/bench/`. React Testing Library is deliberately
  *not* added yet: the logic worth pinning was extractable, and a DOM
  harness can arrive with the first test that genuinely needs to render.
- ADRs — do we backfill the architectural decisions already made
  (Rust-vs-Bun, Tauri-vs-localhost-only, mDNS-vs-Raft, etc.) so
  `/improve-codebase-architecture` has decision history to work
  against?
- **Mode bifurcation discipline** (Simple vs Enterprise). The
  data-model contract is: Enterprise schema is a strict superset of
  Simple. No forked tables; no Simple-only fields that vanish in
  Enterprise. Any divergence breaks the upgrade path and risks the
  "third hybrid mode" failure pattern. **What's needed:** a written
  test that the Enterprise migration set is purely additive, and a
  CI gate that fails if a Simple-mode field is added that's
  incompatible with an Enterprise schema.
- ~~**JobEvent → JobUpdate rename** — bundled with the v0.2 #2 model
  shift, or standalone?~~ **RESOLVED: bundled.** Shipped as part of
  ADR-0001 part 1; `JobEvent` no longer exists in the codebase.
- **Lead aspirational responsibilities** — §5 says Lead *"holds
  canonical config, audit aggregation, license state"*; today
  election picks a Lead but nothing differentiates Lead behaviour.
  What lands in v0.2 (config sync? audit aggregation?) vs stays
  deferred?

## 13. Certification roadmap

| Phase | Target | Cost ballpark | Notes |
| --- | --- | --- | --- |
| Pre-launch | NIST 800-88 R2 + IEEE 2883-2022 conformance self-attestation; published cert spec | Engineering only | Sets the floor everyone else has to beat |
| Year 1 | **ADISA Assurance Level 5** at NIST + IEEE 2883 product claims | ~£30–60k | Matches BitRaser/Certus; minimum for serious procurement |
| Year 1–2 | **Common Criteria EAL 2 / EAL 2+** | ~$150–400k, 9–15 months | Required for most defense/gov |
| Year 2 | **R2v3 / e-Stewards vendor recognition** | Modest | ITAD-side lever |
| Year 2 | **NATO NIAPC** listing | Modest | Defense procurement lever |
| Year 2+ | Government-specific paths (FedRAMP if cloud, etc.) | Variable | Per opportunity |

## 14. Glossary (one-line refresher)

- **R2 / Rev. 2** — NIST SP 800-88 Revision 2.
- **2883** — IEEE 2883-2022, the technical standard NIST defers to.
- **Clear / Purge / Destroy** — R2 sanitization categories.
- **Sanitize** (capitalized as a method name) — NVMe Admin command 0x84.
- **Crypto Erase / CE** — destruction of the encryption key on an SED;
  qualifies as Purge under R2 only with provenance.
- **HPA / DCO** — Host Protected Area / Device Configuration Overlay;
  ATA features that hide sectors.
- **SED** — Self-Encrypting Drive (e.g. TCG Opal).
- **Validate** (capital V, R2-specific) — programmatic, per-media-class
  assurance that a method works on that class.
- **ADISA / NIAP / Common Criteria** — third-party certifications used
  in procurement.
- **R2v3** — Responsible Recycling Standard v3 (ITAD operator
  certification; different "R2" than NIST).
- **mDNS / DNS-SD** — multicast DNS service discovery; how stations
  find each other on a LAN.
- **Lead** — the elected canonical station on a LAN.
- **Sales lead** — a person who submitted the marketing site's
  early-access or contact form: one `leads` row per email address in
  `wipe-hub`, with every submission and its consent text kept. Never the
  elected **Lead** station; code identifiers stay `lead`, prose says
  "sales lead".
- **Hub** — station-side multi-LAN / multi-site coordinator (deferred).
  Distinct from the vendor-side `wipe-hub` (ADR-0007), which is the
  control plane, not a station coordinator.
- **Job** — the outcome-bearing unit of processing one Asset to a
  terminal disposition (Erased / Destroyed / Quarantined / Aborted);
  composes one or more typed events; signs one Certificate.
- **ErasureEvent** — one attempted wipe within a Job; multiple
  possible per Job (retries, method fallback). This is what the v0.1
  code called `Job`.
- **JobActivity** — the sum type a Job composes:
  Diagnostic | HealthCheck | Erasure | Verification | Destruction.
- **JobUpdate** — the low-level streamed record of a running event
  (state change, progress, command, warning). Renamed from `JobEvent`
  in v0.2; `JobEvent` no longer exists.
- **PendingCoSign** — non-terminal Job state on the Destroy path:
  destruction evidence captured, cert generated, awaiting supervisor
  co-sign on the DestructionManifest.
- **DestructionManifest** — auditor-facing grouping of N PendingCoSign
  Jobs signed off by one supervisor action; on co-sign each member
  Job's cert gains a second, independently verifiable signature and
  the Job becomes Destroyed. **Not** a Batch.
- **AssetDisposition** — the resolved terminal outcome
  (Erased / Destroyed / Quarantined) stated explicitly on the cert.
- **Enclosure / Bank / Bay** — the physical bench hierarchy: an
  Enclosure (chassis, dock, carrier) holds one or more Banks; a Bank is
  a grid of Bays; a Bay holds at most one Device. See ADR-0002.
- **BayBinding** — how a Bay resolves to a Device (SES slot, path,
  serial, WWN, explicit id, or unbound).
- **BayTopology** — the station's declared physical layout; served over
  the API so remote tablets render the station's hardware.
- **Bay map** — the on-screen vector rendering of a BayTopology with
  live per-Bay status.
- **Identify mode** — bench-setup flow that learns bay↔drive bindings by
  watching hot-plug: insert a drive, click the bay it went into, a path
  binding is written.
- **Asset** — a specific device-as-customer-property; persists across
  Jobs; distinct from `Device` (hardware metadata).
- **WorkOrder** — the shared ERP-issued id under which a Customer's
  devices flow through logistics → intake → erasure; Wipestation
  references it by id, stores only the erasure-relevant slice.
- **Contract** — long-standing customer agreement, scoped narrowly
  to data-bearing-device erasure terms (not the broader ITAD
  contract); carries default SanitizationProfile.
- **Batch** — ad-hoc operator-UX selection of devices processed with
  shared settings; **not** a WorkOrder.
- **Control plane** — the optional server-side service (`wipe-hub`)
  behind both admin consoles. Optional by design: an air-gapped station
  must reach full product value with the control plane unreachable
  forever.
- **Vendor console** — our side of the control plane: licence issuance,
  entitlements, revocation, cross-tenant support and metering.
- **Fleet console** — the customer's side: their stations, their
  certificates, their seats, their usage.
- **Site licence** — a licence scoped to a site rather than a machine.
  Seat counts are *detectable*, not enforceable, offline (ROADMAP §4.3).
- **Seat** — one station's share of a site licence. In the hub, one
  **Registered station**'s claim on an **Entitlement**'s seat count,
  machine or site scope alike.
- **Registered station** — a station's instance signing key recorded by
  an owner on the portal's Stations page against one of the
  organization's current **Entitlement**s (`stations` table, licence-delivery
  story 1). While not released it holds one of that entitlement's seats,
  and the hub has signed it a **Licence** file the station installs
  offline. Releasing it frees the seat and marks the licence
  `superseded` in the hub only: the file keeps verifying on the station
  until its own `not_after`. When a renewal moves its entitlement's
  `term_end` past its licence's `not_after`, the station shows **Renewal
  available** (`renewal_available`) until an owner reissues it a renewed
  **Licence** to the new term end (licence-delivery story 2); that too
  marks the old licence `superseded` (reason `renewed`) in the hub only,
  keeping the station's seat and registration. Not an mDNS **Station**
  peer, which the hub never sees.
- **Licence** — a vendor-signed `wipe_license::LicenseCertificate` file
  binding one instance key to a customer organization and its
  entitlements (ADR-0005), stored byte for byte by the hub (`licences`
  table). Either a station licence, signed for a **Registered station**
  from its **Entitlement**, or a vendor-issued licence, signed by a vendor
  admin for a trial, evaluation or goodwill grant outside any entitlement
  and seat, with a mandatory audited reason (licence-delivery story 3).
  Status `active`, `superseded` (released or renewed) or `revoked` (by a
  vendor admin, terminal, with a vendor-internal reason the customer never
  sees). Supersession and revocation are recorded in the hub only: the
  file keeps verifying offline until its own `not_after`. Every licence the
  hub signs carries its organization's **Lapse policy** at signing time.
- **Lapse policy** — what a customer organization's stations do once a
  **Licence** lapses (expired, revoked as the station knows it, or clock
  rolled back), ADR-0012: `evaluation` (the default: keep erasing, mark
  certificates `evaluation`) or `stop` (refuse to start new jobs; a running
  job always finishes and is certified; never triggered by an unreachable
  hub). An organization owner sets it on the portal's Stations page; it is
  signed into the licence body (`wipe_license::LapsePolicy`, omitted when
  `evaluation`), so it applies only to licence files issued afterwards and a
  station shows **Updated licence available** until it is reissued
  (licence-delivery story 4). The hub only writes it; stations enforce it
  (`licence-visibility` story 5).
- **Activation** — installing a licence on a station, by file offline or
  by activation code against the control plane.
- **Revocation list** — signed, time-windowed list of invalidated
  licences, pulled when a station is online. A station that never
  connects can never learn that its licence was revoked.
- **Portal (`apps/portal`)** — the customer and vendor-admin web app
  served by `wipe-hub` from its API's origin (ADR-0006 §1); home of the
  vendor and fleet consoles' customer-facing UI. Not the operator's bench
  UI (`apps/desktop`) and not the verification portal.
- **Organization** — the hub's customer tenant (ADR-0007 §3): every hub
  user belongs to exactly one, and every customer-side read is scoped to
  the signed-in user's organization in the data layer. The one
  non-customer organization is the **Vendor organization**, which has no
  owner, only `vendor_admin` users managed by the CLI. Distinct from the
  Enterprise-mode **Customer** on a station.
- **Hub user role** — `owner` / `member` (customer side) or `vendor_admin`
  (our staff, on the vendor organization). Sign-up creates an organization
  and its `owner`. An owner renames the organization, invites people,
  changes members between `owner` and `member` and removes them; a member
  sees the member list only. A customer organization always keeps at least
  one owner; the vendor organization has no owner, only `vendor_admin`
  users, managed by the CLI. A `vendor_admin` is created only by `wipe-hub admin create`, never
  through the web, and counts as one only on the vendor organization with
  a verified email. Not an operator role at the bench.
- **Vendor organization** — the one hub organization flagged `is_vendor`
  (`Vendor staff`), holding our staff's `vendor_admin` users. Created by
  the first `wipe-hub admin create`; never created or joined by sign-up
  or invitation, and never shown in the admin area.
- **Admin area** — the portal's `/admin` pages and the hub's
  `/api/admin/*` routes, where vendor admins search customer organizations
  and users and view one organization (read-only), and list, view, work
  (status, notes), export and erase **sales leads**, create, edit,
  publish and retire **offerings**, and create, publish and withdraw
  **releases** and see who downloaded each. The only cross-organization reads in
  the hub; every search, view, export and change is audited, with ids only
  for anything holding personal data (offering rows, which hold none, may
  carry offering values).
- **Offering** — something the vendor sells (ADR-0007 §4): name,
  description, price (integer minor units + 3-letter currency), billing
  interval (`one_off` or `annual`) and grants (licence scope `machine` or
  `site`; seats, the number of stations the buyer may register for licences
  under it (ADR-0007 §5): one machine licence per station for `machine`
  scope, stations at the site for `site` scope; an erasure quota or unlimited, `wipe_license::Feature`
  entitlements, term in months; an annual offering's term is always 12).
  Lifecycle `draft` → `published` → `retired`, nothing else; a published
  offering's price, interval and grants are frozen (only its name and
  description may change), a retired one cannot change, and none is ever
  deleted. Managed only by vendor admins in the admin area. Published ones
  are public: anyone can read them through `GET /api/offerings`, and the
  marketing site's `/pricing/` lists them when built with
  `PRICING_HUB_ORIGIN`; customers buy one through checkout, which creates an
  **Order**.
  Not a station **Licence** (an offering is what a purchase will grant one
  from).
- **Order** — one customer organization's purchase of one offering
  through a **Payment provider**: it snapshots the offering's name, price
  and grants when created, so retiring the offering never changes it.
  `pending` → `paid` or `pending` → `expired`, and `paid` → `refunded`
  after a full refund; `expired` and `refunded` are terminal. It becomes
  `paid` only when the hub applies a provider event whose signature it has
  verified, never because the browser came back from checkout, and a paid
  order grants exactly one **Entitlement**. Every status change is
  audited. Seen by every role of the organization; only an owner with a
  verified email starts one.
- **Entitlement** — what a customer organization may license because it
  paid for an **Order** (CAP-4): the order's grants (scope, seats, quota,
  features) for a term from the payment to `term_end`. Created only by the
  order's paid transition, one per order. An annual offering's entitlement
  renews: each paid renewal invoice extends the term by 12 months, until
  the owner cancels in Stripe's billing portal (`renews` false; the term
  runs to its end) or Stripe ends the subscription (`cancelled`). A full
  refund marks it `refunded`, which withdraws its seats. Its seats count
  towards the organization's available licence seats while it is
  `current` (not refunded, term not ended). `current`, not `status =
  active`, is what makes seats available: a `cancelled` entitlement keeps
  its seats until `term_end`, so `licence-delivery` gates on `current`.
  Never revokes or shortens an
  issued **Licence**. Not the `wipe_license::Feature` entitlements inside a
  licence (ADR-0005), which an offering grants and an entitlement carries.
- **Release** — one version of the station software the vendor
  distributes (ADR-0007 §5, CAP-1): a SemVer 2.0.0 version (no build
  metadata), release notes
  and a channel (`stable` or `preview`), with one **Artifact** per
  platform. Lifecycle `draft` → `published` → `withdrawn`, plus `draft` →
  `withdrawn`; `withdrawn` is terminal and nothing is ever deleted. Files
  are added and removed only while it is a draft, publishing needs at
  least one, and its version, notes and channel never change. A version
  names one set of files forever: it is never reused, even after
  withdrawal. Created by vendor admins in the admin area or in one step by
  `wipe-hub release publish`. Not a **Licence** and not an **Offering**:
  what a customer downloads, not what they are allowed to run.
- **Artifact** — one file of a **Release** for one platform (a label such
  as `linux-x86_64`), with its file name, size and the SHA-256 the hub
  computed while storing it (never one a client supplied). Its bytes live
  in the hub's `ArtifactStore` (a local directory, `--artifact-dir`, for
  now) under the artifact's id. Not an evidence artifact on a station
  certificate.
- **Download link** — a short-lived signed URL to one **Artifact** of a
  published **Release** (`/api/downloads/<artifact>/file?user=…&expires=…&sig=…`),
  issued by the hub to a signed-in user of an entitled organization (one
  with a current **Entitlement**) and good for five minutes. It is the
  credential on its own (no session needed, so it works from `curl` on a
  bench machine); fetching it re-checks the user and their organization's
  entitlement and records a release download, as that user, in an
  append-only table and the audit log. A link stays usable until it
  expires even after its user signs out everywhere or resets their password.
  It stops working early when its user is removed from the team, their
  organization's entitlement lapses, the release is withdrawn, or the hub
  restarts (the signing key is per process); while the stored file is
  missing or the store fails, fetching it answers 503 and records nothing.
  Not a **Licence** download and not the emailed single-use links of sign-in.
- **Payment provider** — the hub's seam to a payment service
  (`payments::PaymentProvider`, ADR-0007 §4): starts a hosted checkout,
  opens the billing page (cards and payment history) and verifies signed events.
  `Stripe` is the real one (Stripe Checkout, the Stripe billing portal,
  `Stripe-Signature`); `Fake` is in-process with public signing, for tests
  and local use only (it refuses a non-local hub). Not to be confused with
  the station's certificate signing.
- **Payment event** — one signed notification from the payment provider
  (Stripe's `evt_…`, or a Fake event), received at
  `POST /api/payments/webhook` or produced by the Fake test page. The hub
  verifies its signature, stores it once by provider and event id
  (metadata and the payload's SHA-256 only, never the payload) and applies
  it to its **Order** or the order's **Entitlement** in the same
  transaction, recording the outcome (`applied`, `unchanged`, `ignored`,
  `unmatched` or `mismatch`). Besides a completed or expired checkout, an
  event may be a paid renewal, a renewal change, a subscription end or a
  full refund. A replayed event changes nothing. The only way an order or
  entitlement changes.
- **Billing customer** — a customer organization's own customer record at
  the **Payment provider** (Stripe's `cus_…`), kept in `billing_customers`,
  one per organization and provider. The hub never creates one itself: it
  learns it from the organization's first paid **Payment event** and
  replaces it when a later paid event names a different one (for example
  after the vendor deleted the old one in Stripe). Checkout reuses it, and
  the portal's Billing opens the provider's billing portal for it; until
  there is one, Billing answers 409. Not a hub user or an organization.
- **Invitation** — an owner's emailed, single-use offer
  (`{public_base_url}/accept-invite?token=…`, 7 days) for one address to
  join their organization as `owner` or `member`. Accepting creates that
  person's hub user, with a verified email, in the inviting organization;
  inviting the same address again replaces the pending one.
- **Portal session** — a server-side row in the hub's `sessions` table,
  held by the browser as the `wipehub_session` cookie (only its SHA-256 is
  stored). Unrelated to the station's session-scoped operator identity
  (§8).
- **CSRF token** — the hub's double-submit token: 32 random bytes held in
  the `wipehub_csrf` cookie and echoed by the portal in the
  `X-CSRF-Token` header on every state-changing `/api` request. Not tied
  to a portal session, so sign-in and sign-up are covered too.
- **Sign-in throttle** — an attempt counter in the hub's `auth_throttle`
  table, keyed `email:<address>` or `ip:<peer>` (an IPv6 peer by its
  /64). Each sign-in is charged before its password is checked and
  refunded if it succeeds; when failures reach the limit within a fixed
  window, password sign-in for that key is locked. A lockout is a
  throttle in its locked state, not a disabled account.
- **Magic link** — an emailed, single-use link
  (`{public_base_url}/magic-link?token=…`, 15 min) that signs a portal
  user in without a password, once they press the button on the page.
  Following it proves control of the mailbox: it verifies the email and
  clears that email's sign-in lock. When it is what verifies the email,
  it also ends the user's other sessions and makes the password unusable
  until a reset, so whoever pre-registered the address loses access.
- **Password reset link** — an emailed, single-use link
  (`/reset-password?token=…`, 1 h) for choosing a new portal password.
  Completing a reset ends every portal session of the user and does not
  sign them in.
- **Email-send throttle** — the hub's limit on emails it sends, in the
  same `auth_throttle` table as the sign-in throttle but keyed
  `mail:to:<address>` and `mail:ip:<peer>`: 5 per address and 20 per IP
  per hour across sign-up, resend verification, magic link and reset,
  counted whether or not an account exists.
- **Verification portal / playground** — public, client-side certificate
  verification. Runs the same code path as offline `verify-cert`, with
  no network call and no lookup against us.
- **Drift gate** — a CI check that fails when a hand-written artifact
  disagrees with generated truth. Used for the enclosure catalog today
  (ADR-0004) and for the whole documentation site (ROADMAP W-R11).
- **Capability status** — `shipped` / `seam` / `planned`, declared on
  every documentation page and checked against the code. The
  honest-default rule (ADR-0002) applied to prose.
- **Simple / Enterprise mode** — two product SKUs sharing one binary.
  Enterprise adds Customer/Contract/WorkOrder/Asset entities; Simple
  is Jobs + Certs + freeform `asset_tag`/`ticket_ref`. Enterprise
  schema is a strict superset.

---

## How to keep this document honest

- **Grill it.** Run `/grill-with-docs` whenever a non-trivial change
  is in scope. The skill will ask hard questions, walk the decision
  tree, and update sections inline as answers crystallize.
- **ADRs.** When a decision is load-bearing, write it as an
  Architecture Decision Record in `docs/adr/`. Reference the ADR
  number from this file.
- **No silent drift.** If you find code that contradicts a claim
  here, fix one of the two. Don't leave both standing.
- **Glossary discipline.** New domain words land in §14 before they
  go into code, certs, or customer-facing copy.
- **Surfaces.** Product surfaces that do not exist yet — the website,
  the admin consoles, the operator portal's remaining gaps, the
  licensing tiers — are specified in [docs/ROADMAP.md](docs/ROADMAP.md)
  with stable item numbers (**W**/**A**/**O**/**L**). This file stays
  the vision; that file is the backlog. When a roadmap item ships, it
  gets struck through there and recorded in §10 here.
