import { beforeEach, describe, expect, it } from "vitest";
import {
  HubError,
  changePassword,
  resetCsrfCache,
  updateProfile,
  type Account,
  type User,
} from "./auth";
import {
  acceptInvitation,
  changeRole,
  fetchInvitations,
  fetchMembers,
  inviteMember,
  lookupInvitation,
  removeMember,
  renameOrganization,
  revokeInvitation,
  type Invitation,
} from "./team";

const user: User = {
  id: "8a6e0804-2bd0-4672-b79d-d97027f9071a",
  email: "ada@example.com",
  name: "Ada",
  role: "owner",
  email_verified: true,
};

const account: Account = {
  user,
  organization: { id: "0f8fad5b-d9cb-469f-a165-70867728950e", name: "Engines Ltd" },
};

const invitation: Invitation = {
  id: "5b1c2a40-6a43-4b84-9a52-2f0b1d7a3c11",
  email: "grace@example.com",
  role: "member",
  created_at: "2026-11-08T10:00:00Z",
  expires_at: "2026-11-15T10:00:00Z",
};

interface Call {
  url: string;
  method: string;
  body: unknown;
  csrf: string | null;
}

/**
 * A fake hub, as in `auth.test.ts`: `GET /api/auth/csrf` answers
 * `csrf-<n>`; every other call is recorded and answered by `status`/`body`.
 */
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
    const headers = (init?.headers ?? {}) as Record<string, string>;
    calls.push({
      url,
      method,
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
      csrf: headers["X-CSRF-Token"] ?? null,
    });
    const text = body === undefined ? null : JSON.stringify(body);
    return new Response(text, {
      status,
      headers: text ? { "Content-Type": "application/json" } : {},
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

describe("account", () => {
  it("updateProfile posts the name with a CSRF token", async () => {
    const f = respond(200, account);
    expect(await updateProfile("Ada", f.impl)).toEqual(account);
    expect(f.calls).toEqual([
      { url: "/api/account/profile", method: "POST", body: { name: "Ada" }, csrf: "csrf-1" },
    ]);
  });

  it("changePassword sends both passwords and returns the account", async () => {
    const f = respond(200, account);
    expect(await changePassword("old one here", "new one here!", f.impl)).toEqual(account);
    expect(f.calls).toEqual([
      {
        url: "/api/auth/password",
        method: "POST",
        body: { current_password: "old one here", new_password: "new one here!" },
        csrf: "csrf-1",
      },
    ]);
  });

  it("changePassword surfaces a wrong current password", async () => {
    const f = respond(400, { error: "current password is incorrect" });
    const err = await rejection(changePassword("x", "new one here!", f.impl));
    expect(err).toBeInstanceOf(HubError);
    expect((err as HubError).status).toBe(400);
    expect((err as HubError).message).toBe("current password is incorrect");
  });
});

describe("organization", () => {
  it("fetchMembers reads the list without a CSRF token", async () => {
    const f = respond(200, { members: [user] });
    expect(await fetchMembers(f.impl)).toEqual([user]);
    expect(f.calls).toEqual([
      { url: "/api/organization/members", method: "GET", body: undefined, csrf: null },
    ]);
  });

  it("fetchMembers rejects an unexpected body", async () => {
    const f = respond(200, { members: [{ id: 1 }] });
    const err = await rejection(fetchMembers(f.impl));
    expect((err as Error).message).toBe("members returned an unexpected body (HTTP 200)");
  });

  it("renameOrganization posts the name", async () => {
    const f = respond(200, account.organization);
    expect(await renameOrganization("New", f.impl)).toEqual(account.organization);
    expect(f.calls[0]).toEqual({
      url: "/api/organization",
      method: "POST",
      body: { name: "New" },
      csrf: "csrf-1",
    });
  });

  it("an owner-only refusal is a 403 HubError", async () => {
    const f = respond(403, { error: "only an organization owner can manage the team" });
    const err = await rejection(renameOrganization("New", f.impl));
    expect((err as HubError).status).toBe(403);
    expect((err as HubError).message).toBe("only an organization owner can manage the team");
  });

  it("changeRole and removeMember address the member by id", async () => {
    const f = respond(200, { ...user, role: "member" });
    expect(await changeRole(user.id, "member", f.impl)).toEqual({ ...user, role: "member" });
    const g = respond(204);
    await removeMember(user.id, g.impl);
    expect(f.calls[0]).toEqual({
      url: `/api/organization/members/${user.id}/role`,
      method: "POST",
      body: { role: "member" },
      csrf: "csrf-1",
    });
    expect(g.calls[0]).toEqual({
      url: `/api/organization/members/${user.id}/remove`,
      method: "POST",
      body: undefined,
      csrf: "csrf-1",
    });
  });
});

describe("invitations", () => {
  it("inviteMember posts email and role", async () => {
    const f = respond(201, { invitation, invitation_email_sent: false });
    expect(await inviteMember("grace@example.com", "member", f.impl)).toEqual({
      invitation,
      invitation_email_sent: false,
    });
    expect(f.calls[0]).toEqual({
      url: "/api/organization/invitations",
      method: "POST",
      body: { email: "grace@example.com", role: "member" },
      csrf: "csrf-1",
    });
  });

  it("inviteMember keeps the mail limit's Retry-After", async () => {
    const calls: string[] = [];
    const impl = (async (input: RequestInfo | URL) => {
      const url = String(input);
      calls.push(url);
      if (url === "/api/auth/csrf") {
        return new Response(JSON.stringify({ csrf_token: "c" }), { status: 200 });
      }
      return new Response(JSON.stringify({ error: "too many emails requested; try again later" }), {
        status: 429,
        headers: { "Retry-After": "120" },
      });
    }) as typeof fetch;
    const err = await rejection(inviteMember("a@example.com", "owner", impl));
    expect((err as HubError).status).toBe(429);
    expect((err as HubError).retryAfterSecs).toBe(120);
  });

  it("fetchInvitations and revokeInvitation", async () => {
    const f = respond(200, { invitations: [invitation] });
    expect(await fetchInvitations(f.impl)).toEqual([invitation]);
    const g = respond(204);
    await revokeInvitation(invitation.id, g.impl);
    expect(f.calls[0].url).toBe("/api/organization/invitations");
    expect(g.calls[0]).toEqual({
      url: `/api/organization/invitations/${invitation.id}/revoke`,
      method: "POST",
      body: undefined,
      csrf: "csrf-1",
    });
  });

  it("fetchInvitations rejects a role the hub never assigns", async () => {
    const f = respond(200, { invitations: [{ ...invitation, role: "vendor_admin" }] });
    await expect(fetchInvitations(f.impl)).rejects.toThrow("unexpected body");
  });

  it("lookupInvitation returns the preview", async () => {
    const preview = { organization_name: "Engines Ltd", email: "g@example.com", role: "owner" };
    const f = respond(200, preview);
    expect(await lookupInvitation("tok", f.impl)).toEqual(preview);
    expect(f.calls[0]).toEqual({
      url: "/api/invitations/lookup",
      method: "POST",
      body: { token: "tok" },
      csrf: "csrf-1",
    });
  });

  it("lookupInvitation surfaces an unusable token", async () => {
    const f = respond(400, { error: "this invitation is invalid or has expired" });
    const err = await rejection(lookupInvitation("tok", f.impl));
    expect((err as HubError).message).toBe("this invitation is invalid or has expired");
  });

  it("acceptInvitation posts token, name and password and returns the account", async () => {
    const f = respond(201, account);
    const input = { token: "tok", name: "Grace", password: "correct horse battery" };
    expect(await acceptInvitation(input, f.impl)).toEqual(account);
    expect(f.calls[0]).toEqual({
      url: "/api/invitations/accept",
      method: "POST",
      body: input,
      csrf: "csrf-1",
    });
  });
});
