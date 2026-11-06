---
id: SPEC-software-downloads
companions: [../../docs/adr/0006-web-presence-and-local-first-verification.md, ../../docs/adr/0007-hub-accounts-payments-persistence.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# Customers download the software they are entitled to

## Why

Distribution: a paying customer needs the station software, and the vendor needs to know who has which version.

## Capabilities

- **CAP-1**
  - **intent:** A vendor admin publishes a release.
  - **success:** Admin page: version, release notes, channel (`stable|preview`), artifacts (platform label + file upload); the hub computes SHA-256 per artifact; a release can be withdrawn.
- **CAP-2**
  - **intent:** Entitled customers download releases; others cannot.
  - **success:** Portal "Downloads" lists releases for organizations with an active entitlement; download links are short-lived and signed; an unentitled or signed-out request gets 403; checksums shown beside each file.
- **CAP-3**
  - **intent:** The vendor knows who downloaded what.
  - **success:** Every download is recorded (user, organization, artifact, time); admin view per release.
- **CAP-4**
  - **intent:** Storage can move to the cloud later without code changes elsewhere.
  - **success:** `ArtifactStore` trait with a local-directory implementation; tests use a temp directory.
- **CAP-5**
  - **intent:** A developer can publish the locally built station binary as a release.
  - **success:** `wipe-hub release publish --version --notes --file <platform>=<path>` uploads from the command line (used by the end-to-end script).

## Constraints

- Downloads are audited; withdrawn releases stay listed to admins only.

## Non-goals

- Code signing of installers, auto-update, a CDN.

## Success signal

- A customer with an active entitlement downloads the station build published by the CLI and the checksum matches.
