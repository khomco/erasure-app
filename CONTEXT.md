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

| Surface | Ref | State |
| --- | --- | --- |
| Outer-Job composition — `Job` → `ErasureEvent`, new outer `Job` + `JobState`, `JobActivity` sum type, `JobEvent` → `JobUpdate` | §11 #2 / ADR-0001 | ✅ |
| Destroy path — `DestructionEvent`, `PendingCoSign`, `DestructionManifest`, supervisor co-sign producing a second independent signature | ADR-0001 | ✅ |
| Certificate schema v2 — carries the `activities` chain and an explicit `AssetDisposition`; v1 certs remain valid as single-ErasureEvent Jobs | ADR-0001 | ✅ |
| REST: `POST /api/jobs/:id/escalate-to-destroy`, `GET|POST /api/manifests`, `GET /api/manifests/:id`, `POST /api/manifests/:id/cosign` | ADR-0001 | ✅ |
| React UI: activity timeline on Job Detail + **Manifests page** (assembly + co-sign) | ADR-0001 | ✅ |
| At-a-glance bench status overlay on the Devices page | §11 #3 | ✅ |
| 28 tests across 6 crates passing | — | ✅ |
| `DiagnosticEvent` / `HealthCheckEvent` | ADR-0001 | ⛔ schema-only; runner never emits |

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
8. **License token verification** — vendor-signed tokens, embedded
   verifier public key, per-success accounting persisted.
9. **PDF/A-3 cert wrapping** — JSON-LD inside, human-readable PDF
   outside, single attestation artifact.
10. **Configurable bay topology + bay map** *(in progress)* — a station
    declares its physical Enclosures/Banks/Bays in config; the server
    resolves each Bay to a Device and the UI renders a vector bay map
    with live per-Bay status, so the operator can map screen→hardware
    at a glance instead of matching serials. Extends #3, which gave
    per-device status but in device-enumeration order — no relation to
    where the drive physically is. See ADR-0002.
11. **Diagnostic / HealthCheck runtime** — the remainder of ADR-0001.
    `DiagnosticEvent` and `HealthCheckEvent` shipped as schema-only
    types and `JobActivity` variants; the runner never emits them and
    `HealthCheckEvent.attributes` is still an untyped
    `serde_json::Value`. Blocked on the §12 question of whether they
    run always or only when a SanitizationProfile requests them, and
    on what a Critical finding should do to the Job.

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
- TS/UI tests — do we add Vitest + React Testing Library now, or
  rely on the Rust integration tests until the UI surface grows?
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
- **Hub** — multi-LAN / multi-site coordinator (deferred).
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
