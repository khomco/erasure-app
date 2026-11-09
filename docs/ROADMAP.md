# Product surfaces roadmap

**Status:** proposed (2026-08-13); status refreshed 2026-11-05. T0.1 is done;
no W/A/O/L phase has started. Per-item status, readiness and blockers are in
`.scratch/STATUS.md` (from the 2026-11-05 audit); the decisions that block
items are in `.scratch/DECISIONS.md`.

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

**One decided exception — the customer's choice, never the vendor's
([ADR-0012](adr/0012-opt-in-hard-stop-on-licence-lapse.md)):** an
organization may opt in to a `stop` lapse policy. The policy is carried inside
its signed licence, and its stations then refuse to *start* new jobs once that
licence has expired, been revoked or had its clock rolled back. Even then:
- a job already running always finishes;
- an unreachable hub or licence server never triggers it;
- without the opt-in, everything above holds unchanged.

The hub only writes the policy into the licences it signs (licence-delivery
story 4, shipped 2026-11-08); nothing in the hub enforces it or can stop a
station erasing. Station enforcement is `licence-visibility` story 5.

### 0.3 The archive is an index, never an authority

Certificates are offline-verifiable by construction (ADR-0005; `verify-cert`
needs only the vendor root). The moment a cert's validity depends on a lookup
against our archive, we have rebuilt the competitor weakness named in CONTEXT
§1 ("verification of authenticity is vendor-dependent").

**A2/A3 build a cert archive. It is a search index and a convenience.** A cert
that our archive has never seen and a cert we have deleted are both still
valid, and the verification page must say so.

### ~~0.4 The decision history is fragmented across unmerged branches~~

**Resolved 2026-11-05 (T0.1, c9a9214).** Every feature branch is merged to
`main`; ADRs 0001–0005 are in one tree.

### 0.5 There is no CI

No CI configuration exists in the repo. Every item below that says "CI fails
when…" (W-R11, W1.1, W2.1, W2.6, L-R8) assumes infrastructure that has to be
built first. Until then such gates are tests that run with `cargo test`.

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

> **Superseded 2026-11-05 by [ADR-0006](adr/0006-web-presence-and-local-first-verification.md)** (marketing site + portal, local-first) and **[ADR-0007](adr/0007-hub-accounts-payments-persistence.md)** (the hub: accounts, Stripe, licence and software delivery), which also pull A1 and parts of A2/L1 forward. Kept below for its requirements.

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
- ~~W1.1 `sites/www` Astro scaffold, Tailwind theme~~, CI build. *Scaffold and theme
  shipped 2026-11-05 (CONTEXT §10.2); the CI build waits on §0.5.*
- ~~W1.2 Landing page: problem, the offline-verifiable cert, what we are not
  (CONTEXT §3 including "What we are not" — that section is unusually good
  marketing copy already).~~ *Shipped 2026-11-06 (CONTEXT §10.2): hero, problem, the
  offline-verifiable certificate, differentiators, "what we are not" (competitors unnamed)
  and one early-access CTA to a stub `/early-access/`. The copy awaits vendor sign-off.*
- ~~W1.3 Standards page: NIST R2 / IEEE 2883 posture, with certification
  *roadmap* clearly labelled as future (CONTEXT §13).~~ *Shipped 2026-11-06 (CONTEXT §10.2)
  as `/standards/`: the NIST SP 800-88 Rev. 2 / IEEE 2883-2022 self-attestation target, R2v3
  alignment, and a "Certification roadmap: future targets" section with one planned capability
  per target and its year. Shipped with product, segment, pricing, contact and legal pages.
  W-R4 is met; W-R5 only in part: since `offerings-and-payments` story 5 (2026-11-08) a build
  given `PRICING_HUB_ORIGIN` lists the hub's published offerings with live prices (otherwise
  `/pricing/` still says "early-access pricing on request"); the generated tier→feature matrix
  is still W3.3.*
- ~~W1.4 Capability-status component + frontmatter schema (W-R12), with the
  status of the mock backend stated on the record.~~ *Shipped 2026-11-05 as a shared
  capability list (`sites/www/src/data/capabilities.ts`) with the ADR-0006 §2 vocabulary
  `available`/`in-development`/`planned` rather than W-R12's docs-only `shipped`/`seam`/`planned`,
  checked over the built HTML on every `astro build`; `/capabilities` states the mock backend.
  No per-page frontmatter schema exists: for the marketing site the shared list plus the
  `<Capability>` component replace it. W-R12's `shipped`/`seam`/`planned` docs statuses and
  their symbol check remain open for W2 (W2.6).*
- W1.5 Docs skeleton: install, operating modes, quick start, glossary
  (CONTEXT §14 as the source).
