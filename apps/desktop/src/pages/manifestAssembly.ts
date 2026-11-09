import type { DestructionManifest, Job } from "@/api/types";

/**
 * The Jobs the Destruction runs page offers for assembly into a new run,
 * in their original order: Jobs pending destruction on no live run. A run
 * is live unless `rejected`, so `pending`, `awaiting_concurrence` and
 * `completed` runs all hold their Jobs. A drive that was never erased is
 * offered like any other: it is certified from its recorded identifiers
 * when the run destroys it (ADR-0011 §2).
 *
 * Mirrors the refusals of `create_manifest`
 * (`crates/wipe-server/src/handlers.rs`) that an operator can hit from this
 * page: a Job listed on a run that is not `rejected`, a Job not in
 * `PendingDestruction`, and a Job that already carries a `manifest_id`. It
 * does not mirror every server refusal (empty, duplicate or unknown ids).
 * The Jobs and runs lists are polled separately, so either may be a poll
 * behind; checking both keeps a Job off the list as soon as either one
 * reports it assembled. This filter is a convenience only: the server stays
 * the authority.
 */
export function assemblableJobs(
  jobs: Job[],
  manifests: DestructionManifest[],
): Job[] {
  const onLiveManifest = new Set<string>();
  for (const m of manifests) {
    if (m.state === "rejected") continue;
    for (const id of m.job_ids) onLiveManifest.add(id);
  }
  return jobs.filter(
    (j) =>
      j.state.state === "pending_destruction" &&
      !j.manifest_id &&
      !onLiveManifest.has(j.id),
  );
}

/**
 * The selected Job ids that are still offered, in offered order. A Job ticked
 * before another operator assembled it drops out here, so it is neither
 * counted on the button nor posted (which would fail the whole batch with a
 * 409).
 */
export function offeredSelection(
  selected: ReadonlySet<string>,
  offered: Job[],
): string[] {
  return offered.filter((j) => selected.has(j.id)).map((j) => j.id);
}
