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
