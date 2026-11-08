// Mirrors the sales-lead admin routes in crates/wipe-hub/src/admin_routes.rs
// and `wipe_hub::lead_admin::{AdminLead, LeadList, LeadDetail, …}`
// (crates/wipe-hub/src/lead_admin.rs). Change both together.
//
// Vendor admins only; everyone else gets a 403 `HubError`. Every call here
// is audited on the hub (a list run, a view, an export, a change), so callers
// run them on an explicit action, never per keystroke, on a timer, or to
// refresh a page after a change: a change's response is the new state.

import { ensureOk, isObject, json, request, type Fetch } from "./auth";

/** The early-access form's segments (`wipe_hub::leads::SEGMENTS`). */
export const SEGMENTS = ["itad", "enterprise-it", "government", "other"] as const;
export type Segment = (typeof SEGMENTS)[number];

/** A lead's statuses (`wipe_hub::lead_admin::STATUSES`). */
export const STATUSES = ["new", "contacted", "qualified", "closed"] as const;
export type LeadStatus = (typeof STATUSES)[number];

/** The `segment` filter value for leads without a segment (contact form only). */
export const SEGMENT_NONE = "none";

/** A sales lead. Timestamps are RFC 3339 UTC; absent values are `null`. */
export interface AdminLead {
  id: string;
  email: string;
  name: string;
  company: string;
  role: string | null;
  segment: Segment | null;
  status: LeadStatus;
  created_at: string;
  updated_at: string;
  /** The newest submission's consent time. */
  last_submitted_at: string | null;
  submission_count: number;
  /** Set once the lead used an emailed unsubscribe link. */
  unsubscribed_at: string | null;
}

export interface LeadList {
  leads: AdminLead[];
  /** More than the first 100 matched. */
  truncated: boolean;
}

/** One form submission, with the exact consent text agreed to. */
export interface LeadSubmission {
  id: string;
  kind: "early-access" | "contact";
  message: string | null;
  consent_text: string;
  consented_at: string;
}

export interface LeadNote {
  id: string;
  body: string;
  created_at: string;
  /** `null` once the author's account is removed. */
  author: { id: string; name: string; email: string } | null;
}

export interface LeadDetail {
  lead: AdminLead;
  /** Newest first. */
  submissions: LeadSubmission[];
  /** Newest first. */
  notes: LeadNote[];
}

/**
 * The list and export filters. An empty string means any. `segment` is a
 * segment or `"none"`; `from` and `to` are inclusive UTC days, `YYYY-MM-DD`,
 * matching leads with any submission in the range.
 */
export interface LeadFilters {
  segment: string;
  status: string;
  from: string;
  to: string;
}

export const NO_FILTERS: LeadFilters = { segment: "", status: "", from: "", to: "" };

const isNullableString = (v: unknown): v is string | null => v === null || typeof v === "string";

export function isAdminLead(v: unknown): v is AdminLead {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.email === "string" &&
    typeof v.name === "string" &&
    typeof v.company === "string" &&
    isNullableString(v.role) &&
    (v.segment === null || (SEGMENTS as readonly unknown[]).includes(v.segment)) &&
    (STATUSES as readonly unknown[]).includes(v.status) &&
    typeof v.created_at === "string" &&
    typeof v.updated_at === "string" &&
    isNullableString(v.last_submitted_at) &&
    typeof v.submission_count === "number" &&
    isNullableString(v.unsubscribed_at)
  );
}

function isLeadList(v: unknown): v is LeadList {
  return (
    isObject(v) &&
    Array.isArray(v.leads) &&
    v.leads.every(isAdminLead) &&
    typeof v.truncated === "boolean"
  );
}

function isSubmission(v: unknown): v is LeadSubmission {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    (v.kind === "early-access" || v.kind === "contact") &&
    isNullableString(v.message) &&
    typeof v.consent_text === "string" &&
    typeof v.consented_at === "string"
  );
}

