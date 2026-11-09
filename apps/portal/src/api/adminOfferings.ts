// Mirrors the offering admin routes in crates/wipe-hub/src/admin_routes.rs
// and `wipe_hub::offering_admin::{Offering, Grants, OfferingRequest, …}`
// (crates/wipe-hub/src/offering_admin.rs). Change both together.
//
// Vendor admins only; everyone else gets a 403 `HubError`. Every call here
// is audited on the hub (the list, a view, a change), so callers run them on
// an explicit action, never per keystroke, on a timer, or to refresh a page
// after a change: a change's response is the new state.

import { ensureOk, isObject, json, request, type Fetch } from "./auth";

/** Billing intervals (`wipe_hub::offering_admin::INTERVALS`). */
export const INTERVALS = ["one_off", "annual"] as const;
export type Interval = (typeof INTERVALS)[number];

/** Licence scopes (`wipe_hub::offering_admin::SCOPES`). */
export const SCOPES = ["machine", "site"] as const;
export type Scope = (typeof SCOPES)[number];

/**
 * Entitlements an offering may grant: `wipe_license::Feature`'s serde names
 * (`wipe_hub::offering_admin::FEATURES`), in the enum's order.
 */
export const FEATURES = [
  "enterprise_mode",
  "hub_sync",
  "erp_integration",
  "pdf_certificates",
  "cert_anchoring",
  "operator_sso",
] as const;
export type Feature = (typeof FEATURES)[number];

export const FEATURE_LABELS: Record<Feature, string> = {
  enterprise_mode: "Enterprise mode",
  hub_sync: "Hub sync",
  erp_integration: "ERP integration",
  pdf_certificates: "PDF certificates",
  cert_anchoring: "Certificate anchoring",
  operator_sso: "Operator SSO",
};

export const OFFERING_STATUSES = ["draft", "published", "retired"] as const;
export type OfferingStatus = (typeof OFFERING_STATUSES)[number];

/** The only term an annual offering may have, in months. */
export const ANNUAL_TERM_MONTHS = 12;

/** What an offering grants its buyer. */
export interface Grants {
  scope: Scope;
  seats: number;
  /** `null` is unlimited. */
  quota_erasures: number | null;
  /** Sorted and de-duplicated by the hub. */
  features: Feature[];
  term_months: number;
}

/** A create or edit body: every field, always. `price_minor` is integer minor units. */
export interface OfferingInput {
  name: string;
  description: string;
  price_minor: number;
  currency: string;
  interval: Interval;
  grants: Grants;
}

/** An offering. Timestamps are RFC 3339 UTC; absent values are `null`. */
export interface Offering extends OfferingInput {
  id: string;
  status: OfferingStatus;
  created_at: string;
  updated_at: string;
  published_at: string | null;
  retired_at: string | null;
}

export interface OfferingList {
  /** Drafts, then published, then retired; each by name. */
  offerings: Offering[];
}

const isNullableString = (v: unknown): v is string | null => v === null || typeof v === "string";
const isInt = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v);
const oneOf = (values: readonly string[], v: unknown) =>
  typeof v === "string" && values.includes(v);

export function isGrants(v: unknown): v is Grants {
  return (
    isObject(v) &&
    oneOf(SCOPES, v.scope) &&
    isInt(v.seats) &&
    (v.quota_erasures === null || isInt(v.quota_erasures)) &&
    Array.isArray(v.features) &&
    v.features.every((f) => oneOf(FEATURES, f)) &&
    isInt(v.term_months)
  );
}

export function isOffering(v: unknown): v is Offering {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.name === "string" &&
    typeof v.description === "string" &&
    isInt(v.price_minor) &&
    typeof v.currency === "string" &&
    oneOf(INTERVALS, v.interval) &&
    oneOf(OFFERING_STATUSES, v.status) &&
    isGrants(v.grants) &&
    typeof v.created_at === "string" &&
    typeof v.updated_at === "string" &&
    isNullableString(v.published_at) &&
    isNullableString(v.retired_at)
  );
}

function isOfferingList(v: unknown): v is OfferingList {
  return isObject(v) && Array.isArray(v.offerings) && v.offerings.every(isOffering);
}

const isOfferingBody = (v: unknown): v is { offering: Offering } =>
  isObject(v) && isOffering(v.offering);

// ---------------------------------------------------------------------------
// Money, without floats

