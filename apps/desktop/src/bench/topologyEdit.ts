import type {
  Bank,
  Bay,
  BayFormFactor,
  BayOrder,
  BayOrigin,
  BayTopology,
  Enclosure,
  EnclosureKind,
  EnclosureModel,
  NumberingRun,
  TopologyProblem,
  TrayOrientation,
} from "@/api/types";

/**
 * Pure editing helpers for the bench-setup builder.
 *
 * Grid generation mirrors `wipe_common::grid_bank` so the editor can preview a
 * numbering change without a round trip. The *server* remains authoritative:
 * it re-validates on save and its answer is what the operator is shown if the
 * two ever disagree.
 */

export const SCHEMA_VERSION = 1;

export const ENCLOSURE_KINDS: { value: EnclosureKind; label: string }[] = [
  { value: "rackmount", label: "Rackmount chassis" },
  { value: "duplicator", label: "Benchtop duplicator" },
  { value: "dock", label: "Hot-swap dock" },
  { value: "nvme_carrier", label: "NVMe carrier" },
  { value: "usb_caddy", label: "USB caddy" },
  { value: "internal", label: "Internal / not operator-facing" },
];

export const FORM_FACTORS: { value: BayFormFactor; label: string }[] = [
  { value: "3.5in", label: '3.5" (LFF)' },
  { value: "2.5in", label: '2.5" (SFF)' },
  { value: "m2", label: "M.2" },
  { value: "u2", label: "U.2 / U.3" },
  { value: "other", label: "Other" },
];

export const ORIGINS: { value: BayOrigin; row: number; col: number }[] = [
  { value: "top_left", row: 0, col: 0 },
  { value: "top_right", row: 0, col: 1 },
  { value: "bottom_left", row: 1, col: 0 },
  { value: "bottom_right", row: 1, col: 1 },
];

export const DEFAULT_RUN: NumberingRun = {
  order: "row_major",
  origin: "top_left",
  label_start: 1,
};

/** Generate a bank's bays for a grid + numbering run. Mirrors `grid_bank`. */
export function generateBays(
  enclosureId: string,
  bankId: string,
  rows: number,
  cols: number,
  run: NumberingRun,
): Bay[] {
  const bays: Bay[] = [];
  const [outer, inner] =
    run.order === "row_major" ? [rows, cols] : [cols, rows];
  let n = run.label_start;
  for (let o = 0; o < outer; o++) {
    for (let i = 0; i < inner; i++) {
      let row = run.order === "row_major" ? o : i;
      let col = run.order === "row_major" ? i : o;
      if (run.origin === "top_right" || run.origin === "bottom_right") {
        col = cols - 1 - col;
      }
      if (run.origin === "bottom_left" || run.origin === "bottom_right") {
        row = rows - 1 - row;
      }
      bays.push({
        // Position-derived, matching the Rust side: renaming a bay must not
        // change its identity.
        id: `${enclosureId}.${bankId}.r${row}c${col}`,
        label: String(n),
        row,
        col,
        binding: { by: "unbound" },
        disabled: false,
      });
      n += 1;
    }
  }
  return bays;
}

/**
 * Re-run a bank's numbering over a (possibly resized) grid, carrying anything
 * the operator set by hand across by grid position. Mirrors `renumber_bank`.
 */
export function rebuildBank(
  enclosureId: string,
  bank: Bank,
  patch: Partial<Pick<Bank, "rows" | "cols">> & { numbering?: NumberingRun },
): Bank {
  const rows = Math.max(1, patch.rows ?? bank.rows);
  const cols = Math.max(1, patch.cols ?? bank.cols);
  const run = patch.numbering ?? bank.numbering ?? DEFAULT_RUN;

  const prev = new Map(bank.bays.map((b) => [`${b.row}:${b.col}`, b]));
  const bays = generateBays(enclosureId, bank.id, rows, cols, run).map((b) => {
    const was = prev.get(`${b.row}:${b.col}`);
    if (!was) return b;
    // Labels follow the new run; everything physical follows the slot.
    return {
      ...b,
      binding: was.binding,
      disabled: was.disabled,
      form_factor: was.form_factor,
      note: was.note,
    };
  });

  return { ...bank, rows, cols, numbering: run, bays };
}

let seq = 0;
function nextId(prefix: string): string {
  seq += 1;
  return `${prefix}${seq}`;
}

export function newBank(enclosureId: string, index: number): Bank {
  const id = nextId("bank");
  const bank: Bank = {
    id,
    label: index === 0 ? null : `Bank ${String.fromCharCode(65 + index)}`,
    rows: 4,
    cols: 6,
    form_factor: "3.5in",
    orientation: "horizontal",
    numbering: DEFAULT_RUN,
    bays: [],
  };
  return { ...bank, bays: generateBays(enclosureId, id, 4, 6, DEFAULT_RUN) };
}

export function newEnclosure(kind: EnclosureKind = "rackmount"): Enclosure {
  const id = nextId("enc");
  return {
    id,
    label: "New enclosure",
    kind,
    banks: [newBank(id, 0)],
  };
}

/**
 * Expand a catalog model into an ordinary enclosure (ADR-0004).
 *
 * Mirrors `EnclosureModel::expand` in wipe-common. The catalog is a source of
 * *defaults*: from here on this is the operator's layout, editable like any
 * other, and `model_ref` only tells the renderer which artwork to use.
 */
