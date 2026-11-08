import { beforeEach, describe, expect, it } from "vitest";
import {
  CSRF_ERROR,
  HubError,
  emailRequestErrorMessage,
  fetchMe,
  requestMagicLink,
  requestPasswordReset,
  resendVerification,
  resetCsrfCache,
  resetPassword,
  signIn,
  signInErrorMessage,
  signInWithMagicLink,
  signOut,
  signOutEverywhere,
  signUp,
  verifyEmail,
  type Account,
} from "./auth";

const account: Account = {
  user: {
    id: "8a6e0804-2bd0-4672-b79d-d97027f9071a",
    email: "ada@example.com",
    name: "Ada",
    role: "owner",
    email_verified: false,
  },
  organization: { id: "0f8fad5b-d9cb-469f-a165-70867728950e", name: "Engines Ltd" },
};

interface Call {
  url: string;
  method: string;
  body: unknown;
  /** The `X-CSRF-Token` header sent, or `null`. */
  csrf: string | null;
}

type Answer = { status: number; body?: string; headers?: Record<string, string> };

/**
 * A fake hub. `GET /api/auth/csrf` answers `csrf-1`, `csrf-2`, … (one per
 * call, counted in `csrfFetches`); every other call is recorded in `calls`
 * and answered by `answer` (given the call's index among them).
 */
function fakeHub(answer: (call: Call, index: number) => Answer) {
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
    const call: Call = {
      url,
      method,
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
      csrf: headers["X-CSRF-Token"] ?? null,
    };
    calls.push(call);
    const { status, body, headers: extra } = answer(call, calls.length - 1);
    return new Response(body ?? null, {
      status,
      headers: { ...(body ? { "Content-Type": "application/json" } : {}), ...extra },
    });
  }) as typeof fetch;
  return {
    impl,
    calls,
    get csrfFetches() {
      return csrfFetches;
    },
  };
}

/** A fake hub answering every non-CSRF call with `status` and `body`. */
function respond(status: number, body?: string) {
  return fakeHub(() => ({ status, body }));
}

beforeEach(() => resetCsrfCache());

function hubError(status: number, message: string) {
  return respond(status, JSON.stringify({ error: message }));
}

async function expectHubError(p: Promise<unknown>, status: number, message: string) {
  const err = await p.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(err).toBeInstanceOf(HubError);
  expect((err as HubError).status).toBe(status);
  expect((err as HubError).message).toBe(message);
}

describe("signUp", () => {
  const input = {
    email: "ada@example.com",
    password: "correct horse battery",
    name: "Ada",
    organization_name: "Engines Ltd",
  };

  it("posts the input and returns the account", async () => {
    const f = respond(201, JSON.stringify({ ...account, verification_email_sent: true }));
    const r = await signUp(input, f.impl);
    expect(r).toEqual({ ...account, verification_email_sent: true });
    expect(f.calls).toEqual([
      { url: "/api/auth/sign-up", method: "POST", body: input, csrf: "csrf-1" },
    ]);
  });

  it("surfaces a 409 duplicate email", async () => {
    const f = hubError(409, "an account with this email already exists");
    await expectHubError(signUp(input, f.impl), 409, "an account with this email already exists");
  });

  it("surfaces a 400 validation message", async () => {
    const f = hubError(400, "password must be at least 12 characters");
    await expectHubError(signUp(input, f.impl), 400, "password must be at least 12 characters");
  });

  it("rejects an unexpected success body", async () => {
    const f = respond(201, JSON.stringify(account));
    await expect(signUp(input, f.impl)).rejects.toThrow(/unexpected body/);
  });
});

