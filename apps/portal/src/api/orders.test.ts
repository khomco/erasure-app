import { beforeEach, describe, expect, it } from "vitest";
import { HubError, resetCsrfCache } from "./auth";
import {
  abandonFakeCheckout,
  cancelFakeEntitlement,
  fetchCatalog,
  fetchFakeCheckout,
  fetchOrder,
  fetchOrders,
  fetchSeats,
  isCatalogOffering,
  isEntitlement,
  isHttpUrl,
  isOrder,
  isRenewalNotice,
  isSeats,
  openBillingPortal,
  payFakeCheckout,
  renewFakeEntitlement,
  startCheckout,
  startTrial,
  type CatalogOffering,
  type Entitlement,
  type Order,
  type RenewalNotice,
} from "./orders";

const notice: RenewalNotice = {
  headline: "Renews automatically every year at 1,200.00 USD until you cancel.",
  detail:
    "Your first year runs to 8 November 2027. Cancel any time from Billing in your account: " +
    "your current year still runs to its end, and you will not be charged again. " +
    "We email you before each renewal.",
};

const offering: CatalogOffering = {
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  name: "Station — annual, 1 seat",
  description: "One station.",
  price_minor: 120000,
  currency: "USD",
  interval: "annual",
  trial: false,
  grants: {
    scope: "machine",
    seats: 1,
    quota_erasures: null,
    features: ["pdf_certificates"],
    term_months: 12,
  },
  renewal_notice: notice,
};

const trialOffering: CatalogOffering = {
  ...offering,
  id: "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
  name: "Free trial",
  price_minor: 0,
  interval: "one_off",
  trial: true,
  grants: { ...offering.grants, scope: "site", quota_erasures: 25, term_months: 1 },
  renewal_notice: null,
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
  refunded_at: null,
};

