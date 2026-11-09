import { describe, expect, it } from "vitest";

import type { Job, JobStateLabel } from "@/api/types";
import { groupJobs, JOB_GROUP_KINDS, jobGroupOf } from "./jobGroups";

function job(
  id: string,
  state: JobStateLabel,
  created_at = "2026-11-09T10:00:00Z",
  ended_at: string | null = null,
  erasureState?: string,
): Job {
  return {
    id,
    spec: { device_id: `dev-${id}` },
    state: { state },
    activities: erasureState ? [{ type: "erasure", id: `${id}-e`, state: { state: erasureState }, events: [] }] : [],
    manifest_id: null,
    certificate_id: null,
    created_at,
    started_at: null,
    ended_at,
  } as unknown as Job;
}

const ids = (jobs: Job[]) => jobs.map((j) => j.id);
const group = (jobs: Job[], kind: string) => groupJobs(jobs).find((g) => g.kind === kind)!.jobs;

describe("jobGroupOf", () => {
  it.each([
    ["in_progress", "active"],
    ["pending_destruction", "active"],
    ["queued", "queued"],
    ["erased", "finished"],
    ["destroyed", "finished"],
    ["quarantined", "finished"],
    ["aborted", "finished"],
  ] as const)("puts %s under %s", (state, kind) => {
    expect(jobGroupOf(state)).toBe(kind);
  });
});

describe("groupJobs", () => {
  it("counts a mixed bench into Active, Queued, Finished, in that order", () => {
    const groups = groupJobs([
      job("e", "erased"),
      job("w1", "in_progress"),
      job("q1", "queued"),
      job("pd", "pending_destruction"),
      job("d", "destroyed"),
      job("q2", "queued"),
      job("w2", "in_progress"),
      job("x", "quarantined"),
      job("q3", "queued"),
      job("a", "aborted"),
    ]);
    expect(JOB_GROUP_KINDS).toEqual(["active", "queued", "finished"]);
    expect(groups.map((g) => [g.kind, g.jobs.length])).toEqual([
      ["active", 3],
      ["queued", 3],
      ["finished", 4],
    ]);
  });

  it("orders Active and Queued oldest first by created_at", () => {
    const jobs = [
      job("q-late", "queued", "2026-11-09T10:05:00Z"),
      job("q-early", "queued", "2026-11-09T10:00:00Z"),
      job("w-late", "in_progress", "2026-11-09T09:30:00Z"),
      job("w-early", "pending_destruction", "2026-11-09T09:00:00Z"),
    ];
    expect(ids(group(jobs, "queued"))).toEqual(["q-early", "q-late"]);
    expect(ids(group(jobs, "active"))).toEqual(["w-early", "w-late"]);
  });

  it("orders Finished newest first by ended_at, falling back to created_at", () => {
    const jobs = [
      job("e11", "erased", "2026-11-09T09:00:00Z", "2026-11-09T11:00:00Z"),
      job("e1130", "erased", "2026-11-09T09:00:00Z", "2026-11-09T11:30:00Z"),
      job("no-end", "aborted", "2026-11-09T11:15:00Z", null),
    ];
    expect(ids(group(jobs, "finished"))).toEqual(["e1130", "no-end", "e11"]);
  });

  it("orders ties by id and puts unparseable timestamps last, by id, without throwing", () => {
    const jobs = [
      job("c", "queued", "2026-11-09T10:00:00Z"),
      job("a", "queued", "not a date"),
      job("e", "queued", "2026-11-09T09:00:00Z"),
      job("0", "queued", ""),
      job("b", "queued", "2026-11-09T10:00:00Z"),
      job("z", "erased", "garbage", "garbage"),
      job("y", "erased", "2026-11-09T10:00:00Z", "2026-11-09T11:00:00Z"),
      job("m", "aborted", "bad", null),
      job("x", "erased", "2026-11-09T10:00:00Z", "2026-11-09T11:00:00Z"),
      job("w", "erased", "2026-11-09T10:00:00Z", "2026-11-09T12:00:00Z"),
    ];
    expect(() => groupJobs(jobs)).not.toThrow();
    expect(ids(group(jobs, "queued"))).toEqual(["e", "b", "c", "0", "a"]);
    expect(ids(group(jobs, "finished"))).toEqual(["w", "x", "y", "m", "z"]);
    // The same order whatever the input order.
    const reversed = [...jobs].reverse();
    expect(ids(group(reversed, "queued"))).toEqual(["e", "b", "c", "0", "a"]);
    expect(ids(group(reversed, "finished"))).toEqual(["w", "x", "y", "m", "z"]);
  });

  it("orders entirely unparseable timestamps by id", () => {
    const jobs = [job("b", "queued", "?"), job("a", "queued", "?"), job("c", "queued", "")];
    expect(ids(group(jobs, "queued"))).toEqual(["a", "b", "c"]);
  });

  it("keeps a Job whose latest erasure failed in Active", () => {
    expect(ids(group([job("f", "in_progress", undefined, null, "failed")], "active"))).toEqual(["f"]);
  });

  it("always returns all three groups, empty ones with no Jobs", () => {
    const groups = groupJobs([job("e", "erased")]);
    expect(groups.map((g) => [g.kind, ids(g.jobs)])).toEqual([
      ["active", []],
      ["queued", []],
      ["finished", ["e"]],
    ]);
    expect(groupJobs([]).map((g) => g.jobs.length)).toEqual([0, 0, 0]);
  });

  it("does not mutate its input", () => {
    const jobs = [job("q2", "queued", "2026-11-09T10:05:00Z"), job("q1", "queued", "2026-11-09T10:00:00Z")];
    const before = ids(jobs);
    Object.freeze(jobs);
    groupJobs(jobs);
    expect(ids(jobs)).toEqual(before);
  });
});
