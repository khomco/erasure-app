import { beforeEach, describe, expect, it } from "vitest";
import { HubError, resetCsrfCache } from "./auth";
import {
  fetchStations,
  fetchVendorRoot,
  isInstanceKeyId,
  isLicence,
  isStation,
  isVendorRootKey,
  licenceDownloadPath,
  registerBody,
  registerStation,
  reissueStation,
  releaseStation,
  type Licence,
  type Station,
} from "./stations";

const KEY = "ed25519:q83vEjRWeJA0MlR4kKvN7w";

const licence: Licence = {
  id: "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
  station_id: "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  entitlement_id: "5b0d8b4e-6f1e-4c3a-9d2e-1f6c2a7b8c90",
  instance_key_id: KEY,
  root_key_id: "ed25519:AAAAAAAAAAAAAAAAAAAAAA",
  not_after: "2027-11-08T10:01:00Z",
  issued_at: "2026-11-08T10:02:00Z",
  status: "active",
  superseded_at: null,
  revoked_at: null,
};

const station: Station = {
  id: licence.station_id,
  entitlement_id: licence.entitlement_id,
  offering_name: "Station — annual, 1 seat",
  scope: "machine",
  instance_key_id: KEY,
  station_id: "bench-1",
  label: "Bench 1",
  site: null,
  registered_at: "2026-11-08T10:02:00Z",
  released_at: null,
  licence,
  renewal_available: false,
};

interface Call {
  url: string;
  method: string;
  body: unknown;
  csrf: string | null;
}

