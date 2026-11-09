/**
 * Destruction techniques and identity assurance, as the operator reads them
 * (ADR-0011 §4–§5). Mirrors `wipe_common::DestructMethod` and
 * `wipe_common::IdentityAssurance`; the literals are pinned in both
 * `destruction.test.ts` and the Rust tests, so change them together.
 */
import type { Classification, DestructMethod, IdentityAssurance } from "./types";

/** Every technique in `DestructMethod::ALL` order: NIST Destroy first. */
export const DESTRUCTION_TECHNIQUES = [
  "shred",
  "disintegrate",
  "incinerate",
  "pulverize",
  "melt",
  "crush",
  "punch",
  "degauss",
  "drill",
] as const satisfies readonly DestructMethod[];

export const NIST_DESTROY_LABEL = "NIST SP 800-88 Destroy";
export const NOT_NIST_DESTROY_LABEL = "physical damage — not NIST Destroy";
export const SHRED_PULVERIZE_CAUTION =
  "NIST SP 800-88 Rev. 2: shred and pulverize should be avoided for anything but the lowest security categories.";

const NIST_DESTROY: ReadonlySet<DestructMethod> = new Set([
  "shred",
  "disintegrate",
  "incinerate",
  "pulverize",
  "melt",
]);

const NAMES: Record<DestructMethod, string> = {
  shred: "Shred",
  disintegrate: "Disintegrate",
  incinerate: "Incinerate",
  pulverize: "Pulverize",
  melt: "Melt",
  crush: "Crush",
  punch: "Punch",
  degauss: "Degauss",
  drill: "Drill",
};

/** Whether NIST SP 800-88 Rev. 2 §3.1.3 counts the technique as Destroy. */
export function isNistDestroy(technique: DestructMethod): boolean {
  return NIST_DESTROY.has(technique);
}

export function techniqueName(technique: DestructMethod): string {
  return NAMES[technique];
}

export function techniqueStandardLabel(technique: DestructMethod): string {
  return isNistDestroy(technique) ? NIST_DESTROY_LABEL : NOT_NIST_DESTROY_LABEL;
}

/** The shred/pulverize caution above Low classification; null otherwise. */
export function techniqueCaution(
  technique: DestructMethod,
  classification: Classification,
): string | null {
  if (classification === "low") return null;
  return technique === "shred" || technique === "pulverize" ? SHRED_PULVERIZE_CAUTION : null;
}

export function identityAssuranceLabel(assurance: IdentityAssurance): string {
  return assurance.level === "verified" ? `Verified by ${assurance.issuer}` : "Self-asserted";
}
