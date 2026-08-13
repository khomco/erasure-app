import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

import { SLOT_TONE } from "../slotStatus";
import { genericShell } from "./GenericShell";
import { fitToSlot, registeredShells, renderWidth } from "./registry";
import {
  MAX_SHELL_LUMINANCE,
  SHELL_FILL_COLORS,
  SHELL_INK_COLORS,
  relativeLuminance,
} from "./tokens";
import type { ShellDef } from "./types";

/**
 * The legibility contract for enclosure artwork (ADR-0004 §3).
 *
 * These are machine checks rather than review notes on purpose. The catalog is
 * meant to grow — a shell per model, contributed over time, most of them by
 * someone who has the hardware in front of them and not this ADR. Anything the
 * review process is supposed to catch every single time will eventually not be
 * caught, and the failure mode is a bay map that looks better and reads worse.
 *
 * A shell that cannot satisfy these ships spec-only, on the generic shell.
 */

// --- a small SVG geometry reader ------------------------------------------

interface Shape {
  tag: string;
  attrs: Record<string, string>;
  box: { x1: number; y1: number; x2: number; y2: number };
}

const MEASURABLE = ["rect", "circle", "ellipse", "line", "polygon", "text"];

function attrs(raw: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const m of raw.matchAll(/([a-zA-Z-]+)="([^"]*)"/g)) out[m[1]] = m[2];
  return out;
}

function num(a: Record<string, string>, k: string, fallback = 0): number {
  const v = parseFloat(a[k]);
  return Number.isFinite(v) ? v : fallback;
}

function boxOf(tag: string, a: Record<string, string>) {
  switch (tag) {
    case "rect": {
      const x = num(a, "x");
      const y = num(a, "y");
      return { x1: x, y1: y, x2: x + num(a, "width"), y2: y + num(a, "height") };
    }
    case "circle": {
      const r = num(a, "r");
      return {
        x1: num(a, "cx") - r,
        y1: num(a, "cy") - r,
        x2: num(a, "cx") + r,
        y2: num(a, "cy") + r,
      };
    }
    case "ellipse": {
      const rx = num(a, "rx");
      const ry = num(a, "ry");
      return {
        x1: num(a, "cx") - rx,
        y1: num(a, "cy") - ry,
        x2: num(a, "cx") + rx,
        y2: num(a, "cy") + ry,
      };
    }
    case "line":
      return {
        x1: Math.min(num(a, "x1"), num(a, "x2")),
        y1: Math.min(num(a, "y1"), num(a, "y2")),
        x2: Math.max(num(a, "x1"), num(a, "x2")),
        y2: Math.max(num(a, "y1"), num(a, "y2")),
      };
    case "polygon": {
      const pts = (a.points ?? "")
        .trim()
        .split(/\s+/)
        .map((p) => p.split(",").map(Number))
        .filter((p) => p.length === 2 && p.every(Number.isFinite));
      const xs = pts.map((p) => p[0]);
      const ys = pts.map((p) => p[1]);
      return {
        x1: Math.min(...xs),
        y1: Math.min(...ys),
        x2: Math.max(...xs),
        y2: Math.max(...ys),
      };
    }
    default:
      return { x1: 0, y1: 0, x2: 0, y2: 0 };
  }
}

function markupOf(shell: ShellDef): { markup: string; texts: string[] } {
  const markup = renderToStaticMarkup(
    createElement("svg", {}, shell.render()) as never,
  );
  const texts = [...markup.matchAll(/<text[^>]*>([\s\S]*?)<\/text>/g)].map((m) =>
    m[1].replace(/<[^>]*>/g, "").trim(),
  );
  return { markup, texts };
}