- **Exit:** a stranger can tell what this is, what it verifiably does today,
  and what it does not.

**W2 — Documentation that is actually complete, with the drift gates**
- W2.1 `cargo xtask docs` generator + committed artifacts + CI diff gate.
- W2.2 CLI reference (generated) and HTTP API reference (generated from the
  router; a test asserts every `.route()` appears).
  *Partial (drift-tests story 1): `crates/wipe-server/tests/route_golden.rs` pins every
  station route into `docs/generated/http-routes.txt` and fails when a route has no desktop
  client call or the client calls an unrouted path. Drift-tests story 2:
  `crates/wipe-cli/tests/help_golden.rs` pins `wipestation --help` and every subcommand's
  `--help` into `docs/generated/cli/`, failing on changed, missing or stale files. The generated
  reference pages themselves are still open (need W2.1 and the docs site).*
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
  writing. *The mechanism shipped 2026-11-08 with schema v3 (destruction-attestation
  story 4): `verify` dispatches on `cert_format_version`, and committed v2
  fixtures prove v2 certificates still verify. Publishing it as a commitment is
  still open.*

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
- ~~A1.1 `wipe-hub` crate skeleton, Postgres schema, migrations, tenant model.~~
  *Shipped 2026-11-06 (CONTEXT §10.2): hub foundation, then organizations and users
  with `owner`/`member`/`vendor_admin` roles (portal-accounts story 1).*
- ~~A1.2 Licence issuance API over the ADR-0005 chain (the signing logic already
  exists and is tested — this is custody, workflow and attribution around it).~~
  *Shipped 2026-11-08 (CONTEXT §10.2): an owner registers a station against a free
  seat of a current entitlement and the hub signs its licence through
  `wipe_license`, stores the exact file for download and audits who issued and
  downloaded what (licence-delivery story 1). An owner issues
  a station a renewed licence to its entitlement's new term end after a paid
  renewal, on request from the Stations page, never inside the payment path
  (licence-delivery story 2). A vendor admin issues a licence to any customer
  organization outside entitlements and seats (trial, evaluation, goodwill) with
  a mandatory audited reason, inspects any stored or pasted licence file against
  the hub's root, and records a revocation in the hub (licence-delivery story 3).
  An owner sets the organization's lapse policy (ADR-0012), which every licence
  the hub signs carries, and issues each station an updated licence after a
  change (licence-delivery story 4).
  Revocation reaches no station (A2.3), and the root is still file-held (A1.5).*
- A1.3 Vendor console UI: customers, licences, entitlements, issue/inspect.
  *Customers part shipped 2026-11-08: search and audited views (portal-accounts story 5). Offerings part shipped 2026-11-08: vendor admins create, edit, publish and retire offerings, whose grants are what an entitlement will carry (offerings-and-payments story 1). Customers can buy them since 2026-11-08: checkout creates an order that only a verified payment-provider event marks paid, tested end to end with the in-process Fake provider (offerings-and-payments story 2). Since 2026-11-08 they can pay through Stripe: Stripe Checkout, the Stripe billing portal, and a signed `POST /api/payments/webhook` whose events are stored once by id and applied idempotently (offerings-and-payments story 3). Stripe has been exercised only against a local mock of its API and hand-built event fixtures, never a real account, and live payments wait on the sales-tax/VAT decision (DECISIONS D14). Since 2026-11-08 a paid order grants the organization one entitlement (seats, scope, quota, features, term), listed with its available licence seats at `/seats`; annual offerings are Stripe subscriptions whose paid renewals extend the term by a year, an owner cancels in Stripe's billing portal, and a full refund withdraws the seats, none of it touching an issued licence (offerings-and-payments story 4; renewals likewise exercised only against fixtures and the Fake provider's test actions). Issuing licences from entitlements is `licence-delivery`. Licences part shipped 2026-11-08: an organization's page lists every licence it holds and issues a vendor licence; a licence page shows the decoded file, its verification against the hub's root, a download and revocation; `/admin/licences/inspect` decodes and verifies a pasted file (licence-delivery story 3). Releases part shipped 2026-11-08: vendor admins create draft releases (SemVer version, notes, `stable`/`preview` channel), upload one file per platform whose SHA-256 the hub computes while storing it behind an `ArtifactStore` (a local directory for now), publish and withdraw them, and `wipe-hub release publish` does the same in one step from the command line (software-downloads story 1). ~~Customer downloads~~ shipped 2026-11-08: any user of an organization with a current entitlement lists the published releases at `/downloads` with each file's SHA-256 and downloads a file through a five-minute signed link; every download is recorded (user, organization, file, time) and audited, and vendor admins see each release's downloads (software-downloads story 2). Still open: an admin view of an organization's orders and entitlements.*
