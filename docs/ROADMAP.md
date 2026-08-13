# Product surfaces roadmap

**Status:** proposed (2026-08-13) — requirements and sequencing only. Nothing
in this document has been built. Pause point: review before any phase starts.

Companion to [CONTEXT.md](../CONTEXT.md) §9 (pricing), §10 (shipped) and §11
(deferred). CONTEXT stays the product and domain vision; this file is the
**surface-level backlog** — four product surfaces the engine does not have yet,
broken into phases something can actually be built through.

## How to read this

Four surfaces, each with its own letter, so a task can be cited from a commit
message or an ADR and still resolve later:

| Letter | Surface | Who it is for |
| --- | --- | --- |
| **W** | Marketing + documentation website | Buyers evaluating; operators and integrators learning |
| **A** | Admin / control plane | Us (vendor console) and fleet owners (fleet console) |
| **O** | Operator portal | The person standing at the bench |
| **L** | Licensing tiers | All three, end to end |

Item numbering is stable, like CONTEXT §11: **W2.3** is website phase 2, item 3,
forever. Shipped items are struck through rather than deleted.

Phases within a surface are ordered by dependency, not ambition. The
cross-surface sequencing is in [§5](#5-sequencing).

## 0. Things that are true before any of this starts

Read these first. Three of them change what the phases below are allowed to
say, and one of them changes whether the website can launch at all.

### 0.1 The only `DeviceBackend` implementation is still the mock

CONTEXT §11 v0.2 #1. Everything downstream of the trait seam is built and
tested; the only thing that produces command evidence is
`wipe-engine-mock`, which **synthesises** the evidence the product's entire
value proposition rests on.

**This gates the marketing half of W, and nothing else.** A documentation site
can describe a real architecture honestly today. A marketing site that says
"Wipestation sanitizes NVMe drives with NIST 800-88 Rev. 2 Purge" cannot ship
before `wipe-engine-linux` runs on real hardware, because that sentence would
be false, and it would be false in exactly the way the product exists to
attack — see CONTEXT §1 ("the gap between *supports NVMe* and *honestly invokes
Sanitize*"). W1 therefore ships positioning and documentation with an explicit
capability status; the claims that require hardware are gated behind W3.4.

### 0.2 Never block a wipe on licensing

ADR-0005's stance, restated because three surfaces below could quietly erode
it. An unlicensed, expired, quota-exhausted, revoked or clock-rolled-back
station **still erases drives**; what changes is that the certificate is marked
`evaluation` and the operator is told why. Erasure is a safety-adjacent
operation on someone else's schedule — a licence server having a bad day must
never be able to stop a drive from being sanitized.

Anything in **L** or **A** that would gate erasure is out of scope by default
and needs an explicit decision to enter scope.

### 0.3 The archive is an index, never an authority

Certificates are offline-verifiable by construction (ADR-0005; `verify-cert`
needs only the vendor root). The moment a cert's validity depends on a lookup
against our archive, we have rebuilt the competitor weakness named in CONTEXT
§1 ("verification of authenticity is vendor-dependent").

**A2/A3 build a cert archive. It is a search index and a convenience.** A cert
that our archive has never seen and a cert we have deleted are both still
valid, and the verification page must say so.

### 0.4 The decision history is fragmented across unmerged branches

ADRs 0002–0005 exist only on feature branches; `main` has ADR-0001 only.
ADR-0004 is on `feat/enclosure-catalog`, ADR-0005 on
`feat/licensing-attestation`, and neither branch contains the other's ADR.

A documentation site that cites ADRs, and a drift gate that checks docs against
code, both assume one coherent tree. **Merging the outstanding feature branches
to `main` is a prerequisite for W2, not a nicety.** Tracked as T0.1.

---

## 1. W — Marketing + documentation website

One site doing two jobs. Marketing has to convert a buyer who has never heard
of us; documentation has to keep an operator or integrator unblocked at 2am.
They share a domain, a design system and a build, and nothing else.

### 1.1 Requirements

**Marketing (buyer-facing)**

- **W-R1** State what Wipestation is in one screen: sanitization with
  **offline-verifiable** certificates carrying command-level evidence.
- **W-R2** Lead with the differentiator that no competitor can copy cheaply:
  a cert an auditor verifies with a public key and no phone-home. Everything
  else is supporting material.
- **W-R3** Standards posture as fact, not badge soup: NIST SP 800-88 Rev. 2,
  IEEE 2883-2022, what conformance we claim, what we have *not* certified yet
  (CONTEXT §13 is a roadmap, not a set of achievements — the site must not
  blur those).
- **W-R4** Segment pages for the three buyers in CONTEXT §2: ITAD operators,
  enterprise IT, defense/gov. Different proof matters to each.
- **W-R5** Pricing and packaging (CONTEXT §9) with the tier→feature matrix
  generated from the `Feature` enum, not hand-written (see W-R11).
- **W-R6** Comparison content that is checkable. Every competitor claim cites a
  public source and a date. No unsourced claim about a named competitor.

**Documentation (user-facing)**

- **W-R7** Install and boot: single binary, PXE-ephemeral, `--static-dir`,
  operating modes (CONTEXT §7), and what a read-only root implies (ADR-0003).
- **W-R8** Configure a bench: the bay-topology builder, presets, the model
  catalog, identify mode (ADR-0002/0003/0004).
- **W-R9** Run a wipe end to end, read a certificate, verify a certificate —
  including verifying somebody else's, which is the differentiator in
  practice.
- **W-R10** CLI and HTTP API reference, complete and generated (W-R11).
- **W-R11** **Nothing derived from code is written by hand.** CLI reference,
  API route list, cert JSON examples, the tier→feature matrix and the
  enclosure catalog listing are generated into committed artifacts, and CI
  fails on a diff. Same pattern as
  [`docs/design/enclosure-art-pipeline.md`](design/enclosure-art-pipeline.md):
  the check is mechanical because a review step that must catch drift every
  single time eventually will not.
- **W-R12** **Every page declares a capability status** in frontmatter —
  `shipped`, `seam`, or `planned` — rendered as a visible badge. A page marked
  `shipped` must name a code symbol that exists; a test asserts it. This is
  ADR-0002's honest-default rule applied to prose: documenting a seam as though
  it were a feature is the documentation version of drawing a chassis we have
  never seen.
- **W-R13** Licensing and activation: what evaluation means, what a licence
  file is, how to install one, what happens at expiry (§4).
- **W-R14** Searchable, versioned per release, and readable with JavaScript
  disabled.

### 1.2 Architecture — proposed

**One site, in-repo, at `sites/www/`.**

- **Generator: Astro**, with Starlight for the docs collection and hand-built
  marketing routes. Rationale: it is the only common option that does both jobs
  well — mdBook cannot do marketing pages, Docusaurus imposes a React runtime
  on a brochure, and a bespoke SSG means owning a build system nobody asked us
  to own. Astro ships zero JS by default, which is not an aesthetic preference
  on a site whose central claim is "this product does not phone home".
- **In the product repo, not a separate one.** The no-silent-drift gates in
  W-R11/W-R12 only work if docs and code fail the same CI run. A docs repo that
  can go green while the product changes is the drift we are trying to prevent.
- **Content lives at `sites/www/src/content/`** — prose authored there,
  generated fragments committed under `sites/www/src/generated/` and included.
  Prose next to code sounds tidier and is not: doc pages cut across crates, and
  the ones that matter most (verifying a certificate) belong to no crate.
- **Generation via `cargo xtask docs`** (or `scripts/gen-docs.sh` if an xtask
  crate is more machinery than this needs), emitting: clap help per subcommand,
  the Axum route table, serialized cert fixtures, the entitlement matrix, the
  catalog listing.
- **Static hosting.** No server, no database, no analytics that phone home
  about our users' visitors either. The verification playground (W3.2) is
  client-side WASM so the site stays static.
- **Design tokens shared with `apps/desktop`** by copying the Tailwind theme,
  not by extracting a package yet. Extract when a second consumer proves it
  (`apps/console`, A2) — premature extraction costs more than the duplication.

**Load-bearing → ADR candidate 0006** (site architecture, in-repo placement,
and the drift-gate contract).

### 1.3 Phases

**W1 — Skeleton and honest positioning** *(no hardware claims)*
- W1.1 `sites/www` Astro scaffold, Tailwind theme, CI build.
- W1.2 Landing page: problem, the offline-verifiable cert, what we are not
  (CONTEXT §3 including "What we are not" — that section is unusually good
  marketing copy already).
- W1.3 Standards page: NIST R2 / IEEE 2883 posture, with certification
  *roadmap* clearly labelled as future (CONTEXT §13).
- W1.4 Capability-status component + frontmatter schema (W-R12), with the
  status of the mock backend stated on the record.
- W1.5 Docs skeleton: install, operating modes, quick start, glossary
  (CONTEXT §14 as the source).
- **Exit:** a stranger can tell what this is, what it verifiably does today,
  and what it does not.

**W2 — Documentation that is actually complete, with the drift gates**
- W2.1 `cargo xtask docs` generator + committed artifacts + CI diff gate.
- W2.2 CLI reference (generated) and HTTP API reference (generated from the
  router; a test asserts every `.route()` appears).
- W2.3 Bench configuration guide: builder, presets, catalog, identify mode.
- W2.4 Run-a-wipe guide and certificate anatomy, with generated cert examples.
- W2.5 Verify-a-certificate guide, including third-party verification with only
  the vendor root.
- W2.6 `shipped`-status symbol check (W-R12) wired into CI.
- **Exit:** an integrator can go from zero to a verified certificate without
  asking us anything, and CI fails if the code moves out from under the docs.

**W3 — The parts that sell** *(gated on real hardware for 3.4)*
- W3.1 Buyer segment pages (ITAD / enterprise / defense).
- W3.2 **Verification playground** — paste a certificate and a vendor root,
  verify in the browser. `wipe-cert` compiled to WASM, no network call. This is
  the differentiator made touchable, and it is nearly free once the crate
  exists. *(Load-bearing → ADR candidate 0006 covers the WASM build target.)*
- W3.3 Pricing page with the generated tier→feature matrix (depends on L1).
- W3.4 Sourced comparison content and hardware-backed sanitization claims.
  **Blocked on `wipe-engine-linux` (CONTEXT §11 v0.2 #1) — see §0.1.**
- **Exit:** the site can carry a sales conversation.

**W4 — Longevity**
- W4.1 Versioned docs per release; older versions stay published.
- W4.2 Search.
- W4.3 Cert-format compatibility page — publishes whatever stance the CONTEXT
  §12 cert-versioning question resolves to. An auditor's value proposition is
  that an old certificate stays verifiable; that promise belongs on the site in
  writing.

---

## 2. A — Admin / control plane

Two consoles, one service, two very different tenancy positions. Keeping them
straight is most of the design work.

| | **Vendor console** (us) | **Fleet console** (customer) |
| --- | --- | --- |
| Issues licences | ✅ | ❌ (allocates seats it already owns) |
| Sees all tenants | ✅ | own tenant only |
| Revokes | ✅ | ❌ |
| Cert search | across tenants, for support | own certs |
| Metering | billing truth | own usage |
| Catalog overlays | publishes | consumes, may add local |

### 2.1 Requirements

- **A-R1** Licence issuance and lifecycle: issue, inspect, renew, revoke, and
  re-issue to a replacement station. Today this is `wipestation license
  new-root|issue|inspect` — a CLI in front of a signing key. A vendor console
  is that flow with an audit trail and someone other than an engineer able to
  run it.
- **A-R2** Customer and entitlement management: a customer, their contracts,
  the entitlements each licence grants (`Quota`, `Scope`, `Feature`,
  `AllowedMethods` from `wipe-license::entitlement`). The console must not
  invent a second entitlement vocabulary — it edits the one the station
  enforces.
- **A-R3** Activation and reconciliation: the station-side seam already exists
  (`HttpLicenseClient`, ADR-0005) and does nothing. A stands it up: a station
  can activate against the control plane, report its lease watermark, and pull
  a revocation list — all optional, all degrading to offline behaviour.
- **A-R4** Fleet oversight across stations and sites: inventory, last-seen,
  version, licence status, config store tier (ADR-0003 tiers are exactly what a
  fleet owner wants to see — which of my stations cannot save their config?).
- **A-R5** Certificate and audit search, scoped by tenant, with export. **An
  index, not an authority (§0.3).**
- **A-R6** Usage and metering: per-station, per-period, reconcilable against
  the station's own counter, and never a gate (§0.2).
- **A-R7** Every vendor action is attributed and auditable. Issuing a licence
  is a financial act; revoking one can stop a customer being able to prove
  compliance.
- **A-R8** The control plane is **optional infrastructure**. Air-gapped
  customers (CONTEXT §9 Tier 1, the defense segment) must reach full product
  value with the control plane unreachable forever. Every A feature therefore
  needs an offline equivalent or an honest absence — the `ControlPlaneStore`
  precedent from ADR-0003: fail visibly, never silently.

### 2.2 Architecture — proposed

- **New crate `wipe-hub`**, already the named growth slot in
  [ARCHITECTURE.md](ARCHITECTURE.md) §"Where the code will need to grow" #3.
  Multi-tenant, with the vendor as a supertenant rather than a second service:
  the objects are the same (licence, station, certificate, tenant) and two
  services would mean two schemas drifting.
- **Persistence: PostgreSQL via `sqlx`.** This is the product's first
  server-side durable store; the station side stays as ADR-0003 defined it
  (config only, evidence never). Note the tension to resolve in the ADR:
  CONTEXT §11 v0.2 #4 proposes SQLite for the Enterprise data model on the
  *station*. Two different stores for two different jobs is defensible; it must
  be stated, not stumbled into.
- **Frontend: a second Vite app, `apps/console`**, sharing the operator app's
  theme. Not a route inside `apps/desktop` — the desktop app ships to benches
  and PXE images, and must not carry vendor console code it can never use.
- **Protocol: extend the existing REST surface** rather than introduce
  gRPC/protobuf. CONTEXT §12 lists this as open; the argument for REST is that
  stations already speak it and the hub is not on a hot path. Decide in the
  ADR.
- **Tenancy model** — row-level tenant scoping with the vendor supertenant as
  an explicit, audited escalation, not an ambient superuser.

**Load-bearing → ADR candidates 0007** (control-plane persistence and tenancy)
and **0008** (station ↔ hub protocol and the reconciliation contract).

### 2.3 Phases

**A1 — Vendor issuance, with a real audit trail**
- A1.1 `wipe-hub` crate skeleton, Postgres schema, migrations, tenant model.
- A1.2 Licence issuance API over the ADR-0005 chain (the signing logic already
  exists and is tested — this is custody, workflow and attribution around it).
- A1.3 Vendor console UI: customers, licences, entitlements, issue/inspect.
- A1.4 Vendor action audit log.
- A1.5 Key custody decision and implementation — where the vendor root lives
  (HSM/KMS vs file), because a leaked root invalidates the entire trust model.
  **Do not ship A1 with the root on a laptop.**
- **Exit:** a non-engineer can issue a correct licence, and we can prove who
  issued what.

**A2 — Activation and reconciliation online**
- A2.1 Activation endpoint + station-side wiring of `HttpLicenseClient`.
- A2.2 Lease/usage reporting from station to hub, offline-tolerant.
- A2.3 Revocation list: signed, windowed, pulled when online.
  **Honest limit to document (W2, L3): a station that never connects can never
  learn that its licence was revoked.**
- A2.4 Station inventory: last-seen, version, config-store tier, licence state.
- **Exit:** a station can be activated and reconciled without a human copying a
  file, and everything still works if it cannot reach us.

**A3 — Fleet console for customers**
- A3.1 Tenant-scoped fleet view across sites.
- A3.2 Certificate archive ingest + search + export (§0.3 banner on every
  archive view).
- A3.3 Seat allocation across a customer's stations (depends on L2).
- A3.4 Usage/metering views reconcilable with station counters.
- **Exit:** a fleet owner uses it weekly without us.

**A4 — Enterprise readiness**
- A4.1 SSO (OIDC/SAML) for console access.
- A4.2 Retention policies and immutability guarantees for the archive.
- A4.3 Public cert-verification API (CONTEXT §9 Tier 2 headline) — **must
  return the same verdict a fully offline `verify-cert` returns**, and must say
  so on its face.
- A4.4 Analytics: drive-model success rates, throughput (CONTEXT §9).

---

## 3. O — Operator portal

The most mature surface and, per the user, the one that matters most: erasure
is operator-heavy. Devices, Jobs, Job detail, Certificates, Fleet, Manifests,
Bench setup, bay map and identify mode all exist and work.

**Architecture position: this stays `apps/desktop`.** One React app, served by
`wipe-server` at one origin and wrapped by Tauri (CONTEXT §6). The requirement
that constrains everything below: **no capability may exist only in the Tauri
build.** Three frontends, one engine.

### 3.1 Requirements — what a first-class operator portal still needs

- **O-R1 Identity and RBAC.** Operator identity is `localStorage` today —
  explicitly a placeholder (CONTEXT §11 v0.2 #6/#7). Certificates carry an
  operator email that NIST R2 requires and that nothing currently proves. Needs
  real authentication and the four roles (`loader`, `operator`, `supervisor`,
  `auditor`), with supervisor co-sign already modelled by ADR-0001's
  `DestructionManifest`.
  **Unresolved and blocking (CONTEXT §12): how does an operator authenticate at
  a PXE station with no display and no local key?**
- **O-R2 Batch work.** Operators process drives in trays, not one at a time.
  Multi-select on the bay map, start N jobs with a shared spec, watch them as a
  queue. Today every job is created individually.
- **O-R3 An exception inbox.** Failures, quarantine, escalate-to-destroy and
  co-sign exist as API and as the Manifests page, but nothing tells an operator
  *"these four drives need you"*. This is where a busy bench actually loses
  drives.
- **O-R4 Bench ergonomics.** Gloves, poor warehouse lighting, noise, distance.
  Touch targets sized for a gloved hand, a high-contrast mode, and a
  **wall-board view** legible from across a bench.
- **O-R5 Barcode/scanner input.** HID-wedge scanners are how asset tags get
  entered on a real floor. Any field that takes an asset tag must accept a
  scan without focus gymnastics.
- **O-R6 Labels and QR.** Print a label for a sanitized drive. The QR must
  carry enough to verify **offline** (cert id + fingerprint), not merely a URL
  to a site that may not exist in ten years (§0.3).
- **O-R7 Recovery.** A station reboots mid-run. What does the operator see, and
  what happened to the in-flight job? PXE-ephemeral stations lose Job state on
  reboot today — that is a stated property with an unstated operator experience.
- **O-R8 Certificate delivery.** Bulk export per work order; PDF/A-3 wrapping
  (CONTEXT §11 v0.2 #9) so one artifact is both machine- and human-readable.
- **O-R9 Licensing visibility.** The operator must be able to see, without
  leaving the bench, whether this station's certificates are `evaluation` or
  licensed, and why. Ties to L1.
- **O-R10 Accessibility.** Keyboard-only operation and WCAG AA contrast — the
  bay map's status palette already has a luminance discipline (ADR-0004); the
  rest of the UI has no such audit.
- **O-R11 Work-order intake.** Scan an asset, pull its classification from a
  WorkOrder rather than asking the operator to choose (CONTEXT §8 is explicit
  that classification is not an operator decision). **Depends on the Enterprise
  data model, CONTEXT §11 v0.2 #4** — sequenced last for that reason.

### 3.2 Phases

**O1 — Identity and throughput**
- O1.1 Authentication in `wipe-server`; replace `localStorage` identity.
- O1.2 RBAC roles + per-action attribution.
- O1.3 PXE first-boot authentication (resolve CONTEXT §12; likely ADR).
- O1.4 Batch selection on the bay map → N jobs with a shared spec.
- O1.5 Queue view with throughput and ETA.
- **Exit:** a supervisor can prove who ran what, and an operator can start a
  tray in one gesture.

**O2 — Not losing drives**
- O2.1 Exception inbox: failures, quarantine, escalations, awaiting co-sign.
- O2.2 Failure routing (CONTEXT §11 v0.3 #13) surfaced in the inbox.
- O2.3 Reboot/session recovery UX (O-R7).
- O2.4 Label + offline-verifiable QR printing.
- **Exit:** nothing needing a human sits invisible.

**O3 — The bench as a physical place**
- O3.1 Wall-board view.
- O3.2 Gloved-hand touch targets + high-contrast mode.
- O3.3 Scanner input on every asset-tag field.
- O3.4 Accessibility audit and fixes (keyboard, contrast, focus order).
- O3.5 Licence status surface (O-R9).
- **Exit:** it works on a loud, badly-lit floor at arm's length.

**O4 — Enterprise workflow**
- O4.1 Work-order/asset intake (blocked on the Enterprise data model).
- O4.2 Bulk certificate export per work order.
- O4.3 PDF/A-3 certificate wrapping.

---

## 4. L — Licensing tiers, end to end

The chain is built and tested (ADR-0005, `feat/licensing-attestation`): vendor
root → licence certificate → instance key → erasure certificate, Ed25519 over
canonical JSON, entitlements, offline lease with a monotonic watermark and
clock-skew tolerance, `evaluation` marked inside the signed payload, and
`verify-cert --require-licensed`. What does *not* exist is the product around
it: three tiers a customer can buy, activate, renew and outgrow.

### 4.1 The three tiers

| Tier | Scope | What it is | Enforcement posture |
| --- | --- | --- | --- |
| **Free / evaluation** | none | Full erasure capability, certificates permanently marked `evaluation` | Nothing to enforce — the marking is inside the signature and cannot be stripped |
| **Per-machine** | `Scope::Machine` | One station, bound to its instance key and machine fingerprint | Cryptographically bound; a licence for another station refuses to install |
| **Site-wide** | `Scope::Site` *(to define)* | N stations at a site under one licence | **Detectable, not preventable — see 4.3** |

### 4.2 Requirements

- **L-R1 Evaluation is not crippled.** Decide and publish exactly what free
  means. Recommended stance: unlimited erasures, unlimited time, no fleet/hub
  features, certificates permanently and visibly `evaluation`. Rationale: §0.2
  says we never block a wipe, and a time-bombed evaluation contradicts the
  posture the product is sold on. The certificate marking is the entire
  business model — it is what a customer's auditor will not accept.
- **L-R2 Per-machine works end to end**: purchase → issue (A1) → deliver →
  install → visible licensed state (O-R9) → licensed certificates → renewal.
  Every step exists in pieces; none of them are joined.
- **L-R3 Site scope defined honestly.** A site licence must state what it is
  bound to, how many stations it covers, and what happens when that number is
  exceeded (4.3).
- **L-R4 Activation UX** in the operator portal: paste/upload a licence file,
  or enter an activation code that fetches it (A2). Must work with no network,
  by file, forever.
- **L-R5 Expiry and renewal.** Warn at N days. At expiry, the station keeps
  erasing and drops to `evaluation` certificates — with an unmissable banner.
  Never a hard stop (§0.2).
- **L-R6 Revocation** with its honest limit stated in the docs (A2.3).
- **L-R7 Upgrade paths**: machine → site, adding features mid-term, replacing a
  dead station's licence without a support call being the only route.
- **L-R8 One entitlement matrix.** Tier → `Feature` mapping generated from the
  enum and consumed by the website (W3.3), the vendor console (A1.3) and the
  station. A test asserts the matrix covers every `Feature` variant, so adding
  a feature to the enum without pricing it fails CI.
- **L-R9 Metering that reconciles.** The station's counter and the hub's view
  must be comparable, and the honor-system stance in CONTEXT §9 stays: the
  audit log is the truth-of-volume artifact.

### 4.3 The site-licence problem — flagged, not solved

**An offline station cannot count how many other stations share its licence.**
No cryptography fixes this: a site licence that installs on station A installs
on station B, and if neither is online neither can know about the other.

Three candidate positions, all with costs:

1. **LAN quorum.** Stations already discover each other by mDNS and elect a
   lead (CONTEXT §6). The lead counts activations on its LAN and flags
   over-subscription. Works on a shared LAN; blind across air-gapped islands,
   which is precisely where the defense segment lives.
2. **Reconciliation-only.** Unbounded offline; the hub detects
   over-subscription when stations report (A2.2) and it becomes a commercial
   conversation. Requires connectivity we promised not to require.
3. **Per-station activation tokens** issued by the control plane against a site
   licence's seat count. Strongest enforcement; breaks the air-gap promise
   outright.

**Recommended: (1) + (2) — detect, report, never prevent.** Over-subscription
raises a visible station banner and a fleet-console flag; it never stops an
erasure and never invalidates a certificate. This is the same posture ADR-0005
already takes on the TPM counter and clock rollback: state the limit, detect
what is detectable, do not pretend.

**Load-bearing → ADR candidate 0010** (licence scopes, seat model, revocation
distribution and expiry behaviour).

### 4.4 Phases

**L1 — Evaluation and per-machine, joined up**
- L1.1 Decide and document the evaluation stance (L-R1).
- L1.2 Entitlement matrix generator + CI coverage test (L-R8).
- L1.3 Activation UX in the operator portal (file-based, offline).
- L1.4 Licence status surface at the bench (O-R9 / O3.5).
- L1.5 Expiry warnings and the drop-to-evaluation transition (L-R5).
- **Exit:** a customer can buy one station, activate it with no network, and
  see licensed certificates — and see plainly when they stop being licensed.

**L2 — Site scope**
- L2.1 ADR 0010: scope semantics, seat model, honest limits.
- L2.2 `Scope::Site` in `wipe-license` + install/verify tests.
- L2.3 LAN seat awareness via the existing lead election.
- L2.4 Over-subscription signalling: station banner + fleet flag.
- **Exit:** a multi-station site is licensable without a licence file per bench,
  and over-subscription is visible to both sides.

**L3 — Lifecycle**
- L3.1 Renewal flow (vendor console → station, online and by file).
- L3.2 Revocation list end to end + documented limit.
- L3.3 Station replacement / re-issue.
- L3.4 Upgrade machine → site; mid-term feature changes.

**L4 — Commercial**
- L4.1 Metering reconciliation (A3.4).
- L4.2 Optional per-drive consumption pricing (CONTEXT §9 Tier 2).
- L4.3 Free-tier limits for the cloud tier, if any (CONTEXT §12 open question).

---

## 5. Sequencing

Four tiers across the surfaces. Each tier is shippable and leaves the product
in a coherent state; nothing later invalidates something earlier.

### T0 — Foundations *(blocking, small)*

| Item | Why it blocks |
| --- | --- |
| **T0.1** Merge outstanding feature branches to `main` | §0.4 — docs and drift gates assume one tree |
| **T0.2** ADR 0006 (site architecture + drift gates) | W2 cannot start without the gate contract |
| **T0.3** ADR 0007 (control-plane persistence + tenancy) | A1 is a schema decision first |
| **T0.4** ADR 0009 (operator identity, auth, RBAC, PXE first boot) | O1 and every attribution claim |
| **T0.5** Decide the evaluation stance (L1.1) | Appears on the pricing page and in the product |

### T1 — Sell it honestly

**W1, W2, A1, L1** — a public site that tells the truth, documentation with
drift gates, vendor issuance with custody, and per-machine licensing joined end
to end. Parallel-safe: W and A/L touch different trees.

**Exit:** a customer can find us, understand us, buy one station, activate it
offline, and verify a certificate we never see.

### T2 — Make it operable at scale

**O1, O2, A2, W3.1–W3.3** — operator identity and batch work, the exception
inbox, online activation/reconciliation, buyer segment pages, the verification
playground.

**Exit:** a real ITAD floor runs a shift on it without us, and a prospect can
verify a certificate in their own browser.

### T3 — Fleet value and site licensing

**A3, L2, O3** — fleet console, site scope with honest seat detection, bench
ergonomics.

**Exit:** a multi-site customer manages their own fleet, and we can sell a
site licence without lying about what it enforces.

### T4 — Assurance and enterprise

**A4, L3, L4, O4, W4** — SSO, retention, public verification API, licence
lifecycle, work-order intake, versioned docs. Aligns with CONTEXT §13's Year-1
certification targets.

### Hard dependencies

```
T0.1 ────────────────► W2 (drift gates need one tree)
T0.2 ────────────────► W2
T0.3 ────────────────► A1
T0.4 ────────────────► O1 ──────► O2, A3 (attribution), L2.3
T0.5 ────────────────► L1 ──────► W3.3 (pricing page)
A1   ────────────────► A2 ──────► A3, L3
L1   ────────────────► L2 ──────► L4
wipe-engine-linux ───► W3.4 (hardware claims)   ← CONTEXT §11 v0.2 #1
Enterprise data model► O4.1                     ← CONTEXT §11 v0.2 #4
```

## 6. ADR candidates

None of these are written. Each is load-bearing enough that building without it
would bake in a decision nobody made deliberately.

| # | Decision | Blocks |
| --- | --- | --- |
| **0006** | Documentation + marketing site architecture: one in-repo Astro site, content layout, the generated-artifact drift-gate contract, the WASM verification target | W2, W3.2 |
| **0007** | Control-plane persistence and tenancy: Postgres, multi-tenant with a vendor supertenant, and its relationship to the station-side store (ADR-0003) and the proposed Enterprise SQLite schema | A1 |
| **0008** | Station ↔ hub protocol and reconciliation contract: REST vs gRPC, what a station sends, what it does when the hub is unreachable forever | A2 |
| **0009** | Operator identity, authentication and RBAC — including PXE first-boot authentication, an open CONTEXT §12 question with no answer | O1 |
| **0010** | Licence scopes and seat model: site semantics, the honest limits of offline seat counting, revocation distribution, expiry behaviour | L2 |

Numbering continues from ADR-0005 (`feat/licensing-attestation`). If those
branches merge in a different order, renumber before writing, not after.

## 7. Non-goals for this roadmap

Stated so they are declined deliberately rather than forgotten:

- **Mobile erasure** (CONTEXT §11 v0.3 #10) — a separate product surface.
- **Bootable ISO / UEFI signing** (v0.3 #9) — packaging, not a surface.
- **TUI mode** (v0.3 #11).
- **A partner/reseller portal** — a plausible fifth surface; no demand signal
  yet, and it would be a tenancy variant of A.
- **Community/support forum, status page, blog infrastructure** — the site
  should be able to grow these; none are in the phases above.
- **Replacing the audit log with the archive** — see §0.3.
