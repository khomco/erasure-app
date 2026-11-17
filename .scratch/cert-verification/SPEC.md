---
id: SPEC-cert-verification
companions: [../../docs/adr/0005-licensing-and-attestation.md, ../../docs/adr/0006-web-presence-and-local-first-verification.md, ../../docs/adr/0011-destruction-by-operator-attestation.md, ../../docs/ROADMAP.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# Anyone can check a certificate with only the vendor root

## Why

The product's central claim is that a certificate can be checked by anyone, offline, without asking the vendor (CONTEXT §3, ADR-0005 §1). As built, that claim has a gap (ADR-0005 known gap 4). The licence inside a certificate binds the station's key *id* but not the key itself, so `wipestation verify-cert` needs the station's public key as well as the vendor root. An auditor who holds only the published root cannot check a certificate.

This epic closes that gap. It then puts the check where a buyer or auditor can use it: a page on the marketing site that verifies a pasted certificate entirely in the browser, with no network call (ROADMAP W3.2). The ROADMAP calls this "the differentiator made touchable".

Decisions taken by default (AGENTS.md "Questions left for the human"), recorded here:
- **The station's public key travels in the attestation chain, bound by its id.**
  - `AttestationChain` gains the instance public key. The chain sits outside the signed certificate payload, so no signed bytes change and the v2/v3 fixtures still verify.
  - A verifier accepts that key only when its `public_key_id` equals both the certificate signature's `public_key_id` and the vendor-signed licence's `instance_public_key_id`. The id is 128 bits of SHA-256, so it is a sound binding.
  - Licence files do not change. The station adds its own key when it attaches the chain at signing.
- **One verification core, compiled twice.** The CLI and the browser call the same Rust function. Nothing is ported to TypeScript, the same rule ADR-0002 applies to bay matching. The browser build is WebAssembly (WASM, a compact binary format browsers run natively), as ROADMAP §1.2 planned.
- **The page makes no network request after it loads.** Its Content-Security-Policy (CSP, the browser's list of what a page may load) forbids fetching anything at verify time.

## Capabilities

- **CAP-1 Root-only verification**
  - **intent:** An auditor with only the published vendor root can check a licensed certificate's signature, its licence, and that the licence names the key that signed it.
  - **success:**
    - `AttestationChain` gains `instance_public_key_b64` (`#[serde(default, skip_serializing_if = …)]`), set by the station's certificate generator from its own signing key whenever it attaches a chain.
    - A verifier uses that key only when `public_key_id(key)` equals `signature.public_key_id` and `license.body.instance_public_key_id`. Any mismatch is a failed verification that names both ids, never a silent fallback.
    - `wipestation verify-cert` no longer requires `--public-key-b64` when the certificate carries a chain with the key and `--vendor-root-b64` is given. Without either, it refuses and says which is missing. An evaluation certificate (no chain) still needs `--public-key-b64`, and the error says so in those words. The CLI help golden is regenerated.
    - Certificates signed before this change (chain without the key) still verify with `--public-key-b64` exactly as today. The committed v2/v3 fixtures are untouched and still pass.
    - Tests:
      - a licensed mock certificate verified with the root alone;
      - a chain whose key was swapped for another valid key (refused);
      - a chain key that matches the licence but not the signature (refused);
      - an old chain without the key;
      - an evaluation certificate with and without `--public-key-b64`.
    - ADR-0005's known gap 4 is marked resolved with the date, and CONTEXT §10 is updated.
- **CAP-2 One verification core that also builds for the browser**
  - **intent:** The CLI and the browser reach the same verdict on the same file, because they run the same code.
  - **success:**
    - A new crate `wipe-verify` (depends on `wipe-cert` and `wipe-license` only) exposes `verify_document(json: &str, roots: &[VerifyingKey], station_keys: &[VerifyingKey], at: Option<OffsetDateTime>) -> VerificationReport`. The report is a serde type holding:
      - the signature result and the key id used;
      - the chain verdict (`licensed` with customer and licence id, `evaluation`, or `invalid` with a reason);
      - `expired_at_signing`;
      - the certificate's headline fields (format version, job id, disposition, device, method, category, operator, issued at, the destruction record summary);
      - `canonical_sha256_hex`;
      - plain-language `problems` for any failure.

      A file that does not parse is a report with a problem, never a panic.
    - `wipestation verify-cert` builds its output from `verify_document`, so its printed text and exit codes stay as today (existing CLI tests pass unchanged, apart from CAP-1).
    - `crates/wipe-verify-wasm` (a `cdylib` using `wasm-bindgen`) exports `verify(json, roots_b64[], station_keys_b64[]) -> report JSON`. The `wasm32-unknown-unknown` build compiles without the signing paths' OS randomness. The build gates `getrandom`'s JS backend to the wasm target only, or keeps randomness out of the verify path, whichever the plan finds cleaner. Native builds must not change.
    - `rust-toolchain.toml` lists the `wasm32-unknown-unknown` target. `scripts/build-verify-wasm.sh` builds the module into `sites/www/src/generated/verify/` (git-ignored). It needs a `wasm-bindgen-cli` whose version is pinned to the crate's `wasm-bindgen` version, and it fails with the exact install command when the CLI is missing or the wrong version. The site's `build` and `test` scripts run it first.
    - The dependency guard (`deps.rs`) also enforces that `wipe-verify` depends on nothing above `wipe-cert`/`wipe-license`.
    - Tests:
      - native `wipe-verify` tests over every committed fixture and the CAP-1 cases;
      - a vitest in `sites/www` that loads the built WASM in Node and checks it returns the same report as a committed expected-report JSON for each fixture. The expected reports are a golden written by `UPDATE_GOLDENS=1 cargo test -p wipe-verify`.
    - The CI workflow (hardening story 1) builds the WASM in its `web` job. README and AGENTS.md give the new prerequisite.
- **CAP-3 The verification page**
  - **intent:** A buyer or auditor pastes a certificate and sees, without anything leaving their browser, whether it is authentic, whom it was licensed to, and what it records.
  - **success:**
    - `/verify/` on the marketing site has:
      - a certificate input (paste, or choose a `.json` file, read locally);
      - a vendor root field, prefilled from `site.vendorRoots` in `src/config.ts`. That list is empty until a production root exists (DECISIONS D5), and the page then says no production root is published yet and asks for one to be pasted;
      - an optional "station public key" field, shown with an explanation when the certificate carries no licence.
    - It shows the report from CAP-2:
      - a clear verdict line (authentic and licensed / authentic, evaluation / not authentic) with the reason;
      - the licence holder;
      - expired-at-signing as a warning, not a failure;
      - the headline fields, and a destruction record summary for destroyed drives;
      - the SHA-256 the signature covers.
    - The page sends nothing anywhere. Its CSP meta has `connect-src 'self'` only for loading the module, and no `form` action. A test asserts the built page has no third-party origin, no form action, and that verification makes no `fetch`/XHR (happy-dom with network stubbed to throw).
    - Without JavaScript, a `<noscript>` block gives the equivalent `wipestation verify-cert` command.
    - This is the one marketing page allowed a first-party script. `test/pages.test.ts` names it as the single exception, and the site check still refuses third-party scripts.
    - Claims on the page are wrapped in `<Capability id="offline-verification">`; no capability status changes.
    - Header, footer and the product page's offline-certificate section link to it.
    - Tests: the page renders with the site check passing; a DOM test pastes each fixture and checks the verdict line; a tampered fixture shows "not authentic" with the reason; an empty or non-JSON paste shows a plain error; Lighthouse accessibility stays ≥ the current score.
  - **checkpoint:** the page's wording is drafted by the agent, not yet owner-approved; the owner approves the finished story before it counts as done.

## Constraints

- Nothing inside a signed payload changes. New fields are outside it or `#[serde(default, skip_serializing_if = …)]`, and `crates/wipe-cert/tests/fixtures/*` are never edited.
- Dependency direction (AGENTS.md): `wipe-verify` sits above `wipe-cert`/`wipe-license` and below `wipe-cli`; the engine never depends on it.
- The browser holds no secret and makes no network call to verify. The vendor root is public by design.
- The ADR-0006 §2 honest-claims check applies to `/verify/` like any page.

## Non-goals

- Verifying PDF certificates (`certificate-pdf` epic), and matching a printed label's QR to a certificate (`bench-floor` story 2 adds it to this page).
- A hosted verification portal or API (CONTEXT §9 Tier 2, ROADMAP A4.3) and revocation checking (D6).
- Verification on the bench app's Certificate page (the station already holds its roots; a later story).

## Success signal

- Given a licensed certificate from `scripts/commerce-demo.sh` and only the root printed by `wipestation license new-root`, both `wipestation verify-cert --vendor-root-b64 …` and `/verify/` in a browser with the network disconnected say it is authentic and name the customer. A one-character edit to the file makes both say it is not.
