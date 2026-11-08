import type { DestructionManifest, Job } from "@/api/types";

/**
 * The Jobs the Manifests page offers for assembly into a new
 * DestructionManifest, in their original order.
 *
 * Mirrors the refusals of `create_manifest`
 * (`crates/wipe-server/src/handlers.rs`) that an operator can hit from this
 * page: a Job listed on a manifest that is not `rejected`, a Job not in
 * `PendingCoSign`, and a Job that already carries a `manifest_id`. It does
 * not mirror every server refusal (empty, duplicate or unknown ids, a Job
 * with no DestructionEvent). The Jobs and manifests lists are polled
 * separately, so either may be a poll behind; checking both keeps a Job off
 * the list as soon as either one reports it assembled. This filter is a
 * convenience only: the server stays the authority.
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
      j.state.state === "pending_co_sign" &&
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