export function enclosureFromModel(model: EnclosureModel): Enclosure {
  const id = nextId("enc");
  const banks: Bank[] = model.spec.banks.map((spec, i) => {
    const bankId = `b${i + 1}`;
    const run: NumberingRun = {
      order: spec.order,
      origin: spec.origin,
      label_start: spec.label_start,
    };
    return {
      id: bankId,
      label: spec.label ?? null,
      rows: spec.rows,
      cols: spec.cols,
      form_factor: spec.form_factor,
      orientation: spec.orientation,
      numbering: run,
      bays: generateBays(id, bankId, spec.rows, spec.cols, run),
    };
  });
  return {
    id,
    label: `${model.vendor} ${model.product}`,
    kind: model.kind,
    model_ref: model.id,
    banks,
    note: model.spec.notes ?? null,
  };
}

export function emptyTopology(label = "Bench 1"): BayTopology {
  return {
    schema_version: SCHEMA_VERSION,
    label,
    generated: false,
    auto_fill_unbound: true,
    revision: 0,
    enclosures: [],
  };
}

/** Total bays, for headers and the auto-fill warning threshold. */
export function bayCount(t: BayTopology): number {
  return t.enclosures.reduce(
    (a, e) => a + e.banks.reduce((b, k) => b + k.bays.length, 0),
    0,
  );
}

/** The first few labels a run produces, for the live echo under the controls. */
export function labelPreview(
  rows: number,
  cols: number,
  run: NumberingRun,
  take = 7,
): string {
  const total = Math.max(0, rows * cols);
  if (total === 0) return "—";
  const first: number[] = [];
  for (let i = 0; i < Math.min(take, total); i++) first.push(run.label_start + i);
  const last = run.label_start + total - 1;
  const head = first.join(", ");
  return total > take ? `${head} … ${last}` : head;
}

/**
 * Client-side validation for live feedback.
 *
 * Deliberately a subset of `BayTopology::validate` — the rules that change as
 * you type. The server re-runs the full set on save and blocks there; this is
 * a preview, not the gate.
 */
export function validateLocal(t: BayTopology): TopologyProblem[] {
  const out: TopologyProblem[] = [];
  const encIds = new Set<string>();

  for (const enc of t.enclosures) {
    if (encIds.has(enc.id)) {
      out.push({
        severity: "error",
        code: "duplicate_enclosure_id",
        message: `enclosure id \`${enc.id}\` is used more than once`,
        enclosure_id: enc.id,
      });
    }
    encIds.add(enc.id);

    for (const bank of enc.banks) {
      if (bank.rows < 1 || bank.cols < 1) {
        out.push({
          severity: "error",
          code: "empty_grid",
          message: `bank \`${bank.label ?? bank.id}\` needs at least one row and column`,
          enclosure_id: enc.id,
          bank_id: bank.id,
        });
      }
      const seen = new Map<string, number>();
      for (const bay of bank.bays) {
        seen.set(bay.label, (seen.get(bay.label) ?? 0) + 1);
      }
      for (const [label, n] of seen) {
        if (n > 1) {
          out.push({
            severity: "error",
            code: "duplicate_bay_label",
            message: `bank \`${bank.label ?? bank.id}\` has ${n} bays labelled \`${label}\` — labels must be unique within a bank`,
            enclosure_id: enc.id,
            bank_id: bank.id,
          });
        }
      }
    }
  }

  const total = bayCount(t);
  if (total === 0) {
    out.push({
      severity: "warning",
      code: "no_bays",
      message: "this bench has no bays",
    });
  }
  if (t.auto_fill_unbound && total > 8) {
    out.push({
      severity: "warning",
      code: "auto_fill_on_large_bench",
      message: `${total} bays are filled in device-enumeration order, which does not reflect physical position — bind them to be sure the map matches the metal`,
    });
  }
  return out;
}

export function hasErrors(problems: TopologyProblem[]): boolean {
  return problems.some((p) => p.severity === "error");
}

/** Immutable update of one bank inside a topology. */
export function withBank(
  t: BayTopology,
  encId: string,
  bankId: string,
  fn: (bank: Bank) => Bank,
): BayTopology {
  return {
    ...t,
    enclosures: t.enclosures.map((e) =>
      e.id !== encId
        ? e
        : { ...e, banks: e.banks.map((b) => (b.id !== bankId ? b : fn(b))) },
    ),
  };
}

/** Immutable update of one bay. */
export function withBay(
  t: BayTopology,
  bayId: string,
  fn: (bay: Bay) => Bay,
): BayTopology {
  return {
    ...t,
    enclosures: t.enclosures.map((e) => ({
      ...e,
      banks: e.banks.map((b) => ({
        ...b,
        bays: b.bays.map((bay) => (bay.id !== bayId ? bay : fn(bay))),
      })),
    })),
  };
}

export function findBay(t: BayTopology, bayId: string | null) {
  if (!bayId) return null;
  for (const enc of t.enclosures) {
    for (const bank of enc.banks) {
      const bay = bank.bays.find((b) => b.id === bayId);
      if (bay) return { enc, bank, bay };
    }
  }
  return null;
}

export const ORIENTATIONS: { value: TrayOrientation; label: string }[] = [
  { value: "horizontal", label: "Horizontal (wide trays, stacked)" },
  { value: "vertical", label: "Vertical (tall trays, side by side)" },
];

export const ORDERS: { value: BayOrder; label: string }[] = [
  { value: "row_major", label: "Across rows" },
  { value: "column_major", label: "Down columns" },
];
