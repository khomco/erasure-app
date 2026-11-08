import { beforeEach, describe, expect, it } from "vitest";
import { HubError, resetCsrfCache } from "./auth";
import {
  fetchOrganization,
  retryAdminQuery,
  searchOrganizations,
  searchUsers,
  type AdminOrganization,
  type AdminOrganizationDetail,
  type AdminUser,
} from "./admin";

const org: AdminOrganization = {
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  name: "Engines Ltd",
  created_at: "2026-11-08T10:00:00Z",
  user_count: 2,
};

const member = {
  id: "8a6e0804-2bd0-4672-b79d-d97027f9071a",
  email: "ada@example.com",
  name: "Ada",
  role: "owner" as const,
  email_verified: true,
  created_at: "2026-11-08T10:00:00Z",
};

const user: AdminUser = { ...member, organization: { id: org.id, name: org.name } };

const detail: AdminOrganizationDetail = {
  organization: { id: org.id, name: org.name, created_at: org.created_at },
  users: [member],
};

interface Call {
  url: string;
  method: string;
  csrf: string | null;
}

/** A fake hub, as in `team.test.ts`: every call is recorded and answered by `status`/`body`. */
function respond(status: number, body?: unknown) {
  const calls: Call[] = [];
  const impl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const headers = (init?.headers ?? {}) as Record<string, string>;
    calls.push({
      url: String(input),
      method: init?.method ?? "GET",
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

describe("admin searches", () => {
  it("searchOrganizations sends the query, encoded, as a GET without CSRF", async () => {
    const f = respond(200, { organizations: [org], truncated: false });
    expect(await searchOrganizations("100% & co", f.impl)).toEqual({
      organizations: [org],
      truncated: false,
    });
    expect(f.calls).toEqual([
      { url: "/api/admin/organizations?q=100%25+%26+co", method: "GET", csrf: null },
    ]);
  });

  it("an empty query is still sent", async () => {
    const f = respond(200, { users: [], truncated: true });
    expect(await searchUsers("", f.impl)).toEqual({ users: [], truncated: true });
    expect(f.calls[0].url).toBe("/api/admin/users?q=");
  });

  it("searchUsers returns users with their organization", async () => {
    const f = respond(200, { users: [user], truncated: false });
    expect((await searchUsers("ada", f.impl)).users).toEqual([user]);
  });

  it("rejects an unexpected body", async () => {
    const f = respond(200, { users: [member], truncated: false });
    const err = await rejection(searchUsers("ada", f.impl));
    expect((err as Error).message).toBe("user search returned an unexpected body (HTTP 200)");
  });

  it("a customer's refusal is a 403 HubError with the hub's message", async () => {
    const f = respond(403, { error: "only vendor admins can use the admin area" });
    const err = await rejection(searchOrganizations("a", f.impl));
    expect(err).toBeInstanceOf(HubError);
    expect((err as HubError).status).toBe(403);
    expect((err as HubError).message).toBe("only vendor admins can use the admin area");
  });
});

describe("admin organization view", () => {
  it("fetchOrganization addresses the organization by id", async () => {
    const f = respond(200, detail);
    expect(await fetchOrganization(org.id, f.impl)).toEqual(detail);
    expect(f.calls).toEqual([
      { url: `/api/admin/organizations/${org.id}`, method: "GET", csrf: null },
    ]);
  });

  it("an unknown organization is a 404 HubError", async () => {
    const f = respond(404, { error: "no such organization" });
    const err = await rejection(fetchOrganization("x/y", f.impl));
    expect((err as HubError).status).toBe(404);
    expect((err as HubError).message).toBe("no such organization");
    expect(f.calls[0].url).toBe("/api/admin/organizations/x%2Fy");
  });
});

describe("retryAdminQuery", () => {
  it("never retries a 4xx and retries other failures twice", () => {
    expect(retryAdminQuery(0, new HubError(403, "no"))).toBe(false);
    expect(retryAdminQuery(0, new HubError(404, "no"))).toBe(false);
    expect(retryAdminQuery(0, new HubError(400, "no"))).toBe(false);
    expect(retryAdminQuery(0, new HubError(500, "boom"))).toBe(true);
    expect(retryAdminQuery(1, new Error("network"))).toBe(true);
    expect(retryAdminQuery(2, new Error("network"))).toBe(false);
  });
});