/** A fake hub, as in `orders.test.ts`. */
function respond(status: number, body?: unknown) {
  const calls: Call[] = [];
  const impl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const method = init?.method ?? "GET";
    if (url === "/api/auth/csrf" && method === "GET") {
      return new Response(JSON.stringify({ csrf_token: "csrf-1" }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    const sent = (init?.headers ?? {}) as Record<string, string>;
    calls.push({
      url,
      method,
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
      csrf: sent["X-CSRF-Token"] ?? null,
    });
    return new Response(body === undefined ? null : JSON.stringify(body), {
      status,
      headers: { "Content-Type": "application/json" },
    });
  }) as typeof fetch;
  return { impl, calls };
}

beforeEach(() => resetCsrfCache());

async function rejection(p: Promise<unknown>): Promise<unknown> {
  return p.then(
    () => undefined,
    (e: unknown) => e,
  );
}

describe("guards", () => {
  it("accept the hub's shapes", () => {
    expect(isLicence(licence)).toBe(true);
    expect(
      isLicence({ ...licence, status: "superseded", superseded_at: "2026-12-01T00:00:00Z" }),
    ).toBe(true);
    expect(
      isLicence({ ...licence, status: "revoked", revoked_at: "2026-12-02T00:00:00Z" }),
    ).toBe(true);
    expect(isStation(station)).toBe(true);
    expect(isStation({ ...station, renewal_available: true })).toBe(true);
    // The retired lapse-policy fields are no longer required (ADR-0013 §5).
    expect("lapse_policy" in licence).toBe(false);
    expect("lapse_policy_update_available" in station).toBe(false);
    // A hub not yet redeployed may still send them: still accepted.
    expect(isLicence({ ...licence, lapse_policy: "stop" })).toBe(true);
    expect(isStation({ ...station, lapse_policy_update_available: true })).toBe(true);
    expect(isStation({ ...station, licence: null, scope: "site", station_id: null })).toBe(true);
    expect(isVendorRootKey({ public_key_id: KEY, public_key_b64: "abc" })).toBe(true);
  });

  it("reject malformed stations and licences", () => {
    expect(isLicence({ ...licence, status: "cancelled" })).toBe(false);
    expect(isLicence({ ...licence, revoked_at: undefined })).toBe(false);
    expect(isLicence({ ...licence, superseded_at: undefined })).toBe(false);
    expect(isStation({ ...station, scope: "fleet" })).toBe(false);
    expect(isStation({ ...station, released_at: undefined })).toBe(false);
    expect(isStation({ ...station, licence: { ...licence, id: 1 } })).toBe(false);
    expect(isStation({ ...station, label: 3 })).toBe(false);
    expect(isStation({ ...station, renewal_available: undefined })).toBe(false);
    expect(isStation({ ...station, renewal_available: "true" })).toBe(false);
    expect(isVendorRootKey({ public_key_id: KEY })).toBe(false);
  });

  it("check the instance key id's shape like the hub", () => {
    expect(isInstanceKeyId(KEY)).toBe(true);
    expect(isInstanceKeyId(`  ${KEY}\n`)).toBe(true);
    for (const bad of ["", "bench-1", KEY.slice(0, -1), `${KEY}A`, KEY.replace("ed25519", "rsa")]) {
      expect(isInstanceKeyId(bad), bad).toBe(false);
    }
  });
});

describe("registerBody", () => {
  it("trims and drops blank optional fields", () => {
    expect(
      registerBody({
        entitlement_id: "e",
        instance_key_id: ` ${KEY} `,
        station_id: " bench-1 ",
        label: "  ",
        site: "",
      }),
    ).toEqual({ entitlement_id: "e", instance_key_id: KEY, station_id: "bench-1" });
  });
});

describe("calls", () => {
  it("lists stations with a GET", async () => {
    const f = respond(200, { stations: [station] });
    expect(await fetchStations(f.impl)).toEqual([station]);
    expect(f.calls).toEqual([{ url: "/api/stations", method: "GET", body: undefined, csrf: null }]);
  });

  it("registers with the trimmed body and CSRF", async () => {
    const f = respond(201, { station, licence });
    expect(
      await registerStation(
        { entitlement_id: station.entitlement_id, instance_key_id: KEY, station_id: "bench-1" },
        f.impl,
      ),
    ).toEqual({ station, licence });
    expect(f.calls).toEqual([
      {
        url: "/api/stations",
        method: "POST",
        body: { entitlement_id: station.entitlement_id, instance_key_id: KEY, station_id: "bench-1" },
        csrf: "csrf-1",
      },
    ]);
  });

  it("surfaces the hub's refusal", async () => {
    const message = "every seat of this entitlement is in use; release a station or buy more seats";
    const f = respond(409, { error: message });
    const e = await rejection(
      registerStation({ entitlement_id: "e", instance_key_id: KEY }, f.impl),
    );
    expect(e).toBeInstanceOf(HubError);
    expect((e as HubError).status).toBe(409);
    expect((e as HubError).message).toBe(message);
  });

  it("releases with a POST and CSRF", async () => {
    const released = { ...station, released_at: "2026-11-09T00:00:00Z" };
    const f = respond(200, { station: released });
    expect(await releaseStation(station.id, f.impl)).toEqual(released);
    expect(f.calls).toEqual([
      { url: `/api/stations/${station.id}/release`, method: "POST", body: {}, csrf: "csrf-1" },
    ]);
  });

  it("reissues with a POST and CSRF", async () => {
    const renewed: Licence = {
      ...licence,
      id: "9a7b2c1d-0000-4000-8000-000000000002",
      not_after: "2028-11-08T10:01:00Z",
      issued_at: "2027-01-02T00:00:00Z",
    };
    const body = { station: { ...station, licence: renewed }, licence: renewed };
    const f = respond(201, body);
    expect(await reissueStation(station.id, f.impl)).toEqual(body);
    expect(f.calls).toEqual([
      { url: `/api/stations/${station.id}/reissue`, method: "POST", body: {}, csrf: "csrf-1" },
    ]);
  });

  it("surfaces a refused reissue", async () => {
    const message = "this station's licence already runs to the end of its entitlement's term";
    const e = await rejection(reissueStation(station.id, respond(409, { error: message }).impl));
    expect(e).toBeInstanceOf(HubError);
    expect((e as HubError).status).toBe(409);
    expect((e as HubError).message).toBe(message);
  });

  it("reads the vendor root, or its absence", async () => {
    const root = { public_key_id: KEY, public_key_b64: "q83vEjRWeJA" };
    expect(await fetchVendorRoot(respond(200, { root }).impl)).toEqual(root);
    expect(await fetchVendorRoot(respond(200, { root: null }).impl)).toBeNull();
    await expect(fetchVendorRoot(respond(200, { root: "x" }).impl)).rejects.toThrow(
      "vendor root returned an unexpected body",
    );
  });

  it("downloads from the licence's path", () => {
    expect(licenceDownloadPath(licence.id)).toBe(`/api/licences/${licence.id}/download`);
  });
});
