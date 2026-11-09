import { describe, expect, it } from "vitest";

import type {
  DestructionManifest,
  Job,
  JobStateLabel,
  JobUpdate,
  ManifestStateLabel,
} from "@/api/types";
import { runAnchorId } from "@/api/destructionRuns";
import {
  buildInbox,
  failureReason,
  inboxCount,
  inboxHref,
  inboxTarget,
  jobDriveLabel,
  navInboxCount,
  type InboxEntry,
} from "./inbox";

interface JobOpts {
  state?: JobStateLabel;
  created_at?: string;
  manifest_id?: string | null;
  asset_tag?: string | null;
  /** Erasure states, oldest first. */
  erasures?: string[];
  events?: JobUpdate[];
  device?: { vendor: string; model: string; serial: string } | null;
  /** Verifications appended after the erasures: which erasure (index) and per-sample pass/fail. */
  verifications?: { erasure: number; samples: boolean[] }[];
}

function job(id: string, o: JobOpts = {}): Job {
  const activities = (o.erasures ?? []).map((state, i) => ({
    type: "erasure",
    id: `${id}-e${i}`,
    state: { state },
    events: o.events ?? [],
    device_snapshot: o.device === undefined ? undefined : o.device,
  }));
  for (const [i, v] of (o.verifications ?? []).entries()) {
    activities.push({
      type: "verification",
      id: `${id}-v${i}`,
      erasure_event_id: `${id}-e${v.erasure}`,
      report: {
        sample_count: v.samples.length,
        samples: v.samples.map((passed) => ({ passed })),
        all_passed: v.samples.every(Boolean),
      },
    } as never);
  }
  return {
    id,
    spec: { device_id: `dev-${id}`, asset_tag: o.asset_tag ?? null },
    state: { state: o.state ?? "in_progress" },
    activities,
    manifest_id: o.manifest_id ?? null,
    created_at: o.created_at ?? "2026-11-09T10:00:00Z",
  } as unknown as Job;
}

function run(
  id: string,
  job_ids: string[],
  state: ManifestStateLabel = "pending",
  created_at = "2026-11-09T10:00:00Z",
  required_concurrence: "review" | "witness" | null = null,
): DestructionManifest {
  return { id, job_ids, state, created_at, required_concurrence } as DestructionManifest;
}

const failedJob = (id: string, o: JobOpts = {}) => job(id, { erasures: ["failed"], ...o });
const pendingJob = (id: string, o: JobOpts = {}) => job(id, { state: "pending_destruction", ...o });

function summary(entries: InboxEntry[]): string[] {
  return entries.map((e) => `${e.kind}:${e.kind === "run" ? e.run.id : e.job.id}`);
}