- A1.4 Vendor action audit log.
- A1.5 Key custody decision and implementation — where the vendor root lives
  (HSM/KMS vs file), because a leaked root invalidates the entire trust model.
  **Do not ship A1 with the root on a laptop.**
- **Exit:** a non-engineer can issue a correct licence, and we can prove who
  issued what.

**A2 — Activation and reconciliation online**
- A2.1 Activation endpoint + station-side wiring. *Decided 2026-11-08 by
  [ADR-0008](adr/0008-station-enrollment-and-licence-fetch.md): enrollment by
  device authorization approved by an organization owner, and signed licence
  fetch; built by `station-enrollment`.*
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
is operator-heavy. Devices, Jobs, Job detail, Certificates, Fleet, Destruction runs,
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
  `auditor`). ADR-0011 replaced the supervisor co-sign with an optional
  concurrence on a destruction run (`DestructionManifest`), signed inside the
  destroyed drive's v3 certificate; the concurrence policy is set at station
  startup until roles can gate editing it, and every signer's identity is
  self-asserted until SSO (`verified`) exists.
  **Unresolved and blocking (CONTEXT §12): how does an operator authenticate at
  a PXE station with no display and no local key?**
- **O-R2 Batch work.** Operators process drives in trays, not one at a time.
  Multi-select on the bay map, start N jobs with a shared spec, watch them as a
  queue. Today every job is created individually.
- **O-R3 An exception inbox.** Failures, quarantine, record-for-destruction and
  destruction runs (completion, concurrence) exist as API and as the Destruction
  runs page, but nothing tells an operator
  *"these four drives need you"*. This is where a busy bench actually loses
  drives.
- **O-R4 Bench ergonomics.** Gloves, poor warehouse lighting, noise, distance.
  Touch targets sized for a gloved hand, a high-contrast mode, and a
  **wall-board view** legible from across a bench.
- **O-R5 Barcode/scanner input.** HID-wedge scanners are how asset tags get
  entered on a real floor. Any field that takes an asset tag must accept a
  scan without focus gymnastics. *(Addressed for the desktop UI by O3.3.)*
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
- ~~O1.4 Batch selection on the bay map → N jobs with a shared spec.~~ *Shipped
  2026-11-09 (bench-throughput-inbox story 1; CONTEXT §8, §10.2): select mode on the
  Devices bay map ticks idle bays; one shared classification, intent and ticket, a
  per-drive asset tag, one Job created and started per drive through the existing
  routes, and a per-drive results list.*
- O1.5 Queue view with throughput and ETA. *Grouped Jobs list shipped 2026-11-09
  (bench-throughput-inbox story 4; CONTEXT §8, §10.2): Active, Queued and Finished
  sections with counts. Throughput and ETA not built.*
- **Exit:** a supervisor can prove who ran what, and an operator can start a
  tray in one gesture.

**O2 — Not losing drives**
- ~~O2.1 Exception inbox: failures, quarantine, pending destruction, runs awaiting completion or concurrence.~~
  *Shipped 2026-11-09 (bench-throughput-inbox story 2; CONTEXT §8, §10.2): an Inbox page
  (`/inbox`) and nav count derived client-side from the Jobs and runs lists, each item
  linking to where it is resolved; a run counts once for all its drives.*