function shapesOf(shell: ShellDef): Shape[] {
  const { markup, texts } = markupOf(shell);
  const shapes: Shape[] = [];
  let textIndex = 0;
  for (const m of markup.matchAll(/<([a-zA-Z]+)([^>]*?)\/?>/g)) {
    const tag = m[1];
    if (tag === "g" || tag === "svg" || tag === "title") continue;
    const a = attrs(m[2]);
    if (tag === "text") {
      // Text extent is approximated, and deliberately generously: a marking
      // that only *nearly* reaches the bay slot should still fail.
      const content = texts[textIndex++] ?? "";
      const size = num(a, "fontSize", num(a, "font-size", 10));
      const w = content.length * size * 0.62;
      const x = num(a, "x");
      const anchor = a["text-anchor"] ?? a.textAnchor ?? "start";
      const x1 = anchor === "end" ? x - w : anchor === "middle" ? x - w / 2 : x;
      const y = num(a, "y");
      shapes.push({
        tag,
        attrs: a,
        box: { x1, y1: y - size, x2: x1 + w, y2: y + size * 0.3 },
      });
      continue;
    }
    shapes.push({ tag, attrs: a, box: boxOf(tag, a) });
  }
  return shapes;
}

function overlaps(
  a: { x1: number; y1: number; x2: number; y2: number },
  b: { x: number; y: number; w: number; h: number },
): boolean {
  // Zero-area shapes cannot obscure anything.
  if (a.x2 <= a.x1 || a.y2 <= a.y1) return false;
  return a.x1 < b.x + b.w && a.x2 > b.x && a.y1 < b.y + b.h && a.y2 > b.y;
}

/** A shape spanning the whole slot is the chassis backing panel, not detail. */
function contains(
  a: { x1: number; y1: number; x2: number; y2: number },
  b: { x: number; y: number; w: number; h: number },
): boolean {
  return a.x1 <= b.x && a.y1 <= b.y && a.x2 >= b.x + b.w && a.y2 >= b.y + b.h;
}

// --- the rules, as functions so they can be tested against bad artwork -----

/** Rule 1: housing tones only. Shapes may not use the ink tones. */
export function tokenViolations(shell: ShellDef): string[] {
  const out: string[] = [];
  for (const s of shapesOf(shell)) {
    for (const key of ["fill", "stroke"]) {
      const v = s.attrs[key];
      if (!v || v === "none") continue;
      const allowed =
        s.tag === "text" ? [...SHELL_FILL_COLORS, ...SHELL_INK_COLORS] : SHELL_FILL_COLORS;
      if (!allowed.includes(v)) out.push(`${s.tag} ${key}="${v}" is not a housing token`);
    }
  }
  return out;
}

/** Rule 2: shape tones stay under the luminance ceiling. */
export function luminanceViolations(shell: ShellDef): string[] {
  const out: string[] = [];
  for (const s of shapesOf(shell)) {
    if (s.tag === "text") continue;
    for (const key of ["fill", "stroke"]) {
      const v = s.attrs[key];
      if (!v || v === "none") continue;
      if (relativeLuminance(v) > MAX_SHELL_LUMINANCE) {
        out.push(`${s.tag} ${key}="${v}" is too bright for status to sit on`);
      }
    }
  }
  return out;
}

/**
 * Rule 3: no detail inside the bay slot.
 *
 * A shape must either span the whole slot — a flat backing panel, which the
 * bank cage covers anyway — or stay clear of it. Anything that only partly
 * intrudes shows through the gaps between trays and competes with the status
 * colours for the operator's attention.
 */
export function slotViolations(shell: ShellDef): string[] {
  const out: string[] = [];
  for (const s of shapesOf(shell)) {
    if (s.tag !== "text" && contains(s.box, shell.baySlot)) continue;
    if (overlaps(s.box, shell.baySlot)) {
      out.push(`${s.tag} at ${JSON.stringify(s.box)} intrudes on the bay slot`);
    }
  }
  return out;
}

/**
 * Rule 3 has a precondition: the checker must be able to see the geometry.
 * A shell drawn with `<path>`, or moved by a transform, would sail past the
 * slot check while sitting anywhere it liked.
 */
export function unmeasurableViolations(shell: ShellDef): string[] {
  const out: string[] = [];
  for (const s of shapesOf(shell)) {
    if (!MEASURABLE.includes(s.tag)) out.push(`<${s.tag}> cannot be measured`);
    if (s.attrs.transform) out.push(`transform on <${s.tag}> defeats the slot check`);
  }
  return out;
}

