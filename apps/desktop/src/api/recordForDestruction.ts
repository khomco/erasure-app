/**
 * Pure helpers for "Record for destruction" (ADR-0011 §1, first step): the
 * Job-page dialog and the scan screen build their requests here. The
 * statement itself is rendered only by the server (`previewStatement`);
 * nothing here composes its wording.
 */
import type {
  RecordForDestructionRequest,
  ScannedIdentifiers,
  StatementPreviewRequest,
} from "./client";
import type { DestructMethod, OperatorRef } from "./types";

/** The operator's typed inputs, before they are attested. */
export interface RecordForm {
  method: DestructMethod;
  equipment_id: string;
  location: string;
  full_name: string;
  title: string;
  notes: string;
  /**
   * `device`: the server reads the identifiers from the drive. `scanned`:
   * the operator scanned (or typed) them from the drive's labels.
   */
  identifiers:
    | { from: "device" }
    | { from: "scanned"; asset_tag: string; serial: string };
}

export type AddDriveResult =
  | { ok: true; list: ScannedIdentifiers[] }
  | { ok: false; reason: string };

function trimmed(v: string | null | undefined): string | null {
  const t = (v ?? "").trim();
  return t === "" ? null : t;
}

function same(a: string | null, b: string | null): boolean {
  return a != null && b != null && a.toLowerCase() === b.toLowerCase();
}

/**
 * Add one scanned drive to the list, trimmed. Refuses a drive with neither
 * identifier, and one whose asset tag or serial is already listed (the
 * server refuses the whole group in that case).
 */
export function addScannedDrive(
  list: readonly ScannedIdentifiers[],
  drive: { asset_tag?: string | null; serial?: string | null },
): AddDriveResult {
  const asset_tag = trimmed(drive.asset_tag);
  const serial = trimmed(drive.serial);
  if (asset_tag == null && serial == null) {
    return { ok: false, reason: "Scan an asset tag or a serial." };
  }
  for (const d of list) {
    if (same(asset_tag, d.asset_tag)) {
      return { ok: false, reason: `Asset tag ${asset_tag} is already in the list.` };
    }
    if (same(serial, d.serial)) {
      return { ok: false, reason: `Serial ${serial} is already in the list.` };
    }
  }
  return { ok: true, list: [...list, { asset_tag, serial }] };
}

/** Every field the statement names is filled in. */
export function canPreview(form: RecordForm): boolean {
  return [form.equipment_id, form.location, form.full_name, form.title].every(
    (v) => trimmed(v) != null,
  );
}

export function toPreviewRequest(form: RecordForm): StatementPreviewRequest {
  return {
    method: form.method,
    equipment_id: form.equipment_id,
    location: form.location,
    full_name: form.full_name,
    title: form.title,
  };
}

/**
 * A key for the inputs the statement depends on: when it changes, a
 * previewed statement (and the "I attest" tick given against it) is stale.
 */
export function previewKey(form: RecordForm): string {
  return JSON.stringify(toPreviewRequest(form));
}

/** A statement the server rendered, and the key of the inputs it was rendered from. */
export interface ReviewedStatement {
  key: string;
  statement: string;
}

/**
 * Whether "Record for destruction" may be pressed: the operator ticked
 * "I attest" against a statement rendered from the form's current inputs.
 */
export function readyToRecord(
  form: RecordForm,
  reviewed: ReviewedStatement | null,
  attested: boolean,
): boolean {
  return attested && reviewed != null && reviewed.key === previewKey(form);
}

/**
 * The request body for `recordForDestruction`. `identifiers` is sent only
 * when the operator scanned them; the server decides the source.
 */
export function toRecordRequest(
  form: RecordForm,
  operator: OperatorRef,
  attested: boolean,
): RecordForDestructionRequest {
  const body: RecordForDestructionRequest = {
    method: form.method,
    equipment_id: form.equipment_id,
    location: form.location,
    operator,
    full_name: form.full_name,
    title: form.title,
    attest: attested,
    notes: trimmed(form.notes),
  };
  if (form.identifiers.from === "scanned") {
    body.identifiers = {
      asset_tag: trimmed(form.identifiers.asset_tag),
      serial: trimmed(form.identifiers.serial),
    };
  }
  return body;
}
