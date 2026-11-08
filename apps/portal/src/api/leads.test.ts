import { beforeEach, describe, expect, it } from "vitest";
import { CSRF_ERROR, HubError, resetCsrfCache } from "./auth";
import { unsubscribeLead } from "./leads";

interface Call {
  url: string;
  method: string;
  body: unknown;
  csrf: string | null;
}

type Answer = { status: number; body?: unknown };

/**
 * A fake hub, as in `auth.test.ts`: `GET /api/auth/csrf` answers
 * `csrf-<n>`; every other call is recorded and answered by `answer`.
 */
function fakeHub(answer: (index: number) => Answer) {
  const calls: Call[] = [];
  let csrfFetches = 0;
  const impl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const method = init?.method ?? "GET";
    if (url === "/api/auth/csrf" && method === "GET") {
      csrfFetches += 1;
      return new Response(JSON.stringify({ csrf_token: `csrf-${csrfFetches}` }), {
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
    const { status, body } = answer(calls.length - 1);
    return new Response(body === undefined ? null : JSON.stringify(body), {
      status,
      headers: body === undefined ? {} : { "Content-Type": "application/json" },
    });
  }) as typeof fetch;
  return { impl, calls };
}

beforeEach(() => resetCsrfCache());

describe("unsubscribeLead", () => {
  it("posts the token with the CSRF header", async () => {
    const f = fakeHub(() => ({ status: 200, body: { unsubscribed: true } }));
    await unsubscribeLead("tok", f.impl);
    expect(f.calls).toEqual([
      { url: "/api/leads/unsubscribe", method: "POST", body: { token: "tok" }, csrf: "csrf-1" },
    ]);
  });

  it("surfaces the hub's 404 for an invalid link", async () => {
    const f = fakeHub(() => ({ status: 404, body: { error: "this unsubscribe link is not valid" } }));
    const err = await unsubscribeLead("bad", f.impl).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(HubError);
    expect((err as HubError).status).toBe(404);
    expect((err as HubError).message).toBe("this unsubscribe link is not valid");
  });

  it("retries once with a fresh token after a CSRF rejection", async () => {
    const f = fakeHub((i) =>
      i === 0 ? { status: 403, body: { error: CSRF_ERROR } } : { status: 200, body: { unsubscribed: true } },
    );
    await unsubscribeLead("tok", f.impl);
    expect(f.calls.map((c) => c.csrf)).toEqual(["csrf-1", "csrf-2"]);
  });

  it("rejects an unexpected success body", async () => {
    const f = fakeHub(() => ({ status: 200, body: {} }));
    await expect(unsubscribeLead("tok", f.impl)).rejects.toThrow(/unexpected body/);
  });
});
