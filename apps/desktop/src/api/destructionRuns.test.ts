import { describe, expect, it } from "vitest";

import type { AttestationSigner, DestructionManifest, Job, OperatorRef } from "./types";
import {
  canPreviewCompletion,
  canPreviewConcurrence,
  commonEquipmentId,
  completionPreviewKey,
  concurrencePreviewKey,
  concurrenceSignerProblem,
  localInputToRfc3339,
  runDriveLabels,
  toLocalInput,
  readyToComplete,
  readyToConcur,
  runStatusLabel,
  toCompletionPreviewRequest,
  toCompletionRequest,
  toConcurrenceRequest,
  type CompletionForm,
  type ConcurrenceForm,
  type ReviewedCompletion,
} from "./destructionRuns";

const operator: OperatorRef = { id: "op-1", display_name: "Ana Ruiz", email: "ana@example.com" };

function completion(overrides: Partial<CompletionForm> = {}): CompletionForm {
  return {
    completed_at_local: "2026-11-08T09:00:00",
    remnants_inspected: true,
    equipment_cycle_completed: true,
    equipment_id: "SHR-2",
    criterion: "particles ≤ 2 mm",
    video_ref: "",
    full_name: "Ana Ruiz",
    title: "Bench lead",
    attested: true,
    ...overrides,
  };
}

function reviewed(form: CompletionForm): ReviewedCompletion {
  return {
    key: completionPreviewKey(form),
    statement: "Destruction of the device(s) listed above was completed on …",
    completed_at: "2026-11-08T15:04:05Z",
  };
}

function jobWithEquipment(...equipment: Array<string | null>): Job {
  return {
    id: "j",
    activities: equipment.map((e) => ({
      type: "destruction",
      attestation: e == null ? undefined : { equipment_id: e },
    })),
  } as unknown as Job;
}

describe("runStatusLabel", () => {
  const run = (
    state: DestructionManifest["state"],
    required_concurrence: DestructionManifest["required_concurrence"] = null,
  ) => ({ state, required_concurrence });

  it("names each state as the operator reads it", () => {
    expect(runStatusLabel(run("pending"))).toBe("Pending completion");
    expect(runStatusLabel(run("pending", "witness"))).toBe("Pending completion");
    expect(runStatusLabel(run("awaiting_concurrence", "review"))).toBe("Awaiting review");
    expect(runStatusLabel(run("awaiting_concurrence", "witness"))).toBe("Awaiting witness");
    expect(runStatusLabel(run("completed", "witness"))).toBe("Completed");
    expect(runStatusLabel(run("rejected"))).toBe("Rejected");
  });
});

describe("commonEquipmentId", () => {
  it("returns the equipment every member's latest attestation names", () => {
    expect(
      commonEquipmentId([jobWithEquipment("SHR-2"), jobWithEquipment("OLD", " SHR-2 ")]),
    ).toBe("SHR-2");
  });

  it("returns null when they differ, one is missing, or there are no Jobs", () => {
    expect(commonEquipmentId([jobWithEquipment("SHR-2"), jobWithEquipment("CR-1")])).toBeNull();
    expect(commonEquipmentId([jobWithEquipment("SHR-2"), jobWithEquipment(null)])).toBeNull();
    expect(commonEquipmentId([jobWithEquipment("SHR-2"), jobWithEquipment()])).toBeNull();
    expect(commonEquipmentId([])).toBeNull();
  });
});

describe("runDriveLabels", () => {
  const job = (id: string, asset_tag: string | null, serial: string | null): Job =>
    ({
      id,
      activities: [
        { type: "destruction", attestation: { identifiers: { asset_tag: "OLD", serial: null } } },
        { type: "destruction", attestation: { identifiers: { asset_tag, serial } } },
      ],
    }) as unknown as Job;

  it("lists each member's latest identifiers, in run order, else its short Job id", () => {
    const jobs = [
      job("aaaaaaaa-1", "A-17", "S1"),
      job("bbbbbbbb-2", null, " S2 "),
      job("cccccccc-3", " ", null),
    ];
    expect(runDriveLabels(["bbbbbbbb-2", "aaaaaaaa-1", "cccccccc-3", "dddddddd-4"], jobs)).toEqual([
      "serial S2",
      "asset tag A-17 · serial S1",
      "Job cccccccc",
      "Job dddddddd",
    ]);
  });
});

