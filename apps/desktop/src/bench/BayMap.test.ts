import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { Bay, Device, Job, ResolvedBayTopology } from "@/api/types";
import { BayMap } from "./BayMap";
import { deriveSlotStatus } from "./slotStatus";

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

function bay(id: string, label: string, col: number): Bay {
  return { id, label, row: 0, col, binding: { by: "unbound" }, disabled: false };
}

const resolved: ResolvedBayTopology = {
  topology: {
    schema_version: 1,
    label: "Test bench",
    generated: false,
    auto_fill_unbound: false,
    revision: 1,
    enclosures: [
      {
        id: "enc",
        label: "Test",
        kind: "rackmount",
        banks: [
          {
            id: "a",
            label: null,
            rows: 1,
            cols: 2,
            form_factor: "3.5in",
            orientation: "horizontal",
            bays: [bay("bay-1", "1", 0), bay("bay-2", "2", 1)],
          },
        ],
      },
    ],
  },
  occupancy: [
    { bay_id: "bay-1", device_id: "d1" },
    { bay_id: "bay-2", device_id: "d2" },
  ],
  unplaced_devices: [],
};

/** The opening tag of the tray whose aria-label starts with `Bay <label>:`. */
function trayTag(html: string, label: string): string {
  const m = html.match(new RegExp(`<g[^>]*aria-label="Bay ${label}:[^"]*"[^>]*>`));
  expect(m, `tray for bay ${label}`).not.toBeNull();
  return m![0];
}

describe("BayMap batch selection", () => {
  it("draws ticked bays pressed and selected, unticked ones unpressed", () => {
    const html = renderToStaticMarkup(
      createElement(BayMap, {
        resolved,
        devicesById: new Map([
          ["d1", device("d1")],
          ["d2", device("d2")],
        ]),
        jobsByDeviceId: new Map<string, Job>(),
        deriveStatus: deriveSlotStatus,
        onSelect: () => {},
        selectedBayIds: new Set(["bay-1"]),
      }),
    );
    const first = trayTag(html, "1");
    const second = trayTag(html, "2");
    expect(first).toContain('aria-pressed="true"');
    expect(first).toContain(", selected");
    expect(second).toContain('aria-pressed="false"');
    expect(second).not.toContain(", selected");
  });

  it("carries no aria-pressed outside select mode", () => {
    const html = renderToStaticMarkup(
      createElement(BayMap, {
        resolved,
        devicesById: new Map([["d1", device("d1")]]),
        jobsByDeviceId: new Map<string, Job>(),
        deriveStatus: deriveSlotStatus,
        onSelect: () => {},
      }),
    );
    expect(html).not.toContain("aria-pressed");
  });
});
