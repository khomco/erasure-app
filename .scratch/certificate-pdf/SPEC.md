---
id: SPEC-certificate-pdf
companions: [../../docs/adr/0005-licensing-and-attestation.md, ../../docs/adr/0011-destruction-by-operator-attestation.md, ../../docs/ROADMAP.md, ../../CONTEXT.md]
sources: []
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete contract for what to build, test, and validate.

# One file a person can read and a machine can verify

## Why

Customers file erasure certificates with auditors and their own clients, most of whom expect a PDF, while the authoritative certificate is signed JSON. ROADMAP O4.3 and CONTEXT §11 v0.2 #9 call for PDF/A-3: the archival PDF standard (ISO 19005-3) that allows an embedded file. One PDF then shows the certificate to a person and carries the exact signed JSON inside it for a verifier.

Decisions taken by default, recorded here:
- **The PDF is a rendering; the embedded JSON is the record.**
  - The PDF is not itself signed, so no PAdES (PDF digital signatures), and no new trust root.
  - The page says the attached file is the authoritative record and how to verify it.
  - The renderer refuses to render a certificate that does not verify, so a valid-looking PDF of a tampered certificate cannot be produced by our tools.
- **Conformance level PDF/A-3b**, with the JSON attached with `AFRelationship /Source`, an embedded open-licensed font (SIL Open Font License), an sRGB output intent and XMP metadata declaring part 3.
- **Generated on demand**, from the CLI and from the station's memory. Nothing is written to the station's disk (CONTEXT §7 #5).
- **Deterministic output.** Creation and modification dates come from the certificate's `issued_at`, and document ids from its hash, so the same certificate always gives the same bytes and tests can pin them.

## Capabilities

- **CAP-1 The PDF renderer**
  - **intent:** Given a signed certificate, produce a PDF/A-3b file that shows it plainly and embeds it exactly.
  - **success:**
    - A new crate `wipe-cert-pdf` (depends on `wipe-cert`, `wipe-license` and `wipe-verify`; below `wipe-cli`/`wipe-server`) exposes `render(signed_json: &[u8], roots, station_keys) -> Result<Vec<u8>, RenderError>`. It verifies through `wipe-verify` first and refuses an unverifiable certificate with the report's reason. The plan picks the PDF library (pure Rust, permissively licensed, MSRV 1.80) and records why.
    - Page content:
      - a header with the disposition and NIST method;
      - the drive (model, serial, capacity);
      - the job, operator, start and end times;
      - the verification step and its result;
      - for a destroyed drive, the destruction record and witness;
      - the licence holder, or a prominent "Evaluation certificate" notice;
      - the signing key id and the SHA-256 the signature covers;
      - the label QR (`WSC1:…`, `bench-floor` story 2's payload);
      - a footer statement.

      Proposed footer statement: "This page is a readable copy. The authoritative record is the signed file attached to this PDF ({filename}). Check it with `wipestation verify-cert` or at {site}/verify/ using the vendor's published root key."
    - The JSON is embedded byte-for-byte as `certificate-<UUID>.json`, with `AFRelationship /Source` and listed in the catalog's `/AF` array.
    - Tests:
      - extracting the attachment (with a PDF parsing crate in dev-dependencies) returns the input bytes, and they still verify;
      - XMP declares `pdfaid:part 3`, `pdfaid:conformance B`;
      - an OutputIntent with an embedded sRGB profile exists;
      - every font is embedded;
      - no JavaScript, encryption or external links exist;
      - rendering the same certificate twice gives identical bytes;
      - a tampered certificate is refused;
      - each `docs/generated/examples/` certificate (or the fixtures, before `docs-site`) renders.
    - `scripts/verapdf.sh` runs the open-source veraPDF validator over the rendered examples when it is installed and fails on any PDF/A-3b rule. It is documented in README, run by the agent for this story, and its output is recorded in the story result. It is not part of `cargo test`, since it needs Java.
  - **checkpoint:** the page layout's wording is drafted by the agent, not yet owner-approved; the owner approves the finished story before it counts as done.
- **CAP-2 Getting the PDF**
  - **intent:** An operator downloads a certificate's PDF from the bench, and an integrator makes one from any certificate file.
  - **success:**
    - `wipestation cert-pdf <CERT> [--out PATH] [--vendor-root-b64 …] [--public-key-b64 …]` writes the PDF (default `<cert stem>.pdf`). It refuses an unverifiable certificate with the reason. The CLI help golden is regenerated.
    - `GET /api/jobs/{id}/certificate.pdf` on the station renders from memory with the station's own roots and key (`application/pdf`, `Content-Disposition: attachment`). The answer is 404 when the Job has no certificate, as the JSON route does. The route is declared in `app.rs` and called from `client.ts`, and the golden is regenerated.
    - The desktop Certificate page gains **Download PDF** beside the JSON download.
    - Tests: the CLI happy path and refusal; the station route for erased, destroyed, evaluation and no-certificate Jobs; the desktop button calls the route.
    - CONTEXT §10 records the shipped capability and ROADMAP O4.3 is struck.

## Constraints

- The certificate format does not change; the PDF wraps it.
- Dependency direction: `wipe-cert-pdf` sits above `wipe-verify` and is used only by `wipe-cli` and `wipe-server`.
- Nothing about a certificate is written to the station's disk.
- Any font and ICC profile shipped is redistributable, and its licence file is committed beside it.

## Non-goals

- Signing the PDF (PAdES), bulk export per work order (O4.2, waits on D11), branding a PDF per customer, and verifying a PDF on `/verify/` (extract the JSON and paste it).

## Success signal

- An operator downloads a PDF from the bench. veraPDF calls it PDF/A-3b compliant, a person reads it without a manual, and extracting its attachment and running `wipestation verify-cert` on it says authentic.
