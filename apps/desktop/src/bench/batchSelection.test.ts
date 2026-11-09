import { describe, expect, it } from "vitest";

import type { CreateJobRequest } from "@/api/client";
import type {
  Bay,
  BayTopology,
  Device,
  Job,
  OperatorRef,
  ResolvedBayTopology,
} from "@/api/types";
import { makeBayLookup, type BayCellData } from "./BayMap";
import {
  EMPTY_SELECTION,
  batchTargets,
  bayCellsInOrder,
  canStart,
  isSelectable,
  pruneSelection,
  runBatch,
  selectAllIdle,
  toggleBay,
  type BatchClient,
  type BatchSelection,
  type SharedBatchSpec,
} from "./batchSelection";
import { deriveSlotStatus, type SlotStatus, type SlotStatusKind } from "./slotStatus";

function device(id: string): Device {
  return {
    id,
    vendor: "TestCo",
    model: `TM-${id}`,
    serial: `SN-${id}`,
    wwn: null,
    capacity_bytes: 1_000,
    media_type: "ssd_sata",
    bus: "sata",
    firmware: null,
    removable: false,
    block_size: 512,
    path: `/dev/${id}`,
  } as Device;
}

function bay(id: string, disabled = false): Bay {
  return {
    id,
    label: id.toUpperCase(),
    row: 0,
    col: 0,
    binding: { by: "unbound" },
    disabled,
  };
}

const someJob = { id: "j-old" } as Job;

function status(kind: SlotStatusKind): SlotStatus {
  switch (kind) {
    case "empty":
    case "idle":
      return { kind };
    case "wiping":
      return { kind, job: someJob, erasure: null };
    case "failed":
      return { kind, job: someJob, erasure: {} as never };
    default:
      return { kind, job: someJob };
  }
}

/** A populated bay in the given status; `deviceId: null` makes it empty. */
function cell(
  bayId: string,
  deviceId: string | null,
  kind: SlotStatusKind = deviceId ? "idle" : "empty",
  disabled = false,
): BayCellData {
  return {
    bay: bay(bayId, disabled),
    device: deviceId ? device(deviceId) : null,
    job: kind === "idle" || kind === "empty" ? null : someJob,
    status: status(kind),
  };
}

const sel = (entries: [string, string][]): BatchSelection => new Map(entries);

const operator: OperatorRef = { id: "op-1", display_name: "Ada", email: "ada@example.test" };

const shared: SharedBatchSpec = {
  classification: "moderate",
  intent: "recycle",
  operator,
  ticket: "RMA-42",
};

describe("isSelectable / toggleBay", () => {
  it("only an idle, populated, unblanked bay is selectable", () => {
    expect(isSelectable(cell("b1", "d1"))).toBe(true);
    expect(isSelectable(cell("b1", null))).toBe(false);
    expect(isSelectable(cell("b1", null, "empty", true))).toBe(false);
    expect(isSelectable(cell("b1", "d1", "idle", true))).toBe(false);
    for (const kind of [
      "wiping",
      "erased",
      "failed",
      "pending_destruction",
      "destroyed",
      "quarantined",
      "aborted",
    ] as const) {
      expect(isSelectable(cell("b1", "d1", kind)), kind).toBe(false);
    }
  });

  it("ticks an idle bay with the device it holds, and unticks it again", () => {
    const one = toggleBay(EMPTY_SELECTION, cell("b1", "d1"));
    expect([...one]).toEqual([["b1", "d1"]]);
    expect(toggleBay(one, cell("b1", "d1")).size).toBe(0);
  });

  it("leaves the selection unchanged when a non-idle bay is clicked", () => {
    const start = sel([["b1", "d1"]]);
    for (const c of [
      cell("b2", "d2", "wiping"),
      cell("b2", "d2", "erased"),
      cell("b2", "d2", "failed"),
      cell("b2", null),
      cell("b2", null, "empty", true),
    ]) {
      expect(toggleBay(start, c)).toBe(start);
    }
  });
});

