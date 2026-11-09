import type { Job, JobStateLabel } from "@/api/types";

/**
 * A section of the Jobs page (`/jobs`), by outer JobState:
 * - `active`: past the queue and not terminal (`in_progress`,
 *   `pending_destruction` — recorded for destruction, no certificate yet);
 * - `queued`: waiting to be started;
 * - `finished`: terminal (`erased`, `destroyed`, `quarantined`, `aborted`).
 */
export type JobGroupKind = "active" | "queued" | "finished";

/** Section order on the Jobs page, and the order `groupJobs` returns. */
export const JOB_GROUP_KINDS: readonly JobGroupKind[] = ["active", "queued", "finished"];

export interface JobGroup {
  kind: JobGroupKind;
  jobs: Job[];
}

export function jobGroupOf(state: JobStateLabel): JobGroupKind {
  switch (state) {
    case "in_progress":
    case "pending_destruction":
      return "active";
    case "queued":
      return "queued";
    case "erased":
    case "destroyed":
    case "quarantined":
    case "aborted":
      return "finished";
    default: {
      const never: never = state;
      return never;
    }
  }
}

function byId(a: Job, b: Job): number {
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

/**
 * By the given timestamps (ascending, or descending when `newestFirst`).
 * Unparseable timestamps sort after every parseable one, by id among
 * themselves; equal timestamps by id. Transitive, so the order never
 * depends on the engine or the input order.
 */
function byTime(ta: number, tb: number, a: Job, b: Job, newestFirst: boolean): number {
  const badA = Number.isNaN(ta);
  const badB = Number.isNaN(tb);
  if (badA !== badB) return badA ? 1 : -1;
  if (!badA && ta !== tb) return newestFirst ? tb - ta : ta - tb;
  return byId(a, b);
}

/** Oldest first by `created_at`: the order the bench works them. */
function oldestCreatedFirst(a: Job, b: Job): number {
  return byTime(Date.parse(a.created_at), Date.parse(b.created_at), a, b, false);
}

/** Newest first by `ended_at`, else `created_at`: the latest results on top. */
function newestEndedFirst(a: Job, b: Job): number {
  const end = (j: Job) => Date.parse(j.ended_at ?? j.created_at);
  return byTime(end(a), end(b), a, b, true);
}

/**
 * Every Job in its section, all three sections always present in
 * `JOB_GROUP_KINDS` order. Active and Queued oldest first by `created_at`;
 * Finished newest first by `ended_at` (else `created_at`). Ties order by
 * id; unparseable timestamps come after the rest, by id. The input is not
 * mutated.
 */
export function groupJobs(jobs: readonly Job[]): JobGroup[] {
  const groups: Record<JobGroupKind, Job[]> = { active: [], queued: [], finished: [] };
  for (const job of jobs) groups[jobGroupOf(job.state.state)].push(job);
  groups.active.sort(oldestCreatedFirst);
  groups.queued.sort(oldestCreatedFirst);
  groups.finished.sort(newestEndedFirst);
  return JOB_GROUP_KINDS.map((kind) => ({ kind, jobs: groups[kind] }));
}
