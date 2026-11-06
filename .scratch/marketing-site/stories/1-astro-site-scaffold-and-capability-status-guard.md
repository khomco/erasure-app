---
title: 'Astro site scaffold and capability-status guard'
type: 'feature'
created: '2026-11-05'
status: 'done'
baseline_revision: '2a232bc59330b890f00328c477201c24ef911209'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '{project-root}/docs/adr/0006-web-presence-and-local-first-verification.md'
  - '{project-root}/.scratch/marketing-site/SPEC.md'
warnings: [oversized]
deferred:
  - summary: >-
      AGENTS.md opening paragraph still describes the repo as a Rust workspace plus apps/desktop, omitting sites/www and the root pnpm workspace.
    evidence: |-
      Blind Hunter finding; AGENTS.md line 3-5 unchanged by this story. Agent-context edits route to defer.
    location: >-
      AGENTS.md:3
    severity: low
---

<intent-contract>

## Intent

**Problem:** There is no marketing site, and nothing mechanical stops pre-launch copy from presenting simulated or future capabilities (hardware sanitization, certifications, compliance) as current fact — the exact failure ADR-0006 §2 and ROADMAP §0.1 forbid. Stories 2–3 (landing, product/standards/segment/pricing/legal pages) need a scaffold and a guard to build on.

**Approach:** Scaffold a static Astro + Tailwind package `www` at `sites/www` inside a new root pnpm workspace; add a shared capability list (`available` / `in-development` / `planned`), a component that renders a capability with a visible status badge, and a build-time check over the rendered HTML that fails `astro build` on any overclaim, missing meta/OG tags, or third-party script. Ship a minimal home page and a `/capabilities` page that states every status on the record.

## Boundaries & Constraints

**Always:**
- Status vocabulary is exactly `available`, `in-development`, `planned` (ADR-0006 §2) — not ROADMAP W-R12's docs-only `shipped/seam/planned`.
- No capability in the initial list is `available`: the only `DeviceBackend` is the mock and the product is pre-launch. Hardware sanitization items are `in-development` (blocked on `wipe-engine-linux`), certifications (ADISA AL5, Common Criteria EAL 2, NATO NIAPC) are `planned` (CONTEXT §13). The data file carries a comment that statuses are a vendor sign-off decision.
- The guard runs on built HTML (the surface a visitor sees), inside `astro build` via an integration's `astro:build:done` hook, so a violation fails the build (ADR-0006 "fails the site build"). The same pure function is unit-tested.
- Zero client JS by default; every page readable with JavaScript disabled. Analytics hook exists, defaults off, and renders nothing when off.
- `apps/desktop` keeps building and testing unchanged: its `pnpm-lock.yaml` stays authoritative (`sharedWorkspaceLockfile: false` style setting), its `node_modules` and scripts work from `apps/desktop` as before.
- Placeholder brand "Wipestation" and placeholder origin `https://wipestation.example` (overridable by `SITE_URL`) until the vendor decides (SPEC open question).
- Tests need no network and no browser (`pnpm --filter www test`). Lighthouse is a separate local script.

**Never:**
- No landing/product/standards/segment/pricing/contact/legal page copy (stories 2–3); home is a placeholder pointing at `/capabilities`.
- No hardware-sanitization or certification claim as current fact anywhere, including `<title>`, meta and OG tags.
- No server runtime, SSR adapter, hosting, analytics vendor, lead storage, blog, or docs pages.
- No shared design-token package; copy the font stacks from `apps/desktop/tailwind.config.js` (ROADMAP §1.2).
- No escape hatch attribute that silences the guard.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Labelled claim | `<Capability id="nvme-sanitize">` on a page, status `in-development` | Renders `data-capability="nvme-sanitize" data-status="in-development"` with a visible "In development" badge | Build passes |
| Bare overclaim | Page text "Sanitizes NVMe drives with NIST Purge" outside any capability element | Violation names page, capability id, matched text | `astro build` exits non-zero |
| Overclaim in metadata | Same phrase in `<title>`, `meta[name=description]` or `og:*` content | Violation (metadata can carry no badge) | Build fails |
| Unknown id | `<Capability id="nope">` | Render throws naming the id | Build fails |
| Stale status | Element with `data-status="available"` where the list says `planned` | Violation | Build fails |
| Hidden badge | Non-`available` element whose badge is missing, has `hidden`, or class `sr-only` | Violation | Build fails |
| Missing meta | Page without `<title>`, description, canonical, `og:title`, `og:description`, `og:url` | Violation | Build fails |
| Third-party script | `<script src>` or `<link rel=preconnect/dns-prefetch>` to a host other than the site's | Violation | Build fails |
| `available` capability | Guarded phrase of an `available` item appears bare | Allowed | Build passes |

