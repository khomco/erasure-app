// Mirrors the station and licence routes in
// crates/wipe-hub/src/station_routes.rs and the serde shapes
// `wipe_hub::stations::{Station, Licence}` (crates/wipe-hub/src/stations.rs)
// and `wipe_hub::credits::GrantSummary` (crates/wipe-hub/src/credits.rs).
// Change both together. The organization lapse-policy setting is retired
// (ADR-0013 §5): the hub no longer serves it or reports it here.
//
// The licence file itself is never parsed here: the browser downloads the
// hub's stored bytes from `licenceDownloadPath`, exactly as they were signed
// (with a credit grant beside the licence when credits were moved into it).

import { ensureOk, isObject, json, request, type Fetch } from "./auth";
import { SCOPES, type Scope } from "./adminOfferings";

export const LICENCE_STATUSES = ["active", "superseded", "revoked"] as const;
export type LicenceStatus = (typeof LICENCE_STATUSES)[number];

/**
 * A licence the hub issued to a station (its summary, not the file).
 * Timestamps are RFC 3339 UTC; absent values are `null`.
 */
export interface Licence {
  /** The signed licence's `license_id`. */
  id: string;
  /** The station row it was issued for (not the bench's `--station-id`). */
  station_id: string;
  entitlement_id: string;
  instance_key_id: string;
  root_key_id: string;
  not_after: string;
  issued_at: string;
  status: LicenceStatus;
  superseded_at: string | null;
  /**
   * Set when a vendor admin revoked it. The hub records this only: the file
   * keeps working offline until its own expiry. The reason is never shown here.
   */
  revoked_at: string | null;
}

/** A registered station and its most recently issued licence. */
export interface Station {
  id: string;
  entitlement_id: string;
  /** The entitlement's offering name. */
  offering_name: string;
  scope: Scope;
  instance_key_id: string;
  /** The bench's `--station-id`, as entered. */
  station_id: string | null;
  label: string | null;
  site: string | null;
  registered_at: string;
  /** Set once released: its seat is free again. */
  released_at: string | null;
  licence: Licence | null;
  /**
   * A renewed licence can be issued: the station is not released, its
   * entitlement is current and its latest licence ends before the
   * entitlement's (renewed) term end. Computed by the hub.
   */
  renewal_available: boolean;
  /**
   * The credit balance the station last reported, and when; `null` until it
   * reports (connected stations report from a later release).
   */
  credit_balance: number | null;
  credit_balance_reported_at: string | null;
  /** Credits granted to this station from its entitlement's pool. */
  credits_granted: number;
  /**
   * When a credit report from this station last did not add up (it claimed
   * more credits than it was ever granted, or its spent count went
   * backwards); `null` if never. Sticky: a report that adds up never clears
   * it, and each mismatch moves it to the latest.
   */
  credit_report_mismatch_at: string | null;
}

/** A credit grant issued with a licence: its id and how many credits. */
export interface CreditGrant {
  id: string;
  credits: number;
}

export interface Registered {
  station: Station;
  licence: Licence;
  /** The credit grant inside the new licence file, if any. */
  credit_grant: CreditGrant | null;
}

/** What an owner enters to register a station; blank optional fields are omitted. */
export interface RegisterInput {
  entitlement_id: string;
  instance_key_id: string;
  station_id?: string;
  label?: string;
  site?: string;
  /**
   * Credits to move from a quota entitlement's pool into the licence file
   * (a whole number, 0 or more). Omitted: the hub gives the smaller of the
   * target reserve and what is left. Never sent for an unlimited entitlement.
   */
  credits?: number;
}

/** The vendor root public key: what a station passes as `--vendor-root-b64`. */
export interface VendorRootKey {
  public_key_id: string;
  public_key_b64: string;
}

/**
 * The shape of a station's instance key id, as the hub checks it:
 * `ed25519:` and 22 characters of unpadded base64.
 */
export const INSTANCE_KEY_ID = /^ed25519:[A-Za-z0-9+/]{22}$/;

export function isInstanceKeyId(v: string): boolean {
  return INSTANCE_KEY_ID.test(v.trim());
}

const isNullableString = (v: unknown): v is string | null => v === null || typeof v === "string";
const isInt = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v);
const isNullableInt = (v: unknown): v is number | null => v === null || isInt(v);
const oneOf = (values: readonly string[], v: unknown) =>
  typeof v === "string" && values.includes(v);

export function isLicence(v: unknown): v is Licence {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.station_id === "string" &&
    typeof v.entitlement_id === "string" &&
    typeof v.instance_key_id === "string" &&
    typeof v.root_key_id === "string" &&
    typeof v.not_after === "string" &&
    typeof v.issued_at === "string" &&
    oneOf(LICENCE_STATUSES, v.status) &&
    isNullableString(v.superseded_at) &&
    isNullableString(v.revoked_at)
  );
}

