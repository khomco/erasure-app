import { describe, expect, it } from "vitest";

import type { NumberingRun } from "@/api/types";
import {
  DEFAULT_RUN,
  emptyTopology,
  generateBays,
  hasErrors,
  labelPreview,
  newBank,
  newEnclosure,
  rebuildBank,
  validateLocal,
} from "./topologyEdit";

const run = (over: Partial<NumberingRun> = {}): NumberingRun => ({
  ...DEFAULT_RUN,
  ...over,
});

const at = (bays: ReturnType<typeof generateBays>, label: string) => {
  const b = bays.find((x) => x.label === label);
  if (!b) throw new Error(`no bay labelled ${label}`);
  return [b.row, b.col];
};

describe("generateBays — numbering runs", () => {
  it("walks across rows from the top left", () => {
    const bays = generateBays("e", "a", 2, 3, run());
    expect(bays).toHaveLength(6);
    expect(at(bays, "1")).toEqual([0, 0]);
    expect(at(bays, "3")).toEqual([0, 2]);
    expect(at(bays, "4")).toEqual([1, 0]);
  });

  it("walks down columns when asked", () => {
    // The reference chassis numbers this way; getting it wrong puts every
    // label on the wrong tray.
    const bays = generateBays("e", "a", 4, 2, run({ order: "column_major" }));
    expect(at(bays, "1")).toEqual([0, 0]);
    expect(at(bays, "4")).toEqual([3, 0]);
    expect(at(bays, "5")).toEqual([0, 1]);
  });

  it("flips the grid for each origin corner", () => {
    const rows = 2, cols = 3;
    expect(at(generateBays("e", "a", rows, cols, run({ origin: "top_left" })), "1")).toEqual([0, 0]);
    expect(at(generateBays("e", "a", rows, cols, run({ origin: "top_right" })), "1")).toEqual([0, 2]);
    expect(at(generateBays("e", "a", rows, cols, run({ origin: "bottom_left" })), "1")).toEqual([1, 0]);
    expect(at(generateBays("e", "a", rows, cols, run({ origin: "bottom_right" })), "1")).toEqual([1, 2]);
  });

  it("honours a start offset so a second bank can continue the run", () => {
    const bays = generateBays("e", "b", 2, 2, run({ label_start: 25 }));
    expect(bays.map((b) => b.label)).toEqual(["25", "26", "27", "28"]);
  });

  it("supports 0-based benches", () => {
    const bays = generateBays("e", "a", 1, 3, run({ label_start: 0 }));
    expect(bays.map((b) => b.label)).toEqual(["0", "1", "2"]);
  });

  it("gives every bay a position-derived id, so labels can be renamed freely", () => {
    const bays = generateBays("e", "a", 2, 2, run());
    const ids = new Set(bays.map((b) => b.id));
    expect(ids.size).toBe(4);
    for (const b of bays) expect(b.id).toBe(`e.a.r${b.row}c${b.col}`);
  });
});

describe("rebuildBank", () => {
  it("keeps operator edits on the same physical slot when renumbering", () => {
    const enc = newEnclosure();
    let bank = newBank(enc.id, 0); // 4x6
    bank = rebuildBank(enc.id, bank, { rows: 2, cols: 2 });

    const target = bank.bays.find((b) => b.row === 1 && b.col === 1)!;
    target.binding = { by: "path", path: "/dev/sdz" };
    target.disabled = false;
    target.form_factor = "m2";

    const renumbered = rebuildBank(enc.id, bank, {
      numbering: run({ order: "column_major", label_start: 100 }),
    });

    const same = renumbered.bays.find((b) => b.row === 1 && b.col === 1)!;
    expect(same.binding).toEqual({ by: "path", path: "/dev/sdz" });
    expect(same.form_factor).toBe("m2");
    // ...but the label follows the new run.
    expect(renumbered.bays.map((b) => b.label).sort()).toEqual(
      ["100", "101", "102", "103"].sort(),
    );
  });

  it("drops bays that fall outside a shrunken grid", () => {
    const enc = newEnclosure();
    let bank = rebuildBank(enc.id, newBank(enc.id, 0), { rows: 3, cols: 3 });
    expect(bank.bays).toHaveLength(9);
    bank = rebuildBank(enc.id, bank, { rows: 2, cols: 2 });
    expect(bank.bays).toHaveLength(4);
    expect(bank.bays.every((b) => b.row < 2 && b.col < 2)).toBe(true);
  });

  it("never produces a zero-sized grid", () => {
    const enc = newEnclosure();
    const bank = rebuildBank(enc.id, newBank(enc.id, 0), { rows: 0, cols: 0 });
    expect(bank.rows).toBeGreaterThanOrEqual(1);
    expect(bank.cols).toBeGreaterThanOrEqual(1);
  });
});

describe("labelPreview", () => {
  it("elides a long run but still shows where it ends", () => {
    // This echo is how an operator checks the run against the metal without
    // counting squares, so the last label matters as much as the first.
    expect(labelPreview(4, 6, run())).toBe("1, 2, 3, 4, 5, 6, 7 … 24");
  });

  it("shows a short run in full", () => {
    expect(labelPreview(1, 2, run())).toBe("1, 2");
  });

  it("handles an empty grid", () => {
    expect(labelPreview(0, 0, run())).toBe("—");
  });
});

describe("validateLocal", () => {
  it("passes a freshly seeded enclosure", () => {
    const t = { ...emptyTopology(), enclosures: [newEnclosure()] };
    expect(hasErrors(validateLocal(t))).toBe(false);
  });

  it("blocks duplicate labels within a bank", () => {
    const enc = newEnclosure();
    enc.banks[0].bays[1].label = enc.banks[0].bays[0].label;
    const problems = validateLocal({ ...emptyTopology(), enclosures: [enc] });
    expect(hasErrors(problems)).toBe(true);
    expect(problems.some((p) => p.code === "duplicate_bay_label")).toBe(true);
  });

  it("warns but does not block when auto-fill covers a large bench", () => {
    // Enumeration order is a guess; on a big bench it is the kind of guess an
    // operator stops double-checking.
    const t = { ...emptyTopology(), enclosures: [newEnclosure()] }; // 24 bays
    const problems = validateLocal(t);
    expect(problems.some((p) => p.code === "auto_fill_on_large_bench")).toBe(true);
    expect(hasErrors(problems)).toBe(false);
  });

  it("does not nag about auto-fill on a small bench", () => {
    const enc = newEnclosure();
    enc.banks[0] = rebuildBank(enc.id, enc.banks[0], { rows: 1, cols: 2 });
    const problems = validateLocal({ ...emptyTopology(), enclosures: [enc] });
    expect(problems.some((p) => p.code === "auto_fill_on_large_bench")).toBe(false);
  });

  it("notes an empty bench", () => {
    const problems = validateLocal(emptyTopology());
    expect(problems.some((p) => p.code === "no_bays")).toBe(true);
  });
});
