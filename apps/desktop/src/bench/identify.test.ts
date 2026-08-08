import { describe, expect, it } from "vitest";

import type { BayTopology, Device } from "@/api/types";
import {
  bindBayToDevice,
  baysBoundToPath,
  diffDevices,
  identifyProgress,
  nextPrompt,
  unbindBay,
  type IdentifyPrompt,
} from "./identify";
import { emptyTopology, generateBays, DEFAULT_RUN } from "./topologyEdit";

function device(id: string, path: string, serial = `SN-${id}`): Device {
  return {
    id,
    vendor: "TestCo",
    model: "TM-1",
    serial,
    wwn: null,
    capacity_bytes: 1_000,
    media_type: "ssd_sata",
    bus: "sata",
    firmware: null,
    removable: false,
    block_size: 512,
    path,
  } as Device;
}

/** A bench with `n` bays in one bank, all unbound. */
function bench(n: number): BayTopology {
  const t = emptyTopology();
  return {
    ...t,
    enclosures: [
      {
        id: "enc",
        label: "Test",
        kind: "rackmount",
        banks: [
          {
            id: "a",
            label: null,
            rows: n,
            cols: 1,
            form_factor: "3.5in",
            orientation: "horizontal",
            numbering: DEFAULT_RUN,
            bays: generateBays("enc", "a", n, 1, DEFAULT_RUN),
          },
        ],
      },
    ],
  };
}

const bayId = (t: BayTopology, label: string) =>
  t.enclosures[0].banks[0].bays.find((b) => b.label === label)!.id;
const bay = (t: BayTopology, label: string) =>
  t.enclosures[0].banks[0].bays.find((b) => b.label === label)!;

describe("diffDevices", () => {
  it("reports drives that appeared and disappeared", () => {
    const before = [device("a", "/dev/sda"), device("b", "/dev/sdb")];
    const after = [device("b", "/dev/sdb"), device("c", "/dev/sdc")];
    const { appeared, removed } = diffDevices(before, after);
    expect(appeared.map((d) => d.id)).toEqual(["c"]);
    expect(removed.map((d) => d.id)).toEqual(["a"]);
  });

  it("treats a reused port as a different drive", () => {
    // The regression this guards: keying on path would call this "no change"
    // and silently skip a bay the operator is waiting to map.
    const before = [device("a", "/dev/sda")];
    const after = [device("b", "/dev/sda")];
    const { appeared, removed } = diffDevices(before, after);
    expect(appeared.map((d) => d.id)).toEqual(["b"]);
    expect(removed.map((d) => d.id)).toEqual(["a"]);
  });

  it("is quiet when nothing changed", () => {
    const list = [device("a", "/dev/sda")];
    const { appeared, removed } = diffDevices(list, [...list]);
    expect(appeared).toHaveLength(0);
    expect(removed).toHaveLength(0);
  });
});

describe("nextPrompt", () => {
  it("asks about an insertion before a removal", () => {
    // An operator pushing trays home expects to be asked about the tray they
    // just pushed, not one they pulled a minute ago.
    const queue: IdentifyPrompt[] = [
      { kind: "removed", device: device("a", "/dev/sda") },
      { kind: "appeared", device: device("b", "/dev/sdb") },
    ];
    expect(nextPrompt(queue)).toMatchObject({ kind: "appeared" });
  });

  it("returns null on an empty queue", () => {
    expect(nextPrompt([])).toBeNull();
  });
});

describe("bindBayToDevice", () => {
  it("writes a path binding, not a positional one", () => {
    const t = bench(3);
    const next = bindBayToDevice(t, bayId(t, "2"), device("a", "/dev/sdb"));
    expect(bay(next, "2").binding).toEqual({ by: "path", path: "/dev/sdb" });
    expect(bay(next, "1").binding).toEqual({ by: "unbound" });
  });

  it("moves a path to the newly clicked bay rather than leaving two claims", () => {
    // Two bays claiming one port makes the map ambiguous. The most recent
    // answer is the one the operator just gave with their hands.
    let t = bench(3);
    t = bindBayToDevice(t, bayId(t, "1"), device("a", "/dev/sdb"));
    t = bindBayToDevice(t, bayId(t, "3"), device("a", "/dev/sdb"));

    expect(bay(t, "1").binding).toEqual({ by: "unbound" });
    expect(bay(t, "3").binding).toEqual({ by: "path", path: "/dev/sdb" });
    expect(baysBoundToPath(t, "/dev/sdb")).toHaveLength(1);
  });

  it("leaves other bays' bindings alone", () => {
    let t = bench(3);
    t = bindBayToDevice(t, bayId(t, "1"), device("a", "/dev/sda"));
    t = bindBayToDevice(t, bayId(t, "2"), device("b", "/dev/sdb"));
    expect(bay(t, "1").binding).toEqual({ by: "path", path: "/dev/sda" });
    expect(bay(t, "2").binding).toEqual({ by: "path", path: "/dev/sdb" });
  });

  it("does not mutate the input topology", () => {
    const t = bench(2);
    const before = JSON.stringify(t);
    bindBayToDevice(t, bayId(t, "1"), device("a", "/dev/sda"));
    expect(JSON.stringify(t)).toBe(before);
  });
});

describe("unbindBay", () => {
  it("clears the binding for the named bay only", () => {
    let t = bench(2);
    t = bindBayToDevice(t, bayId(t, "1"), device("a", "/dev/sda"));
    t = bindBayToDevice(t, bayId(t, "2"), device("b", "/dev/sdb"));
    t = unbindBay(t, bayId(t, "1"));
    expect(bay(t, "1").binding).toEqual({ by: "unbound" });
    expect(bay(t, "2").binding).toEqual({ by: "path", path: "/dev/sdb" });
  });
});

describe("identifyProgress", () => {
  it("counts declared bindings against mappable bays", () => {
    let t = bench(4);
    expect(identifyProgress(t)).toEqual({ identified: 0, total: 4 });
    t = bindBayToDevice(t, bayId(t, "2"), device("a", "/dev/sda"));
    expect(identifyProgress(t)).toEqual({ identified: 1, total: 4 });
  });

  it("excludes blanked-off bays from the total", () => {
    // A bay that can never hold a drive would make "k of N" unreachable.
    const t = bench(4);
    t.enclosures[0].banks[0].bays[3].disabled = true;
    expect(identifyProgress(t).total).toBe(3);
  });
});
