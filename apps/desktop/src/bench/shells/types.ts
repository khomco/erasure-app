import type { ReactNode } from "react";

import type { EnclosureKind } from "@/api/types";

/**
 * Enclosure shell artwork (ADR-0004 §3).
 *
 * A shell is the *housing* — chassis, vents, buttons, rack ears — and nothing
 * else. It declares a rectangle, the **bay slot**, into which `BayMap` draws
 * the status layer using exactly the same code it uses for a generic
 * enclosure. That separation is what protects legibility: recognisable art
 * cannot quietly degrade the thing the bay map exists for.
 *
 * Three rules, enforced by `shells/contract.test.ts` rather than left to
 * reviewer diligence:
 *
 *  1. **No status colour in shell artwork.** Housing tones only, from
 *     `tokens.ts`. Every colour a bay carries comes from the status layer.
 *  2. **Tones stay inside the luminance band** so every status colour keeps
 *     its contrast against them.
 *  3. **The bay slot is exclusive.** No detail inside it. A shape must either
 *     span the whole slot — a flat backing panel, which the bank cage covers
 *     anyway — or stay clear of it. Half-intruding logos, vent holes and
 *     markings show through the gaps between trays and compete with status.
 *
 * A model whose art cannot satisfy these ships spec-only, with the generic
 * shell. Recognition is worth something; legibility is worth more.
 */
export interface ShellDef {
  /** Registry key. Matches `EnclosureModel.art` in the catalog. */
  readonly key: string;
  /** Human name, for the picker and for contract-test failure messages. */
  readonly title: string;
  /** Which enclosure kinds this artwork is appropriate for. */
  readonly kinds: readonly EnclosureKind[];
  /** Intrinsic drawing size. The whole shell lives in `0 0 w h`. */
  readonly viewBox: { w: number; h: number };
  /**
   * Where the bank grid is drawn. Declared, not inferred, so the contract
   * test can assert nothing else paints inside it.
   */
  readonly baySlot: { x: number; y: number; w: number; h: number };
  /** The housing. Must not draw inside `baySlot`. */
  render(): ReactNode;
}

/** Everything a shell may need. Deliberately tiny — shells are decoration. */
export interface ShellContext {
  label: string;
  kind: EnclosureKind;
}
