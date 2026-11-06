/**
 * The single source of truth for every capability the marketing site may
 * mention, and the status it may be described with (ADR-0006 §2).
 *
 * STATUSES ARE A VENDOR SIGN-OFF DECISION. Do not promote anything to
 * `available` because the code for it exists: `available` means a customer can
 * buy it and it works on real hardware. Today the only `DeviceBackend` is the
 * mock (ROADMAP §0.1) and the product is pre-launch, so nothing here is
 * `available`. Hardware sanitization is `in-development` (blocked on
 * `wipe-engine-linux`); certifications are `planned` (CONTEXT §13).
 *
 * `guardedPhrases` are assertion-shaped patterns. The build-time check
 * (`src/lib/site-check.ts`) fails the build when a guarded phrase of a
 * non-`available` capability appears in rendered HTML outside that
 * capability's `[data-capability]` element — including `<title>`, the meta
 * description and OG tags, which can carry no badge. Prefer a pattern that
 * over-matches: the fix for a false positive is to wrap the copy in
 * `<Capability>` (which adds a visible badge), never to weaken the pattern.
 * Every pattern must match the capability's own `label` or `summary` (tested).
 */

export const CAPABILITY_STATUSES = ["available", "in-development", "planned"] as const;
export type CapabilityStatus = (typeof CAPABILITY_STATUSES)[number];

export interface CapabilityDef {
  id: string;
  label: string;
  status: CapabilityStatus;
  summary: string;
  guardedPhrases: RegExp[];
}

/** Shared by every certification-type item: "certified"/"certification" is always a claim
 * (but "certificate" is not). Sharing it lets the enclosing item own the words. */
const CERTIFICATION = /\bcertifi(ed|cations?)\b/i;

/**
 * A gap of up to N characters that stops only at a sentence-ending period, so
 * "Erases 2.5in drives" and "e.g. wipes SSDs" are still read as one claim.
 */
const GAP = (n: number) => `(?:[^.]|\\.(?=\\S)){0,${n}}`;

/** Generic drive-sanitization claims, shared by every hardware-sanitization item. */
const SANITIZE_DRIVES = new RegExp(
  `\\bsaniti[sz](e[sd]?|ing)\\b${GAP(60)}\\b(drives?|disks?|NVMe|SSDs?|HDDs?|media)\\b`,
  "i",
);
const WIPE_DRIVES = new RegExp(
  `\\b(wipe[sd]?|wiping|eras(e[sd]?|ing))\\b${GAP(60)}\\b(drives?|disks?|SSDs?|HDDs?|media)\\b`,
  "i",
);
const NIST_CLEAR_PURGE = /\bNIST (Clear|Purge)\b/i;

