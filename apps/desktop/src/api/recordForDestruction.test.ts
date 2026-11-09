import { describe, expect, it } from "vitest";

import type { OperatorRef } from "./types";
import {
  addScannedDrive,
  canPreview,
  previewKey,
  readyToRecord,
  toPreviewRequest,
  toRecordRequest,
  type RecordForm,
} from "./recordForDestruction";

const operator: OperatorRef = { id: "op-1", display_name: "Ana Ruiz", email: "ana@example.com" };

function form(overrides: Partial<RecordForm> = {}): RecordForm {
  return {
    method: "shred",
    equipment_id: "SHR-2",
    location: "Bay 3",
    full_name: "Ana Ruiz",
    title: "Bench lead",
    notes: "",
    identifiers: { from: "device" },
    ...overrides,
  };
}

describe("addScannedDrive", () => {
  it("trims and appends, keeping a missing identifier null", () => {
    const r = addScannedDrive([], { asset_tag: "  A-1 ", serial: "  " });
    expect(r).toEqual({ ok: true, list: [{ asset_tag: "A-1", serial: null }] });
  });

  it("refuses a drive with no identifier", () => {
    const r = addScannedDrive([], { asset_tag: " ", serial: "" });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/asset tag or a serial/);
  });

  it("refuses a repeated asset tag or serial, ignoring case", () => {
    const list = [{ asset_tag: "A-1", serial: "S1" }];
    const tag = addScannedDrive(list, { asset_tag: "a-1", serial: "S2" });
    expect(tag).toEqual({ ok: false, reason: "Asset tag a-1 is already in the list." });
    const serial = addScannedDrive(list, { asset_tag: "A-2", serial: " S1 " });
    expect(serial).toEqual({ ok: false, reason: "Serial S1 is already in the list." });
    expect(list).toHaveLength(1);
  });

  it("does not treat two missing identifiers as a repeat", () => {
    const r = addScannedDrive([{ asset_tag: null, serial: "S1" }], { serial: "S2" });
    expect(r.ok).toBe(true);
  });
});

describe("canPreview", () => {
  it("needs equipment, location, full name and title", () => {
    expect(canPreview(form())).toBe(true);
    for (const field of ["equipment_id", "location", "full_name", "title"] as const) {
      expect(canPreview(form({ [field]: "  " }))).toBe(false);
    }
  });

  it("does not need notes", () => {
    expect(canPreview(form({ notes: "" }))).toBe(true);
  });
});

describe("previewKey", () => {
  it("changes with the statement's inputs and not with notes or identifiers", () => {
    const base = previewKey(form());
    expect(previewKey(form({ location: "Bay 4" }))).not.toBe(base);
    expect(previewKey(form({ method: "crush" }))).not.toBe(base);
    expect(previewKey(form({ notes: "x" }))).toBe(base);
    expect(
      previewKey(form({ identifiers: { from: "scanned", asset_tag: "A", serial: "" } })),
    ).toBe(base);
    expect(toPreviewRequest(form())).toEqual({
      method: "shred",
      equipment_id: "SHR-2",
      location: "Bay 3",
      full_name: "Ana Ruiz",
      title: "Bench lead",
    });
  });
});

describe("toRecordRequest", () => {
  it("omits identifiers when they are read from the device", () => {
    const body = toRecordRequest(form(), operator, true);
    expect(body).toEqual({
      method: "shred",
      equipment_id: "SHR-2",
      location: "Bay 3",
      operator,
      full_name: "Ana Ruiz",
      title: "Bench lead",
      attest: true,
      notes: null,
    });
    expect("identifiers" in body).toBe(false);
  });

  it("sends trimmed scanned identifiers and notes", () => {
    const body = toRecordRequest(
      form({
        notes: " bricked ",
        identifiers: { from: "scanned", asset_tag: " ", serial: " S1 " },
      }),
      operator,
      false,
    );
    expect(body.identifiers).toEqual({ asset_tag: null, serial: "S1" });
    expect(body.notes).toBe("bricked");
    expect(body.attest).toBe(false);
  });
});

describe("readyToRecord", () => {
  const reviewed = { key: previewKey(form()), statement: "I, Ana Ruiz, …" };

  it("is true when attested against the current inputs", () => {
    expect(readyToRecord(form(), reviewed, true)).toBe(true);
  });

  it("is false when not attested", () => {
    expect(readyToRecord(form(), reviewed, false)).toBe(false);
  });

  it("is false when no statement was reviewed", () => {
    expect(readyToRecord(form(), null, true)).toBe(false);
  });

  it("is false once any statement input changes", () => {
    const changes: Partial<RecordForm>[] = [
      { method: "crush" },
      { equipment_id: "SHR-3" },
      { location: "Bay 4" },
      { full_name: "Ana R." },
      { title: "Supervisor" },
    ];
    for (const change of changes) {
      expect(readyToRecord(form(change), reviewed, true)).toBe(false);
    }
  });
});
