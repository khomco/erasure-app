// Mirrors the customer catalog, checkout, order, billing and Fake checkout
// routes in crates/wipe-hub/src/order_routes.rs and the serde shapes
// `wipe_hub::orders::{CatalogOffering, Order}` (crates/wipe-hub/src/orders.rs).
// Change both together.
//
// An order becomes `paid` only when the hub applies a verified provider event:
// nothing here, and no redirect back from a provider, marks one paid.

import { ensureOk, isObject, json, request, type Fetch } from "./auth";
import { INTERVALS, isGrants, type Grants, type Interval } from "./adminOfferings";

/** A published offering, as a customer sees it. */
export interface CatalogOffering {
  id: string;
  name: string;
  description: string;
  /** Integer minor units. */
  price_minor: number;
  currency: string;
  interval: Interval;
  grants: Grants;
}

export const ORDER_STATUSES = ["pending", "paid", "expired"] as const;
export type OrderStatus = (typeof ORDER_STATUSES)[number];

export const PROVIDERS = ["fake", "stripe"] as const;
export type Provider = (typeof PROVIDERS)[number];

/**
 * An order: the offering's name and terms as they were when it was created.
 * Timestamps are RFC 3339 UTC; absent values are `null`.
 */
export interface Order {
  id: string;
  offering_id: string;
  offering_name: string;
  price_minor: number;
  currency: string;
  interval: Interval;
  grants: Grants;
  status: OrderStatus;
  provider: Provider;
  created_at: string;
  updated_at: string;
  paid_at: string | null;
  expired_at: string | null;
}

export interface CheckoutStarted {
  order: Order;
  /** Where to send the buyer: the provider's hosted checkout page. */
  checkout_url: string;
}

const isInt = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v);
const isNullableString = (v: unknown): v is string | null => v === null || typeof v === "string";
const oneOf = (values: readonly string[], v: unknown) =>
  typeof v === "string" && values.includes(v);

export function isCatalogOffering(v: unknown): v is CatalogOffering {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.name === "string" &&
    typeof v.description === "string" &&
    isInt(v.price_minor) &&
    typeof v.currency === "string" &&
    oneOf(INTERVALS, v.interval) &&
    isGrants(v.grants)
  );
}

export function isOrder(v: unknown): v is Order {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.offering_id === "string" &&
    typeof v.offering_name === "string" &&
    isInt(v.price_minor) &&
    typeof v.currency === "string" &&
    oneOf(INTERVALS, v.interval) &&
    isGrants(v.grants) &&
    oneOf(ORDER_STATUSES, v.status) &&
    oneOf(PROVIDERS, v.provider) &&
    typeof v.created_at === "string" &&
    typeof v.updated_at === "string" &&
    isNullableString(v.paid_at) &&
    isNullableString(v.expired_at)
  );
}

const isCatalog = (v: unknown): v is { offerings: CatalogOffering[] } =>
  isObject(v) && Array.isArray(v.offerings) && v.offerings.every(isCatalogOffering);

const isOrderList = (v: unknown): v is { orders: Order[] } =>
  isObject(v) && Array.isArray(v.orders) && v.orders.every(isOrder);

const isOrderBody = (v: unknown): v is { order: Order } => isObject(v) && isOrder(v.order);

const isCheckoutStarted = (v: unknown): v is CheckoutStarted =>
  isObject(v) && isOrder(v.order) && typeof v.checkout_url === "string";

const isUrlBody = (v: unknown): v is { url: string } => isObject(v) && typeof v.url === "string";

/**
 * Whether `url` is an absolute http(s) URL: the only kind the portal will
 * send the browser to from a hub answer (never `javascript:` or `data:`).
 */
export function isHttpUrl(url: string): boolean {
  try {
    const parsed = new URL(url);
    return parsed.protocol === "http:" || parsed.protocol === "https:";
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------
// Calls

/** `GET /api/offerings` (signed in): published offerings, by name. */
export async function fetchCatalog(fetchImpl: Fetch = fetch): Promise<CatalogOffering[]> {
  const res = await ensureOk(await request(fetchImpl, "GET", "/api/offerings"));
  return (await json(res, isCatalog, "catalog")).offerings;
}

/**
 * `POST /api/checkout` (a verified owner, 201): a pending order and the
 * provider's checkout URL.
 */
export async function startCheckout(
  offeringId: string,
  fetchImpl: Fetch = fetch,
): Promise<CheckoutStarted> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/checkout", { offering_id: offeringId }),
  );
  return json(res, isCheckoutStarted, "checkout");
}

/** `GET /api/orders` (any role): the organization's orders, newest first. */
export async function fetchOrders(fetchImpl: Fetch = fetch): Promise<Order[]> {
  const res = await ensureOk(await request(fetchImpl, "GET", "/api/orders"));
  return (await json(res, isOrderList, "order list")).orders;
}

/** `GET /api/orders/{id}` (any role). */
export async function fetchOrder(id: string, fetchImpl: Fetch = fetch): Promise<Order> {
  const res = await ensureOk(
    await request(fetchImpl, "GET", `/api/orders/${encodeURIComponent(id)}`),
  );
  return (await json(res, isOrderBody, "order")).order;
}

function fakeSessionPath(session: string, action?: string): string {
  const base = `/api/payments/fake/sessions/${encodeURIComponent(session)}`;
  return action ? `${base}/${action}` : base;
}

/** `GET /api/payments/fake/sessions/{session}` (owner): the checkout's order. */
export async function fetchFakeCheckout(session: string, fetchImpl: Fetch = fetch): Promise<Order> {
  const res = await ensureOk(await request(fetchImpl, "GET", fakeSessionPath(session)));
  return (await json(res, isOrderBody, "test checkout")).order;
}

/** `POST …/{session}/pay` (owner): the hub verifies a signed test event; the paid order. */
export async function payFakeCheckout(session: string, fetchImpl: Fetch = fetch): Promise<Order> {
  const res = await ensureOk(await request(fetchImpl, "POST", fakeSessionPath(session, "pay"), {}));
  return (await json(res, isOrderBody, "test payment")).order;
}

/** `POST …/{session}/abandon` (owner): the expired order. */
export async function abandonFakeCheckout(
  session: string,
  fetchImpl: Fetch = fetch,
): Promise<Order> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", fakeSessionPath(session, "abandon"), {}),
  );
  return (await json(res, isOrderBody, "test abandon")).order;
}

/** `POST /api/billing/portal` (owner): the provider's billing page URL. */
export async function openBillingPortal(fetchImpl: Fetch = fetch): Promise<string> {
  const res = await ensureOk(await request(fetchImpl, "POST", "/api/billing/portal", {}));
  return (await json(res, isUrlBody, "billing")).url;
}