// --- every registered shell, plus the fallback at several sizes -----------

const SHELLS: ShellDef[] = [
  ...registeredShells(),
  // The fallback is held to the same contract, because it is the shell most
  // enclosures will actually get.
  genericShell("rackmount", { w: 600, h: 300 }),
  genericShell("dock", { w: 140, h: 90 }),
  genericShell("nvme_carrier", { w: 320, h: 120 }),
];

describe.each(SHELLS.map((s) => [s.title, s] as const))("%s", (_title, shell) => {
  it("uses only housing tones", () => {
    expect(tokenViolations(shell)).toEqual([]);
  });

  it("keeps shape tones under the luminance ceiling", () => {
    expect(luminanceViolations(shell)).toEqual([]);
  });

  it("puts no detail inside its own bay slot", () => {
    expect(slotViolations(shell)).toEqual([]);
  });

  it("only uses geometry the contract checker can measure", () => {
    expect(unmeasurableViolations(shell)).toEqual([]);
  });

  it("declares a bay slot that fits inside its own viewBox", () => {
    const { x, y, w, h } = shell.baySlot;
    expect(w).toBeGreaterThan(0);
    expect(h).toBeGreaterThan(0);
    expect(x).toBeGreaterThanOrEqual(0);
    expect(y).toBeGreaterThanOrEqual(0);
    expect(x + w).toBeLessThanOrEqual(shell.viewBox.w);
    expect(y + h).toBeLessThanOrEqual(shell.viewBox.h);
  });

  it("draws something — an empty shell is a silently missing chassis", () => {
    expect(shapesOf(shell).length).toBeGreaterThan(0);
  });
});

// --- the checker itself ----------------------------------------------------
//
// A contract test that cannot fail is decoration. These are shells written to
// break each rule; if one of them stops being reported, the rule has quietly
// stopped being enforced.

function badShell(render: ShellDef["render"]): ShellDef {
  return {
    key: "test/bad",
    title: "deliberately bad",
    kinds: ["dock"],
    viewBox: { w: 200, h: 200 },
    baySlot: { x: 50, y: 50, w: 100, h: 100 },
    render,
  };
}

describe("the contract checker catches violations", () => {
  it("flags a status colour used as housing", () => {
    const shell = badShell(() =>
      createElement("rect", { x: 0, y: 0, width: 10, height: 10, fill: SLOT_TONE.wiping.accent }),
    );
    expect(tokenViolations(shell)).toHaveLength(1);
  });

  it("flags housing brighter than the ceiling", () => {
    const shell = badShell(() =>
      createElement("rect", { x: 0, y: 0, width: 10, height: 10, fill: "#8a94a6" }),
    );
    expect(luminanceViolations(shell)).toHaveLength(1);
  });

  it("flags a vent hole punched through the bay slot", () => {
    const shell = badShell(() =>
      createElement("circle", { cx: 100, cy: 100, r: 4, fill: "#05080d" }),
    );
    expect(slotViolations(shell)).toHaveLength(1);
  });

  it("flags a logo that only half intrudes", () => {
    const shell = badShell(() =>
      createElement("rect", { x: 40, y: 40, width: 30, height: 30, fill: "#151a22" }),
    );
    expect(slotViolations(shell)).toHaveLength(1);
  });

  it("flags a text marking laid across the bays", () => {
    const shell = badShell(() =>
      createElement("text", { x: 60, y: 100, fontSize: 10, fill: "#8a94a6" }, "WIPESTATION"),
    );
    expect(slotViolations(shell)).toHaveLength(1);
  });

  it("allows a flat backing panel spanning the whole slot", () => {
    const shell = badShell(() =>
      createElement("rect", { x: 0, y: 0, width: 200, height: 200, fill: "#151a22" }),
    );
    expect(slotViolations(shell)).toEqual([]);
  });

  it("refuses a <path> it cannot measure", () => {
    const shell = badShell(() =>
      createElement("path", { d: "M0 0 L200 200", fill: "#151a22" }),
    );
    expect(unmeasurableViolations(shell)).toHaveLength(1);
  });

  it("refuses a transform that would move art into the slot unseen", () => {
    const shell = badShell(() =>
      createElement("rect", {
        x: 0,
        y: 0,
        width: 10,
        height: 10,
        fill: "#151a22",
        transform: "translate(90, 90)",
      }),
    );
    expect(unmeasurableViolations(shell)).toHaveLength(1);
    // ...and note the slot check alone would have missed it entirely.
    expect(slotViolations(shell)).toEqual([]);
  });
});