/**
 * Digits after the decimal point for `currency` (USD 2, JPY 0), from Intl;
 * `null` for a code Intl refuses. Any well-formed code Intl does not know
 * gets its default of 2.
 */
export function fractionDigits(currency: string): number | null {
  try {
    return new Intl.NumberFormat("en", { style: "currency", currency }).resolvedOptions()
      .maximumFractionDigits ?? null;
  } catch {
    return null;
  }
}

/**
 * The amount typed as `text` (`"12"`, `"12.5"`, `"1200.00"`) in integer minor
 * units of `currency`, by string arithmetic, never a float; `null` when it is
 * not a plain non-negative decimal, has more decimals than the currency
 * (`"12.345"` USD, `"12.5"` JPY), or is beyond `Number.MAX_SAFE_INTEGER`.
 */
export function toMinorUnits(text: string, currency: string): number | null {
  const digits = fractionDigits(currency);
  if (digits === null) return null;
  const match = /^(\d+)(?:\.(\d+))?$/.exec(text.trim());
  if (!match) return null;
  const [, whole, frac = ""] = match;
  if (frac.length > digits) return null;
  const minor = (whole + frac.padEnd(digits, "0")).replace(/^0+(?=\d)/, "");
  const n = Number(minor);
  return Number.isSafeInteger(n) ? n : null;
}

/** `minor` as a plain decimal for an input box: `120000` USD is `"1200.00"`. */
export function minorToDecimal(minor: number, currency: string): string {
  const digits = fractionDigits(currency) ?? 2;
  const s = String(Math.abs(Math.trunc(minor))).padStart(digits + 1, "0");
  const sign = minor < 0 ? "-" : "";
  if (digits === 0) return sign + s;
  return `${sign}${s.slice(0, -digits)}.${s.slice(-digits)}`;
}

/** `minor` for display: `120000` USD is `"1,200.00 USD"`. */
export function formatPrice(minor: number, currency: string): string {
  const [whole, frac] = minorToDecimal(minor, currency).split(".");
  const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${frac === undefined ? grouped : `${grouped}.${frac}`} ${currency}`;
}

// ---------------------------------------------------------------------------
// Calls

function offeringPath(id: string, action?: string): string {
  const base = `/api/admin/offerings/${encodeURIComponent(id)}`;
  return action ? `${base}/${action}` : base;
}

/** `GET /api/admin/offerings` (vendor admin): every offering. Audited. */
export async function listOfferings(fetchImpl: Fetch = fetch): Promise<OfferingList> {
  const res = await ensureOk(await request(fetchImpl, "GET", "/api/admin/offerings"));
  return json(res, isOfferingList, "offering list");
}

/** `GET /api/admin/offerings/{id}`: one offering. Audited. */
export async function fetchOffering(id: string, fetchImpl: Fetch = fetch): Promise<Offering> {
  const res = await ensureOk(await request(fetchImpl, "GET", offeringPath(id)));
  return (await json(res, isOfferingBody, "offering")).offering;
}

/** `POST /api/admin/offerings` (201): the new draft. Audited. */
export async function createOffering(
  input: OfferingInput,
  fetchImpl: Fetch = fetch,
): Promise<Offering> {
  const res = await ensureOk(await request(fetchImpl, "POST", "/api/admin/offerings", input));
  return (await json(res, isOfferingBody, "new offering")).offering;
}

/**
 * `POST /api/admin/offerings/{id}` with every field: the updated offering.
 * A published offering's terms are frozen (409). Audited.
 */
export async function updateOffering(
  id: string,
  input: OfferingInput,
  fetchImpl: Fetch = fetch,
): Promise<Offering> {
  const res = await ensureOk(await request(fetchImpl, "POST", offeringPath(id), input));
  return (await json(res, isOfferingBody, "offering update")).offering;
}

/** `POST /api/admin/offerings/{id}/publish`: draft to published. Audited. */
export async function publishOffering(id: string, fetchImpl: Fetch = fetch): Promise<Offering> {
  const res = await ensureOk(await request(fetchImpl, "POST", offeringPath(id, "publish"), {}));
  return (await json(res, isOfferingBody, "offering publish")).offering;
}

/** `POST /api/admin/offerings/{id}/retire`: published to retired, for good. Audited. */
export async function retireOffering(id: string, fetchImpl: Fetch = fetch): Promise<Offering> {
  const res = await ensureOk(await request(fetchImpl, "POST", offeringPath(id, "retire"), {}));
  return (await json(res, isOfferingBody, "offering retire")).offering;
}