describe("buildInbox", () => {
  it("is empty when nothing waits on a human", () => {
    const jobs = [
      job("a", { state: "erased", erasures: ["completed"] }),
      job("b", { state: "destroyed" }),
      job("c", { state: "aborted", erasures: ["failed"] }),
      job("d", { state: "in_progress", erasures: ["running"] }),
      job("e", { state: "queued" }),
    ];
    const runs = [run("r1", ["b"], "completed"), run("r2", [], "rejected")];
    expect(buildInbox(jobs, runs)).toEqual([]);
    expect(inboxCount(jobs, runs)).toBe(0);
  });

  it("lists a failed erasure, linking to the Job", () => {
    const entries = buildInbox([failedJob("a")], []);
    expect(summary(entries)).toEqual(["failed_erasure:a"]);
    expect(inboxHref(entries[0])).toBe("/jobs/a");
  });

  it("lists a queued Job whose latest erasure failed", () => {
    expect(summary(buildInbox([failedJob("a", { state: "queued" })], []))).toEqual(["failed_erasure:a"]);
  });

  it("lists a completed erasure whose verification failed, with the sample count", () => {
    const samples = [true, false, true, true, false, true, true, true];
    const entries = buildInbox([job("a", { erasures: ["completed"], verifications: [{ erasure: 0, samples }] })], []);
    expect(summary(entries)).toEqual(["failed_erasure:a"]);
    expect(entries[0].kind === "failed_erasure" && entries[0].reason).toBe(
      "Verification failed: 2 of 8 samples failed",
    );
    expect(inboxHref(entries[0])).toBe("/jobs/a");
  });

  it("does not list a completed erasure whose verification passed or has not run", () => {
    expect(
      buildInbox([job("a", { erasures: ["completed"], verifications: [{ erasure: 0, samples: [true, true] }] })], []),
    ).toEqual([]);
    expect(buildInbox([job("a", { erasures: ["completed"] })], [])).toEqual([]);
  });

  it("reads only the latest verification naming the latest erasure", () => {
    // An earlier erasure's failed verification does not count against a later one.
    const olderFailed = job("a", {
      erasures: ["completed", "completed"],
      verifications: [{ erasure: 0, samples: [false] }],
    });
    expect(buildInbox([olderFailed], [])).toEqual([]);
    // A re-verification that passed supersedes a failed one.
    const reverified = job("b", {
      erasures: ["completed"],
      verifications: [
        { erasure: 0, samples: [false] },
        { erasure: 0, samples: [true] },
      ],
    });
    expect(buildInbox([reverified], [])).toEqual([]);
  });

  it("does not list an erased Job even if its verification failed", () => {
    const j = job("a", { state: "erased", erasures: ["completed"], verifications: [{ erasure: 0, samples: [false] }] });
    expect(buildInbox([j], [])).toEqual([]);
  });

  it("does not list a Job whose earlier erasure failed but latest completed", () => {
    expect(buildInbox([job("a", { erasures: ["failed", "completed"] })], [])).toEqual([]);
  });

  it("lists a quarantined Job, linking to the Job", () => {
    const entries = buildInbox([job("a", { state: "quarantined" })], []);
    expect(summary(entries)).toEqual(["quarantined:a"]);
    expect(inboxHref(entries[0])).toBe("/jobs/a");
  });

  it("lists a Job recorded for destruction on no run, linking to Destruction runs", () => {
    const entries = buildInbox([pendingJob("a")], []);
    expect(summary(entries)).toEqual(["pending_destruction:a"]);
    expect(inboxHref(entries[0])).toBe("/manifests");
  });

  it("lists a pending run once, with its drives, and not its Jobs", () => {
    const jobs = [
      pendingJob("a", { manifest_id: "r1", asset_tag: "AT-1" }),
      pendingJob("b", { manifest_id: "r1" }),
      pendingJob("c"),
    ];
    const runs = [run("r1", ["a", "b", "c"])];
    const entries = buildInbox(jobs, runs);
    expect(summary(entries)).toEqual(["run:r1"]);
    expect(inboxHref(entries[0])).toBe("/manifests#run-r1");
    expect(entries[0].kind === "run" && entries[0].drives).toHaveLength(3);
    expect(inboxCount(jobs, runs)).toBe(1);
  });

  it("does not list a pending Job covered by its run only through manifest_id", () => {
    const jobs = [pendingJob("a", { manifest_id: "r1" }), pendingJob("b", { manifest_id: "r1" })];
    const runs = [run("r1", ["b"])];
    expect(summary(buildInbox(jobs, runs))).toEqual(["run:r1"]);
  });

  it("links a run to its card's anchor on Destruction runs", () => {
    const [entry] = buildInbox([], [run("r1", [])]);
    expect(inboxTarget(entry)).toEqual({ to: "/manifests", hash: runAnchorId("r1") });
    expect(inboxHref(entry)).toBe(`/manifests#${runAnchorId("r1")}`);
    expect(runAnchorId("r1")).toBe("run-r1");
  });

  it("lists a run awaiting concurrence", () => {
    const entries = buildInbox([pendingJob("a", { manifest_id: "r1" })], [
      run("r1", ["a"], "awaiting_concurrence", undefined, "witness"),
    ]);
    expect(summary(entries)).toEqual(["run:r1"]);
  });

  it.each<ManifestStateLabel>(["completed", "rejected"])("does not list a %s run", (state) => {
    expect(buildInbox([], [run("r1", ["a"], state)])).toEqual([]);
  });

  it.each<[string, DestructionManifest[]]>([
    ["missing", []],
    ["completed", [run("r1", ["a"], "completed")]],
    ["rejected", [run("r1", ["a"], "rejected")]],
  ])("lists a pending Job whose run is %s", (_label, runs) => {
    expect(summary(buildInbox([pendingJob("a", { manifest_id: "r1" })], runs))).toEqual([
      "pending_destruction:a",
    ]);
  });

  it("orders groups failed, quarantined, pending destruction, run; count 5", () => {
    const jobs = [
      pendingJob("p1", { manifest_id: "r1" }),
      pendingJob("u1"),
      job("q1", { state: "quarantined" }),
      failedJob("f1"),
      failedJob("f2"),
    ];
    const runs = [run("r1", ["p1"])];
    expect(summary(buildInbox(jobs, runs))).toEqual([
      "failed_erasure:f1",
      "failed_erasure:f2",
      "quarantined:q1",
      "pending_destruction:u1",
      "run:r1",
    ]);
    expect(inboxCount(jobs, runs)).toBe(5);
  });

  it("sorts each group oldest first, ties by id", () => {
    const jobs = [
      failedJob("c", { created_at: "2026-11-09T12:00:00Z" }),
      failedJob("b", { created_at: "2026-11-09T09:00:00Z" }),
      failedJob("a", { created_at: "2026-11-09T12:00:00Z" }),
    ];
    const runs = [
      run("r2", [], "pending", "2026-11-09T11:00:00Z"),
      run("r1", [], "awaiting_concurrence", "2026-11-09T12:00:00Z"),
      run("r0", [], "pending", "2026-11-09T10:00:00Z"),
    ];
    expect(summary(buildInbox(jobs, runs))).toEqual([
      "failed_erasure:b",
      "failed_erasure:a",
      "failed_erasure:c",
      "run:r0",
      "run:r2",
      "run:r1",
    ]);
  });

  it("drops an entry once it is resolved", () => {
    expect(inboxCount([failedJob("a")], [])).toBe(1);
    expect(inboxCount([failedJob("a", { state: "aborted" })], [])).toBe(0);
  });
});

