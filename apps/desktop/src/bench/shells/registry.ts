import type { EnclosureKind } from "@/api/types";
import { genericShell } from "./GenericShell";
import { MODEL_SHELLS } from "./models";
import type { ShellDef } from "./types";

/**
 * Shell lookup and slot fitting (ADR-0004 §3).
 *
 * `EnclosureModel.art` is a key into this registry. A key we do not have —
 * catalog newer than the UI, or a site overlay naming art we never shipped —
 * falls through to the generic shell rather than failing to render. The bay
 * map is the thing an operator uses to find a drive; it does not get to be
 * unavailable because artwork is missing.
 */

const BY_KEY = new Map<string, ShellDef>(MODEL_SHELLS.map((s) => [s.key, s]));

export function shellByKey(key: string | null | undefined): ShellDef | null {
  if (!key) return null;
  return BY_KEY.get(key) ?? null;
}

export function registeredShells(): readonly ShellDef[] {
  return MODEL_SHELLS;
}

/**
 * The shell to draw for one enclosure.
 *
 * `art` is honoured only when the shell also declares the enclosure's kind.
 * A dock rendered in a rackmount chassis because a catalog entry pointed at
 * the wrong art would be a lie told confidently, so a mismatch degrades to
 * generic instead.
 */
export function shellFor(
  kind: EnclosureKind,
  art: string | null | undefined,
  content: { w: number; h: number },
): { shell: ShellDef; recognised: boolean } {
  const found = shellByKey(art);
  if (found && found.kinds.includes(kind)) return { shell: found, recognised: true };
  return { shell: genericShell(kind, content), recognised: false };
}

/**
 * Fit the bank layer into a shell's declared bay slot.
 *
 * Uniform scale, centred: the grid pitch is a physical fact about the chassis
 * and must not be stretched to fill artwork.
 */
export function fitToSlot(
  slot: { x: number; y: number; w: number; h: number },
  content: { w: number; h: number },
): { scale: number; x: number; y: number } {
  if (content.w <= 0 || content.h <= 0) return { scale: 1, x: slot.x, y: slot.y };
  const scale = Math.min(slot.w / content.w, slot.h / content.h);
  return {
    scale,
    x: slot.x + (slot.w - content.w * scale) / 2,
    y: slot.y + (slot.h - content.h * scale) / 2,
  };
}

/**
 * Rendered width in CSS pixels for a shell whose slot was fitted at `scale`.
 *
 * Chosen so bays come out at their natural size regardless of how much
 * housing surrounds them — otherwise a chassis with a generous bezel would
 * shrink its own bay labels below readability. Clamped because artwork with
 * an extreme slot ratio should not be able to demand a 6000px canvas.
 */
export function renderWidth(shell: ShellDef, scale: number): number {
  const k = Math.min(2.5, Math.max(0.6, 1 / (scale || 1)));
  return Math.round(shell.viewBox.w * k);
}

export { genericShell };
export type { ShellDef };
