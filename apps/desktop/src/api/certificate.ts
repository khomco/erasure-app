/**
 * How the certificate page reads a signed certificate (schema v3,
 * ADR-0011 §2). Pure helpers, so the wording is tested without rendering.
 */
import { formatBytes } from "./client";
import type {
  CertDestruction,
  DestructionAttestation,
  IdentifierSource,
  SignedCertificate,
} from "./types";

/** Where a destroyed drive's recorded identifiers came from. */
export function identifierSourceLabel(source: IdentifierSource): string {
  return source === "scanned"
    ? "Scanned from the drive's labels"
    : "Read from the device";
}

/** The run's concurrence, or why there is none. */
export function concurrenceLabel(record: CertDestruction): string {
  const c = record.concurrence;
  if (c) {
    const act = c.kind === "witness" ? "Witnessed" : "Reviewed";
    return `${act} by ${c.signer.full_name}, ${c.signer.title}`;
  }
  return record.required_concurrence
    ? `Missing: a ${record.required_concurrence} was required`
    : "None required by the site's policy";
}

/**
 * The device line: the snapshot read from the drive, or, for a drive that
 * was never erased, a plain statement that nothing was read from it.
 */
export function deviceSummary(cert: SignedCertificate): { title: string; detail: string } {
  const d = cert.certificate.device;
  if (!d) {
    return {
      title: "Not read from the device",
      detail: "Never erased: identified by the asset tag or serial recorded for destruction",
    };
  }
  return {
    title: `${d.vendor} ${d.model}`,
    detail: `${d.serial} · ${formatBytes(d.capacity_bytes)}`,
  };
}

/** The attested Destruction activity a v3 destruction record names. */
export function recordedForDestruction(
  cert: SignedCertificate,
): DestructionAttestation | undefined {
  const id = cert.certificate.destruction?.destruction_event_id;
  if (!id) return undefined;
  for (const a of cert.certificate.activities) {
    if (a.type === "destruction" && a.id === id) return a.attestation;
  }
  return undefined;
}

/** How a certificate is marked for licensing, as `licenceSummary` reads it. */
export type LicenceKind = "evaluation" | "licensed" | "inconsistent" | "unmarked" | "unreadable";

export interface LicenceSummary {
  kind: LicenceKind;
  title: string;
  detail: string;
}

const VERIFY_HINT =
  "This page does not check the licence chain; verify it offline with wipestation verify-cert --vendor-root-b64.";

/**
 * The licence marking of a certificate (ADR-0005 §5), classified the way
 * `wipestation verify-cert` does without a vendor root. The evaluation
 * reading comes from the signed `certificate.evaluation`, never from the
 * absence of a chain, and the chain is only read, never verified: the
 * browser holds no vendor root. Never throws, whatever the JSON holds.
 */
export function licenceSummary(cert: SignedCertificate): LicenceSummary {
  // Read as untyped JSON: older certificates lack `evaluation`, and the
  // chain is opaque on the Rust certificate side.
  const raw = cert as unknown as {
    certificate?: { evaluation?: unknown } | null;
    attestation?: unknown;
  } | null;
  const evaluation = raw?.certificate?.evaluation === true;
  const chain = raw?.attestation;
  const hasChain = chain !== undefined && chain !== null;

  if (!hasChain) {
    return evaluation
      ? {
          kind: "evaluation",
          title: "Evaluation certificate",
          detail:
            "Produced without a valid licence. The erasure and its signature are still valid; this certificate is not evidence of a licensed deployment.",
        }
      : {
          kind: "unmarked",
          title: "No licence attached",
          detail:
            "The certificate carries no licence and does not mark itself evaluation. Certificates signed before licensing look like this; check any other offline with wipestation verify-cert.",
        };
  }

  const licence = readLicence(chain);
  if (evaluation) {
    return {
      kind: "inconsistent",
      title: "Marked evaluation, yet carries a licence",
      detail: `Warning: the signed certificate says it is an evaluation certificate, but a licence${
        licence ? ` naming ${licence.customer}` : ""
      } is attached. The two disagree. ${VERIFY_HINT}`,
    };
  }
  if (!licence) {
    return {
      kind: "unreadable",
      title: "Licence attached but unreadable",
      detail: `A licence is attached that this page cannot read. ${VERIFY_HINT}`,
    };
  }
  return {
    kind: "licensed",
    title: `Licensed to ${licence.customer}`,
    detail: `Per the attached licence${licence.licenseId ? ` ${licence.licenseId}` : ""}. ${VERIFY_HINT}`,
  };
}

/** Customer and licence id from an attached chain, or null if it cannot be read. */
function readLicence(chain: unknown): { customer: string; licenseId: string | null } | null {
  const body = asRecord(asRecord(asRecord(chain)?.license)?.body);
  const entitlements = asRecord(body?.entitlements);
  const name = entitlements?.customer_name;
  if (typeof name !== "string") return null;
  const id = entitlements?.customer_id;
  const licenseId = body?.license_id;
  return {
    customer: typeof id === "string" ? `${name} (${id})` : name,
    licenseId: typeof licenseId === "string" ? licenseId : null,
  };
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : undefined;
}

/** Header pill for a licence marking, or null when none is shown. */
export function licencePill(
  kind: LicenceKind,
): { label: string; tone: "pill-warning" | "pill-info" } | null {
  switch (kind) {
    case "evaluation":
    case "inconsistent":
      return { label: "Evaluation", tone: "pill-warning" };
    case "unreadable":
      return { label: "Licence unreadable", tone: "pill-warning" };
    case "licensed":
      return { label: "Licensed", tone: "pill-info" };
    case "unmarked":
      return null;
  }
}

/** Whether the Licence field's title is shown in warning colour. */
export function licenceIsWarning(kind: LicenceKind): boolean {
  return licencePill(kind)?.tone === "pill-warning";
}
