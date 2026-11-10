import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { Entitlement } from "@/api/orders";
import type { Station } from "@/api/stations";
import {
  installCommand,
  KeyIdHowTo,
  preselected,
  RenewalAction,
  RenewedNotice,
  StationRow,
  seatChoices,
  stationName,
  stationState,
  SupersessionNote,
  VendorRootSection,
} from "./StationsPage";

const machine: Entitlement = {
  id: "5b0d8b4e-6f1e-4c3a-9d2e-1f6c2a7b8c90",
  order_id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  offering_name: "Station — annual, 1 seat",
  interval: "annual",
  grants: { scope: "machine", seats: 1, quota_erasures: null, features: [], term_months: 12 },
  term_start: "2026-11-08T10:00:00Z",
  term_end: "2027-11-08T10:00:00Z",
  status: "active",
  renews: true,
  current: true,
  cancelled_at: null,
  refunded_at: null,
};

const site: Entitlement = {
  ...machine,
  id: "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
  interval: "one_off",
  grants: { scope: "site", seats: 2, quota_erasures: 5000, features: [], term_months: 24 },
  renews: false,
};

const station: Station = {
  id: "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  entitlement_id: machine.id,
  offering_name: machine.offering_name,
  scope: "machine",
  instance_key_id: "ed25519:q83vEjRWeJA0MlR4kKvN7w",
  station_id: "bench-1",
  label: "Bench 1",
  site: null,
  registered_at: "2026-11-08T10:02:00Z",
  released_at: null,
  licence: {
    id: "a1b2c3d4-0000-4000-8000-000000000001",
    station_id: "7c9e6679-7425-40de-944b-e07fc1f90ae7",
    entitlement_id: machine.id,
    instance_key_id: "ed25519:q83vEjRWeJA0MlR4kKvN7w",
    root_key_id: "ed25519:AAAAAAAAAAAAAAAAAAAAAA",
    not_after: "2027-11-08T10:00:00Z",
    issued_at: "2026-11-08T10:02:00Z",
    status: "active",
    superseded_at: null,
    revoked_at: null,
  },
  renewal_available: false,
};

/** The markup's text with tags removed. */
function text(html: string): string {
  return html.replace(/<[^>]+>/g, "").replace(/&lt;/g, "<").replace(/&gt;/g, ">");
}

describe("seats", () => {
  it("count only stations not released, and only current entitlements", () => {
    const released = { ...station, id: "x", released_at: "2026-11-09T00:00:00Z" };
    const lapsed = { ...site, id: "lapsed", current: false };
    const choices = seatChoices([machine, site, lapsed], [station, released]);
    expect(choices.map((c) => [c.entitlement.id, c.free])).toEqual([
      [machine.id, 0],
      [site.id, 2],
    ]);
  });

  it("preselect the only entitlement with a free seat", () => {
    expect(preselected(seatChoices([machine, site], [station]))).toBe(site.id);
    expect(preselected(seatChoices([machine, site], []))).toBe("");
    expect(preselected(seatChoices([machine], [station]))).toBe("");
  });
});

describe("station text", () => {
  it("names a station by label and id", () => {
    expect(stationName(station)).toBe("Bench 1 (bench-1)");
    expect(stationName({ ...station, label: null })).toBe("bench-1");
    expect(stationName({ ...station, station_id: null })).toBe("Bench 1");
    expect(stationName({ ...station, label: null, station_id: null })).toBe(station.instance_key_id);
  });

  it("states licensed, released or unlicensed", () => {
    expect(stationState(station, new Date("2026-12-01T00:00:00Z"))).toBe(
      "Licensed to 2027-11-08",
    );
    expect(stationState({ ...station, released_at: "2026-12-01T00:00:00Z" })).toBe(
      "Released on 2026-12-01",
    );
    expect(stationState({ ...station, licence: null })).toBe("No licence");
  });

  it("states an active licence past its not_after as expired", () => {
    const now = new Date("2027-11-08T10:00:00Z");
    expect(stationState(station, now)).toBe("Expired on 2027-11-08");
    expect(stationState(station, new Date("2027-11-09T00:00:00Z"))).toBe("Expired on 2027-11-08");
    expect(stationState(station, new Date("2027-11-08T09:59:59Z"))).toBe("Licensed to 2027-11-08");
  });
});

