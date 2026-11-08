import { describe, expect, it } from "vitest";

import type {
  DestructionManifest,
  Job,
  JobStateLabel,
  ManifestStateLabel,
} from "@/api/types";
import { assemblableJobs, offeredSelection } from "./manifestAssembly";

function job(
  id: string,
  state: JobStateLabel = "pending_co_sign",
  manifest_id: string | null = null,
): Job {
  return { id, state: { state }, manifest_id } as Job;
}

function manifest(
  id: string,
  job_ids: string[],
  state: ManifestStateLabel = "pending",
): DestructionManifest {
  return { id, job_ids, state } as DestructionManifest;
}

const ids = (jobs: Job[]) => jobs.map((j) => j.id);

describe("assemblableJobs", () => {
  it("offers a PendingCoSign Job with no manifest_id on no manifest", () => {
    expect(ids(assemblableJobs([job("a")], []))).toEqual(["a"]);
  });

  it("does not offer a Job already stamped with a manifest_id", () => {
    expect(assemblableJobs([job("a", "pending_co_sign", "m1")], [])).toEqual([]);
  });

  it("does not offer a Job listed on a pending manifest before the Jobs poll shows the stamp", () => {
    expect(assemblableJobs([job("a")], [manifest("m1", ["a"])])).toEqual([]);
  });

  it("does not offer a Job listed on a signed manifest", () => {
    expect(
      assemblableJobs([job("a")], [manifest("m1", ["a"], "signed")]),
    ).toEqual([]);
  });

  it("offers a Job listed only on a rejected manifest", () => {
    expect(
      ids(assemblableJobs([job("a")], [manifest("m1", ["a"], "rejected")])),
    ).toEqual(["a"]);
  });

  it("does not offer a Job on a live manifest even if also on a rejected one", () => {
    expect(
      assemblableJobs(
        [job("a")],
        [manifest("m1", ["a"], "rejected"), manifest("m2", ["a"])],
      ),
    ).toEqual([]);
  });

  it.each<JobStateLabel>([
    "queued",
    "in_progress",
    "erased",
    "destroyed",
    "quarantined",
    "aborted",
  ])("does not offer a %s Job", (state) => {
    expect(assemblableJobs([job("a", state)], [])).toEqual([]);
  });

  it("preserves the Jobs order and keeps only the offered ones", () => {
    const jobs = [
      job("c"),
      job("b", "erased"),
      job("a"),
      job("d", "pending_co_sign", "m1"),
      job("e"),
    ];
    expect(ids(assemblableJobs(jobs, [manifest("m2", ["e"])]))).toEqual([
      "c",
      "a",
    ]);
  });

  it("returns nothing for empty inputs", () => {
    expect(assemblableJobs([], [])).toEqual([]);
    expect(assemblableJobs([], [manifest("m1", ["a"])])).toEqual([]);
  });
});

describe("offeredSelection", () => {
  it("keeps selected ids that are still offered, in offered order", () => {
    const offered = [job("a"), job("b"), job("c")];
    expect(offeredSelection(new Set(["c", "a"]), offered)).toEqual(["a", "c"]);
  });

  it("drops a selected Job that stopped being offered", () => {
    const before = [job("a"), job("b")];
    const selected = new Set(["a", "b"]);
    expect(offeredSelection(selected, before)).toEqual(["a", "b"]);
    // Another operator assembled "b"; the next manifests poll lists it.
    const after = assemblableJobs(before, [manifest("m1", ["b"])]);
    const submitted = offeredSelection(selected, after);
    expect(submitted).toEqual(["a"]);
  });

  it("returns nothing when nothing is selected or nothing is offered", () => {
    expect(offeredSelection(new Set(), [job("a")])).toEqual([]);
    expect(offeredSelection(new Set(["a"]), [])).toEqual([]);
  });
});
