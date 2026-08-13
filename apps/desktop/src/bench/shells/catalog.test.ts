import { describe, expect, it } from "vitest";

import bundled from "../../../../../crates/wipe-common/data/catalog.json";
import type { EnclosureCatalog, EnclosureModel } from "@/api/types";
import { enclosureFromModel } from "../topologyEdit";
import { registeredShells, shellByKey, shellFor } from "./registry";

/**
 * Where the Rust catalog meets the TypeScript artwork (ADR-0004).
 *
 * The bundled catalog is imported straight from the crate that serves it, so
 * the two halves cannot drift: an entry naming art nobody wrote, or art
 * pointed at the wrong kind of enclosure, fails here rather than showing up as
 * a wrong-looking chassis on someone's bench.
 */

const catalog = bundled as unknown as EnclosureCatalog;
const models: EnclosureModel[] = catalog.models;

describe("bundled catalog against the shell registry", () => {
  it("names only artwork that exists", () => {
    for (const m of models) {
      if (!m.art) continue;
      expect(shellByKey(m.art), `${m.id} names unknown art "${m.art}"`).not.toBeNull();
    }
  });

  it("names artwork appropriate to the enclosure kind", () => {
    for (const m of models) {
      if (!m.art) continue;
      const shell = shellByKey(m.art)!;
      expect(
        shell.kinds,
        `${m.id} is a ${m.kind} but "${m.art}" is for ${shell.kinds.join("/")}`,
      ).toContain(m.kind);
    }
  });

  it("renders every model, with or without bespoke art", () => {
    // The catalog is allowed to grow faster than the artwork does. A model
    // with no art is not a gap — it is the ordinary case, drawn generic and
    // labelled as such.
    for (const m of models) {
      const enc = enclosureFromModel(m);
      const { shell, recognised } = shellFor(enc.kind, m.art, { w: 400, h: 200 });
      expect(shell.baySlot.w).toBeGreaterThan(0);
      expect(recognised).toBe(!!m.art);
    }
  });

  it("has at least one model still on the generic shell", () => {
    // Guards a specific kind of rot: if every bundled model ends up with
    // bespoke art, the fallback path stops being exercised by anything a
    // reviewer actually looks at, and quietly breaks.
    expect(models.some((m) => !m.art)).toBe(true);
  });

  it("expands models to the bay count they advertise", () => {
    // Mirrors the Rust `every_model_expands_into_a_savable_enclosure` test;
    // the point here is that the *editor's* expansion agrees with it.
    for (const m of models) {
      const enc = enclosureFromModel(m);
      const bays = enc.banks.reduce((a, b) => a + b.bays.length, 0);
      const advertised = m.spec.banks.reduce((a, b) => a + b.rows * b.cols, 0);
      expect(bays, m.id).toBe(advertised);
      expect(enc.model_ref).toBe(m.id);
      // Bay ids are position-derived, so they must be unique within an
      // enclosure or bindings would collide.
      const ids = enc.banks.flatMap((b) => b.bays.map((y) => y.id));
      expect(new Set(ids).size, `${m.id} has duplicate bay ids`).toBe(ids.length);
    }
  });

  it("expands bays unbound — the catalog knows shape, never occupancy", () => {
    for (const m of models) {
      for (const bank of enclosureFromModel(m).banks) {
        for (const bay of bank.bays) expect(bay.binding).toEqual({ by: "unbound" });
      }
    }
  });

  it("continues chassis numbering across banks where the model says so", () => {
    const arma = models.find((m) => m.id === "arma/industrial-4u-32");
    expect(arma, "the reference chassis should still be in the catalog").toBeTruthy();
    const enc = enclosureFromModel(arma!);
    const labels = enc.banks.flatMap((b) => b.bays.map((y) => y.label));
    expect(labels).toContain("17");
    expect(labels).toContain("32");
    expect(new Set(labels).size).toBe(labels.length);
  });

  it("keeps every registered shell reachable from some model", () => {
    // Art nobody references is art nobody is looking at. Either a catalog
    // entry points at it or it should not be in the registry.
    const used = new Set(models.map((m) => m.art).filter(Boolean));
    for (const shell of registeredShells()) {
      expect(used.has(shell.key), `no catalog model uses "${shell.key}"`).toBe(true);
    }
  });
});
