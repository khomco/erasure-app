// Mirrors the licence admin routes in crates/wipe-hub/src/admin_routes.rs and
// `wipe_hub::licence_admin::{AdminLicence, Verification, LicenceDetail,
// Inspected, IssueRequest}` (crates/wipe-hub/src/licence_admin.rs), plus the
// serde shape of `wipe_license::LicenseBody`. Change them together.
//
// Vendor admins only; everyone else gets a 403 `HubError`. Every call here is
// audited on the hub (a list, a view, an inspection, an issue, a revocation),
// so callers run them on an explicit action, never per keystroke or on a
// timer, and show a change's answer instead of refetching.

import { ensureOk, isObject, isOrganization, json, request, type Fetch, type Organization } from "./auth";
import { FEATURES, SCOPES, type Feature, type Scope } from "./adminOfferings";
import { LICENCE_STATUSES, type LicenceStatus } from "./stations";

/**
 * `wipe_license::LapsePolicy`: a signed field ADR-0012 added and ADR-0013 §5
 * retired. Older files may still carry it, so a decoded body accepts it; the
 * portal never displays it.
 */
export const LAPSE_POLICIES = ["evaluation", "stop"] as const;
export type LapsePolicy = (typeof LAPSE_POLICIES)[number];

/** Where a licence came from: a registered station, or a vendor admin. */
export const LICENCE_SOURCES = ["station", "vendor"] as const;
export type LicenceSource = (typeof LICENCE_SOURCES)[number];

export const VERIFICATION_STATUSES = ["verified", "failed", "no_root"] as const;
export type VerificationStatus = (typeof VERIFICATION_STATUSES)[number];

/** The longest vendor-issued licence, in days (`licence_admin::DAYS_MAX`). */
export const DAYS_MAX = 1095;
/** The longest issue or revocation reason (`licence_admin::REASON_MAX_CHARS`). */
export const REASON_MAX_CHARS = 500;
/** The largest file the hub inspects, in bytes (`licence_admin::INSPECT_MAX_BYTES`). */
export const INSPECT_MAX_BYTES = 64 * 1024;

/** The station a licence was issued for. */
export interface AdminLicenceStation {
  /** The `stations` row. */
  id: string;
  /** The bench's `--station-id`. */
  station_id: string | null;
  label: string | null;
}

/** A licence as the admin area sees it. Timestamps are RFC 3339 UTC. */
export interface AdminLicence {
  /** The signed licence's `license_id`. */
  id: string;
  organization: Organization;
  source: LicenceSource;
  /** `null` for a vendor-issued licence. */
  station: AdminLicenceStation | null;
  entitlement_id: string | null;
  instance_key_id: string;
  root_key_id: string;
  not_after: string;
  issued_at: string;
  status: LicenceStatus;
  superseded_at: string | null;
  revoked_at: string | null;
  /** Why a vendor admin issued it (vendor-issued only). */
  issue_reason: string | null;
  /** Vendor-internal: never shown to the customer. */
  revocation_reason: string | null;
}

/** `wipe_license::Quota`. */
export type Quota = { kind: "unlimited" } | { kind: "count"; erasures: number };
/** `wipe_license::Scope`. */
export type LicenceScope = { kind: "machine"; fingerprint: string } | { kind: "site"; site_id: string };

/** The signed body of a licence file (`wipe_license::LicenseBody`), decoded. */
export interface LicenseBody {
  license_format_version: number;
  license_id: string;
  root_key_id: string;
  instance_public_key_id: string;
  entitlements: {
    customer_id: string;
    customer_name: string;
    quota: Quota;
    scope: LicenceScope;
    not_before: string;
    not_after: string;
    features: string[];
    allowed_methods: { kind: string; classes?: string[] };
    machine_binding?: string;
  };
  issued_at: string;
  /** Retired (ADR-0013 §5): parsed if present, never displayed. */
  lapse_policy?: LapsePolicy;
}

/** Whether a licence file verifies against the hub's vendor root. */
export interface Verification {
  status: VerificationStatus;
  /** The hub's root key id; `null` when it has none. */
  root_key_id: string | null;
  /** Why it failed. */
  detail: string | null;
}

export interface LicenceDetail {
  licence: AdminLicence;
  body: LicenseBody;
  verification: Verification;
}