</intent-contract>

## Code Map

- `apps/desktop/package.json`, `apps/desktop/pnpm-lock.yaml` -- read-only; must keep working. Desktop is Tailwind 3; the site is an independent package and may use Tailwind 4.
- `apps/desktop/tailwind.config.js` -- source of the `sans`/`mono` font stacks to copy into the site theme.
- `.gitignore` -- already ignores `node_modules/`, `dist/`; add `sites/www/.astro/`.
- `CONTEXT.md` §1–§3 (positioning, differentiators), §4 and §13 (standards, certification roadmap) -- source for capability names and statuses.
- `docs/ROADMAP.md` §1.3 W1.1, W1.4 -- mark shipped (W1.1 without "CI build": ROADMAP §0.5, no CI exists).
- `CONTEXT.md` §10.2 table -- add a row for this surface.
- `README.md` (layout table ~L48, test table ~L167) and `AGENTS.md` "Running and verifying" -- add the site commands.
- Registry (checked 2026-11-05): astro 7.4.7 (node ≥22.23; local node 24.15, pnpm 9.12), @astrojs/sitemap 3.7.4, tailwindcss + @tailwindcss/vite 4.3.4, lighthouse 13.6.0; Google Chrome is installed locally.

## Tasks & Acceptance

**Execution:**
- `pnpm-workspace.yaml` (root) -- create listing `apps/*` and `sites/*`, keeping per-package lockfiles; add a minimal private root `package.json` only if pnpm requires one -- SPEC constraint; desktop lock untouched.
- `sites/www/package.json` -- name `www`, private, type module; scripts `dev`, `build` (`astro build`), `preview`, `typecheck` (`astro check` or `tsc --noEmit`), `test` (`vitest run`), `lighthouse`; Node engine matching Astro -- `pnpm --filter www dev` is the success signal.
- `sites/www/astro.config.mjs` -- `output: 'static'`, `site` from `SITE_URL` with placeholder default, `@astrojs/sitemap`, Tailwind via `@tailwindcss/vite`, and the guard integration -- CAP-4.
- `sites/www/src/data/capabilities.ts` -- typed list: `id`, `label`, `status`, `summary`, `guardedPhrases: RegExp[]`; covers CONTEXT §3 differentiators 1–11 plus NIST 800-88 Rev. 2 / IEEE 2883 conformance (self-attestation, in-development), ADISA AL5, Common Criteria EAL 2, NATO NIAPC (planned); export `getCapability(id)` that throws on unknown id -- CAP-3 single source.
- `sites/www/src/components/Capability.astro`, `StatusBadge.astro` -- render label (or slot) with `data-capability`/`data-status` and a visible badge (`data-status-badge`) for non-`available` -- labelling is automatic, not remembered.
- `sites/www/src/lib/site-check.ts` -- pure `checkPage(html, path, capabilities, siteOrigin): Violation[]` implementing every matrix row (parse with a small dependency such as `node-html-parser`; text outside `[data-capability]` plus title/meta/OG content is scanned) -- unit-testable core.
- `sites/www/src/integrations/site-check.ts` -- `astro:build:done` hook: run `checkPage` on every emitted `.html`, assert `sitemap-index.xml` exists, print all violations, throw if any -- build fails on overclaim.
- `sites/www/src/config.ts` -- site name, placeholder origin, `analytics: { enabled: false }`; `src/components/Analytics.astro` renders nothing when disabled -- tracker hook off by default.
- `sites/www/src/layouts/Base.astro` -- html lang, title, description, canonical, OG tags, skip link, header nav, footer with a visible "Pre-launch: nothing here is generally available yet" notice, `<Analytics />` -- shared shell for stories 2–3.
- `sites/www/src/styles/global.css` -- Tailwind import + `@theme` font tokens copied from desktop.
- `sites/www/src/pages/index.astro` (placeholder), `src/pages/capabilities.astro` (every capability with its badge and summary, mock-backend status stated) -- something real to build and check.
- `sites/www/test/site-check.test.ts` -- one test per matrix row against fixture HTML and a fixture capability list (including one `available` item), plus a test that every real capability's `guardedPhrases` matches its own `label` or `summary` and that `getCapability` throws on unknown ids.
- `sites/www/scripts/lighthouse.mjs` -- build, start `astro preview`, run Lighthouse (local Chrome, headless) on every sitemap URL, exit non-zero if performance or accessibility < 90 -- CAP-4 local check.
- `sites/www/README.md`, `README.md`, `AGENTS.md`, `CONTEXT.md` §10.2, `docs/ROADMAP.md` W1.1/W1.4 -- docs in the same commit.

