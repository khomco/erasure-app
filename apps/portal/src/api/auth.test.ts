import { describe, expect, it } from "vitest";
import {
  HubError,
  fetchMe,
  resendVerification,
  signIn,
  signOut,
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
}

/** A fetch that records its call and answers `status` with `body`. */
function respond(status: number, body?: string) {
  const calls: Call[] = [];
  const impl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({
      url: String(input),
      method: init?.method ?? "GET",
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
    });
    return new Response(body ?? null, {
      status,
      headers: body ? { "Content-Type": "application/json" } : {},
    });
  }) as typeof fetch;
  return { impl, calls };
}

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
    expect(f.calls).toEqual([{ url: "/api/auth/sign-up", method: "POST", body: input }]);
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
      },
    ]);
  });

  it("surfaces the generic 401", async () => {
    const f = hubError(401, "invalid email or password");
    await expectHubError(signIn("a@b.co", "x", f.impl), 401, "invalid email or password");
  });

  it("falls back to a generic message for a non-JSON error", async () => {
    const f = respond(502, undefined);
    await expectHubError(signIn("a@b.co", "x", f.impl), 502, "request failed (HTTP 502)");
  });
});

describe("signOut", () => {
  it("posts and resolves on 204", async () => {
    const f = respond(204);
    await expect(signOut(f.impl)).resolves.toBeUndefined();
    expect(f.calls).toEqual([{ url: "/api/auth/sign-out", method: "POST", body: undefined }]);
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
    expect(f.calls[0]).toMatchObject({ url: "/api/auth/me", method: "GET" });
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
      { url: "/api/auth/verify-email", method: "POST", body: { token: "tok" } },
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
      { url: "/api/auth/verification-email", method: "POST", body: undefined },
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
