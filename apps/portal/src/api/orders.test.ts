import { beforeEach, describe, expect, it } from "vitest";
import { HubError, resetCsrfCache } from "./auth";
import {
  abandonFakeCheckout,
  fetchCatalog,
  fetchFakeCheckout,
  fetchOrder,
  fetchOrders,
  isCatalogOffering,
  isHttpUrl,
  isOrder,
  openBillingPortal,
  payFakeCheckout,
  startCheckout,
  type CatalogOffering,
  type Order,
} from "./orders";

const offering: CatalogOffering = {
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
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

const order: Order = {
  id: "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  offering_id: offering.id,
  offering_name: offering.name,
  price_minor: 120000,
  currency: "USD",
  interval: "annual",
  grants: offering.grants,
  status: "pending",
  provider: "fake",
  created_at: "2026-11-08T10:00:00Z",
  updated_at: "2026-11-08T10:00:00Z",
  paid_at: null,
  expired_at: null,
};

interface Call {
  url: string;
  method: string;
  body: unknown;
  csrf: string | null;
}

/** A fake hub, as in `adminOfferings.test.ts`. */
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
    expect(isCatalogOffering(offering)).toBe(true);
    expect(isOrder(order)).toBe(true);
    expect(isOrder({ ...order, status: "paid", paid_at: "2026-11-08T10:01:00Z" })).toBe(true);
  });

  it("reject unknown statuses, providers, intervals and float prices", () => {
    expect(isOrder({ ...order, status: "refunded" })).toBe(false);
    expect(isOrder({ ...order, provider: "paypal" })).toBe(false);
    expect(isOrder({ ...order, interval: "monthly" })).toBe(false);
    expect(isOrder({ ...order, price_minor: 12.5 })).toBe(false);
    expect(isOrder({ ...order, paid_at: undefined })).toBe(false);
    expect(isCatalogOffering({ ...offering, grants: { ...offering.grants, seats: "1" } })).toBe(
      false,
    );
    expect(isCatalogOffering({ ...offering, price_minor: "120000" })).toBe(false);
  });

  it("only follow http(s) URLs", () => {
    expect(isHttpUrl("http://127.0.0.1:7900/checkout/fake?session=fake_cs_1")).toBe(true);
    expect(isHttpUrl("https://checkout.stripe.com/c/pay/cs_test_1")).toBe(true);
    for (const bad of ["javascript:alert(1)", "data:text/html,hi", "/checkout/fake", "", "ftp://x"]) {
      expect(isHttpUrl(bad), bad).toBe(false);
    }
  });
});

describe("calls", () => {
  it("fetches the catalog with a GET", async () => {
    const f = respond(200, { offerings: [offering] });
    expect(await fetchCatalog(f.impl)).toEqual([offering]);
    expect(f.calls).toEqual([{ url: "/api/offerings", method: "GET", body: undefined, csrf: null }]);
  });

  it("starts a checkout with the offering id and CSRF", async () => {
    const started = { order, checkout_url: "http://127.0.0.1:7900/checkout/fake?session=fake_cs_1" };
    const f = respond(201, started);
    expect(await startCheckout(offering.id, f.impl)).toEqual(started);
    expect(f.calls).toEqual([
      { url: "/api/checkout", method: "POST", body: { offering_id: offering.id }, csrf: "csrf-1" },
    ]);
    const refused = respond(403, { error: "verify your email address before buying" });
    const err = await rejection(startCheckout(offering.id, refused.impl));
    expect(err).toBeInstanceOf(HubError);
    expect((err as HubError).message).toBe("verify your email address before buying");
  });

  it("lists and fetches orders", async () => {
    const list = respond(200, { orders: [order] });
    expect(await fetchOrders(list.impl)).toEqual([order]);
    expect(list.calls).toEqual([{ url: "/api/orders", method: "GET", body: undefined, csrf: null }]);
    const one = respond(200, { order });
    expect(await fetchOrder("a/b", one.impl)).toEqual(order);
    expect(one.calls).toEqual([{ url: "/api/orders/a%2Fb", method: "GET", body: undefined, csrf: null }]);
    const bad = respond(200, { orders: [{ ...order, status: "refunded" }] });
    expect(await rejection(fetchOrders(bad.impl))).toBeInstanceOf(Error);
  });

  it("drives the test checkout page", async () => {
    const read = respond(200, { order });
    expect(await fetchFakeCheckout("fake_cs_1", read.impl)).toEqual(order);
    expect(read.calls).toEqual([
      { url: "/api/payments/fake/sessions/fake_cs_1", method: "GET", body: undefined, csrf: null },
    ]);
    const paidOrder = { ...order, status: "paid" as const, paid_at: "2026-11-08T10:01:00Z" };
    const pay = respond(200, { order: paidOrder });
    expect(await payFakeCheckout("fake_cs_1", pay.impl)).toEqual(paidOrder);
    expect(pay.calls).toEqual([
      { url: "/api/payments/fake/sessions/fake_cs_1/pay", method: "POST", body: {}, csrf: "csrf-1" },
    ]);
    const expiredOrder = { ...order, status: "expired" as const, expired_at: "2026-11-08T10:01:00Z" };
    const abandon = respond(200, { order: expiredOrder });
    expect(await abandonFakeCheckout("fake_cs_1", abandon.impl)).toEqual(expiredOrder);
    expect(abandon.calls).toEqual([
      {
        url: "/api/payments/fake/sessions/fake_cs_1/abandon",
        method: "POST",
        body: {},
        csrf: "csrf-1",
      },
    ]);
    const ended = respond(409, { error: "this checkout has already ended" });
    const err = await rejection(payFakeCheckout("fake_cs_1", ended.impl));
    expect((err as HubError).status).toBe(409);
  });

  it("opens billing with CSRF", async () => {
    const f = respond(200, { url: "http://127.0.0.1:7900/billing/fake" });
    expect(await openBillingPortal(f.impl)).toBe("http://127.0.0.1:7900/billing/fake");
    expect(f.calls).toEqual([
      { url: "/api/billing/portal", method: "POST", body: {}, csrf: "csrf-1" },
    ]);
  });
});
