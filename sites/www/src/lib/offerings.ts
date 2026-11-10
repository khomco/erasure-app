/**
 * The hub's published offerings, read at build time for `/pricing/`
 * (offerings-and-payments story 5, CAP-6, ADR-0006 §1).
 *
 * Only a build given `PRICING_HUB_ORIGIN` reads them. Such a build fetches
 * `<origin>/api/offerings` once, checks the body strictly, and throws on any
 * failure, so `astro build` fails rather than publishing stale or malformed
 * prices. Nothing here runs in the browser.
 *
 * Mirrors `wipe_hub::orders::CatalogOffering` (crates/wipe-hub/src/orders.rs)
 * and the portal's `CatalogOffering` (apps/portal/src/api/orders.ts); change
 * them together.
 */

export interface Grants {
  scope: "machine" | "site";
  seats: number;
  /** `null` is unlimited. */
  quota_erasures: number | null;
  features: string[];
  term_months: number;
}

/** The hub's owner-approved auto-renewal notice; the page shows `headline` only. */
export interface RenewalNotice {
  headline: string;
}

export interface CatalogOffering {
  id: string;
  name: string;
  description: string;
  /** Integer minor units. */
  price_minor: number;
  currency: string;
  interval: "one_off" | "annual";
  /**
   * A free trial (ADR-0013 §2): price 0, one-off, a counted erasure quota.
   * Shown as "Free trial" with a "Start free trial" link to the portal.
   */
  trial: boolean;
  grants: Grants;
  /** Non-null exactly when `interval` is `annual`. */
  renewal_notice: RenewalNotice | null;
}

/** How long the build waits for the hub before failing. */
export const DEFAULT_TIMEOUT_MS = 10_000;

const VARIABLE = "PRICING_HUB_ORIGIN";

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const isIntAtLeast = (v: unknown, min: number): v is number =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= min;

/**
 * The offerings in a `GET /api/offerings` body. Throws
 * `offerings[i].field: …` (or `offerings: …`) on the first violation.
 */
export function parseCatalog(json: unknown): CatalogOffering[] {
  if (!isObject(json) || !Array.isArray(json.offerings)) {
    throw new Error("offerings: missing, or not an array");
  }
  return json.offerings.map((o, i) => parseOffering(o, `offerings[${i}]`));
}

function parseOffering(o: unknown, at: string): CatalogOffering {
  const fail = (field: string, why: string): never => {
    throw new Error(`${at}${field ? "." + field : ""}: ${why}`);
  };
  if (!isObject(o)) fail("", "not an object");
  const r = o as Record<string, unknown>;
  for (const f of ["id", "name", "description"] as const) {
    if (typeof r[f] !== "string") fail(f, "not a string");
  }
  if (typeof r.trial !== "boolean") fail("trial", "not a boolean");
  const trial = r.trial as boolean;
  if (trial) {
    if (r.price_minor !== 0) fail("price_minor", "not 0 on a free trial");
  } else if (!isIntAtLeast(r.price_minor, 1)) {
    fail("price_minor", "not a positive safe integer");
  }
  if (typeof r.currency !== "string" || !/^[A-Z]{3}$/.test(r.currency)) {
    fail("currency", `${JSON.stringify(r.currency)} is not a three-letter upper-case code`);
  }
  if (r.interval !== "one_off" && r.interval !== "annual") {
    fail("interval", `${JSON.stringify(r.interval)} is not one_off or annual`);
  }
  const g = r.grants;
  if (!isObject(g)) fail("grants", "not an object");
  const grants = g as Record<string, unknown>;
  if (grants.scope !== "machine" && grants.scope !== "site") {
    fail("grants.scope", `${JSON.stringify(grants.scope)} is not machine or site`);
  }
  if (!isIntAtLeast(grants.seats, 1)) fail("grants.seats", "not an integer of at least 1");
  if (grants.quota_erasures !== null && !isIntAtLeast(grants.quota_erasures, 1)) {
    fail("grants.quota_erasures", "neither null nor an integer of at least 1");
  }
  if (
    !Array.isArray(grants.features) ||
    !grants.features.every((f) => typeof f === "string")
  ) {
    fail("grants.features", "not an array of strings");
  }
  if (!isIntAtLeast(grants.term_months, 1)) {
    fail("grants.term_months", "not an integer of at least 1");
  }
  if (trial && r.interval !== "one_off") fail("interval", "not one_off on a free trial");
  if (trial && grants.quota_erasures === null) {
    fail("grants.quota_erasures", "unlimited on a free trial");
  }
  const n = r.renewal_notice;
  if (n !== null && !(isObject(n) && typeof n.headline === "string")) {
    fail("renewal_notice", "neither null nor an object with a string headline");
  }
  if (r.interval === "annual" && n === null) {
    fail("renewal_notice", "missing on an annual offering");
  }
  if (r.interval === "one_off" && n !== null) {
    fail("renewal_notice", "present on a one-off offering");
  }
  return {
    id: r.id as string,
    name: r.name as string,
    description: r.description as string,
    price_minor: r.price_minor as number,
    currency: r.currency as string,
    interval: r.interval as CatalogOffering["interval"],
    trial,
    grants: {
      scope: grants.scope as Grants["scope"],
      seats: grants.seats as number,
      quota_erasures: grants.quota_erasures as number | null,
      features: [...(grants.features as string[])],
      term_months: grants.term_months as number,
    },
    renewal_notice: n === null ? null : { headline: (n as { headline: string }).headline },
  };
}