// --- registry-level invariants --------------------------------------------

describe("shell registry", () => {
  it("shares no colour with the status palette", () => {
    // Checked once over the palette rather than per shape: if a housing tone
    // were also a status signal, a chassis feature and a wiping drive would
    // be the same colour, and no amount of review would make that legible.
    const signal = new Set(
      Object.values(SLOT_TONE).flatMap((t) => [t.fill, t.stroke, t.accent]),
    );
    for (const c of [...SHELL_FILL_COLORS, ...SHELL_INK_COLORS]) {
      expect(signal.has(c), `${c} is both a housing tone and a status colour`).toBe(false);
    }
  });

  it("has no duplicate keys", () => {
    const keys = registeredShells().map((s) => s.key);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("declares at least one kind per shell", () => {
    for (const s of registeredShells()) expect(s.kinds.length).toBeGreaterThan(0);
  });
});

// --- fitting ---------------------------------------------------------------

describe("fitToSlot", () => {
  it("scales uniformly, so bay pitch is never stretched to fill artwork", () => {
    const slot = { x: 10, y: 20, w: 400, h: 100 };
    const fit = fitToSlot(slot, { w: 200, h: 100 });
    expect(fit.scale).toBe(1); // limited by height, not width
    expect(fit.x).toBe(10 + (400 - 200) / 2);
    expect(fit.y).toBe(20);
  });

  it("shrinks content that is larger than the slot", () => {
    const fit = fitToSlot({ x: 0, y: 0, w: 100, h: 100 }, { w: 400, h: 200 });
    expect(fit.scale).toBe(0.25);
  });

  it("survives an empty enclosure rather than dividing by zero", () => {
    expect(fitToSlot({ x: 5, y: 6, w: 10, h: 10 }, { w: 0, h: 0 })).toEqual({
      scale: 1,
      x: 5,
      y: 6,
    });
  });

  it("grows the rendered canvas so bays stay their natural size", () => {
    const shell = registeredShells()[0];
    expect(renderWidth(shell, 0.5)).toBeGreaterThan(renderWidth(shell, 1));
    // ...but not without limit, or one odd catalog entry could demand a
    // canvas nobody's screen can show.
    expect(renderWidth(shell, 0.01)).toBeLessThanOrEqual(shell.viewBox.w * 2.5);
  });
});

// --- the fallback ----------------------------------------------------------

describe("generic shell", () => {
  it("says it is generic, in the artwork itself", () => {
    expect(markupOf(genericShell("dock", { w: 120, h: 80 })).texts.join(" ")).toContain(
      "generic",
    );
  });

  it("sizes itself around whatever it has to hold", () => {
    const small = genericShell("dock", { w: 120, h: 80 });
    const large = genericShell("dock", { w: 900, h: 400 });
    expect(small.baySlot.w).toBe(120);
    expect(large.baySlot.w).toBe(900);
    expect(large.viewBox.h).toBeGreaterThan(small.viewBox.h);
  });

  it("gives every enclosure kind a shell, so nothing is unrenderable", () => {
    const kinds = [
      "rackmount",
      "duplicator",
      "dock",
      "nvme_carrier",
      "usb_caddy",
      "internal",
    ] as const;
    for (const k of kinds) {
      const s = genericShell(k, { w: 200, h: 100 });
      expect(s.baySlot.w).toBe(200);
      expect(markupOf(s).texts.join(" ")).toContain("generic");
      expect(slotViolations(s)).toEqual([]);
    }
  });
});
