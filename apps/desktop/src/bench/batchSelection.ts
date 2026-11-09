import type { CreateJobRequest } from "@/api/client";
import type { BayTopology, Classification, Device, Intent, OperatorRef } from "@/api/types";
import type { BayCellData, BayLookup } from "./BayMap";

/**
 * Batch start from the bay map (ROADMAP O1.4).
 *
 * A Batch is an ad-hoc operator selection (CONTEXT §5), never a persisted
 * entity: this module holds the selection, decides what may join it, and
 * runs one create-then-start per drive through the existing Job routes.
 *
 * Bay↔device comes only from the server-resolved occupancy, via the
 * `BayCellData` the bay map's lookup produces (ADR-0002). Nothing here
 * matches bays to devices on its own.
 */

/** Selected bays: bay id → the device id that bay held when it was ticked. */
export type BatchSelection = ReadonlyMap<string, string>;

export const EMPTY_SELECTION: BatchSelection = new Map();

/** One drive a batch will start a Job on. */
export interface BatchTarget {
  bayId: string;
  bayLabel: string;
  device: Device;
}

/** The part of the spec every drive in a batch shares. */
export interface SharedBatchSpec {
  classification: Classification;
  intent: Intent;
  operator: OperatorRef;
  ticket: string;
}

export type BatchOutcome =
  | { kind: "started"; jobId: string }
  | { kind: "not_started"; jobId: string; error: string }
  | { kind: "not_created"; error: string };

export interface BatchResult {
  target: BatchTarget;
  outcome: BatchOutcome;
}

/** The two routes a batch uses; injected so tests need no `fetch`. */
export interface BatchClient {
  createJob: (spec: CreateJobRequest) => Promise<{ job_id: string }>;
  startJob: (id: string) => Promise<unknown>;
}

/**
 * Only an idle bay may join a batch: a drive is present and no Job has ever
 * targeted it. Re-wiping a drive with history (erased, quarantined,
 * destroyed, aborted) stays a deliberate per-drive action.
 */
export function isSelectable(cell: BayCellData): boolean {
  return !cell.bay.disabled && cell.device !== null && cell.status.kind === "idle";
}

/** Tick or untick a bay. Ticking a bay that is not selectable is a no-op. */
export function toggleBay(sel: BatchSelection, cell: BayCellData): BatchSelection {
  if (sel.has(cell.bay.id)) {
    const next = new Map(sel);
    next.delete(cell.bay.id);
    return next;
  }
  if (!isSelectable(cell) || !cell.device) return sel;
  const next = new Map(sel);
  next.set(cell.bay.id, cell.device.id);
  return next;
}

/** Add every selectable bay to the selection. */
export function selectAllIdle(
  sel: BatchSelection,
  cells: readonly BayCellData[],
): BatchSelection {
  let next: Map<string, string> | null = null;
  for (const cell of cells) {
    if (!isSelectable(cell) || !cell.device) continue;
    if (sel.get(cell.bay.id) === cell.device.id) continue;
    next ??= new Map(sel);
    next.set(cell.bay.id, cell.device.id);
  }
  return next ?? sel;
}

/**
 * Drop every entry whose bay no longer holds the same device, or is no
 * longer idle. Returns `sel` itself when nothing changed, so a caller can
 * run it on every refetch without causing a re-render.
 */
export function pruneSelection(
  sel: BatchSelection,
  cells: readonly BayCellData[],
): BatchSelection {
  if (sel.size === 0) return sel;
  const byBay = new Map(cells.map((c) => [c.bay.id, c]));
  let next: Map<string, string> | null = null;
  for (const [bayId, deviceId] of sel) {
    const cell = byBay.get(bayId);
    const keep = !!cell && cell.device?.id === deviceId && isSelectable(cell);
    if (keep) continue;
    next ??= new Map(sel);
    next.delete(bayId);
  }
  return next ?? sel;
}

/** Every bay of a topology in bay-map order (enclosure → bank → bay). */
export function bayCellsInOrder(topology: BayTopology, lookup: BayLookup): BayCellData[] {
  const cells: BayCellData[] = [];
  for (const enc of topology.enclosures) {
    for (const bank of enc.banks) {
      for (const bay of bank.bays) cells.push(lookup(bay));
    }
  }
  return cells;
}

/**
 * The drives a batch would start now, in bay-map order. A stale entry (bay
 * emptied, drive swapped, no longer idle) is left out even before the
 * selection is pruned, so nothing is ever sent for it.
 */
export function batchTargets(
  sel: BatchSelection,
  cells: readonly BayCellData[],
): BatchTarget[] {
  const targets: BatchTarget[] = [];
  for (const cell of cells) {
    const deviceId = sel.get(cell.bay.id);
    if (!deviceId || !cell.device || cell.device.id !== deviceId) continue;
    if (!isSelectable(cell)) continue;
    targets.push({ bayId: cell.bay.id, bayLabel: cell.bay.label, device: cell.device });
  }
  return targets;
}

/** Whether Begin / Start may be pressed. */
export function canStart({
  targets,
  operator,
  running,
}: {
  targets: readonly BatchTarget[];
  operator: OperatorRef | null;
  running: boolean;
}): boolean {
  return targets.length > 0 && operator !== null && !running;
}

function blankToNull(v: string | undefined): string | null {
  const t = (v ?? "").trim();
  return t === "" ? null : t;
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** The create request for one drive: shared fields plus its own asset tag. */
export function jobRequestFor(
  target: BatchTarget,
  shared: SharedBatchSpec,
  assetTag: string | undefined,
): CreateJobRequest {
  return {
    device_id: target.device.id,
    classification: shared.classification,
    intent: shared.intent,
    operator: shared.operator,
    asset_tag: blankToNull(assetTag),
    site_label: null,
    ticket_ref: blankToNull(shared.ticket),
  };
}

/**
 * Create and start one Job per target, sequentially and in the order given.
 * One drive's failure never stops the others; each target gets exactly one
 * result. A Job that was created but not started is left `Queued` and its
 * id reported, so the operator can open it. Nothing is retried.
 *
 * `assetTags` is keyed by device id.
 */
export async function runBatch(
  targets: readonly BatchTarget[],
  shared: SharedBatchSpec,
  assetTags: ReadonlyMap<string, string>,
  client: BatchClient,
): Promise<BatchResult[]> {
  const results: BatchResult[] = [];
  for (const target of targets) {
    let jobId: string;
    try {
      const created = await client.createJob(
        jobRequestFor(target, shared, assetTags.get(target.device.id)),
      );
      jobId = created.job_id;
    } catch (e) {
      results.push({ target, outcome: { kind: "not_created", error: message(e) } });
      continue;
    }
    try {
      await client.startJob(jobId);
      results.push({ target, outcome: { kind: "started", jobId } });
    } catch (e) {
      results.push({ target, outcome: { kind: "not_started", jobId, error: message(e) } });
    }
  }
  return results;
}
