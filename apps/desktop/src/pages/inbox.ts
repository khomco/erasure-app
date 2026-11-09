import { runAnchorId, runDriveLabels } from "@/api/destructionRuns";
import type { DestructionManifest, ErasureEvent, Job, VerificationEvent } from "@/api/types";
import { latestErasure } from "@/api/ws";
import { deriveSlotStatus } from "@/bench/slotStatus";

/**
 * One item waiting on a human (the Inbox, `/inbox`). Derived, never stored:
 * the Inbox is exactly what `GET /api/jobs` and `GET /api/manifests` say now,
 * so resolving an item anywhere removes it on the next poll.
 */
export type InboxEntry =
  /**
   * The latest erasure failed, or it completed and its sampled verification
   * failed. `reason` is the line the row shows for either case.
   */
  | { kind: "failed_erasure"; job: Job; erasure: ErasureEvent; reason: string }
  | { kind: "quarantined"; job: Job }
  | { kind: "pending_destruction"; job: Job }
  | { kind: "run"; run: DestructionManifest; drives: string[] };

export type InboxKind = InboxEntry["kind"];

/** Group order on the page, and the order `buildInbox` returns. */
export const INBOX_KINDS: InboxKind[] = [
  "failed_erasure",
  "quarantined",
  "pending_destruction",
  "run",
];

/** A run that waits on a human: completion (`pending`) or concurrence. */
function runWaits(run: DestructionManifest): boolean {
  return run.state === "pending" || run.state === "awaiting_concurrence";
}

function byAge<T extends { id: string; created_at: string }>(a: T, b: T): number {
  const ta = Date.parse(a.created_at);
  const tb = Date.parse(b.created_at);
  if (ta !== tb && !Number.isNaN(ta) && !Number.isNaN(tb)) return ta - tb;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

/**
 * Every item waiting on a human, grouped in `INBOX_KINDS` order, each group
 * oldest first by `created_at` (ties by id).
 *
 * A pending-destruction Job on a run that is itself in the Inbox is not
 * listed on its own: the run's completion covers it, so the count matches
 * the actions a human must take. A Job whose run is missing, completed or
 * rejected is still listed — nothing pending is hidden.
 */
export function buildInbox(jobs: readonly Job[], manifests: readonly DestructionManifest[]): InboxEntry[] {
  const runs = manifests.filter(runWaits).sort(byAge);
  const onWaitingRun = new Set<string>();
  for (const run of runs) {
    onWaitingRun.add(run.id);
    for (const id of run.job_ids) onWaitingRun.add(id);
  }

  const failed: InboxEntry[] = [];
  const quarantined: InboxEntry[] = [];
  const pending: InboxEntry[] = [];
  for (const job of [...jobs].sort(byAge)) {
    const status = deriveSlotStatus(job);
    if (status.kind === "failed") {
      failed.push({
        kind: "failed_erasure",
        job,
        erasure: status.erasure,
        reason: `Erasure failed: ${failureReason(status.erasure)}`,
      });
    } else if (status.kind === "wiping" && status.erasure?.state.state === "completed") {
      const report = latestVerification(job, status.erasure.id)?.report;
      if (report && report.all_passed === false) {
        failed.push({ kind: "failed_erasure", job, erasure: status.erasure, reason: verificationReason(report) });
      }
    } else if (status.kind === "quarantined") {
      quarantined.push({ kind: "quarantined", job });
    } else if (status.kind === "pending_destruction") {
      const covered =
        onWaitingRun.has(job.id) || (job.manifest_id != null && onWaitingRun.has(job.manifest_id));
      if (!covered) pending.push({ kind: "pending_destruction", job });
    }
  }

  return [
    ...failed,
    ...quarantined,
    ...pending,
    ...runs.map((run): InboxEntry => ({ kind: "run", run, drives: runDriveLabels(run.job_ids, jobs) })),
  ];
}

/** The nav count: null until both lists have loaded. */
export function inboxCount(
  jobs: readonly Job[] | undefined,
  manifests: readonly DestructionManifest[] | undefined,
): number | null {
  if (!jobs || !manifests) return null;
  return buildInbox(jobs, manifests).length;
}

/** A loaded-or-failing list, as a query hands it over. */
export interface InboxSource<T> {
  data: readonly T[] | undefined;
  error: unknown;
}

/**
 * The nav item's count: hidden (null) until both lists load, while either
 * request is failing (even with an earlier answer cached), and at zero.
 */
export function navInboxCount(
  jobs: InboxSource<Job>,
  manifests: InboxSource<DestructionManifest>,
): number | null {
  if (jobs.error || manifests.error) return null;
  const n = inboxCount(jobs.data, manifests.data);
  return n ? n : null;
}

function trimmed(v: string | null | undefined): string | null {
  const t = (v ?? "").trim();
  return t === "" ? null : t;
}

/**
 * How an Inbox Job entry names its drive: asset tag, else the latest
 * erasure's device snapshot (vendor model · serial), else the device id.
 */
export function jobDriveLabel(job: Job): string {
  const tag = trimmed(job.spec.asset_tag);
  if (tag) return `asset tag ${tag}`;
  const device = latestErasure(job)?.device_snapshot;
  if (device) {
    const name = [trimmed(device.vendor), trimmed(device.model)].filter(Boolean).join(" ");
    const parts = [name, trimmed(device.serial)].filter(Boolean);
    if (parts.length > 0) return parts.join(" · ");
  }
  return job.spec.device_id;
}

/** Why a failed erasure failed: its last `failed` event's reason. */
export function failureReason(erasure: ErasureEvent): string {
  const events = erasure.events ?? [];
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i].event;
    if (e.kind === "failed") return trimmed(e.reason) ?? "no reason recorded";
  }
  return "no reason recorded";
}

/**
 * The newest Verification activity on `job` naming erasure `erasureId`.
 *
 * The runner marks an erasure `completed` before verifying it and, when the
 * sampled verification fails, leaves the Job `in_progress` waiting on a human
 * (abort or record for destruction) — the bay map still draws it as wiping.
 */
function latestVerification(job: Job, erasureId: string): VerificationEvent | undefined {
  for (let i = job.activities.length - 1; i >= 0; i--) {
    const a = job.activities[i];
    if (a.type === "verification" && a.erasure_event_id === erasureId) return a;
  }
  return undefined;
}

/** "Verification failed: N of M samples failed" for a failed report. */
export function verificationReason(report: VerificationEvent["report"]): string {
  const samples = report.samples ?? [];
  const failedCount = samples.filter((s) => !s.passed).length;
  const total = report.sample_count ?? samples.length;
  return `Verification failed: ${failedCount} of ${total} samples failed`;
}

/** Where an entry is resolved: a path and an optional hash. */
export function inboxTarget(entry: InboxEntry): { to: string; hash?: string } {
  switch (entry.kind) {
    case "failed_erasure":
    case "quarantined":
      return { to: `/jobs/${entry.job.id}` };
    case "pending_destruction":
      return { to: "/manifests" };
    case "run":
      return { to: "/manifests", hash: runAnchorId(entry.run.id) };
  }
}

/** `inboxTarget` as one href, e.g. `/manifests#run-<id>`. */
export function inboxHref(entry: InboxEntry): string {
  const { to, hash } = inboxTarget(entry);
  return hash ? `${to}#${hash}` : to;
}