describe("signIn", () => {
  it("posts credentials and returns the account", async () => {
    const f = respond(200, JSON.stringify(account));
    expect(await signIn("ada@example.com", "pw", f.impl)).toEqual(account);
    expect(f.calls).toEqual([
      {
        url: "/api/auth/sign-in",
        method: "POST",
        body: { email: "ada@example.com", password: "pw" },
        csrf: "csrf-1",
      },
    ]);
  });

  it("surfaces the generic 401", async () => {
    const f = hubError(401, "invalid email or password");
    await expectHubError(signIn("a@b.co", "x", f.impl), 401, "invalid email or password");
  });

  it("surfaces a 429 lockout with the hub's message", async () => {
    const f = hubError(429, "too many failed sign-in attempts; try again later");
    await expectHubError(
      signIn("a@b.co", "x", f.impl),
      429,
      "too many failed sign-in attempts; try again later",
    );
  });

  it("falls back to a generic message for a non-JSON error", async () => {
    const f = respond(502, undefined);
    await expectHubError(signIn("a@b.co", "x", f.impl), 502, "request failed (HTTP 502)");
  });

  it("carries Retry-After on a 429 and says how many minutes to wait", async () => {
    const locked = (retryAfter?: string) =>
      fakeHub(() => ({
        status: 429,
        body: JSON.stringify({ error: "too many failed sign-in attempts; try again later" }),
        headers:
          retryAfter === undefined ? undefined : ({ "Retry-After": retryAfter } as Record<string, string>),
      }));
    const errorFor = (retryAfter?: string) =>
      signIn("a@b.co", "x", locked(retryAfter).impl).then(
        () => undefined,
        (e: unknown) => e,
      );

    const err = await errorFor("895");
    expect(err).toBeInstanceOf(HubError);
    expect((err as HubError).retryAfterSecs).toBe(895);
    expect(signInErrorMessage(err)).toBe(
      "Too many failed sign-in attempts. Try again in 15 minutes.",
    );
    expect(signInErrorMessage(await errorFor("1"))).toBe(
      "Too many failed sign-in attempts. Try again in 1 minute.",
    );
    expect(signInErrorMessage(await errorFor("61"))).toBe(
      "Too many failed sign-in attempts. Try again in 2 minutes.",
    );
    // No usable Retry-After: the hub's own message.
    for (const header of [undefined, "Wed, 21 Oct 2015 07:28:00 GMT"]) {
      const e = await errorFor(header);
      expect((e as HubError).retryAfterSecs).toBeNull();
      expect(signInErrorMessage(e)).toBe("too many failed sign-in attempts; try again later");
    }
  });

  it("describes other errors by their message", () => {
    expect(signInErrorMessage(new HubError(401, "invalid email or password"))).toBe(
      "invalid email or password",
    );
    expect(signInErrorMessage("nope")).toBe("Something went wrong");
  });
});

describe("signOut", () => {
  it("posts and resolves on 204", async () => {
    const f = respond(204);
    await expect(signOut(f.impl)).resolves.toBeUndefined();
    expect(f.calls).toEqual([
      { url: "/api/auth/sign-out", method: "POST", body: undefined, csrf: "csrf-1" },
    ]);
  });

  it("surfaces an error", async () => {
    const f = hubError(500, "internal error");
    await expectHubError(signOut(f.impl), 500, "internal error");
  });
});

describe("fetchMe", () => {
  it("returns the account when signed in", async () => {
    const f = respond(200, JSON.stringify(account));
    expect(await fetchMe(f.impl)).toEqual(account);
    expect(f.calls).toEqual([{ url: "/api/auth/me", method: "GET", body: undefined, csrf: null }]);
    expect(f.csrfFetches).toBe(0);
  });

  it("returns null when not signed in", async () => {
    const f = hubError(401, "not signed in");
    expect(await fetchMe(f.impl)).toBeNull();
  });

  it("throws other errors", async () => {
    const f = hubError(500, "internal error");
    await expectHubError(fetchMe(f.impl), 500, "internal error");
  });

  it("throws on a body that is not an account", async () => {
    const f = respond(200, JSON.stringify({ user: { id: 1 } }));
    await expect(fetchMe(f.impl)).rejects.toThrow(/unexpected body/);
  });
});

describe("verifyEmail", () => {
  it("posts the token", async () => {
    const f = respond(200, JSON.stringify({ email_verified: true }));
    await verifyEmail("tok", f.impl);
    expect(f.calls).toEqual([
      { url: "/api/auth/verify-email", method: "POST", body: { token: "tok" }, csrf: "csrf-1" },
    ]);
  });

  it("surfaces an invalid or expired link", async () => {
    const f = hubError(400, "this verification link is invalid or has expired");
    await expectHubError(
      verifyEmail("tok", f.impl),
      400,
      "this verification link is invalid or has expired",
    );
  });
});