describe("selectAllIdle", () => {
  it("adds every idle bay and nothing else", () => {
    const cells = [
      cell("b1", "d1"),
      cell("b2", "d2", "wiping"),
      cell("b3", null),
      cell("b4", "d4"),
      cell("b5", "d5", "idle", true),
    ];
    expect([...selectAllIdle(EMPTY_SELECTION, cells)]).toEqual([
      ["b1", "d1"],
      ["b4", "d4"],
    ]);
  });

  it("returns the same selection when everything idle is already ticked", () => {
    const start = sel([["b1", "d1"]]);
    expect(selectAllIdle(start, [cell("b1", "d1"), cell("b2", "d2", "erased")])).toBe(start);
  });
});

describe("pruneSelection", () => {
  const start = sel([
    ["b1", "d1"],
    ["b2", "d2"],
    ["b3", "d3"],
    ["b4", "d4"],
  ]);

  it("drops a bay whose drive was swapped, pulled, or is no longer idle", () => {
    const cells = [
      cell("b1", "d1"), // unchanged
      cell("b2", "d9"), // different drive in the bay
      cell("b3", null), // drive pulled
      cell("b4", "d4", "wiping"), // another Job started on it
    ];
    expect([...pruneSelection(start, cells)]).toEqual([["b1", "d1"]]);
  });

  it("drops a bay that is gone from the topology", () => {
    expect([...pruneSelection(sel([["gone", "d1"]]), [cell("b1", "d1")])]).toEqual([]);
  });

  it("returns the same selection when nothing is stale", () => {
    const cells = [cell("b1", "d1"), cell("b2", "d2"), cell("b3", "d3"), cell("b4", "d4")];
    expect(pruneSelection(start, cells)).toBe(start);
  });
});

describe("bayCellsInOrder / batchTargets", () => {
  it("walks enclosures, then banks, then bays", () => {
    const topology = {
      enclosures: [
        {
          banks: [{ bays: [bay("a1"), bay("a2")] }, { bays: [bay("b1")] }],
        },
        { banks: [{ bays: [bay("c1")] }] },
      ],
    } as unknown as BayTopology;
    const ids = bayCellsInOrder(topology, (b) => cell(b.id, null)).map((c) => c.bay.id);
    expect(ids).toEqual(["a1", "a2", "b1", "c1"]);
  });

  it("lists targets in bay-map order, not the order they were ticked", () => {
    const cells = [cell("b1", "d1"), cell("b2", "d2"), cell("b3", "d3")];
    let s = EMPTY_SELECTION;
    s = toggleBay(s, cells[2]);
    s = toggleBay(s, cells[0]);
    expect(batchTargets(s, cells).map((t) => t.bayId)).toEqual(["b1", "b3"]);
    expect(batchTargets(s, cells)[0]).toMatchObject({ bayLabel: "B1", device: { id: "d1" } });
  });

  it("leaves out a stale entry even before pruning", () => {
    const s = sel([
      ["b1", "d1"],
      ["b2", "d2"],
    ]);
    const cells = [cell("b1", "d1"), cell("b2", "d2", "wiping")];
    expect(batchTargets(s, cells).map((t) => t.bayId)).toEqual(["b1"]);
  });
});

describe("makeBayLookup feeding the selection", () => {
  it("ticks only the occupied bay with no Job, with that bay's own device", () => {
    const resolved = {
      topology: {
        enclosures: [{ banks: [{ bays: [bay("free"), bay("held"), bay("fresh")] }] }],
      },
      occupancy: [
        { bay_id: "held", device_id: "d-quarantined" },
        { bay_id: "fresh", device_id: "d-new" },
      ],
      unplaced_devices: [],
    } as unknown as ResolvedBayTopology;
    const devicesById = new Map([
      ["d-quarantined", device("d-quarantined")],
      ["d-new", device("d-new")],
    ]);
    const quarantined = {
      id: "j-q",
      state: { state: "quarantined" },
      activities: [],
    } as unknown as Job;
    const jobsByDeviceId = new Map([["d-quarantined", quarantined]]);
    const cells = bayCellsInOrder(
      resolved.topology,
      makeBayLookup(resolved, devicesById, jobsByDeviceId, deriveSlotStatus),
    );
    expect(cells.map((c) => c.status.kind)).toEqual(["empty", "quarantined", "idle"]);
    expect([...selectAllIdle(EMPTY_SELECTION, cells)]).toEqual([["fresh", "d-new"]]);
  });
});