describe("inboxCount", () => {
  it("is null until both lists have loaded", () => {
    expect(inboxCount(undefined, [])).toBeNull();
    expect(inboxCount([], undefined)).toBeNull();
    expect(inboxCount(undefined, undefined)).toBeNull();
    expect(inboxCount([], [])).toBe(0);
  });
});

describe("navInboxCount", () => {
  const ok = <T,>(data: T[] | undefined) => ({ data, error: null });

  it("shows the count once both lists load", () => {
    expect(navInboxCount(ok([failedJob("a")]), ok([]))).toBe(1);
  });

  it("is hidden at zero and while loading", () => {
    expect(navInboxCount(ok([]), ok([]))).toBeNull();
    expect(navInboxCount(ok(undefined), ok([]))).toBeNull();
  });

  it("is hidden while either list fails, even with earlier data cached", () => {
    const boom = new Error("502 Bad Gateway: station unreachable");
    expect(navInboxCount({ data: [failedJob("a")], error: boom }, ok([]))).toBeNull();
    expect(navInboxCount(ok([failedJob("a")]), { data: [], error: boom })).toBeNull();
  });
});

describe("jobDriveLabel", () => {
  it("prefers the asset tag", () => {
    expect(
      jobDriveLabel(failedJob("a", { asset_tag: "AT-7", device: { vendor: "V", model: "M", serial: "S" } })),
    ).toBe("asset tag AT-7");
  });

  it("falls back to the latest erasure's device snapshot", () => {
    expect(jobDriveLabel(failedJob("a", { device: { vendor: "Acme", model: "X1", serial: "SN9" } }))).toBe(
      "Acme X1 · SN9",
    );
  });

  it("falls back to the device id", () => {
    expect(jobDriveLabel(pendingJob("a"))).toBe("dev-a");
  });
});

describe("failureReason", () => {
  const erasureOf = (j: Job) => {
    const [entry] = buildInbox([j], []);
    if (entry.kind !== "failed_erasure") throw new Error("expected a failed erasure");
    return entry.erasure;
  };

  it("reads the last failed event's reason", () => {
    const events: JobUpdate[] = [
      { at: "t1", event: { kind: "failed", reason: "first" } },
      { at: "t2", event: { kind: "warning", code: "w", message: "m" } },
      { at: "t3", event: { kind: "failed", reason: "device stopped responding" } },
    ];
    expect(failureReason(erasureOf(failedJob("a", { events })))).toBe("device stopped responding");
  });

  it("says no reason recorded when there is none", () => {
    expect(failureReason(erasureOf(failedJob("a")))).toBe("no reason recorded");
  });

  it("is the entry's reason line for a failed erasure", () => {
    const events: JobUpdate[] = [{ at: "t", event: { kind: "failed", reason: "medium error" } }];
    const [entry] = buildInbox([failedJob("a", { events })], []);
    expect(entry.kind === "failed_erasure" && entry.reason).toBe("Erasure failed: medium error");
    const [bare] = buildInbox([failedJob("b")], []);
    expect(bare.kind === "failed_erasure" && bare.reason).toBe("Erasure failed: no reason recorded");
  });
});
