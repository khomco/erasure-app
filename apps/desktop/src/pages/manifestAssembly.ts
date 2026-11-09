import type { DestructionManifest, Job } from "@/api/types";

function hasErasureActivity(job: Job): boolean {
  return (job.activities ?? []).some((a) => a.type === "erasure");
}

/**
 * Jobs pending destruction that are on no live destruction run: the
 * candidates for a new one. A run is live unless `rejected`, so `pending`,
 * `awaiting_concurrence` and `completed` runs all hold their Jobs.
 */
function unassembled(jobs: Job[], manifests: DestructionManifest[]): Job[] {
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
 * The Jobs the Destruction runs page offers for assembly into a new run,
 * in their original order.
 *
 * Mirrors the refusals of `create_manifest`
 * (`crates/wipe-server/src/handlers.rs`) that an operator can hit from this
 * page: a Job listed on a run that is not `rejected`, a Job not in
 * `PendingDestruction`, a Job that already carries a `manifest_id`, and a
 * Job with no erasure activity (it has no certificate, so the server
 * refuses it until destruction-attestation story 4). It does not mirror
 * every server refusal (empty, duplicate or unknown ids, a certificate still
 * being signed). The Jobs and runs lists are polled separately, so either
 * may be a poll behind; checking both keeps a Job off the list as soon as
 * either one reports it assembled. This filter is a convenience only: the
 * server stays the authority.
 */
export function assemblableJobs(
  jobs: Job[],
  manifests: DestructionManifest[],
): Job[] {
  return unassembled(jobs, manifests).filter(hasErasureActivity);
}

/**
 * Jobs pending destruction, on no run, that cannot join one yet because
 * they were never erased and so have no certificate. The page lists them in
 * a note saying why they are not offered.
 */
export function uncertifiableJobs(
  jobs: Job[],
  manifests: DestructionManifest[],
): Job[] {
  return unassembled(jobs, manifests).filter((j) => !hasErasureActivity(j));
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