/**
 * Fetches and checks `<origin>/api/offerings`. Every failure (network,
 * timeout, non-2xx status, bad JSON or shape) throws
 * `pricing: could not read <url> (PRICING_HUB_ORIGIN): <cause>`.
 */
export async function loadOfferings(
  origin: string,
  {
    fetchImpl = fetch,
    timeoutMs = DEFAULT_TIMEOUT_MS,
  }: { fetchImpl?: typeof fetch; timeoutMs?: number } = {},
): Promise<CatalogOffering[]> {
  const url = `${origin}/api/offerings`;
  const fail = (cause: string): never => {
    throw new Error(`pricing: could not read ${url} (${VARIABLE}): ${cause}`);
  };
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    let res: Response;
    try {
      res = await fetchImpl(url, {
        headers: { accept: "application/json" },
        signal: controller.signal,
      });
    } catch (e) {
      return controller.signal.aborted
        ? fail(`timed out after ${timeoutMs} ms`)
        : fail(describe(e));
    }
    if (!res.ok) fail(`HTTP status ${res.status}`);
    let body: unknown;
    try {
      body = await res.json();
    } catch (e) {
      return controller.signal.aborted
        ? fail(`timed out after ${timeoutMs} ms`)
        : fail(`the body is not JSON (${describe(e)})`);
    }
    try {
      return parseCatalog(body);
    } catch (e) {
      return fail(describe(e));
    }
  } finally {
    clearTimeout(timer);
  }
}

/** An error and its causes (Node's fetch hides ECONNREFUSED in `cause`). */
function describe(e: unknown): string {
  const parts: string[] = [];
  let cur: unknown = e;
  for (let depth = 0; cur !== undefined && cur !== null && depth < 4; depth++) {
    if (cur instanceof Error) {
      const code = (cur as { code?: unknown }).code;
      parts.push(typeof code === "string" && !cur.message.includes(code) ? `${cur.message} (${code})` : cur.message);
      cur = cur.cause;
    } else {
      parts.push(String(cur));
      break;
    }
  }
  return parts.join(": ") || "unknown error";
}

// ---------------------------------------------------------------------------
// Labels

/** Minor-unit digits of `currency` (2 for USD, 0 for JPY), or `null` if unknown. */
function fractionDigits(currency: string): number | null {
  try {
    return (
      new Intl.NumberFormat("en", { style: "currency", currency }).resolvedOptions()
        .maximumFractionDigits ?? null
    );
  } catch {
    return null;
  }
}

/** `minor` as a plain decimal: `120000` USD is `"1200.00"`. */
function minorToDecimal(minor: number, currency: string): string {
  const digits = fractionDigits(currency) ?? 2;
  const s = String(Math.abs(Math.trunc(minor))).padStart(digits + 1, "0");
  const sign = minor < 0 ? "-" : "";
  if (digits === 0) return sign + s;
  return `${sign}${s.slice(0, -digits)}.${s.slice(-digits)}`;
}

/**
 * `minor` for display: `120000` USD is `"1,200.00 USD"`. A copy of the
 * portal's `formatPrice` (apps/portal/src/api/adminOfferings.ts), so
 * `/pricing/` and the portal's `/buy` show the same string; change both
 * together.
 */
export function formatPrice(minor: number, currency: string): string {
  const [whole, frac] = minorToDecimal(minor, currency).split(".");
  const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${frac === undefined ? grouped : `${grouped}.${frac}`} ${currency}`;
}

/** Shown in place of a price for a free trial; the portal's `/buy` says the same. */
export const FREE_TRIAL_LABEL = "Free trial";

/**
 * Where a free trial's "Start free trial" link goes: the portal's `/buy`,
 * served by the hub the prices were read from, which asks a signed-out
 * visitor to sign in first.
 */
export function trialHref(origin: string): string {
  return `${origin}/buy`;
}

const grouped = (n: number) => String(n).replace(/\B(?=(\d{3})+(?!\d))/g, ",");

export function intervalLabel(interval: CatalogOffering["interval"]): string {
  return interval === "annual" ? "per year" : "one-off payment";
}

export function seatsLabel(seats: number): string {
  return seats === 1 ? "1 seat" : `${grouped(seats)} seats`;
}

export function scopeLabel(scope: Grants["scope"]): string {
  return scope === "site" ? "Site licence" : "Machine licence";
}

export function quotaLabel(quota: number | null): string {
  if (quota === null) return "Unlimited erasures";
  return quota === 1 ? "1 erasure" : `${grouped(quota)} erasures`;
}

export function termLabel(months: number): string {
  return `${months}-month term`;
}