describe("completion form", () => {
  it("previews only the fields the statement names, with a blank video as none", () => {
    expect(toCompletionPreviewRequest(completion({ video_ref: "  " }))).toEqual({
      completed_at: localInputToRfc3339("2026-11-08T09:00:00"),
      equipment_id: "SHR-2",
      criterion: "particles ≤ 2 mm",
      video_ref: null,
    });
    expect(canPreviewCompletion(completion())).toBe(true);
    expect(canPreviewCompletion(completion({ criterion: " " }))).toBe(false);
    expect(canPreviewCompletion(completion({ equipment_id: "" }))).toBe(false);
    expect(canPreviewCompletion(completion({ completed_at_local: "" }))).toBe(false);
    expect(canPreviewCompletion(completion({ completed_at_local: "not a time" }))).toBe(false);
  });

  it("converts the local completion time to RFC 3339 UTC, to the second", () => {
    const local = new Date(2026, 10, 8, 9, 0, 5);
    expect(toLocalInput(local)).toBe("2026-11-08T09:00:05");
    expect(localInputToRfc3339(toLocalInput(local))).toBe(
      local.toISOString().replace(/\.\d{3}Z$/, "Z"),
    );
    expect(localInputToRfc3339("2026-11-08T09:00:05")).toMatch(/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/);
    expect(localInputToRfc3339(" ")).toBeNull();
  });

  it("makes the reviewed statement stale when the completion time changes", () => {
    const f = completion();
    const later = completion({ completed_at_local: "2026-11-08T14:00:00" });
    expect(completionPreviewKey(later)).not.toBe(completionPreviewKey(f));
    expect(readyToComplete(later, reviewed(f))).toBe(false);
  });

  it("is ready only when verified, signed, attested and the statement is current", () => {
    const f = completion();
    expect(readyToComplete(f, reviewed(f))).toBe(true);
    expect(readyToComplete(f, null)).toBe(false);
    for (const change of [
      { remnants_inspected: false },
      { equipment_cycle_completed: false },
      { full_name: " " },
      { title: "" },
      { attested: false },
    ]) {
      expect(readyToComplete({ ...f, ...change }, reviewed(f))).toBe(false);
    }
    // An edit to a statement input makes the reviewed statement stale.
    expect(readyToComplete(completion({ criterion: "platters will not engage" }), reviewed(f))).toBe(
      false,
    );
    expect(completionPreviewKey(completion({ video_ref: "CAM-3" }))).not.toBe(completionPreviewKey(f));
  });

  it("sends back the reviewed statement's time", () => {
    const f = completion({ video_ref: " CAM-3 14:02 " });
    expect(toCompletionRequest(f, operator, reviewed(f))).toEqual({
      completed_at: "2026-11-08T15:04:05Z",
      remnants_inspected: true,
      equipment_cycle_completed: true,
      equipment_id: "SHR-2",
      criterion: "particles ≤ 2 mm",
      video_ref: "CAM-3 14:02",
      operator,
      full_name: "Ana Ruiz",
      title: "Bench lead",
      attest: true,
    });
  });
});

describe("concurrence form", () => {
  const form = (overrides: Partial<ConcurrenceForm> = {}): ConcurrenceForm => ({
    full_name: "Ben Ode",
    title: "Quality lead",
    attested: true,
    ...overrides,
  });

  const completer: AttestationSigner = {
    operator,
    full_name: "Ana Ruiz",
    title: "Bench lead",
    identity_assurance: { level: "self_asserted" },
  };
  const ben: OperatorRef = { id: "op-2", display_name: "Ben", email: "ben@example.com" };

  it("is ready only when attested against a statement for the current name and title", () => {
    const f = form();
    const preview = { key: concurrencePreviewKey(f), statement: "I, Ben Ode, …" };
    expect(canPreviewConcurrence(f)).toBe(true);
    expect(canPreviewConcurrence(form({ title: " " }))).toBe(false);
    expect(readyToConcur(f, preview, ben, completer)).toBe(true);
    expect(readyToConcur(form({ attested: false }), preview, ben, completer)).toBe(false);
    expect(readyToConcur(form({ full_name: "Ben O." }), preview, ben, completer)).toBe(false);
    expect(readyToConcur(f, null, ben, completer)).toBe(false);
  });

  it("refuses the completion's signer, by operator id or by name ignoring case and spacing", () => {
    expect(concurrenceSignerProblem(form(), ben, completer)).toBeNull();
    expect(concurrenceSignerProblem(form(), operator, completer)).toMatch(/sign in as/);
    const sameName = form({ full_name: "  ana   RUIZ " });
    expect(concurrenceSignerProblem(sameName, ben, completer)).toMatch(/someone else/);
    const preview = { key: concurrencePreviewKey(sameName), statement: "…" };
    expect(readyToConcur(sameName, preview, ben, completer)).toBe(false);
    const own = { key: concurrencePreviewKey(form()), statement: "…" };
    expect(readyToConcur(form(), own, operator, completer)).toBe(false);
  });

  it("sends no kind: the run decides it", () => {
    expect(toConcurrenceRequest(form(), operator)).toEqual({
      operator,
      full_name: "Ben Ode",
      title: "Quality lead",
      attest: true,
    });
  });
});
