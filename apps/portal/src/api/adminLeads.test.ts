import { beforeEach, describe, expect, it } from "vitest";
import { HubError, resetCsrfCache } from "./auth";
import {
  NO_FILTERS,
  addLeadNote,
  attachmentName,
  eraseLead,
  exportLeads,
  fetchLead,
  listLeads,
  setLeadStatus,
  type AdminLead,
  type LeadDetail,
  type LeadNote,
} from "./adminLeads";

const lead: AdminLead = {
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  email: "ada@example.com",
  name: "Ada Lovelace",
  company: "Engines Ltd",
  role: "Head of operations",
  segment: "itad",
  status: "new",
  created_at: "2026-11-08T10:00:00Z",
  updated_at: "2026-11-08T10:00:00Z",
  last_submitted_at: "2026-11-08T10:00:00Z",
  submission_count: 1,
  unsubscribed_at: null,
};

const note: LeadNote = {
  id: "8a6e0804-2bd0-4672-b79d-d97027f9071a",
  body: "called, follow up",
  created_at: "2026-11-08T11:00:00Z",
  author: { id: "5b3c4c43-6a4f-4b8e-9d0e-3c3f0a1d9b2e", name: "Vera", email: "vera@example.com" },
};

const detail: LeadDetail = {
  lead,
  submissions: [
    {
      id: "c2f1d7a4-6a2e-4f6e-9f7b-1d1b1e1f1a1b",
      kind: "early-access",
      message: null,
      consent_text: "I agree.",
      consented_at: "2026-11-08T10:00:00Z",
    },
  ],
  notes: [note, { ...note, id: "x", author: null }],
};

interface Call {
  url: string;
  method: string;
  body: unknown;
  csrf: string | null;
}

/**
 * A fake hub, as in `team.test.ts`: `GET /api/auth/csrf` answers `csrf-1`;
 * every other call is recorded and answered with `status`, `body` (JSON
 * unless a string) and `headers`.
 */
function respond(status: number, body?: unknown, headers: Record<string, string> = {}) {
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
    const sent = (init?.headers ?? {}) as Record<string, string>;
    calls.push({
      url,
      method,
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
      csrf: sent["X-CSRF-Token"] ?? null,
    });
    const text = body === undefined ? null : typeof body === "string" ? body : JSON.stringify(body);
    return new Response(text, {
      status,
      headers: { ...(typeof body === "object" ? { "Content-Type": "application/json" } : {}), ...headers },
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

describe("lead list", () => {
  it("sends only the filters that are set, as a GET without CSRF", async () => {
    const f = respond(200, { leads: [lead], truncated: false });
    const got = await listLeads({ ...NO_FILTERS, segment: "none", from: "2026-11-01" }, f.impl);
    expect(got).toEqual({ leads: [lead], truncated: false });
    expect(f.calls).toEqual([
      {
        url: "/api/admin/leads?segment=none&from=2026-11-01",
        method: "GET",
        body: undefined,
        csrf: null,
      },
    ]);
  });

  it("no filters is the bare path", async () => {
    const f = respond(200, { leads: [], truncated: true });
    await listLeads(NO_FILTERS, f.impl);
    expect(f.calls[0].url).toBe("/api/admin/leads");
  });

  it("accepts an unsubscribed contact-only lead and rejects an unknown status", async () => {
    const contact = { ...lead, role: null, segment: null, unsubscribed_at: "2026-11-09T08:00:00Z" };
    const ok = respond(200, { leads: [contact], truncated: false });
    expect((await listLeads(NO_FILTERS, ok.impl)).leads).toEqual([contact]);
    const bad = respond(200, { leads: [{ ...lead, status: "won" }], truncated: false });
    const err = await rejection(listLeads(NO_FILTERS, bad.impl));
    expect((err as Error).message).toBe("lead list returned an unexpected body (HTTP 200)");
  });

  it("a bad filter is a 400 HubError with the hub's message", async () => {
    const f = respond(400, { error: "bad filter" });
    const err = await rejection(listLeads({ ...NO_FILTERS, status: "open" }, f.impl));
    expect(err).toBeInstanceOf(HubError);
    expect((err as HubError).status).toBe(400);
    expect((err as HubError).message).toBe("bad filter");
  });
});

describe("one lead", () => {
  it("fetchLead addresses the lead by encoded id", async () => {
    const f = respond(200, detail);
    expect(await fetchLead(lead.id, f.impl)).toEqual(detail);
    const missing = respond(404, { error: "no such sales lead" });
    const err = await rejection(fetchLead("x/y", missing.impl));
    expect((err as HubError).status).toBe(404);
    expect(missing.calls[0].url).toBe("/api/admin/leads/x%2Fy");
  });

  it("setLeadStatus posts the status with CSRF and returns the lead", async () => {
    const updated = { ...lead, status: "contacted" as const };
    const f = respond(200, { lead: updated });
    expect(await setLeadStatus(lead.id, "contacted", f.impl)).toEqual(updated);
    expect(f.calls).toEqual([
      {
        url: `/api/admin/leads/${lead.id}/status`,
        method: "POST",
        body: { status: "contacted" },
        csrf: "csrf-1",
      },
    ]);
  });

  it("addLeadNote posts the body and returns the note", async () => {
    const f = respond(201, { note });
    expect(await addLeadNote(lead.id, "called, follow up", f.impl)).toEqual(note);
    expect(f.calls[0]).toEqual({
      url: `/api/admin/leads/${lead.id}/notes`,
      method: "POST",
      body: { body: "called, follow up" },
      csrf: "csrf-1",
    });
  });

  it("eraseLead posts with CSRF", async () => {
    const f = respond(200, {});
    await eraseLead(lead.id, f.impl);
    expect(f.calls).toEqual([
      { url: `/api/admin/leads/${lead.id}/erase`, method: "POST", body: {}, csrf: "csrf-1" },
    ]);
  });
});

describe("export", () => {
  it("posts the set filters with CSRF and returns the CSV and the hub's file name", async () => {
    // The hub's byte-order mark must reach the saved file untouched.
    const csv = "\ufeffid,email\r\n";
    const f = respond(200, csv, {
      "Content-Type": "text/csv; charset=utf-8",
      "Content-Disposition": 'attachment; filename="sales-leads-2026-11-08.csv"',
    });
    const out = await exportLeads({ ...NO_FILTERS, status: "new" }, f.impl);
    expect(out.filename).toBe("sales-leads-2026-11-08.csv");
    const bytes = new Uint8Array(await out.blob.arrayBuffer());
    expect([...bytes.slice(0, 3)]).toEqual([0xef, 0xbb, 0xbf]);
    expect(bytes).toEqual(new TextEncoder().encode(csv));
    expect(f.calls).toEqual([
      { url: "/api/admin/leads/export", method: "POST", body: { status: "new" }, csrf: "csrf-1" },
    ]);
  });

  it("a refusal is a HubError", async () => {
    const f = respond(403, { error: "only vendor admins can use the admin area" });
    const err = await rejection(exportLeads(NO_FILTERS, f.impl));
    expect((err as HubError).status).toBe(403);
  });

  it("attachmentName reads the quoted file name", () => {
    expect(attachmentName('attachment; filename="a.csv"')).toBe("a.csv");
    expect(attachmentName("attachment")).toBeNull();
    expect(attachmentName(null)).toBeNull();
  });
});