export const capabilities: readonly CapabilityDef[] = [
  // ---- CONTEXT §3 differentiators 1–11 ----------------------------------
  {
    id: "command-level-evidence",
    label: "Command-level evidentiary certificate",
    status: "in-development",
    summary:
      "Designed to record every issued opcode, action byte, return status and log-page read in the certificate, so an auditor sees proof, not prose. Today that evidence is synthesised by the simulated device backend.",
    guardedPhrases: [/\bcommand[- ]level\b/i, /\b(every|each) issued opcode\b/i],
  },
  {
    id: "outcome-bearing-job",
    label: "One Job per Asset, every attempt in one certificate",
    status: "in-development",
    summary:
      "Every attempt on an asset (erasures, retries, verification, and destruction if it comes to that) composes into one signed certificate per asset with the full evidence chain.",
    guardedPhrases: [/\bone (signed )?certificate per (asset|drive)\b/i, /\bevery attempt\b/i],
  },
  {
    id: "offline-verification",
    label: "Offline-verifiable certificates",
    status: "in-development",
    summary:
      "Certificates carry an Ed25519 signature over canonical JSON, so anyone with the public key can check one without contacting the vendor. Working in pre-release builds against the simulated backend.",
    guardedPhrases: [
      /\boffline[- ]verif(y|iable|ied|ication)\b/i,
      /\bwithout (contacting|phoning|calling) (the vendor|home|us)\b/i,
      /\bEd25519\b/i,
    ],
  },
  {
    id: "erp-integration",
    label: "ITAD-ERP integration",
    status: "in-development",
    summary:
      "REST in and out, webhooks on Job state changes and certificate signing, and asset-id lookup callbacks, so the ERP stays the system of record.",
    guardedPhrases: [/\bERP integration\b/i, /\bwebhooks?\b/i],
  },
  {
    id: "per-success-licensing",
    label: "Per-success licensing",
    status: "in-development",
    summary:
      "A licence credit is consumed only on a verified Clear or Purge success; failures, aborts and re-runs are free.",
    guardedPhrases: [/\bper[- ]success\b/i, /\b(failures|re-?runs)\b(?:[^.]|\.(?=\S)){0,40}\bfree\b/i],
  },
  {
    id: "drive-sanitization",
    label: "Drive sanitization on real hardware",
    status: "in-development",
    summary:
      "Designed to sanitize SSDs and HDDs to NIST Clear or Purge with command-by-command evidence. Pre-release builds only erase simulated drives; wiping real media needs the hardware backend, which is not built yet.",
    guardedPhrases: [SANITIZE_DRIVES, WIPE_DRIVES, NIST_CLEAR_PURGE],
  },
  {
    id: "nvme-sanitize",
    label: "Honest NVMe Sanitize",
    status: "in-development",
    summary:
      "Designed to sanitize NVMe drives with the NVMe Sanitize command (Crypto Erase when self-encrypting, Block Erase otherwise) with an explicit fallback order and full log-page capture, reaching NIST Purge. It never quietly falls back to a plain overwrite that erases drives less thoroughly than claimed. Needs the real hardware backend, which is not built yet.",
    guardedPhrases: [
      /\bNVMe Sanitize\b/i,
      SANITIZE_DRIVES,
      WIPE_DRIVES,
      /\bcrypto(graphic)? erase\b/i,
      /\bblock erase\b/i,
      NIST_CLEAR_PURGE,
    ],
  },
  {
    id: "hpa-dco-clear",
    label: "HPA/DCO detection and clear before Purge",
    status: "in-development",
    summary:
      "Detects HPA and DCO hidden regions and clears them first, refusing to report NIST Purge if non-addressable regions were not reached. Needs the real hardware backend.",
    guardedPhrases: [
      /\bHPA\b/,
      /\bDCO\b/,
      /\bhidden (areas?|regions?)\b/i,
      /\bnon-addressable\b/i,
      NIST_CLEAR_PURGE,
    ],
  },
  {
    id: "destruction-attestation",
    label: "Destruction by operator attestation",
    status: "in-development",
    summary:
      "When an asset is physically destroyed, for example shredded, the operator attests the destruction, optionally with concurrence from a second person, and the attestation joins the asset's chain of custody.",
    guardedPhrases: [
      new RegExp(`\\bdestruction\\b${GAP(40)}\\b(certificates?|attestations?|attests?|records?)\\b`, "i"),
      /\bchain[- ]of[- ]custody\b/i,
      /\bshred(s|ded|ding|der|ders)?\b/i,
      /\bphysically destr(oy|oys|oyed|oying|uction)\b/i,
    ],
  },
  {
    id: "validate-workflow",
    label: "Programmatic Validate workflow",
    status: "planned",
    summary:
      "A built-in registry for the NIST Rev. 2 Validate step (per-media-class assurance), distinct from the verification run after each erasure.",
    guardedPhrases: [/\bValidate (step|workflow|registry)\b/i],
  },
  {
    id: "open-cert-format",
    label: "Open certificate format",
    status: "in-development",
    summary:
      "The JSON-LD certificate schema is to be published so third parties can build their own verifiers.",
    guardedPhrases: [/\bJSON-LD\b/i, /\b(open|published) (cert|certificate) (format|spec|schema)\b/i],
  },
  {
    id: "secure-boot",
    label: "UEFI Secure Boot bootable image",
    status: "planned",
    summary:
      "A bootable image with a signed shim and GRUB, so no workflow asks you to disable Secure Boot.",
    guardedPhrases: [/\bSecure Boot\b/i, /\bbootable (image|USB|ISO)\b/i],
  },
  {
    id: "mobile-erasure",
    label: "Bundled mobile device erasure",
    status: "planned",
    summary: "Phase 2: mobile device erasure in the same product, not a second SKU.",
    guardedPhrases: [/\bmobile\b(?:[^.]|\.(?=\S)){0,30}\b(erasure|wip(e|ing)|saniti[sz]ation)\b/i],
  },

  // ---- Standards conformance and certification (CONTEXT §4, §13) -------
  {
    id: "nist-ieee-conformance",
    label: "NIST SP 800-88 Rev. 2 / IEEE 2883-2022 conformance (self-attested)",
    status: "in-development",
    summary:
      "The pre-launch target is a published self-attestation of conformance to NIST SP 800-88 Rev. 2 and IEEE 2883-2022. It is a self-attestation, not an independent assessment, and it is not issued yet.",
    guardedPhrases: [
      /\b(conforms?|conformant|conformance|compliant|compliance)\b(?:[^.]|\.(?=\S)){0,40}\b(NIST|800-88|Rev\. ?2|IEEE|2883)/i,
      /\b(NIST|800-88|Rev\. ?2|IEEE|2883)\b(?:[^.]|\.(?=\S)){0,40}\b(conformant|conformance|compliant|compliance)\b/i,
      /\bself-attest(ed|ation)\b/i,
    ],
  },
  {
    id: "independent-certification",
    label: "Independent product certification",
    status: "planned",
    summary:
      "No independent certification of any kind is held today; each target below carries its own status, and nothing on this site is certified.",
    guardedPhrases: [CERTIFICATION],
  },
  {
    id: "adisa-al5",
    label: "ADISA Assurance Level 5",
    status: "planned",
    summary:
      "Independent product certification by ADISA at Assurance Level 5 (AL5) is a year-one target. Wipestation is not ADISA certified today.",
    guardedPhrases: [/\bADISA\b/i, /\bAssurance Level 5\b/i, /\bAL5\b/i, CERTIFICATION],
  },
  {
    id: "common-criteria-eal2",
    label: "Common Criteria EAL 2",
    status: "planned",
    summary:
      "Common Criteria evaluation at EAL 2 or EAL 2+ is a year one-to-two target for government and defense procurement. Nothing is Common Criteria certified today.",
    guardedPhrases: [/\bCommon Criteria\b/i, /\bEAL ?2\b/i, CERTIFICATION],
  },
  {
    id: "nato-niapc",
    label: "NATO NIAPC listing",
    status: "planned",
    summary:
      "A listing in the NATO Information Assurance Product Catalogue (NIAPC) is a year-two target. Wipestation is not listed or certified today.",
    guardedPhrases: [/\bNIAPC\b/i, /\bNATO\b(?:[^.]|\.(?=\S)){0,40}\b(list(ed|ing)|catalogue)\b/i, CERTIFICATION],
  },
  {
    id: "r2v3-recognition",
    label: "R2v3 / e-Stewards vendor recognition",
    status: "planned",
    summary:
      "Recognition of Wipestation by the R2v3 and e-Stewards certification programmes is a year-two target. Those programmes certify ITAD operators; the product holds no such recognition today.",
    guardedPhrases: [/\bR2v3\b/i, /\be-Stewards\b/i, CERTIFICATION],
  },
];

/** Look up a capability; an unknown id is an authoring error and throws. */
export function getCapability(id: string, list: readonly CapabilityDef[] = capabilities): CapabilityDef {
  const found = list.find((c) => c.id === id);
  if (!found) {
    throw new Error(`Unknown capability id "${id}" — add it to src/data/capabilities.ts first`);
  }
  return found;
}

export const STATUS_LABEL: Record<CapabilityStatus, string> = {
  available: "Available",
  "in-development": "In development",
  planned: "Planned",
};