describe("renewal", () => {
  const renewable = { ...station, renewal_available: true };

  function render(s: Station, canRenew: boolean): string {
    return text(
      renderToStaticMarkup(
        createElement(
          QueryClientProvider,
          { client: new QueryClient() },
          createElement(RenewalAction, { station: s, canRenew }),
        ),
      ),
    );
  }

  it("states a renewal is available, with the current file's end", () => {
    expect(stationState(renewable, new Date("2026-12-01T00:00:00Z"))).toBe(
      "Renewal available (current licence to 2027-11-08)",
    );
    expect(stationState(renewable, new Date("2027-12-01T00:00:00Z"))).toBe(
      "Renewal available (current licence expired on 2027-11-08)",
    );
    expect(stationState({ ...renewable, released_at: "2026-12-01T00:00:00Z" })).toBe(
      "Released on 2026-12-01",
    );
  });

  it("lets an owner issue the renewed licence", () => {
    expect(render(renewable, true)).toContain("Issue renewed licence");
  });

  it("tells a member an owner can issue it", () => {
    const t = render(renewable, false);
    expect(t).toContain("An owner can issue the renewed licence.");
    expect(t).not.toContain("Renewal available");
    expect(t).not.toContain("Issue renewed licence");
  });

  function renderRow(s: Station, canRelease: boolean): string {
    return text(
      renderToStaticMarkup(
        createElement(
          QueryClientProvider,
          { client: new QueryClient() },
          createElement(
            "table",
            null,
            createElement("tbody", null, createElement(StationRow, { station: s, canRelease })),
          ),
        ),
      ),
    );
  }

  it("offers the renewal in an owner's station row and tells a member in theirs", () => {
    const owner = renderRow(renewable, true);
    expect(owner).toContain("Issue renewed licence");
    expect(owner).toContain("Release");
    const member = renderRow(renewable, false);
    expect(member).toContain("An owner can issue the renewed licence.");
    expect(member).not.toContain("Issue renewed licence");
    expect(renderRow(station, true)).not.toContain("Issue renewed licence");
  });

  it("offers nothing when no renewal is available or the station is released", () => {
    expect(render(station, true)).toBe("");
    expect(render({ ...renewable, released_at: "2026-12-01T00:00:00Z" }, true)).toBe("");
  });

  it("links the renewed licence and says to install it", () => {
    const renewed = {
      ...station.licence!,
      id: "b2c3d4e5-0000-4000-8000-000000000002",
      not_after: "2028-11-08T10:00:00Z",
    };
    const html = renderToStaticMarkup(createElement(RenewedNotice, { licence: renewed }));
    expect(html).toContain(`href="/api/licences/${renewed.id}/download"`);
    const t = text(html);
    expect(t).toContain("to 2028-11-08");
    expect(t).toContain("Download the renewed licence");
    expect(t).toContain("until then it keeps using the old file");
  });
});

describe("a revoked licence", () => {
  const revoked: Station = {
    ...station,
    licence: { ...station.licence!, status: "revoked", revoked_at: "2026-12-02T09:00:00Z" },
  };

  function renderRow(s: Station): string {
    return renderToStaticMarkup(
      createElement(
        QueryClientProvider,
        { client: new QueryClient() },
        createElement(
          "table",
          null,
          createElement("tbody", null, createElement(StationRow, { station: s, canRelease: true })),
        ),
      ),
    );
  }

  it("reads Revoked on its date, even when a renewal would otherwise be due", () => {
    expect(stationState(revoked, new Date("2026-12-03T00:00:00Z"))).toBe("Revoked on 2026-12-02");
    expect(stationState({ ...revoked, released_at: "2026-12-04T00:00:00Z" })).toBe(
      "Released on 2026-12-04",
    );
  });

  it("has no download link but can still be released", () => {
    const html = renderRow(revoked);
    expect(html).not.toContain("/download");
    const t = text(html);
    expect(t).toContain("Revoked on 2026-12-02");
    expect(t).not.toContain("Download licence");
    expect(t).toContain("Release");
    expect(renderRow(station)).toContain(`/api/licences/${station.licence!.id}/download`);
  });

  it("is explained to every member", () => {
    const t = text(renderToStaticMarkup(createElement(SupersessionNote)));
    expect(t).toContain("the vendor revoked shows Revoked");
  });
});