describe("resendVerification", () => {
  it("posts and resolves on 202", async () => {
    const f = respond(202, JSON.stringify({ verification_email_sent: true }));
    await resendVerification(f.impl);
    expect(f.calls).toEqual([
      { url: "/api/auth/verification-email", method: "POST", body: undefined, csrf: "csrf-1" },
    ]);
  });

  it.each([
    [401, "not signed in"],
    [409, "this email address is already verified"],
    [502, "the verification email could not be sent; try again later"],
  ])("surfaces %i", async (status, message) => {
    await expectHubError(resendVerification(hubError(status, message).impl), status, message);
  });
});

describe("signOutEverywhere", () => {
  it("posts and resolves on 204", async () => {
    const f = respond(204);
    await expect(signOutEverywhere(f.impl)).resolves.toBeUndefined();
    expect(f.calls).toEqual([
      { url: "/api/auth/sign-out-everywhere", method: "POST", body: undefined, csrf: "csrf-1" },
    ]);
  });

  it("surfaces 401 when not signed in", async () => {
    const f = hubError(401, "not signed in");
    await expectHubError(signOutEverywhere(f.impl), 401, "not signed in");
  });
});

describe("CSRF token", () => {
  const posts: [string, (f: typeof fetch) => Promise<unknown>][] = [
    ["sign-up", (f) => signUp({ email: "a", password: "b", name: "c", organization_name: "d" }, f)],
    ["sign-in", (f) => signIn("a@b.co", "pw", f)],
    ["sign-out", (f) => signOut(f)],
    ["sign-out-everywhere", (f) => signOutEverywhere(f)],
    ["verify-email", (f) => verifyEmail("tok", f)],
    ["verification-email", (f) => resendVerification(f)],
    ["magic-link", (f) => requestMagicLink("a@b.co", f)],
    ["magic-link/sign-in", (f) => signInWithMagicLink("tok", f)],
    ["password-reset", (f) => requestPasswordReset("a@b.co", f)],
    ["password-reset/complete", (f) => resetPassword("tok", "pw", f)],
  ];

  it.each(posts)("%s sends X-CSRF-Token from /api/auth/csrf", async (_, call) => {
    const f = respond(500, JSON.stringify({ error: "internal error" }));
    await call(f.impl).catch(() => undefined);
    expect(f.csrfFetches).toBe(1);
    expect(f.calls).toHaveLength(1);
    expect(f.calls[0].method).toBe("POST");
    expect(f.calls[0].csrf).toBe("csrf-1");
  });

  it("is fetched once and reused across POSTs", async () => {
    const f = respond(204);
    await signOut(f.impl);
    await signOut(f.impl);
    await resendVerification(f.impl).catch(() => undefined);
    expect(f.csrfFetches).toBe(1);
    expect(f.calls.map((c) => c.csrf)).toEqual(["csrf-1", "csrf-1", "csrf-1"]);
  });

  it("refetches and retries once after a CSRF 403", async () => {
    const f = fakeHub((_, i) =>
      i === 0
        ? { status: 403, body: JSON.stringify({ error: CSRF_ERROR }) }
        : { status: 200, body: JSON.stringify(account) },
    );
    expect(await signIn("ada@example.com", "pw", f.impl)).toEqual(account);
    expect(f.csrfFetches).toBe(2);
    expect(f.calls.map((c) => [c.url, c.csrf, c.body])).toEqual([
      ["/api/auth/sign-in", "csrf-1", { email: "ada@example.com", password: "pw" }],
      ["/api/auth/sign-in", "csrf-2", { email: "ada@example.com", password: "pw" }],
    ]);
  });

  it("gives up after a second CSRF 403", async () => {
    const f = hubError(403, CSRF_ERROR);
    await expectHubError(signOut(f.impl), 403, CSRF_ERROR);
    expect(f.calls).toHaveLength(2);
    expect(f.csrfFetches).toBe(2);
  });

  it("does not retry other 403s", async () => {
    const f = hubError(403, "forbidden");
    await expectHubError(signOut(f.impl), 403, "forbidden");
    expect(f.calls).toHaveLength(1);
    expect(f.csrfFetches).toBe(1);
  });

  it("does not cache a failed token fetch", async () => {
    let fail = true;
    const calls: string[] = [];
    const impl = (async (input: RequestInfo | URL) => {
      const url = String(input);
      calls.push(url);
      if (url === "/api/auth/csrf") {
        if (fail) return new Response(null, { status: 502 });
        return new Response(JSON.stringify({ csrf_token: "good" }), { status: 200 });
      }
      return new Response(null, { status: 204 });
    }) as typeof fetch;
    await expectHubError(signOut(impl), 502, "request failed (HTTP 502)");
    fail = false;
    await signOut(impl);
    expect(calls).toEqual(["/api/auth/csrf", "/api/auth/csrf", "/api/auth/sign-out"]);
  });
});

