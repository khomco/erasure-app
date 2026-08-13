/**
 * Housing tones for enclosure shell artwork (ADR-0004 §3).
 *
 * Deliberately a narrow, dark band. Every status colour in `slotStatus.ts`
 * has to keep its contrast against whatever housing sits behind it, and the
 * cheapest way to guarantee that across a growing catalog is to give authors
 * a small fixed palette rather than a colour picker.
 *
 * `shells/contract.test.ts` asserts that registered shells use only these
 * values, so an author who reaches for a nicer grey finds out immediately
 * rather than at review time — or, worse, on someone's bench.
 */

/** The only colours shell artwork may use. */
export const SHELL_TOKENS = {
  /** Deepest recess — vent holes, slot mouths, screw holes. */
  void: "#05080d",
  /** Recessed panel, cage interior. */
  dark: "#0d1117",
  /** Default housing. */
  body: "#151a22",
  /** Raised or lit face — front panels, rails. */
  lit: "#1e252f",
  /** Edge / outline. */
  edge: "#2b3442",
  /** Softer internal division. */
  edgeSoft: "#222a36",
  /** Text on housing. Deliberately not any bay-label grey: a marking
   *  silkscreened on the chassis must not read as a bay's own label. */
  ink: "#8a94a6",
  /** Dimmed housing text — model names, unit markings. */
  inkDim: "#5b6472",
} as const;

export type ShellToken = keyof typeof SHELL_TOKENS;

/**
 * Tones for shapes. Held below the luminance ceiling so no status colour can
 * lose contrast against the housing it sits on.
 */
export const SHELL_FILL_COLORS: readonly string[] = [
  SHELL_TOKENS.void,
  SHELL_TOKENS.dark,
  SHELL_TOKENS.body,
  SHELL_TOKENS.lit,
  SHELL_TOKENS.edge,
  SHELL_TOKENS.edgeSoft,
];

/**
 * Tones for text only. These are *above* the ceiling on purpose — housing
 * markings have to be readable — which is why they are barred from shapes.
 */
export const SHELL_INK_COLORS: readonly string[] = [
  SHELL_TOKENS.ink,
  SHELL_TOKENS.inkDim,
];

/** All permitted colour values, for the contract test. */
export const ALLOWED_SHELL_COLORS: readonly string[] = Object.values(SHELL_TOKENS);

/**
 * Relative luminance ceiling for housing shapes (0..1).
 *
 * Chosen so the darkest status stroke (`empty`, #1e293b) still separates from
 * the lightest permitted housing. Enforced by the contract test.
 */
export const MAX_SHELL_LUMINANCE = 0.06;

export function relativeLuminance(hex: string): number {
  const m = /^#([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return 1; // unknown format: fail the check rather than pass it
  const n = parseInt(m[1], 16);
  const srgb = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * srgb[0] + 0.7152 * srgb[1] + 0.0722 * srgb[2];
}

export const MONO = "ui-monospace, SFMono-Regular, Menlo, monospace";
export const SANS = "Inter, Helvetica, Arial, sans-serif";