**Acceptance Criteria:**
- Given a fresh clone, when `pnpm install` runs at the root, then `pnpm --filter www build`, `pnpm --filter www test` and `cd apps/desktop && pnpm typecheck && pnpm test` all pass.
- Given the built site, when `dist/` is inspected, then it contains `index.html`, `capabilities/index.html`, `sitemap-index.xml`, and no `<script>` tag that loads from another origin.
- Given a temporary page containing a bare guarded phrase for an `in-development` capability, when `pnpm --filter www build` runs, then it exits non-zero and prints the page path and capability id (verify, then delete the page).
- Given `/capabilities` rendered with JavaScript disabled, when a visitor reads it, then every capability shows its label and a visible status badge, and none reads as `available`.
- Given local Chrome, when `pnpm --filter www lighthouse` runs, then performance and accessibility are ≥ 90 on every page.

## Spec Change Log

## Review Triage Log

### 2026-11-06 — Review pass
- verdicts: 46 findings — high 0, medium 16, low 25, false 5, maybe-false 0
- findings:
  - `[medium]` `[patch]` Blind: any `data-status-badge` element silences the scan (escape hatch) — badge text now skipped only for a real badge of its nearest capability with matching status; test added.
  - `[medium]` `[patch]` Blind: badge words never checked — visible badge text must equal `STATUS_LABEL[status]`; fixtures use real labels; wrong-text test added.
  - `[low]` `[reject]` Blind: one wrapper can cover a whole section — the wrapper still shows a visible badge; limiting wrapper size adds rules for an unlikely misuse.
  - `[low]` `[patch]` Blind: responsive/utility hiding classes missed — classes compared by last `:` segment incl. `opacity-0`; opacity regex widened; tests added.
  - `[medium]` `[patch]` Blind: third-party check narrow (img/iframe/media/icon/manifest/CSS url) — checkPage covers these attributes and rels; new `checkCss` scans inline and emitted CSS; "zero client JS" half is already true of dist (no scripts) and not a spec rule.
  - `[low]` `[reject]` Blind: JSON-LD not scanned — no structured data exists; adding a branch for a hypothetical page is unwarranted now.
  - `[medium]` `[patch]` Blind: compliance claims unguarded (R2v3, "certification") — added `r2v3-recognition` (planned) and broadened the shared certification pattern to `certifi(ed|cations?)`; HIPAA/FIPS/DoD are outside CONTEXT's claims and were not added.
  - `[medium]` `[patch]` Blind: generic erase/certified phrases force the wrong badge (NVMe/ADISA) — added `drive-sanitization` (in-development) and `independent-certification` (planned) owning the generic patterns.
  - `[medium]` `[patch]` Blind: destruction missing from the list — added `destruction-attestation` (in-development, ADR-0011) with destruction/chain-of-custody/shred guards.
  - `[false]` `[reject]` Blind: `sites/www/pnpm-lock.yaml` not in the diff — it exists on disk and was excluded from the review diff only for size; it is committed with this story.
  - `[low]` `[defer]` Blind: no root package/`pnpm test`, AGENTS.md intro omits sites/www — root script rejected (spec: root manifest only if required; `pnpm -r test` works); AGENTS.md intro deferred (agent-context file).
  - `[medium]` `[patch]` Blind: rendered output/integration untested — new `test/site-check-integration.test.ts` drives the build hook over temp dirs (clean, nested overclaim, foreign CSS import, missing sitemap).
  - `[low]` `[patch]` Blind: missing sitemap reported as `missing-meta` — new `missing-artifact` rule.
  - `[low]` `[patch]` Blind: ROADMAP W1.4 struck incl. unbuilt frontmatter schema — note now says no per-page schema exists and W-R12 stays open for W2.
  - `[low]` `[patch]` Blind: dead Home nav entry and trailing-slash `aria-current` — entry removed, path normalised.
  - `[low]` `[reject]` Blind: malformed `SITE_URL` gives a confusing error; `LH_PORT` undocumented — rare developer case; validation adds a guard.
  - `[medium]` `[patch]` Edge: stray badge silences scan — same group as the Blind escape-hatch row.
  - `[low]` `[reject]` Edge: claim split across an unrelated capability boundary passes — requires deliberate contrived markup; fix needs cross-context scanning.
  - `[low]` `[patch]` Edge: soft hyphen/zero-width split words — stripped before every scan; test added.
  - `[low]` `[patch]` Edge: any period in the gap ends the match ("Erases 2.5in drives") — gap now stops only at a sentence-ending period; tests added.
  - `[low]` `[patch]` Edge: responsive hidden badge — same group as the Blind hiding row.
  - `[low]` `[reject]` Edge: badge inside `<template>` counts visible — contrived; adds an ancestor branch.
  - `[medium]` `[patch]` Edge: badge text not compared to label — same group as the Blind badge-words row.
  - `[low]` `[patch]` Edge: `available` owner exempts shared phrases — owners limited to non-available capabilities; test added.
  - `[low]` `[patch]` Edge: `value`/`aria-description` not read — added to READ_ATTRIBUTES; rows added.
  - `[medium]` `[patch]` Edge: third-party via img/iframe/source/srcset/icon/style url — same group as the Blind third-party row.
  - `[low]` `[reject]` Edge: foreign `<base href>` redirects relative scripts — unlikely authoring; adds a branch.
  - `[low]` `[patch]` Edge: body `<svg><title>` masks missing head title — head title only; test added.
  - `[low]` `[reject]` Edge: zero emitted pages passes — a build with no pages is not reachable from this site; adds a guard.
  - `[low]` `[reject]` Edge: Lighthouse port collision / early preview exit — developer-local script, rare; adds handling.
  - `[low]` `[reject]` Edge: `SITE_URL` with a path breaks Lighthouse — placeholder origin has no path; rare.
  - `[medium]` `[patch]` Edge (claim): data-status-badge escape hatch contradicts spec — same group as escape-hatch row.
  - `[low]` `[reject]` Edge (claim): split-claim/owner exemption lets another capability own phrases — owner-available half patched above; split half rejected as above.
  - `[medium]` `[patch]` Edge (claim): visible badge check weak — same group as badge-words/hiding rows.
  - `[low]` `[reject]` Edge (claim): base href redirection — same as the base-href row.
  - `[medium]` `[patch]` VerificationGap: build hook untested — same group as the integration-test row.
  - `[low]` `[patch]` VerificationGap: preload/modulepreload/prefetch, twitter:*, aria-label/title/placeholder, invisible/visibility/opacity branches untested — rows added to the existing tables.
  - `[false]` `[reject]` VerificationGap other: lockfile not in diff — same refutation as the Blind lockfile row.
  - `[false]` `[reject]` Intent: real pages checked only at build — ADR-0006 §2 says the test "fails the site build"; the build is the gate.
  - `[low]` `[reject]` Intent: components tested only by imitation — any drift in component output makes every real capability fail `hidden-badge`/`stale-status` at build, so it cannot pass silently.
  - `[medium]` `[patch]` Intent: integration has no test — same group as the integration-test row.
  - `[medium]` `[patch]` Intent: regex coverage misses R2v3/"certification" — same group as the compliance row.
  - `[low]` `[patch]` Intent: visibility via attributes only — same group as the hiding row (stylesheet-CSS hiding remains out of reach of an HTML check).
  - `[medium]` `[patch]` Intent: third-party partial — same group as the third-party row.
  - `[false]` `[reject]` Intent: diff enforces mentions, not "presents as available" — the spec deliberately chose mention-based, false-positive-preferring guarding; stricter, not divergent.
  - `[false]` `[reject]` Intent: `dev` does not flag overclaims — the gate is the build (ADR-0006); dev is the preview signal only.