describe("canStart", () => {
  const targets = batchTargets(sel([["b1", "d1"]]), [cell("b1", "d1")]);

  it("needs at least one target, an operator, and no run in flight", () => {
    expect(canStart({ targets, operator, running: false })).toBe(true);
    expect(canStart({ targets: [], operator, running: false })).toBe(false);
    expect(canStart({ targets, operator: null, running: false })).toBe(false);
    // Double confirm: the second click lands while the first run is going.
    expect(canStart({ targets, operator, running: true })).toBe(false);
  });
});

describe("runBatch", () => {
  const cells = [cell("b1", "d1"), cell("b2", "d2"), cell("b3", "d3")];
  const targets = batchTargets(selectAllIdle(EMPTY_SELECTION, cells), cells);

  function fakeClient(opts: { failCreate?: string; failStart?: string } = {}) {
    const created: CreateJobRequest[] = [];
    const started: string[] = [];
    const client: BatchClient = {
      createJob: async (spec) => {
        if (spec.device_id === opts.failCreate) {
          throw new Error("404 Not Found: DeviceNotFound");
        }
        created.push(spec);
        return { job_id: `job-${spec.device_id}` };
      },
      startJob: async (id) => {
        if (id === `job-${opts.failStart}`) {
          throw new Error("409 Conflict: job not queued");
        }
        started.push(id);
        return { ok: true };
      },
    };
    return { client, created, started };
  }

  it("creates and starts one Job per drive, in order", async () => {
    const { client, created, started } = fakeClient();
    const results = await runBatch(targets, shared, new Map(), client);
    expect(created.map((c) => c.device_id)).toEqual(["d1", "d2", "d3"]);
    expect(started).toEqual(["job-d1", "job-d2", "job-d3"]);
    expect(results.map((r) => [r.target.bayId, r.outcome])).toEqual([
      ["b1", { kind: "started", jobId: "job-d1" }],
      ["b2", { kind: "started", jobId: "job-d2" }],
      ["b3", { kind: "started", jobId: "job-d3" }],
    ]);
  });

  it("carries the shared fields, the operator and each drive's own asset tag", async () => {
    const { client, created } = fakeClient();
    const tags = new Map([
      ["d1", "AT-1"],
      ["d2", "   "],
    ]);
    await runBatch(targets, shared, tags, client);
    for (const spec of created) {
      expect(spec).toMatchObject({
        classification: "moderate",
        intent: "recycle",
        operator,
        ticket_ref: "RMA-42",
        site_label: null,
      });
    }
    expect(created.map((c) => c.asset_tag)).toEqual(["AT-1", null, null]);
  });

  it("sends a null ticket when the shared ticket is blank", async () => {
    const { client, created } = fakeClient();
    await runBatch(targets.slice(0, 1), { ...shared, ticket: "" }, new Map(), client);
    expect(created[0].ticket_ref).toBeNull();
  });

  it("keeps going after a create failure and reports it verbatim", async () => {
    const { client, started } = fakeClient({ failCreate: "d2" });
    const results = await runBatch(targets, shared, new Map(), client);
    expect(results[1].outcome).toEqual({
      kind: "not_created",
      error: "404 Not Found: DeviceNotFound",
    });
    expect(started).toEqual(["job-d1", "job-d3"]);
    expect(results.map((r) => r.outcome.kind)).toEqual(["started", "not_created", "started"]);
  });

  it("reports a created but unstarted Job with its id and does not retry", async () => {
    const { client, started } = fakeClient({ failStart: "d1" });
    const results = await runBatch(targets, shared, new Map(), client);
    expect(results[0].outcome).toEqual({
      kind: "not_started",
      jobId: "job-d1",
      error: "409 Conflict: job not queued",
    });
    expect(started).toEqual(["job-d2", "job-d3"]);
  });

  it("returns no results for no targets", async () => {
    const { client, created } = fakeClient();
    expect(await runBatch([], shared, new Map(), client)).toEqual([]);
    expect(created).toEqual([]);
  });
});