- O2.2 Failure routing (CONTEXT §11 v0.3 #13) surfaced in the inbox.
- O2.3 Reboot/session recovery UX (O-R7).
- O2.4 Label + offline-verifiable QR printing.
- **Exit:** nothing needing a human sits invisible.

**O3 — The bench as a physical place**
- O3.1 Wall-board view.
- O3.2 Gloved-hand touch targets + high-contrast mode.
- ~~O3.3 Scanner input on every asset-tag field.~~ *Shipped 2026-11-09
  (bench-throughput-inbox story 3; CONTEXT §8, §10.2): a scanner's Enter in any
  desktop asset-tag field moves to the next scan field, or stays in the last one;
  it never starts, records or submits anything.*
- O3.4 Accessibility audit and fixes (keyboard, contrast, focus order).
- ~~O3.5 Licence status surface (O-R9).~~ Shipped 2026-11-08 as the bench licence banner (licence-visibility story 3).
- **Exit:** it works on a loud, badly-lit floor at arm's length.

**O4 — Enterprise workflow**
- O4.1 Work-order/asset intake (blocked on the Enterprise data model).
- O4.2 Bulk certificate export per work order.
- O4.3 PDF/A-3 certificate wrapping.

---

## 4. L — Licensing tiers, end to end

The chain is built and tested (ADR-0005): vendor
root → licence certificate → instance key → erasure certificate, Ed25519 over
canonical JSON, entitlements, offline lease with a monotonic watermark (~~held in RAM only, so it resets on
restart~~ persisted in `lease-state.json` since 2026-11-08, RAM-only where the
config directory is not writable — see `.scratch/licensing-as-built`) and
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
  means. Stance (ratified by ADR-0012, 2026-11-08): unlimited erasures, unlimited time, no fleet/hub
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
  by file, forever. The code path is ADR-0008: the bench shows a code and an
  owner approves it in the portal (`station-enrollment`).
- **L-R5 Expiry and renewal.** Warn at 30, 7 and 1 days. At expiry, the station keeps
  erasing and drops to `evaluation` certificates, with an unmissable banner.
  The exception is a licence carrying the customer's opt-in `stop` lapse policy
  (ADR-0012). Its station then refuses to start new jobs, but never interrupts a
  running one. Since 2026-11-08 the hub signs the policy into every licence it
  issues (licence-delivery story 4); stations enforce it from `licence-visibility`
  story 5.
  Annual offerings renew automatically as Stripe subscriptions (D15). Since
  2026-11-08 an owner issues each renewed station a renewed licence file from
  the portal and installs it by hand (licence-delivery story 2). A connected
  station fetches its renewed licence by itself under ADR-0008
  (`station-enrollment` story 5).
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
- ~~L1.1 Decide and document the evaluation stance (L-R1).~~ Decided by ADR-0012 (2026-11-08).
- L1.2 Entitlement matrix generator + CI coverage test (L-R8).
- L1.3 Activation UX in the operator portal (file-based, offline).
  *Hub side shipped 2026-11-08: the customer portal's Stations page issues the
  licence file and shows the `wipestation serve --license … --vendor-root-b64 …`
  line (licence-delivery story 1). Station side: `POST /api/license` installs it
  without a restart, checked against the `--vendor-root-b64` roots, held in memory
  until restart (licence-visibility story 4). The bench UI still has no activation
  screen, and a licence installed at runtime is not persisted.*
- ~~L1.4 Licence status surface at the bench (O-R9 / O3.5).~~ Shipped 2026-11-08: a persistent banner above every bench page (licence-visibility story 3).
- L1.5 Expiry warnings and the drop-to-evaluation transition (L-R5).
  *Expiry warnings shipped 2026-11-08 (licence-visibility story 3); the `stop` refusal at lapse is licence-visibility story 5.*
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
| ~~**T0.1** Merge outstanding feature branches to `main`~~ | **Done 2026-11-05** (c9a9214) |
| **T0.2** ADR 0006 (site architecture + drift gates) | W2 cannot start without the gate contract |
| **T0.3** ADR 0007 (control-plane persistence + tenancy) | A1 is a schema decision first |
| **T0.4** ADR 0009 (operator identity, auth, RBAC, PXE first boot) | O1 and every attribution claim |
| ~~**T0.5** Decide the evaluation stance (L1.1)~~ | **Decided 2026-11-08 (ADR-0012).** The ADR-0005 §5 default is ratified: never time-bombed, certificates marked `evaluation`. It adds a customer opt-in `stop` lapse policy. |

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

0006 and 0007 were written on 2026-11-05 in a different shape from the rows
below (see the ADRs). 0008 was written on 2026-11-08 for enrollment and licence
fetch only; its other scope below is still open. 0009–0010 are still unwritten. ADR-0011 (destruction by
operator attestation) took the next free number. Each remaining one is
load-bearing enough that building without it would bake in a decision nobody
made deliberately.

| # | Decision | Blocks |
| --- | --- | --- |
| **0006** | Documentation + marketing site architecture: one in-repo Astro site, content layout, the generated-artifact drift-gate contract, the WASM verification target | W2, W3.2 |
| **0007** | Control-plane persistence and tenancy: Postgres, multi-tenant with a vendor supertenant, and its relationship to the station-side store (ADR-0003) and the proposed Enterprise SQLite schema | A1 |
| **0008** | Station ↔ hub protocol and reconciliation contract: REST vs gRPC, what a station sends, what it does when the hub is unreachable forever. *Partly written 2026-11-08: enrollment and licence fetch (REST, signed by the instance key). Usage reporting and revocation distribution remain.* | A2 |
| **0009** | Operator identity, authentication and RBAC — including PXE first-boot authentication, an open CONTEXT §12 question with no answer | O1 |
| **0010** | Licence scopes and seat model: site semantics, the honest limits of offline seat counting, revocation distribution, expiry behaviour | L2 |

Numbering continues from ADR-0005.

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