export function isStation(v: unknown): v is Station {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.entitlement_id === "string" &&
    typeof v.offering_name === "string" &&
    oneOf(SCOPES, v.scope) &&
    typeof v.instance_key_id === "string" &&
    isNullableString(v.station_id) &&
    isNullableString(v.label) &&
    isNullableString(v.site) &&
    typeof v.registered_at === "string" &&
    isNullableString(v.released_at) &&
    (v.licence === null || isLicence(v.licence)) &&
    typeof v.renewal_available === "boolean" &&
    isNullableInt(v.credit_balance) &&
    isNullableString(v.credit_balance_reported_at) &&
    isInt(v.credits_granted) &&
    isNullableString(v.credit_report_mismatch_at)
  );
}

export function isCreditGrant(v: unknown): v is CreditGrant {
  return isObject(v) && typeof v.id === "string" && isInt(v.credits);
}

export function isVendorRootKey(v: unknown): v is VendorRootKey {
  return isObject(v) && typeof v.public_key_id === "string" && typeof v.public_key_b64 === "string";
}

const isStationList = (v: unknown): v is { stations: Station[] } =>
  isObject(v) && Array.isArray(v.stations) && v.stations.every(isStation);

const isRegistered = (v: unknown): v is Registered =>
  isObject(v) &&
  isStation(v.station) &&
  isLicence(v.licence) &&
  (v.credit_grant === null || isCreditGrant(v.credit_grant));

const isStationBody = (v: unknown): v is { station: Station } =>
  isObject(v) && isStation(v.station);

const isRootBody = (v: unknown): v is { root: VendorRootKey | null } =>
  isObject(v) && (v.root === null || isVendorRootKey(v.root));

/** Trim every field and drop blank optional ones. */
export function registerBody(input: RegisterInput): RegisterInput {
  const body: RegisterInput = {
    entitlement_id: input.entitlement_id,
    instance_key_id: input.instance_key_id.trim(),
  };
  for (const key of ["station_id", "label", "site"] as const) {
    const value = input[key]?.trim();
    if (value) body[key] = value;
  }
  if (input.credits !== undefined) body.credits = input.credits;
  return body;
}

/**
 * The credits the register form suggests for an entitlement's pool: the
 * smaller of its target reserve and what is left, as the hub's default.
 */
export function suggestedCredits(pool: { available: number; target_reserve: number }): number {
  return Math.max(0, Math.min(pool.target_reserve, pool.available));
}

// ---------------------------------------------------------------------------
// Calls

/** `GET /api/stations` (any role): released stations last, newest first. */
export async function fetchStations(fetchImpl: Fetch = fetch): Promise<Station[]> {
  const res = await ensureOk(await request(fetchImpl, "GET", "/api/stations"));
  return (await json(res, isStationList, "station list")).stations;
}

/** `POST /api/stations` (owner, 201): the station and its new licence. */
export async function registerStation(
  input: RegisterInput,
  fetchImpl: Fetch = fetch,
): Promise<Registered> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/stations", registerBody(input)),
  );
  return json(res, isRegistered, "station registration");
}

/** `POST /api/stations/{id}/release` (owner): the released station. */
export async function releaseStation(id: string, fetchImpl: Fetch = fetch): Promise<Station> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", `/api/stations/${encodeURIComponent(id)}/release`, {}),
  );
  return (await json(res, isStationBody, "station release")).station;
}

/**
 * `POST /api/stations/{id}/reissue` (owner, 201): the station and its renewed
 * licence, to the entitlement's current term end, or, with `credits` (1 or
 * more, quota entitlements only), a new licence file carrying that many
 * credits from the pool. The old licence is superseded; its file still works
 * until its own expiry.
 */
export async function reissueStation(
  id: string,
  fetchImpl: Fetch = fetch,
  credits?: number,
): Promise<Registered> {
  const body = credits === undefined ? {} : { credits };
  const res = await ensureOk(
    await request(fetchImpl, "POST", `/api/stations/${encodeURIComponent(id)}/reissue`, body),
  );
  return json(res, isRegistered, "licence reissue");
}

/** `GET /api/licensing/root` (any role): the vendor root public key, or `null`. */
export async function fetchVendorRoot(fetchImpl: Fetch = fetch): Promise<VendorRootKey | null> {
  const res = await ensureOk(await request(fetchImpl, "GET", "/api/licensing/root"));
  return (await json(res, isRootBody, "vendor root")).root;
}

/**
 * Where the browser downloads a licence file (any role): the hub answers the
 * stored file as an attachment and records the download.
 */
export function licenceDownloadPath(licenceId: string): string {
  return `/api/licences/${encodeURIComponent(licenceId)}/download`;
}
