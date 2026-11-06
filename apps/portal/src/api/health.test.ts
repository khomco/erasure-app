import { describe, expect, it } from "vitest";
import { fetchHealth } from "./health";

function respond(status: number, body: string): typeof fetch {
  return (async () =>
    new Response(body, {
      status,
      headers: { "Content-Type": "application/json" },
    })) as typeof fetch;
}

describe("fetchHealth", () => {
  it("returns the report on 200 ok", async () => {
    const r = await fetchHealth(respond(200, '{"status":"ok","database":"up","mail":"up"}'));
    expect(r).toEqual({ status: "ok", database: "up", mail: "up" });
  });

  it("returns the report on 503 degraded", async () => {
    const r = await fetchHealth(
      respond(503, '{"status":"degraded","database":"down","mail":"up"}'),
    );
    expect(r).toEqual({ status: "degraded", database: "down", mail: "up" });
  });

  it("throws on a non-JSON body", async () => {
    await expect(fetchHealth(respond(200, "<!doctype html>"))).rejects.toThrow(/non-JSON/);
  });

  it("throws on JSON that is not a health report", async () => {
    await expect(fetchHealth(respond(404, '{"error":"not found"}'))).rejects.toThrow(
      /unexpected body/,
    );
  });

  it("throws on a network error", async () => {
    const failing = (async () => {
      throw new TypeError("network down");
    }) as typeof fetch;
    await expect(fetchHealth(failing)).rejects.toThrow(/network down/);
  });

  it("requests /api/health", async () => {
    let url = "";
    const spy = (async (input: RequestInfo | URL) => {
      url = String(input);
      return new Response('{"status":"ok","database":"up","mail":"up"}', { status: 200 });
    }) as typeof fetch;
    await fetchHealth(spy);
    expect(url).toBe("/api/health");
  });
});