describe("requestMagicLink and requestPasswordReset", () => {
  it.each([
    ["/api/auth/magic-link", requestMagicLink],
    ["/api/auth/password-reset", requestPasswordReset],
  ])("%s posts the email and resolves on 202", async (url, call) => {
    const f = respond(202, "{}");
    await expect(call("ada@example.com", f.impl)).resolves.toBeUndefined();
    expect(f.calls).toEqual([
      { url, method: "POST", body: { email: "ada@example.com" }, csrf: "csrf-1" },
    ]);
  });

  it.each([requestMagicLink, requestPasswordReset])(
    "surfaces the email-send limit with its Retry-After",
    async (call) => {
      const f = fakeHub(() => ({
        status: 429,
        body: JSON.stringify({ error: "too many emails requested; try again later" }),
        headers: { "Retry-After": "3600" },
      }));
      const err = await call("a@b.co", f.impl).then(
        () => undefined,
        (e: unknown) => e,
      );
      expect((err as HubError).status).toBe(429);
      expect((err as HubError).retryAfterSecs).toBe(3600);
      expect(emailRequestErrorMessage(err)).toBe(
        "Too many emails requested. Try again in 60 minutes.",
      );
    },
  );

  it("surfaces a 400 for an invalid email", async () => {
    const f = hubError(400, "email is not a valid email address");
    await expectHubError(
      requestMagicLink("nope", f.impl),
      400,
      "email is not a valid email address",
    );
  });
});

describe("signInWithMagicLink", () => {
  it("posts the token and returns the account", async () => {
    const f = respond(200, JSON.stringify(account));
    expect(await signInWithMagicLink("tok", f.impl)).toEqual(account);
    expect(f.calls).toEqual([
      {
        url: "/api/auth/magic-link/sign-in",
        method: "POST",
        body: { token: "tok" },
        csrf: "csrf-1",
      },
    ]);
  });

  it("surfaces an invalid or expired link", async () => {
    const f = hubError(400, "this sign-in link is invalid or has expired");
    await expectHubError(
      signInWithMagicLink("tok", f.impl),
      400,
      "this sign-in link is invalid or has expired",
    );
  });

  it("rejects a body that is not an account", async () => {
    const f = respond(200, JSON.stringify({ ok: true }));
    await expect(signInWithMagicLink("tok", f.impl)).rejects.toThrow(/unexpected body/);
  });
});

describe("resetPassword", () => {
  it("posts the token and new password", async () => {
    const f = respond(200, JSON.stringify({ password_reset: true }));
    await expect(resetPassword("tok", "a new passphrase", f.impl)).resolves.toBeUndefined();
    expect(f.calls).toEqual([
      {
        url: "/api/auth/password-reset/complete",
        method: "POST",
        body: { token: "tok", password: "a new passphrase" },
        csrf: "csrf-1",
      },
    ]);
  });

  it.each([
    "this password reset link is invalid or has expired",
    "password must be at least 12 characters",
  ])("surfaces the 400 %s", async (message) => {
    await expectHubError(resetPassword("tok", "x", hubError(400, message).impl), 400, message);
  });

  it("rejects an unexpected success body", async () => {
    const f = respond(200, JSON.stringify({}));
    await expect(resetPassword("tok", "x", f.impl)).rejects.toThrow(/unexpected body/);
  });
});

describe("emailRequestErrorMessage", () => {
  it("rounds the wait up to whole minutes", () => {
    const limited = (secs: number) => new HubError(429, "too many emails requested", secs);
    expect(emailRequestErrorMessage(limited(1))).toBe(
      "Too many emails requested. Try again in 1 minute.",
    );
    expect(emailRequestErrorMessage(limited(3590))).toBe(
      "Too many emails requested. Try again in 60 minutes.",
    );
  });

  it("describes other errors by their message", () => {
    expect(emailRequestErrorMessage(new HubError(429, "too many emails requested"))).toBe(
      "too many emails requested",
    );
    expect(emailRequestErrorMessage(new HubError(409, "an account with this email already exists"))).toBe(
      "an account with this email already exists",
    );
    expect(emailRequestErrorMessage(undefined)).toBe("Something went wrong");
  });
});