export function isLeadNote(v: unknown): v is LeadNote {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.body === "string" &&
    typeof v.created_at === "string" &&
    (v.author === null ||
      (isObject(v.author) &&
        typeof v.author.id === "string" &&
        typeof v.author.name === "string" &&
        typeof v.author.email === "string"))
  );
}

function isLeadDetail(v: unknown): v is LeadDetail {
  return (
    isObject(v) &&
    isAdminLead(v.lead) &&
    Array.isArray(v.submissions) &&
    v.submissions.every(isSubmission) &&
    Array.isArray(v.notes) &&
    v.notes.every(isLeadNote)
  );
}

/** Only the filters that are set, so an unset one is absent, not empty. */
function setFilters(filters: LeadFilters): Partial<LeadFilters> {
  const out: Partial<LeadFilters> = {};
  for (const key of ["segment", "status", "from", "to"] as const) {
    if (filters[key] !== "") out[key] = filters[key];
  }
  return out;
}

function leadPath(id: string, action?: string): string {
  const base = `/api/admin/leads/${encodeURIComponent(id)}`;
  return action ? `${base}/${action}` : base;
}

/** `GET /api/admin/leads?…` (vendor admin): at most 100 leads, latest first. Audited. */
export async function listLeads(filters: LeadFilters, fetchImpl: Fetch = fetch): Promise<LeadList> {
  const query = new URLSearchParams(setFilters(filters)).toString();
  const path = query ? `/api/admin/leads?${query}` : "/api/admin/leads";
  const res = await ensureOk(await request(fetchImpl, "GET", path));
  return json(res, isLeadList, "lead list");
}

/** `GET /api/admin/leads/{id}`: the lead, its submissions and notes. Audited. */
export async function fetchLead(id: string, fetchImpl: Fetch = fetch): Promise<LeadDetail> {
  const res = await ensureOk(await request(fetchImpl, "GET", leadPath(id)));
  return json(res, isLeadDetail, "lead");
}

/** `POST /api/admin/leads/{id}/status`: the updated lead. Audited when it changes. */
export async function setLeadStatus(
  id: string,
  status: LeadStatus,
  fetchImpl: Fetch = fetch,
): Promise<AdminLead> {
  const res = await ensureOk(await request(fetchImpl, "POST", leadPath(id, "status"), { status }));
  const body = await json(
    res,
    (v): v is { lead: AdminLead } => isObject(v) && isAdminLead(v.lead),
    "lead status",
  );
  return body.lead;
}

/** `POST /api/admin/leads/{id}/notes` (201): the new note. Audited. */
export async function addLeadNote(
  id: string,
  body: string,
  fetchImpl: Fetch = fetch,
): Promise<LeadNote> {
  const res = await ensureOk(await request(fetchImpl, "POST", leadPath(id, "notes"), { body }));
  const out = await json(
    res,
    (v): v is { note: LeadNote } => isObject(v) && isLeadNote(v.note),
    "lead note",
  );
  return out.note;
}

/** `POST /api/admin/leads/{id}/erase`: deletes the lead, its submissions and notes. Audited. */
export async function eraseLead(id: string, fetchImpl: Fetch = fetch): Promise<void> {
  await ensureOk(await request(fetchImpl, "POST", leadPath(id, "erase"), {}));
}

/** The file name in a `Content-Disposition: attachment; filename="…"` header. */
export function attachmentName(header: string | null): string | null {
  const match = header?.match(/filename="([^"]+)"/);
  return match ? match[1] : null;
}

/**
 * `POST /api/admin/leads/export` with the CSRF token: every matching lead as
 * CSV, uncapped, and the hub's file name for it. Audited.
 */
export async function exportLeads(
  filters: LeadFilters,
  fetchImpl: Fetch = fetch,
): Promise<{ blob: Blob; filename: string }> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/admin/leads/export", setFilters(filters)),
  );
  const filename = attachmentName(res.headers.get("Content-Disposition")) ?? "sales-leads.csv";
  return { blob: await res.blob(), filename };
}
