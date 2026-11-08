import { describe, expect, it } from "vitest";
import { BANNER_HEADING, bannerLines, fetchDeployment } from "./deployment";

function respond(status: number, body: string): typeof fetch {
  return (async () =>
    new Response(body, {
      status,
      headers: { "Content-Type": "application/json" },
    })) as typeof fetch;
}

const PAYMENTS = "Payments are simulated by the test payment provider; no money moves.";
const ROOT = "The vendor root is held in a file, so licences signed here are for testing only.";

// The I/O matrix of commerce-end-to-end story 2, as the hub sends it.
const DEFAULT_LOCAL =
  '{"development":true,"payment_provider":"fake","vendor_root":"none","reasons":["fake_payment_provider"]}';
const DEMO =
  '{"development":true,"payment_provider":"fake","vendor_root":"file","reasons":["fake_payment_provider","file_vendor_root"]}';
const STRIPE_FILE_ROOT =
  '{"development":true,"payment_provider":"stripe","vendor_root":"file","reasons":["file_vendor_root"]}';
const STRIPE_NO_ROOT =
  '{"development":false,"payment_provider":"stripe","vendor_root":"none","reasons":[]}';

describe("fetchDeployment and bannerLines", () => {
  it("default local hub: payments simulated", async () => {
    const r = await fetchDeployment(respond(200, DEFAULT_LOCAL));
    expect(r.development).toBe(true);
    expect(bannerLines(r)).toEqual([PAYMENTS]);
  });

  it("demo hub: both reasons, provider first", async () => {
    const r = await fetchDeployment(respond(200, DEMO));
    expect(bannerLines(r)).toEqual([PAYMENTS, ROOT]);
  });

  it("Stripe hub with a file-held root: the root reason only", async () => {
    const r = await fetchDeployment(respond(200, STRIPE_FILE_ROOT));
    expect(r.development).toBe(true);
    expect(bannerLines(r)).toEqual([ROOT]);
  });

  it("Stripe hub without a root: not development, no lines", async () => {
    const r = await fetchDeployment(respond(200, STRIPE_NO_ROOT));
    expect(r.development).toBe(false);
    expect(bannerLines(r)).toEqual([]);
  });

  it("ignores reasons it does not know, including prototype names", () => {
    expect(
      bannerLines({
        development: true,
        payment_provider: "fake",
        vendor_root: "none",
        reasons: ["hsm_offline", "constructor", "fake_payment_provider"],
      }),
    ).toEqual([PAYMENTS]);
  });

  it("gives no lines when development is false, whatever the reasons", () => {
    expect(
      bannerLines({
        development: false,
        payment_provider: "fake",
        vendor_root: "file",
        reasons: ["fake_payment_provider"],
      }),
    ).toEqual([]);
  });

  it("accepts a vendor root custody it does not know", async () => {
    const r = await fetchDeployment(
      respond(
        200,
        '{"development":true,"payment_provider":"fake","vendor_root":"hsm","reasons":["fake_payment_provider"]}',
      ),
    );
    expect(r.vendor_root).toBe("hsm");
    expect(bannerLines(r)).toEqual([PAYMENTS]);
  });

  it("throws on a non-JSON body", async () => {
    await expect(fetchDeployment(respond(200, "<!doctype html>"))).rejects.toThrow(/non-JSON/);
  });

  it("throws on JSON that is not a deployment report", async () => {
    await expect(fetchDeployment(respond(404, '{"error":"not found"}'))).rejects.toThrow(
      /unexpected body/,
    );
    await expect(
      fetchDeployment(
        respond(200, '{"development":"yes","payment_provider":"fake","vendor_root":"none","reasons":[]}'),
      ),
    ).rejects.toThrow(/unexpected body/);
  });

  it("throws on a network error", async () => {
    const failing = (async () => {
      throw new TypeError("network down");
    }) as typeof fetch;
    await expect(fetchDeployment(failing)).rejects.toThrow(/network down/);
  });

  it("requests /api/deployment", async () => {
    let url = "";
    const spy = (async (input: RequestInfo | URL) => {
      url = String(input);
      return new Response(STRIPE_NO_ROOT, { status: 200 });
    }) as typeof fetch;
    await fetchDeployment(spy);
    expect(url).toBe("/api/deployment");
  });

  it("has a fixed heading", () => {
    expect(BANNER_HEADING).toBe("Development hub — not for production use.");
  });
});
