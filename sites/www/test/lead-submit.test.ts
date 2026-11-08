// The form script's request and status logic (lead-capture story 1), against
// a fake hub. No network, no browser.
import { describe, expect, it } from "vitest";
import { consentText } from "../src/data/consent";
import {
  CSRF_ERROR,
  NETWORK_ERROR,
  SUCCESS,
  TOO_MANY,
  leadBody,
  parseHubOrigin,
  sentence,
  submitLead,
  tryAgainIn,
  type LeadBody,
} from "../src/lib/lead-submit";

const HUB = "http://localhost:7900";

interface Call {
  url: string;
  method: string;
  credentials: RequestCredentials | undefined;
  csrf: string | null;
  body: unknown;
}

type Answer = { status: number; body?: unknown; headers?: Record<string, string> };

/**
 * A fake hub: `GET /api/auth/csrf` answers `csrf-<n>`; each `POST /api/leads`
 * is answered by `answer(index)`. Every call is recorded.
 */
function fakeHub(answer: (index: number) => Answer | "network-error") {
  const calls: Call[] = [];
  let csrf = 0;
  let posts = 0;
  const impl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const headers = (init?.headers ?? {}) as Record<string, string>;
    const call: Call = {
      url: String(input),
      method: init?.method ?? "GET",
      credentials: init?.credentials,
      csrf: headers["X-CSRF-Token"] ?? null,
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
    };
    calls.push(call);
    if (call.url === `${HUB}/api/auth/csrf`) {
      csrf += 1;
      return new Response(JSON.stringify({ csrf_token: `csrf-${csrf}` }), { status: 200 });
    }
    const a = answer(posts++);
    if (a === "network-error") throw new TypeError("Failed to fetch");
    return new Response(a.body === undefined ? null : JSON.stringify(a.body), {
      status: a.status,
      headers: a.headers,
    });
  }) as typeof fetch;
  return { impl, calls };
}

const body: LeadBody = {
  kind: "early-access",
  name: "Ada",
  email: "ada@example.com",
  company: "Engines",
  role: "Ops",
  segment: "itad",
  message: "",
  consent: true,
  consent_text: consentText("early-access"),
  website: "",
};

describe("parseHubOrigin", () => {
  it("is empty when unset, so the forms stay closed", () => {
    for (const v of [undefined, null, "", "   "]) expect(parseHubOrigin(v)).toBe("");
  });

  it("normalizes an http(s) origin", () => {
    expect(parseHubOrigin("http://localhost:7900")).toBe("http://localhost:7900");
    expect(parseHubOrigin(" http://localhost:7900/ ")).toBe("http://localhost:7900");
    expect(parseHubOrigin("https://Hub.Example.com:443")).toBe("https://hub.example.com");
  });

  it("rejects anything that is not a bare origin", () => {
    for (const bad of [
      "localhost:7900",
      "hub.example.com",
      "ftp://hub.example.com",
      "https://hub.example.com/api",
      "https://hub.example.com/?x=1",
      "https://hub.example.com/#x",
      "https://user@hub.example.com",
    ]) {
      expect(() => parseHubOrigin(bad), bad).toThrow(/PUBLIC_HUB_ORIGIN/);
    }
  });
});

describe("leadBody", () => {
  const data = (entries: [string, string][]) => {
    const d = new FormData();
    for (const [k, v] of entries) d.append(k, v);
    return d;
  };

  it("sends the message with LF line breaks, so the browser's maxlength fits the hub's limit", () => {
    // A textarea's submission value uses CRLF; 2500 lines of "a" plus 2499
    // breaks is 4999 characters as the visitor typed it.
    const typed = Array(2500).fill("a").join("\r\n");
    const b = leadBody("contact", data([["message", typed]]));
    expect(b.message).toBe(Array(2500).fill("a").join("\n"));
    expect(b.message.length).toBe(4999);
    expect(b.message).not.toContain("\r");
  });

  it("builds an early-access body with the site's own consent text", () => {
    const d = data([
      ["name", "Ada"],
      ["email", "ada@example.com"],
      ["company", "Engines"],
      ["role", "Ops"],
      ["segment", "itad"],
      ["message", ""],
      ["website", ""],
      ["consent", "on"],
    ]);
    expect(leadBody("early-access", d)).toEqual(body);
  });

  it("omits role and segment from a contact body, and reads an unticked box as no consent", () => {
    const d = data([
      ["name", "Ada"],
      ["email", "ada@example.com"],
      ["company", "Engines"],
      ["role", "ignored"],
      ["message", "Hello"],
    ]);
    expect(leadBody("contact", d)).toEqual({
      kind: "contact",
      name: "Ada",
      email: "ada@example.com",
      company: "Engines",
      message: "Hello",
      consent: false,
      consent_text: consentText("contact"),
      website: "",
    });
  });
});

