import { beforeEach, describe, expect, it } from "vitest";
import { HubError, resetCsrfCache } from "./auth";
import {
  FEATURES,
  createOffering,
  fetchOffering,
  formatPrice,
  fractionDigits,
  listOfferings,
  minorToDecimal,
  publishOffering,
  retireOffering,
  toMinorUnits,
  updateOffering,
  type Offering,
  type OfferingInput,
} from "./adminOfferings";

const input: OfferingInput = {
  name: "Station — annual, 1 seat",
  description: "One station.",
  price_minor: 120000,
  currency: "USD",
  interval: "annual",
  grants: {
    scope: "machine",
    seats: 1,
    quota_erasures: null,
    features: ["pdf_certificates"],
    term_months: 12,
  },
};

const offering: Offering = {
  ...input,
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  status: "draft",
  created_at: "2026-11-08T10:00:00Z",
  updated_at: "2026-11-08T10:00:00Z",
  published_at: null,
  retired_at: null,
};

interface Call {
  url: string;
  method: string;
  body: unknown;
  csrf: string | null;
}

/** A fake hub, as in `adminLeads.test.ts`. */
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

describe("money", () => {
  it("knows each currency's minor digits", () => {
    expect(fractionDigits("USD")).toBe(2);
    expect(fractionDigits("JPY")).toBe(0);
    expect(fractionDigits("US")).toBeNull();
  });

  it("parses amounts by string arithmetic", () => {
    expect(toMinorUnits("12", "USD")).toBe(1200);
    expect(toMinorUnits("12.5", "USD")).toBe(1250);
    expect(toMinorUnits(" 1200.00 ", "USD")).toBe(120000);
    expect(toMinorUnits("0.07", "USD")).toBe(7);
    // 0.1 + 0.2 territory: no float ever enters.
    expect(toMinorUnits("999999.99", "USD")).toBe(99999999);
    expect(toMinorUnits("1500", "JPY")).toBe(1500);
  });

  it("rejects too many decimals and anything not a plain decimal", () => {
    expect(toMinorUnits("12.345", "USD")).toBeNull();
    expect(toMinorUnits("12.5", "JPY")).toBeNull();
    for (const bad of ["", "12.", ".5", "-1", "1,200", "1e3", "12 USD", "abc"]) {
      expect(toMinorUnits(bad, "USD"), bad).toBeNull();
    }
    expect(toMinorUnits("12", "nope")).toBeNull();
    expect(toMinorUnits("99999999999999999999", "USD")).toBeNull();
  });

  it("formats minor units back without floats", () => {
    expect(minorToDecimal(120000, "USD")).toBe("1200.00");
    expect(minorToDecimal(7, "USD")).toBe("0.07");
    expect(minorToDecimal(1500, "JPY")).toBe("1500");
    expect(formatPrice(120000, "USD")).toBe("1,200.00 USD");
    expect(formatPrice(99999999, "USD")).toBe("999,999.99 USD");
    expect(formatPrice(1500000, "JPY")).toBe("1,500,000 JPY");
    for (const n of [1, 99, 100, 123456]) {
      expect(toMinorUnits(minorToDecimal(n, "EUR"), "EUR")).toBe(n);
    }
  });
});

describe("offering calls", () => {
  it("lists with a GET without CSRF", async () => {
    const f = respond(200, { offerings: [offering] });
    expect(await listOfferings(f.impl)).toEqual({ offerings: [offering] });
    expect(f.calls).toEqual([
      { url: "/api/admin/offerings", method: "GET", body: undefined, csrf: null },
    ]);
  });

  it("rejects an unknown feature or status in the hub's answer", async () => {
    const bad = respond(200, {
      offerings: [{ ...offering, grants: { ...offering.grants, features: ["teleport"] } }],
    });
    const err = await rejection(listOfferings(bad.impl));
    expect((err as Error).message).toBe("offering list returned an unexpected body (HTTP 200)");
    const bad2 = respond(200, { offering: { ...offering, status: "archived" } });
    expect(await rejection(fetchOffering(offering.id, bad2.impl))).toBeInstanceOf(Error);
  });

  it("fetches one by encoded id", async () => {
    const f = respond(200, { offering });
    expect(await fetchOffering(offering.id, f.impl)).toEqual(offering);
    const missing = respond(404, { error: "no such offering" });
    const err = await rejection(fetchOffering("x/y", missing.impl));
    expect((err as HubError).status).toBe(404);
    expect(missing.calls[0].url).toBe("/api/admin/offerings/x%2Fy");
  });

  it("creates with the full body and CSRF", async () => {
    const f = respond(201, { offering });
    expect(await createOffering(input, f.impl)).toEqual(offering);
    expect(f.calls).toEqual([
      { url: "/api/admin/offerings", method: "POST", body: input, csrf: "csrf-1" },
    ]);
  });

  it("updates with the full body; a frozen term is a 409 HubError", async () => {
    const f = respond(200, { offering });
    expect(await updateOffering(offering.id, input, f.impl)).toEqual(offering);
    expect(f.calls).toEqual([
      { url: `/api/admin/offerings/${offering.id}`, method: "POST", body: input, csrf: "csrf-1" },
    ]);
    const frozen = respond(409, { error: "a published offering's price … cannot change" });
    const err = await rejection(updateOffering(offering.id, input, frozen.impl));
    expect(err).toBeInstanceOf(HubError);
    expect((err as HubError).status).toBe(409);
  });

  it("publishes and retires with CSRF", async () => {
    const published = { ...offering, status: "published" as const, published_at: "2026-11-08T11:00:00Z" };
    const p = respond(200, { offering: published });
    expect(await publishOffering(offering.id, p.impl)).toEqual(published);
    expect(p.calls).toEqual([
      { url: `/api/admin/offerings/${offering.id}/publish`, method: "POST", body: {}, csrf: "csrf-1" },
    ]);
    const retired = { ...published, status: "retired" as const, retired_at: "2026-11-09T11:00:00Z" };
    const r = respond(200, { offering: retired });
    expect(await retireOffering(offering.id, r.impl)).toEqual(retired);
    expect(r.calls).toEqual([
      { url: `/api/admin/offerings/${offering.id}/retire`, method: "POST", body: {}, csrf: "csrf-1" },
    ]);
  });

  it("mirrors wipe_license::Feature", () => {
    expect(FEATURES).toEqual([
      "enterprise_mode",
      "hub_sync",
      "erp_integration",
      "pdf_certificates",
      "cert_anchoring",
      "operator_sso",
    ]);
  });
});