const entitlement: Entitlement = {
  id: "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
  order_id: order.id,
  offering_name: offering.name,
  interval: "annual",
  grants: offering.grants,
  term_start: "2026-11-08T10:01:00Z",
  term_end: "2027-11-08T10:01:00Z",
  status: "active",
  renews: true,
  current: true,
  cancelled_at: null,
  refunded_at: null,
  credits: null,
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
    expect(
      isOrder({
        ...order,
        status: "refunded",
        paid_at: "2026-11-08T10:01:00Z",
        refunded_at: "2026-11-09T10:01:00Z",
      }),
    ).toBe(true);
    expect(isCatalogOffering({ ...offering, interval: "one_off", renewal_notice: null })).toBe(true);
    expect(isRenewalNotice(notice)).toBe(true);
    expect(isEntitlement(entitlement)).toBe(true);
    expect(
      isEntitlement({
        ...entitlement,
        status: "cancelled",
        renews: false,
        cancelled_at: "2026-12-01T00:00:00Z",
      }),
    ).toBe(true);
    expect(isSeats({ entitlements: [entitlement], available_seats: 1 })).toBe(true);
    expect(isSeats({ entitlements: [], available_seats: 0 })).toBe(true);
  });

  it("reject malformed notices, entitlements and seat counts", () => {
    expect(isCatalogOffering({ ...offering, renewal_notice: undefined })).toBe(false);
    expect(isCatalogOffering({ ...offering, renewal_notice: "Renews" })).toBe(false);
    expect(isRenewalNotice({ headline: notice.headline })).toBe(false);
    expect(isEntitlement({ ...entitlement, status: "expired" })).toBe(false);
    expect(isEntitlement({ ...entitlement, renews: "yes" })).toBe(false);
    expect(isEntitlement({ ...entitlement, current: undefined })).toBe(false);
    expect(isEntitlement({ ...entitlement, refunded_at: undefined })).toBe(false);
    // The credit pool: null when unlimited, every count an integer.
    const pool = { purchased: 5000, granted: 50, returned: 0, available: 4950, target_reserve: 50 };
    expect(isEntitlement({ ...entitlement, credits: pool })).toBe(true);
    expect(isEntitlement({ ...entitlement, credits: undefined })).toBe(false);
    expect(isEntitlement({ ...entitlement, credits: { ...pool, available: "4950" } })).toBe(false);
    expect(isEntitlement({ ...entitlement, credits: { ...pool, target_reserve: undefined } })).toBe(
      false,
    );
    expect(isSeats({ entitlements: [entitlement], available_seats: 1.5 })).toBe(false);
    expect(isSeats({ entitlements: [{ ...entitlement, id: 1 }], available_seats: 1 })).toBe(false);
    expect(isSeats({ available_seats: 1 })).toBe(false);
  });

  it("reject unknown statuses, providers, intervals and float prices", () => {
    expect(isOrder({ ...order, status: "cancelled" })).toBe(false);
    expect(isOrder({ ...order, refunded_at: undefined })).toBe(false);
    expect(isOrder({ ...order, provider: "paypal" })).toBe(false);
    expect(isOrder({ ...order, interval: "monthly" })).toBe(false);
    expect(isOrder({ ...order, price_minor: 12.5 })).toBe(false);
    expect(isOrder({ ...order, paid_at: undefined })).toBe(false);
    expect(isCatalogOffering({ ...offering, grants: { ...offering.grants, seats: "1" } })).toBe(
      false,
    );
    expect(isCatalogOffering({ ...offering, price_minor: "120000" })).toBe(false);
    expect(isCatalogOffering({ ...offering, trial: undefined })).toBe(false);
    expect(isCatalogOffering({ ...offering, trial: "no" })).toBe(false);
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

  it("takes a free trial with the offering id and CSRF", async () => {
    expect(isCatalogOffering(trialOffering)).toBe(true);
    const trialOrder: Order = {
      ...order,
      offering_id: trialOffering.id,
      offering_name: trialOffering.name,
      price_minor: 0,
      interval: "one_off",
      grants: trialOffering.grants,
      status: "paid",
      provider: "trial",
      paid_at: "2026-11-10T10:00:00Z",
    };
    const trialEntitlement: Entitlement = {
      ...entitlement,
      order_id: trialOrder.id,
      offering_name: trialOrder.offering_name,
      interval: "one_off",
      grants: trialOrder.grants,
      renews: false,
    };
    const taken = { order: trialOrder, entitlement: trialEntitlement };
    const f = respond(201, taken);
    expect(await startTrial(trialOffering.id, f.impl)).toEqual(taken);
    expect(f.calls).toEqual([
      {
        url: "/api/trials",
        method: "POST",
        body: { offering_id: trialOffering.id },
        csrf: "csrf-1",
      },
    ]);
    const again = respond(409, { error: "Your organization has already taken this free trial." });
    const err = await rejection(startTrial(trialOffering.id, again.impl));
    expect(err).toBeInstanceOf(HubError);
    expect((err as HubError).status).toBe(409);
    expect((err as HubError).message).toBe("Your organization has already taken this free trial.");
    const noEntitlement = respond(201, { order: trialOrder });
    expect(await rejection(startTrial(trialOffering.id, noEntitlement.impl))).toBeInstanceOf(Error);
  });

  it("lists and fetches orders", async () => {
    const list = respond(200, { orders: [order] });
    expect(await fetchOrders(list.impl)).toEqual([order]);
    expect(list.calls).toEqual([{ url: "/api/orders", method: "GET", body: undefined, csrf: null }]);
    const one = respond(200, { order });
    expect(await fetchOrder("a/b", one.impl)).toEqual(order);
    expect(one.calls).toEqual([{ url: "/api/orders/a%2Fb", method: "GET", body: undefined, csrf: null }]);
    const bad = respond(200, { orders: [{ ...order, status: "cancelled" }] });
    expect(await rejection(fetchOrders(bad.impl))).toBeInstanceOf(Error);
  });

  it("drives the test checkout page", async () => {
    const read = respond(200, { order, renewal_notice: notice });
    expect(await fetchFakeCheckout("fake_cs_1", read.impl)).toEqual({
      order,
      renewal_notice: notice,
    });
    const oneOff = respond(200, { order: { ...order, interval: "one_off" }, renewal_notice: null });
    expect((await fetchFakeCheckout("fake_cs_2", oneOff.impl)).renewal_notice).toBeNull();
    const noNotice = respond(200, { order });
    expect(await rejection(fetchFakeCheckout("fake_cs_3", noNotice.impl))).toBeInstanceOf(Error);
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

  it("reads licence seats with a GET", async () => {
    const seats = { entitlements: [entitlement], available_seats: 1 };
    const f = respond(200, seats);
    expect(await fetchSeats(f.impl)).toEqual(seats);
    expect(f.calls).toEqual([
      { url: "/api/entitlements", method: "GET", body: undefined, csrf: null },
    ]);
    const bad = respond(200, { entitlements: [entitlement] });
    expect(await rejection(fetchSeats(bad.impl))).toBeInstanceOf(Error);
  });

  it("drives the test billing page's renew and cancel", async () => {
    const renewed = { ...entitlement, term_end: "2028-11-08T10:01:00Z" };
    const renew = respond(200, { entitlement: renewed });
    expect(await renewFakeEntitlement("a/b", renew.impl)).toEqual(renewed);
    expect(renew.calls).toEqual([
      {
        url: "/api/payments/fake/entitlements/a%2Fb/renew",
        method: "POST",
        body: {},
        csrf: "csrf-1",
      },
    ]);
    const stopped = { ...entitlement, renews: false };
    const cancel = respond(200, { entitlement: stopped });
    expect(await cancelFakeEntitlement(entitlement.id, cancel.impl)).toEqual(stopped);
    expect(cancel.calls).toEqual([
      {
        url: `/api/payments/fake/entitlements/${entitlement.id}/cancel`,
        method: "POST",
        body: {},
        csrf: "csrf-1",
      },
    ]);
    const refused = respond(409, { error: "this subscription is not renewing" });
    const err = await rejection(renewFakeEntitlement(entitlement.id, refused.impl));
    expect((err as HubError).status).toBe(409);
    expect((err as HubError).message).toBe("this subscription is not renewing");
  });
});