## Design Notes

Guarded phrases are assertion-shaped and per capability, e.g. `nvme-sanitize`: `/\bNVMe Sanitize\b/i`, `/\bsaniti[sz]e[sd]?\b[^.]{0,60}\b(drives?|NVMe|SSDs?|HDDs?|media)\b/i`; `adisa-al5`: `/\bADISA\b/i`; certification in general: `/\bcertified\b/i`. Rule: a guarded phrase of a non-`available` capability may appear only inside that capability's `[data-capability]` element. Prefer false positives (the author wraps the phrase, gaining a badge) over a missed overclaim; story 2–3 copy wraps rather than rewording the guard.

## Verification

**Commands:**
- `pnpm install` (root) -- expected: succeeds; `git diff apps/desktop/pnpm-lock.yaml` empty.
- `pnpm --filter www build` -- expected: exit 0, guard reports 0 violations.
- `pnpm --filter www test` and `pnpm --filter www typecheck` -- expected: pass.
- `cd apps/desktop && pnpm typecheck && pnpm test` -- expected: 200 tests pass.
- `pnpm --filter www lighthouse` -- expected: all scores ≥ 90.

## Auto Run Result

Status: done

**Summary:** Static Astro 7 + Tailwind 4 package `www` at `sites/www` in a new root pnpm workspace (per-package lockfiles via root `.npmrc`; `apps/desktop` lock unchanged). A shared capability list (19 entries, none `available`) with `<Capability>`/`<StatusBadge>` components, and a build-time check (`astro:build:done`) over rendered HTML and emitted CSS. The check fails the build on bare guarded phrases (body, read attributes, title, meta/OG/twitter), unknown ids, stale statuses, missing/hidden/mis-worded badges, missing meta or sitemap, and third-party fetches. Placeholder home, a `/capabilities` page stating the simulated backend, analytics hook off, sitemap, and a local Lighthouse script.

