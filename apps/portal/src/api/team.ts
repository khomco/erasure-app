// Mirrors the organization, team and invitation routes in
// crates/wipe-hub/src/team_routes.rs and `wipe_hub::team::{Invitation,
// InvitationPreview}` (crates/wipe-hub/src/team.rs). Change both together.
//
// The hub decides who may do what (owners manage the team; an organization
// keeps at least one owner). The portal only hides controls a member cannot
// use; it never re-implements those rules.

import {
  ensureOk,
  isAccount,
  isObject,
  isOrganization,
  isUser,
  json,
  request,
  type Account,
  type Fetch,
  type Organization,
  type User,
} from "./auth";

/** A role an owner can assign. */
export type AssignableRole = "owner" | "member";

/** A pending invitation. Timestamps are RFC 3339 UTC strings. */
export interface Invitation {
  id: string;
  email: string;
  role: AssignableRole;
  created_at: string;
  expires_at: string;
}

export interface InviteResult {
  invitation: Invitation;
  /** `false` when the hub could not hand the email to SMTP; inviting again re-sends. */
  invitation_email_sent: boolean;
}

/** What an invitee sees before accepting. */
export interface InvitationPreview {
  organization_name: string;
  email: string;
  role: AssignableRole;
}

export interface AcceptInput {
  token: string;
  name: string;
  password: string;
}

function isAssignableRole(v: unknown): v is AssignableRole {
  return v === "owner" || v === "member";
}

export function isInvitation(v: unknown): v is Invitation {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.email === "string" &&
    isAssignableRole(v.role) &&
    typeof v.created_at === "string" &&
    typeof v.expires_at === "string"
  );
}

function isInviteResult(v: unknown): v is InviteResult {
  return isObject(v) && isInvitation(v.invitation) && typeof v.invitation_email_sent === "boolean";
}

function isPreview(v: unknown): v is InvitationPreview {
  return (
    isObject(v) &&
    typeof v.organization_name === "string" &&
    typeof v.email === "string" &&
    isAssignableRole(v.role)
  );
}

function isMembers(v: unknown): v is { members: User[] } {
  return isObject(v) && Array.isArray(v.members) && v.members.every(isUser);
}

function isInvitations(v: unknown): v is { invitations: Invitation[] } {
  return isObject(v) && Array.isArray(v.invitations) && v.invitations.every(isInvitation);
}

/** `GET /api/organization/members` (any role): the organization's users. */
export async function fetchMembers(fetchImpl: Fetch = fetch): Promise<User[]> {
  const res = await ensureOk(await request(fetchImpl, "GET", "/api/organization/members"));
  return (await json(res, isMembers, "members")).members;
}

/** `POST /api/organization` (owner): rename the organization. */
export async function renameOrganization(
  name: string,
  fetchImpl: Fetch = fetch,
): Promise<Organization> {
  const res = await ensureOk(await request(fetchImpl, "POST", "/api/organization", { name }));
  return json(res, isOrganization, "rename organization");
}

/**
 * `POST /api/organization/invitations` (owner, 201): email an invitation.
 * Counts against the email-send limit (429, see `emailRequestErrorMessage`).
 */
export async function inviteMember(
  email: string,
  role: AssignableRole,
  fetchImpl: Fetch = fetch,
): Promise<InviteResult> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/organization/invitations", { email, role }),
  );
  return json(res, isInviteResult, "invite");
}

/** `GET /api/organization/invitations` (owner): pending invitations. */
export async function fetchInvitations(fetchImpl: Fetch = fetch): Promise<Invitation[]> {
  const res = await ensureOk(await request(fetchImpl, "GET", "/api/organization/invitations"));
  return (await json(res, isInvitations, "invitations")).invitations;
}

/** `POST /api/organization/invitations/{id}/revoke` (owner, 204). */
export async function revokeInvitation(id: string, fetchImpl: Fetch = fetch): Promise<void> {
  await ensureOk(
    await request(
      fetchImpl,
      "POST",
      `/api/organization/invitations/${encodeURIComponent(id)}/revoke`,
    ),
  );
}

/** `POST /api/organization/members/{id}/role` (owner): the updated member. */
export async function changeRole(
  id: string,
  role: AssignableRole,
  fetchImpl: Fetch = fetch,
): Promise<User> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", `/api/organization/members/${encodeURIComponent(id)}/role`, {
      role,
    }),
  );
  return json(res, isUser, "change role");
}

/** `POST /api/organization/members/{id}/remove` (owner, 204). */
export async function removeMember(id: string, fetchImpl: Fetch = fetch): Promise<void> {
  await ensureOk(
    await request(fetchImpl, "POST", `/api/organization/members/${encodeURIComponent(id)}/remove`),
  );
}

/**
 * `POST /api/invitations/lookup` (anonymous): the pending invitation's
 * organization name, email and role. Consumes nothing. An unusable token is a 400 `HubError`.
 */
export async function lookupInvitation(
  token: string,
  fetchImpl: Fetch = fetch,
): Promise<InvitationPreview> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/invitations/lookup", { token }),
  );
  return json(res, isPreview, "invitation lookup");
}

/**
 * `POST /api/invitations/accept` (anonymous, 201): create the account in the
 * inviting organization and sign in.
 */
export async function acceptInvitation(
  input: AcceptInput,
  fetchImpl: Fetch = fetch,
): Promise<Account> {
  const res = await ensureOk(await request(fetchImpl, "POST", "/api/invitations/accept", input));
  return json(res, isAccount, "accept invitation");
}