export interface Inspected {
  body: LicenseBody;
  verification: Verification;
  /** The hub's record of this `license_id`, if it holds one. */
  licence: AdminLicence | null;
}

/** What a vendor admin types to issue a licence. */
export interface IssueInput {
  instance_key_id: string;
  scope: Scope;
  /** Required for machine scope: the bench's `--station-id`. */
  station_id?: string;
  /** Site scope only; blank is the organization. */
  site?: string;
  /** `null` is unlimited. */
  quota_erasures: number | null;
  features: Feature[];
  days: number;
  reason: string;
}

const isNullableString = (v: unknown): v is string | null => v === null || typeof v === "string";
const isInt = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v);
const oneOf = (values: readonly string[], v: unknown) =>
  typeof v === "string" && values.includes(v);

function isStationRef(v: unknown): v is AdminLicenceStation {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    isNullableString(v.station_id) &&
    isNullableString(v.label)
  );
}

export function isAdminLicence(v: unknown): v is AdminLicence {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    isOrganization(v.organization) &&
    oneOf(LICENCE_SOURCES, v.source) &&
    (v.station === null || isStationRef(v.station)) &&
    isNullableString(v.entitlement_id) &&
    typeof v.instance_key_id === "string" &&
    typeof v.root_key_id === "string" &&
    typeof v.not_after === "string" &&
    typeof v.issued_at === "string" &&
    oneOf(LICENCE_STATUSES, v.status) &&
    isNullableString(v.superseded_at) &&
    isNullableString(v.revoked_at) &&
    isNullableString(v.issue_reason) &&
    isNullableString(v.revocation_reason)
  );
}

function isQuota(v: unknown): v is Quota {
  return isObject(v) && (v.kind === "unlimited" || (v.kind === "count" && isInt(v.erasures)));
}

function isScope(v: unknown): v is LicenceScope {
  return (
    isObject(v) &&
    ((v.kind === "machine" && typeof v.fingerprint === "string") ||
      (v.kind === "site" && typeof v.site_id === "string"))
  );
}

export function isLicenseBody(v: unknown): v is LicenseBody {
  if (!isObject(v) || !isObject(v.entitlements)) return false;
  const e = v.entitlements;
  return (
    isInt(v.license_format_version) &&
    typeof v.license_id === "string" &&
    typeof v.root_key_id === "string" &&
    typeof v.instance_public_key_id === "string" &&
    typeof v.issued_at === "string" &&
    typeof e.customer_id === "string" &&
    typeof e.customer_name === "string" &&
    isQuota(e.quota) &&
    isScope(e.scope) &&
    typeof e.not_before === "string" &&
    typeof e.not_after === "string" &&
    Array.isArray(e.features) &&
    e.features.every((f) => typeof f === "string") &&
    isObject(e.allowed_methods) &&
    typeof e.allowed_methods.kind === "string" &&
    (e.machine_binding === undefined || typeof e.machine_binding === "string") &&
    (v.lapse_policy === undefined || oneOf(LAPSE_POLICIES, v.lapse_policy))
  );
}

export function isVerification(v: unknown): v is Verification {
  return (
    isObject(v) &&
    oneOf(VERIFICATION_STATUSES, v.status) &&
    isNullableString(v.root_key_id) &&
    isNullableString(v.detail)
  );
}

const isLicenceList = (v: unknown): v is { licences: AdminLicence[] } =>
  isObject(v) && Array.isArray(v.licences) && v.licences.every(isAdminLicence);

const isLicenceBody = (v: unknown): v is { licence: AdminLicence } =>
  isObject(v) && isAdminLicence(v.licence);

export function isLicenceDetail(v: unknown): v is LicenceDetail {
  return (
    isObject(v) && isAdminLicence(v.licence) && isLicenseBody(v.body) && isVerification(v.verification)
  );
}

export function isInspected(v: unknown): v is Inspected {
  return (
    isObject(v) &&
    isLicenseBody(v.body) &&
    isVerification(v.verification) &&
    (v.licence === null || isAdminLicence(v.licence))
  );
}

// ---------------------------------------------------------------------------
// Pure helpers