describe("guidance every member sees", () => {
  it("shows how to read the instance key id", () => {
    const t = text(renderToStaticMarkup(createElement(KeyIdHowTo)));
    expect(t).toContain("GET /api/public_key");
    expect(t).toContain("public_key_id");
    expect(t).toContain("signing key public id:");
  });

  it("says a released licence keeps working until it expires", () => {
    const t = text(renderToStaticMarkup(createElement(SupersessionNote)));
    expect(t).toContain("keeps working until its own expiry date");
    expect(t).toContain("does not send revocation or supersession to stations");
  });

  it("says a renewed licence works only once installed, and the old file until it expires", () => {
    const t = text(renderToStaticMarkup(createElement(SupersessionNote)));
    expect(t).toContain("uses the renewed licence only after the renewed file is installed");
    expect(t).toContain("The old file keeps working until its own expiry date");
  });

  it("shows the vendor root public key and the install command", () => {
    const root = { public_key_id: "ed25519:AAAAAAAAAAAAAAAAAAAAAA", public_key_b64: "Zm9vYmFy" };
    const t = text(renderToStaticMarkup(createElement(VendorRootSection, { root })));
    expect(t).toContain(root.public_key_id);
    expect(t).toContain(root.public_key_b64);
    expect(t).toContain(installCommand(root));
    expect(installCommand(root)).toBe(
      "wipestation serve --station-id <station id> --license <licence file> " +
        "--vendor-root-b64 Zm9vYmFy",
    );
  });

  it("says when the hub has no vendor root", () => {
    const t = text(renderToStaticMarkup(createElement(VendorRootSection, { root: null })));
    expect(t).toContain("no vendor root configured");
  });
});

describe("no lapse policy (retired, ADR-0013 §5)", () => {
  function renderIn(node: ReturnType<typeof createElement>): string {
    return text(
      renderToStaticMarkup(createElement(QueryClientProvider, { client: new QueryClient() }, node)),
    );
  }

  function renderRow(s: Station, canRelease: boolean): string {
    return renderIn(
      createElement(
        "table",
        null,
        createElement("tbody", null, createElement(StationRow, { station: s, canRelease })),
      ),
    );
  }

  it("renders no lapse-policy setting, text or reissue reason", () => {
    const renewable: Station = { ...station, renewal_available: true };
    const now = new Date("2026-12-01T00:00:00Z");
    const rendered = [
      renderRow(station, true),
      renderRow(station, false),
      renderRow(renewable, true),
      renderRow(renewable, false),
      text(renderToStaticMarkup(createElement(SupersessionNote))),
      text(renderToStaticMarkup(createElement(RenewedNotice, { licence: station.licence! }))),
      stationState(station, now),
      stationState(renewable, now),
    ];
    for (const t of rendered) {
      expect(t).not.toMatch(/lapse/i);
      expect(t).not.toContain("Updated licence");
      expect(t).not.toContain("Stop new jobs");
    }
  });

  it("still offers owners a renewal, and tells members", () => {
    const renewable: Station = { ...station, renewal_available: true };
    expect(renderRow(renewable, true)).toContain("Issue renewed licence");
    expect(renderRow(renewable, false)).toContain("An owner can issue the renewed licence.");
    expect(renderRow(station, true)).not.toContain("Issue renewed licence");
  });
});
