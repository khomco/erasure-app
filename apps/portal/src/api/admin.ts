// Mirrors the admin area's routes in crates/wipe-hub/src/admin_routes.rs and
// `wipe_hub::admin::{OrganizationSearch, UserSearch, AdminOrganizationDetail}`
// (crates/wipe-hub/src/admin.rs). Change both together.
//
// Only vendor admins get answers; everyone else gets a 403 `HubError`. The
// portal only hides the link to the admin area; the hub enforces who may
// use it. Every call here is an audited view on the hub, so callers run
// them on an explicit action, never per keystroke or on a timer.

import {
  HubError,
  ensureOk,
  isObject,
  isOrganization,
  json,
  request,
  type Fetch,
  type Organization,
  type Role,
} from "./auth";
import { LAPSE_POLICIES, type LapsePolicy } from "./stations";

/** A customer organization in search results. Timestamps are RFC 3339 UTC. */
export interface AdminOrganization {
  id: string;
  name: string;
  created_at: string;
  /** Its customer users (owners and members). */
  user_count: number;
}

export interface OrganizationSearch {
  organizations: AdminOrganization[];
  /** More than the first 100 matched. */
  truncated: boolean;
}

/** A user on an organization's detail page. */
export interface AdminMember {
  id: string;
  email: string;
  name: string;
  role: Role;
  email_verified: boolean;
  created_at: string;
}

/** A user in search results, with their organization. */
export interface AdminUser extends AdminMember {
  organization: Organization;
}

export interface UserSearch {
  users: AdminUser[];
  truncated: boolean;
}

export interface AdminOrganizationDetail {
  organization: { id: string; name: string; created_at: string };
  users: AdminMember[];
  /** What the organization's stations do when a licence lapses (ADR-0012); read-only here. */
  lapse_policy: LapsePolicy;
}

function isRole(v: unknown): v is Role {
  return v === "owner" || v === "member" || v === "vendor_admin";
}

export function isAdminOrganization(v: unknown): v is AdminOrganization {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.name === "string" &&
    typeof v.created_at === "string" &&
    typeof v.user_count === "number"
  );
}

export function isAdminMember(v: unknown): v is AdminMember {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.email === "string" &&
    typeof v.name === "string" &&
    isRole(v.role) &&
    typeof v.email_verified === "boolean" &&
    typeof v.created_at === "string"
  );
}

export function isAdminUser(v: unknown): v is AdminUser {
  return isAdminMember(v) && isObject(v) && isOrganization(v.organization);
}

function isOrganizationSearch(v: unknown): v is OrganizationSearch {
  return (
    isObject(v) &&
    Array.isArray(v.organizations) &&
    v.organizations.every(isAdminOrganization) &&
    typeof v.truncated === "boolean"
  );
}

function isUserSearch(v: unknown): v is UserSearch {
  return (
    isObject(v) &&
    Array.isArray(v.users) &&
    v.users.every(isAdminUser) &&
    typeof v.truncated === "boolean"
  );
}

function isOrganizationDetail(v: unknown): v is AdminOrganizationDetail {
  return (
    isObject(v) &&
    isObject(v.organization) &&
    typeof v.organization.id === "string" &&
    typeof v.organization.name === "string" &&
    typeof v.organization.created_at === "string" &&
    Array.isArray(v.users) &&
    v.users.every(isAdminMember) &&
    typeof v.lapse_policy === "string" &&
    (LAPSE_POLICIES as readonly string[]).includes(v.lapse_policy)
  );
}

function withQuery(path: string, q: string): string {
  return `${path}?${new URLSearchParams({ q }).toString()}`;
}

/**
 * `GET /api/admin/organizations?q=` (vendor admin): customer organizations
 * whose name, or any user's email or name, contains `q`. Audited.
 */
export async function searchOrganizations(
  q: string,
  fetchImpl: Fetch = fetch,
): Promise<OrganizationSearch> {
  const res = await ensureOk(
    await request(fetchImpl, "GET", withQuery("/api/admin/organizations", q)),
  );
  return json(res, isOrganizationSearch, "organization search");
}

/** `GET /api/admin/users?q=` (vendor admin): customer users by email or name. Audited. */
export async function searchUsers(q: string, fetchImpl: Fetch = fetch): Promise<UserSearch> {
  const res = await ensureOk(await request(fetchImpl, "GET", withQuery("/api/admin/users", q)));
  return json(res, isUserSearch, "user search");
}

/**
 * `GET /api/admin/organizations/{id}` (vendor admin): one customer
 * organization and its users. Audited. Unknown ids are a 404 `HubError`.
 */
export async function fetchOrganization(
  id: string,
  fetchImpl: Fetch = fetch,
): Promise<AdminOrganizationDetail> {
  const res = await ensureOk(
    await request(fetchImpl, "GET", `/api/admin/organizations/${encodeURIComponent(id)}`),
  );
  return json(res, isOrganizationDetail, "organization");
}

/**
 * React Query `retry` for admin views: never retry a 4xx (a refusal or an
 * unknown id will not change, and each attempt is an audited request), and
 * retry anything else at most twice.
 */
export function retryAdminQuery(failureCount: number, error: unknown): boolean {
  if (error instanceof HubError && error.status >= 400 && error.status < 500) return false;
  return failureCount < 2;
}
