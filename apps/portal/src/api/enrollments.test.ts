import { beforeEach, describe, expect, it } from "vitest";
import { HubError, resetCsrfCache } from "./auth";
import {
  approveEnrollment,
  denyEnrollment,
  enrollmentErrorMessage,
  isPendingEnrollment,
  lookupEnrollment,
  type PendingEnrollment,
} from "./enrollments";
import type { Licence, Station } from "./stations";

const KEY = "ed25519:q83vEjRWeJA0MlR4kKvN7w";

const enrollment: PendingEnrollment = {
  user_code: "BCDF-GHJK",
  station_id: "bench-1",
  label: "Bench 1",
  instance_key_id: KEY,
  requested_from: "203.0.113.7",
  requested_at: "2026-11-09T10:02:00Z",
  expires_at: "2026-11-09T10:12:00Z",
};

const licence: Licence = {
  id: "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
  station_id: "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  entitlement_id: "5b0d8b4e-6f1e-4c3a-9d2e-1f6c2a7b8c90",
  instance_key_id: KEY,
  root_key_id: "ed25519:AAAAAAAAAAAAAAAAAAAAAA",
  not_after: "2027-11-08T10:01:00Z",
  issued_at: "2026-11-09T10:03:00Z",
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
  registered_at: "2026-11-09T10:03:00Z",
  released_at: null,
  licence,
  renewal_available: false,
  credit_balance: null,
  credit_balance_reported_at: null,
  credits_granted: 0,
};

interface Call {
  url: string;
  method: string;
  body: unknown;
  csrf: string | null;
}

/** A fake hub, as in `stations.test.ts`. */
function respond(status: number, body?: unknown, headers: Record<string, string> = {}) {
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
      headers: { "Content-Type": "application/json", ...headers },
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

describe("guard", () => {
  it("accepts the hub's shape, with or without label and address", () => {
    expect(isPendingEnrollment(enrollment)).toBe(true);
    expect(isPendingEnrollment({ ...enrollment, label: null, requested_from: null })).toBe(true);
  });

  it("rejects malformed enrollments", () => {
    expect(isPendingEnrollment({ ...enrollment, label: undefined })).toBe(false);
    expect(isPendingEnrollment({ ...enrollment, requested_from: undefined })).toBe(false);
    expect(isPendingEnrollment({ ...enrollment, station_id: null })).toBe(false);
    expect(isPendingEnrollment({ ...enrollment, expires_at: 1 })).toBe(false);
    expect(isPendingEnrollment(null)).toBe(false);
  });
});

describe("calls", () => {
  it("look up a code in the body, trimmed, with the CSRF token", async () => {
    const hub = respond(200, { enrollment });
    expect(await lookupEnrollment("  bcdf-ghjk ", hub.impl)).toEqual(enrollment);
    expect(hub.calls).toEqual([
      { url: "/api/enrollments/lookup", method: "POST", body: { code: "bcdf-ghjk" }, csrf: "csrf-1" },
    ]);
  });

  it("approve with the code and the chosen entitlement", async () => {
    const hub = respond(201, { station, licence });
    expect(await approveEnrollment("BCDF-GHJK", licence.entitlement_id, hub.impl)).toEqual({
      station,
      licence,
    });
    expect(hub.calls).toEqual([
      {
        url: "/api/enrollments/approve",
        method: "POST",
        body: { code: "BCDF-GHJK", entitlement_id: licence.entitlement_id },
        csrf: "csrf-1",
      },
    ]);
  });

  it("deny with the code", async () => {
    const hub = respond(200, {});
    await denyEnrollment("BCDF-GHJK", hub.impl);
    expect(hub.calls).toEqual([
      { url: "/api/enrollments/deny", method: "POST", body: { code: "BCDF-GHJK" }, csrf: "csrf-1" },
    ]);
  });

  it("raise the hub's message for a refusal", async () => {
    const message = "no station is waiting with this code";
    const err = await rejection(lookupEnrollment("BCDF-GHJK", respond(404, { error: message }).impl));
    expect(err).toBeInstanceOf(HubError);
    expect((err as HubError).status).toBe(404);
    expect((err as HubError).message).toBe(message);

    const seat = "every seat of this entitlement is in use";
    const refused = await rejection(
      approveEnrollment("BCDF-GHJK", "e", respond(409, { error: seat }).impl),
    );
    expect((refused as HubError).status).toBe(409);
    expect(enrollmentErrorMessage(refused)).toBe(seat);
  });

  it("refuse an unexpected body", async () => {
    const err = await rejection(lookupEnrollment("BCDF-GHJK", respond(200, { enrollment: {} }).impl));
    expect(String(err)).toMatch(/enrollment lookup returned an unexpected body/);
    const bad = await rejection(
      approveEnrollment("BCDF-GHJK", "e", respond(201, { station, licence: {} }).impl),
    );
    expect(String(bad)).toMatch(/enrollment approval returned an unexpected body/);
  });
});

describe("error text", () => {
  it("turns a lockout into minutes to wait, rounded up", async () => {
    const locked = await rejection(
      lookupEnrollment(
        "BCDF-GHJK",
        respond(429, { error: "too many wrong enrollment codes; try again later" }, {
          "Retry-After": "61",
        }).impl,
      ),
    );
    expect(enrollmentErrorMessage(locked)).toBe("Too many wrong codes. Try again in 2 minutes.");
    expect(enrollmentErrorMessage(new HubError(429, "x", 30))).toBe(
      "Too many wrong codes. Try again in 1 minute.",
    );
    expect(enrollmentErrorMessage(new HubError(429, "locked", null))).toBe("locked");
    expect(enrollmentErrorMessage("nope")).toBe("Something went wrong");
  });
});