describe("submitLead", () => {
  it("fetches the CSRF token, then posts with it, both with credentials", async () => {
    const hub = fakeHub(() => ({ status: 202, body: {} }));
    expect(await submitLead(HUB, body, hub.impl)).toEqual({ ok: true, message: SUCCESS["early-access"] });
    expect(hub.calls).toEqual([
      { url: `${HUB}/api/auth/csrf`, method: "GET", credentials: "include", csrf: null, body: undefined },
      { url: `${HUB}/api/leads`, method: "POST", credentials: "include", csrf: "csrf-1", body },
    ]);
  });

  it("retries once with a fresh token after a CSRF 403", async () => {
    const hub = fakeHub((i) => (i === 0 ? { status: 403, body: { error: CSRF_ERROR } } : { status: 202, body: {} }));
    expect((await submitLead(HUB, body, hub.impl)).ok).toBe(true);
    expect(hub.calls.filter((c) => c.method === "POST").map((c) => c.csrf)).toEqual(["csrf-1", "csrf-2"]);
  });

  it("does not retry a second CSRF 403", async () => {
    const hub = fakeHub(() => ({ status: 403, body: { error: CSRF_ERROR } }));
    const outcome = await submitLead(HUB, body, hub.impl);
    expect(outcome.ok).toBe(false);
    expect(outcome.message).toMatch(/HTTP 403/);
    expect(hub.calls.filter((c) => c.method === "POST")).toHaveLength(2);
  });

  it("shows the hub's 400 message", async () => {
    const hub = fakeHub(() => ({ status: 400, body: { error: "email is not a valid email address" } }));
    expect(await submitLead(HUB, body, hub.impl)).toEqual({
      ok: false,
      message: "Please check the form: email is not a valid email address.",
    });
  });

  it("shows a 429 in the hub's own words, with Retry-After in minutes", async () => {
    const perIp = fakeHub(() => ({
      status: 429,
      body: { error: "too many submissions; try again later" },
      headers: { "Retry-After": "3541" },
    }));
    expect(await submitLead(HUB, body, perIp.impl)).toEqual({
      ok: false,
      message: "Too many submissions; try again later. Try again in 60 minutes.",
    });
    // The email-send limit says something different, and the form says so.
    const mail = fakeHub(() => ({
      status: 429,
      body: { error: "too many emails requested; try again later" },
      headers: { "Retry-After": "90" },
    }));
    expect(await submitLead(HUB, body, mail.impl)).toEqual({
      ok: false,
      message: "Too many emails requested; try again later. Try again in 2 minutes.",
    });
    const bare = fakeHub(() => ({ status: 429 }));
    expect(await submitLead(HUB, body, bare.impl)).toEqual({
      ok: false,
      message: `${TOO_MANY} Try again later.`,
    });
  });

  it("never claims an email was sent", () => {
    for (const text of Object.values(SUCCESS)) {
      expect(text).not.toMatch(/sent|confirmation|inbox/i);
    }
  });

  it("reports a network error", async () => {
    const hub = fakeHub(() => "network-error");
    expect(await submitLead(HUB, body, hub.impl)).toEqual({ ok: false, message: NETWORK_ERROR });
  });

  it("reports an unreachable hub when the CSRF fetch fails", async () => {
    const impl = (async () => {
      throw new TypeError("Failed to fetch");
    }) as typeof fetch;
    expect(await submitLead(HUB, body, impl)).toEqual({ ok: false, message: NETWORK_ERROR });
  });
});

describe("sentence", () => {
  it("capitalises and ends with a full stop", () => {
    expect(sentence("too many; try again later")).toBe("Too many; try again later.");
    expect(sentence(" done. ")).toBe("Done.");
    expect(sentence("")).toBe("");
  });
});

describe("tryAgainIn", () => {
  it("rounds up to whole minutes", () => {
    expect(tryAgainIn("1")).toBe("Try again in 1 minute.");
    expect(tryAgainIn("60")).toBe("Try again in 1 minute.");
    expect(tryAgainIn("61")).toBe("Try again in 2 minutes.");
    expect(tryAgainIn(null)).toBe("Try again later.");
    expect(tryAgainIn("Wed, 21 Oct 2026 07:28:00 GMT")).toBe("Try again later.");
  });
});