/** Trim every field, drop blank optional ones and the other scope's field. */
export function issueBody(input: IssueInput): IssueInput {
  const body: IssueInput = {
    instance_key_id: input.instance_key_id.trim(),
    scope: input.scope,
    quota_erasures: input.quota_erasures,
    features: [...input.features],
    days: input.days,
    reason: input.reason.trim(),
  };
  const field = input.scope === "machine" ? "station_id" : "site";
  const value = input[field]?.trim();
  if (value) body[field] = value;
  return body;
}

/** The date part of an RFC 3339 UTC timestamp. */
function day(timestamp: string): string {
  return timestamp.slice(0, 10);
}

/** "Active", "Superseded on …" or "Revoked on …". */
export function licenceStatusText(l: AdminLicence): string {
  if (l.status === "revoked" && l.revoked_at) return `Revoked on ${day(l.revoked_at)}`;
  if (l.status === "superseded" && l.superseded_at) return `Superseded on ${day(l.superseded_at)}`;
  return "Active";
}

/** "Bench 1 (bench-1)", "Vendor-issued", … */
export function licenceHolderText(l: AdminLicence): string {
  if (l.source === "vendor" || l.station === null) return "Vendor-issued";
  const { label, station_id } = l.station;
  if (label && station_id) return `${label} (${station_id})`;
  return label ?? station_id ?? "Station";
}

/** "Unlimited erasures" or "5,000 erasures". */
export function quotaText(q: Quota): string {
  return q.kind === "unlimited" ? "Unlimited erasures" : `${q.erasures.toLocaleString("en")} erasures`;
}

/** "Machine mf1:…" or "Site Floor 2". */
export function scopeText(s: LicenceScope): string {
  return s.kind === "machine" ? `Machine ${s.fingerprint}` : `Site ${s.site_id}`;
}

/** The expiry date `days` whole days after `now`, as the hub computes it. */
export function expiryAfter(days: number, now: Date = new Date()): string {
  return new Date(now.getTime() + days * 86_400_000).toISOString().slice(0, 10);
}

export { FEATURES, SCOPES };

// ---------------------------------------------------------------------------
// Calls

const organizationLicencesPath = (orgId: string) =>
  `/api/admin/organizations/${encodeURIComponent(orgId)}/licences`;

function licencePath(id: string, action?: string): string {
  const base = `/api/admin/licences/${encodeURIComponent(id)}`;
  return action ? `${base}/${action}` : base;
}

/** `GET /api/admin/organizations/{id}/licences`: every licence, newest first. Audited. */
export async function listOrganizationLicences(
  orgId: string,
  fetchImpl: Fetch = fetch,
): Promise<AdminLicence[]> {
  const res = await ensureOk(await request(fetchImpl, "GET", organizationLicencesPath(orgId)));
  return (await json(res, isLicenceList, "licence list")).licences;
}

/**
 * `POST /api/admin/organizations/{id}/licences` (201): sign a licence for the
 * organization outside any entitlement and seat. Audited with its reason.
 */
export async function issueLicence(
  orgId: string,
  input: IssueInput,
  fetchImpl: Fetch = fetch,
): Promise<AdminLicence> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", organizationLicencesPath(orgId), issueBody(input)),
  );
  return (await json(res, isLicenceBody, "issued licence")).licence;
}

/** `GET /api/admin/licences/{id}`: the record, decoded body and verification. Audited. */
export async function fetchLicence(id: string, fetchImpl: Fetch = fetch): Promise<LicenceDetail> {
  const res = await ensureOk(await request(fetchImpl, "GET", licencePath(id)));
  return json(res, isLicenceDetail, "licence");
}

/** `POST /api/admin/licences/inspect`: decode and verify a pasted file. Audited. */
export async function inspectLicence(document: string, fetchImpl: Fetch = fetch): Promise<Inspected> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/admin/licences/inspect", { document }),
  );
  return json(res, isInspected, "licence inspection");
}

/**
 * `POST /api/admin/licences/{id}/revoke`: mark it revoked, for good. Recorded
 * in the hub only; stations are not told. Audited with the reason.
 */
export async function revokeLicence(
  id: string,
  reason: string,
  fetchImpl: Fetch = fetch,
): Promise<AdminLicence> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", licencePath(id, "revoke"), { reason: reason.trim() }),
  );
  return (await json(res, isLicenceBody, "licence revocation")).licence;
}

/** Where the browser downloads a licence file as a vendor admin. Audited. */
export function adminLicenceDownloadPath(id: string): string {
  return licencePath(id, "download");
}
