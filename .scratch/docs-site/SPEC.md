---
id: SPEC-docs-site
companions: [../../docs/adr/0006-web-presence-and-local-first-verification.md, ../../docs/adr/0005-licensing-and-attestation.md, ../../docs/adr/0008-station-enrollment-and-licence-fetch.md, ../../docs/adr/0013-licensing-controls-every-erasure.md, ../../docs/ROADMAP.md, ../../CONTEXT.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# Documentation an integrator can follow from zero to a verified certificate

## Why

ROADMAP W1.5 and W2 (W-R7 to W-R13) have no documentation site yet. A buyer or integrator cannot learn how to install a station, set up a bench, run an erasure, read a certificate, verify it, or license a station without asking us. The generated goldens from `drift-tests` (CLI help, HTTP routes, licence features) already exist and were built to feed this. The W2 exit is: "an integrator can go from zero to a verified certificate without asking us anything, and CI fails if the code moves out from under the docs".

Decisions taken by default, recorded here:
- **Docs live in the marketing site** under `/docs/`, as ADR-0006 "Consequences" planned. They are an Astro content collection with a hand-built layout, not Starlight: the site ships no JavaScript and Starlight would add some. Search (W4.2) and per-release versions (W4.1) are out of scope.
- **No `cargo xtask docs`.** The committed goldens under `docs/generated/` *are* the generated artifacts (W2.1). The site reads them at build time and never copies them. One new golden, for example certificates, is added the same way.
- **W-R12 uses the site's existing capability vocabulary** (`available`/`in-development`/`planned` in `capabilities.ts`), as ROADMAP W1.4 already noted. W2.6 becomes the drift checks in CAP-1.
- **The glossary is curated, not dumped.** CONTEXT §14 holds internal notes. The docs glossary is customer-facing text in `src/data/glossary.ts`, and a test requires each term to exist in CONTEXT §14 under the same name.
- **Every page says what is simulated.** Until a real backend ships (D12), guides describe the simulated backend as such; the site check enforces this.

## Capabilities

- **CAP-1 The docs framework and its drift gates**
  - **intent:** Docs pages share a layout and navigation, and the build fails when docs mention a command, flag or route the product does not have.
  - **success:**
    - Content collection `sites/www/src/content/docs/`. Frontmatter (`title`, `description`, `order`, `section`, `capabilities: string[]`) is validated by a schema, and an unknown capability id fails the build.
    - `/docs/` index plus `/docs/<slug>/` pages share a `Docs.astro` layout with a section sidebar, previous/next links and an "edit history" line giving the page's source path. It uses no `<script>`.
    - The existing site check runs over docs pages unchanged. The honest-claims rules apply to docs prose.
    - New build-time drift checks (`src/lib/docs-check.ts`, unit-tested on fixtures and run in `astro:build:done`) fail the build when:
      - a docs page mentions `wipestation <sub>` or a `--flag` that does not appear in `docs/generated/cli/*.txt`;
      - a docs page mentions `METHOD /api/…` or `/api/…` that does not match a line of `docs/generated/http-routes.txt` (path parameters match `:id` or `{id}`);
      - a docs page names a licence feature not in `docs/generated/entitlements.txt`.
    - Header and footer link `/docs/`. The sitemap includes docs pages.
    - Tests: schema refusal of an unknown capability; each drift rule with a passing and a failing fixture; the docs layout renders with one `h1`, title and description.
- **CAP-2 Generated reference pages**
  - **intent:** CLI, HTTP API and licence-feature references are read from the code's goldens, so they cannot drift.
  - **success:**
    - `/docs/reference/cli/` renders every `docs/generated/cli/*.txt` (command tree in order, each help text verbatim in a `<pre>`).
    - `/docs/reference/http-api/` lists every route from `http-routes.txt` grouped by area, each with a one-line description from `src/data/http-api.ts`. A www test fails when a route has no description or a description names a route that no longer exists.
    - `/docs/reference/licence-features/` renders `entitlements.txt`.
    - New golden `docs/generated/examples/` holds:
      - a licensed erased certificate;
      - an evaluation erased certificate;
      - a destroyed certificate with a witness;
      - the vendor root and station public keys that verify them.

      They are written by a `wipe-cli` test from fixed seed keys and fixed timestamps (deterministic bytes), regenerated with `UPDATE_GOLDENS=1`, and stale or missing files fail. The test also verifies each with `verify-cert`'s code path (root-only for the licensed ones, after `cert-verification` story 1).
    - AGENTS.md's goldens paragraph lists the new golden.
- **CAP-3 Getting started**
  - **intent:** A newcomer learns what Wipestation is today, installs it and gets a first certificate.
  - **success:**
    - Pages (W1.5, W-R7):
      - **Overview**: what it does, what is simulated today, the three surfaces;
      - **Install**: download from the portal and check the SHA-256, `wipestation serve`, `--key-path`, `--static-dir`, the desktop app;
      - **Operating modes**: CONTEXT §7, with its "Today" lines kept honest;
      - **Quick start**: start a station on the simulated backend, create and run a Job, download the certificate, verify it;
      - **Glossary**: `glossary.ts`, every term tested against CONTEXT §14.
    - Every command in the quick start is exercised by a test. A `wipe-cli` or shell-script test runs the same steps against a temporary station and fails if they stop working; the plan chooses where it lives, and the page text and the test share one source list of commands.
  - **checkpoint:** the page wording is drafted by the agent, not yet owner-approved; the owner approves the finished story before it counts as done.
- **CAP-4 Operating guides**
  - **intent:** An operator or integrator can set up a bench and run, record and read erasures and destructions.
  - **success:**
    - Pages (W2.3, W2.4):
      - **Configure a bench**: the bench-setup builder, presets and the enclosure catalog (`wipestation bay-presets`), identify mode, where the configuration is stored (ADR-0003 tiers);
      - **Run an erasure**: classification, the Job, batch start, the Inbox, aborts;
      - **Record a destruction**: ADR-0011 runs, concurrence, the witness;
      - **Certificate anatomy**: every field of the generated example certificates, explained, with the JSON shown from `docs/generated/examples/` at build time.
    - The anatomy page has a www test that fails when an example certificate gains or loses a top-level or `certificate` field the page does not explain.
  - **checkpoint:** the page wording is drafted by the agent, not yet owner-approved; the owner approves the finished story before it counts as done.
- **CAP-5 Verification and licensing**
  - **intent:** An auditor can verify a certificate with only the vendor root, and a customer understands evaluation, trials, licences, credits, enrollment and renewal.
  - **success:**
    - **Verify a certificate** (W2.5): with `verify-cert` and the vendor root only, with `/verify/` (`cert-verification` story 3), what each verdict means (authentic, evaluation, chain invalid, expired at signing), and that the vendor's site is an index, never the authority (ROADMAP §0.3).
    - **Licensing and activation** (W-R13): ADR-0013 in customer terms. It covers:
      - the trial licence;
      - registering a station from the bench (ADR-0008) or installing a licence file;
      - credits and the organization pool;
      - what is checked at the start of each Job and that nothing stops mid-erasure;
      - renewal and the 30-day grace;
      - what happens at expiry.

      It uses the approved "Licensing decides at the start, never mid-erasure" wording verbatim where it applies.
    - The documented commands in both pages pass the CAP-1 drift checks, and the verify page's example commands run in a test against `docs/generated/examples/`.
  - **checkpoint:** the page wording is drafted by the agent, not yet owner-approved; the owner approves the finished story before it counts as done.

## Constraints

- Static output, no JavaScript on docs pages, no third-party origins (ADR-0006, the existing site check).
- Generated content is read from `docs/generated/` at build time and never pasted into pages (W-R11).
- Honest claims: nothing describes hardware sanitization or a certification as available. Copy that would need a badge is wrapped in `<Capability>`.
- Docs that describe behaviour change in the same commit as the code from now on (AGENTS.md). This epic adds the gates; it does not change product behaviour.

## Non-goals

- Search (W4.2), versioned docs per release (W4.1), the cert-format compatibility promise (W4.3, waits on D7).
- Buyer segment pages (W3.1, already partly shipped), comparison content (W3.4, waits on D12).
- Hub/portal operator documentation (self-hosting the hub; not decided).

## Success signal

- Someone new follows `/docs/` from Install to Verify a certificate on a laptop and ends with a certificate that `/verify/` calls authentic, without reading the repository. Renaming a CLI flag without updating the docs fails `pnpm --filter www build`.