**Files changed:**
- `pnpm-workspace.yaml`, `.npmrc` — root workspace (`apps/*`, `sites/*`), per-package lockfiles.
- `.gitignore` — ignore `sites/www/.astro/`.
- `sites/www/package.json`, `pnpm-lock.yaml`, `tsconfig.json`, `astro.config.mjs` — package, static output, sitemap, Tailwind, guard integration.
- `sites/www/src/data/capabilities.ts` — capability list, statuses, guarded phrases.
- `sites/www/src/lib/site-check.ts` — pure `checkPage`/`checkCss`.
- `sites/www/src/integrations/site-check.ts` — build hook that fails the build.
- `sites/www/src/components/{Capability,StatusBadge,Analytics}.astro`, `src/layouts/Base.astro`, `src/config.ts`, `src/styles/global.css` — components, shell, config, theme.
- `sites/www/src/pages/{index,capabilities}.astro` — placeholder home and status page.
- `sites/www/test/site-check.test.ts`, `test/site-check-integration.test.ts` — 109 tests.
- `sites/www/scripts/lighthouse.mjs` — local Lighthouse gate (≥ 90).
- `sites/www/README.md`, `README.md`, `AGENTS.md`, `CONTEXT.md` §10.2, `docs/ROADMAP.md` W1.1/W1.4 — docs.

**Review findings:** 46 findings. 17 patch groups applied: badge escape hatch, badge wording, responsive hiding, third-party resources + CSS, owner exemption, invisible chars, read attributes, head title, `missing-artifact` rule, sentence-aware gaps, generic drive-sanitization/certification entries, R2v3, destruction, integration test, branch-coverage rows, nav, ROADMAP note. 1 deferred: AGENTS.md intro paragraph. Rejected, with the reason in the triage log: wide wrappers, JSON-LD, root `pnpm test` script, malformed SITE_URL/LH_PORT, cross-boundary split claims, `<template>` badges, `<base href>`, zero-page build, Lighthouse port/path cases, component-render tests. The 7 `false` rows are refuted above.

**Follow-up review recommended: true.** Patched by verdict: medium 8 groups, low 9. The specific unverified risk: the guard grew substantially in this pass (new resource checks, CSS scanning, four new capabilities with shared patterns, owner-exemption rule change), and those additions themselves have not been reviewed.

**Verification:** root `pnpm install` OK, `apps/desktop/pnpm-lock.yaml` unchanged; `pnpm --filter www build` exit 0 (2 pages, 0 violations); `pnpm --filter www test` 109/109; `pnpm --filter www typecheck` 0 errors; `apps/desktop` typecheck OK and 200/200 tests; `pnpm --filter www lighthouse` performance 100 / accessibility 100 on both pages; a temporary page with "Sanitizes NVMe drives with NIST Purge" made the build exit 1 with 5 overclaim violations (page deleted).

**Residual risks:** guarded phrases over-match by design, so stories 2–3 will wrap a lot of CONTEXT-derived copy; capability statuses, brand and origin await vendor sign-off; hiding through stylesheet CSS (rather than classes/attributes) is not detectable; no CI runs the build (ROADMAP §0.5).

