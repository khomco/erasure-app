import { beforeEach, describe, expect, it } from "vitest";
import { HubError, resetCsrfCache } from "./auth";
import {
  isDownloadLink,
  isDownloadRelease,
  isNotEntitled,
  NOT_ENTITLED,
  listDownloads,
  requestDownloadLink,
  type DownloadRelease,
} from "./downloads";

const SHA = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const ARTIFACT = "7c9e6679-7425-40de-944b-e07fc1f90ae7";

const release: DownloadRelease = {
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  version: "0.2.0",
  notes: "First build.",
  channel: "stable",
  published_at: "2026-11-08T10:00:00Z",
  artifacts: [
    { id: ARTIFACT, platform: "linux-x86_64", filename: "wipestation", size_bytes: 3, sha256: SHA },
  ],
};

const link = {
  url: `/api/downloads/${ARTIFACT}/file?user=u&expires=1&sig=s`,
  expires_at: "2026-11-08T10:05:00Z",
};

interface Call {
  url: string;
  method: string;
  csrf: string | null;
}

/** A fake hub, as in `stations.test.ts`. */
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
    const sent = (init?.headers ?? {}) as Record<string, string>;
    calls.push({ url, method, csrf: sent["X-CSRF-Token"] ?? null });
    return new Response(body === undefined ? null : JSON.stringify(body), {
      status,
      headers: { "Content-Type": "application/json" },
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

describe("guards", () => {
  it("accepts the hub's shapes and refuses others", () => {
    expect(isDownloadRelease(release)).toBe(true);
    expect(isDownloadRelease({ ...release, channel: "beta" })).toBe(false);
    expect(
      isDownloadRelease({ ...release, artifacts: [{ ...release.artifacts[0], sha256: SHA.toUpperCase() }] }),
    ).toBe(false);
    expect(isDownloadRelease({ ...release, published_at: null })).toBe(false);
  });

  it("accepts only links to the hub's own download path", () => {
    expect(isDownloadLink(link)).toBe(true);
    expect(isDownloadLink({ ...link, url: "https://evil.example/api/downloads/x" })).toBe(false);
    expect(isDownloadLink({ ...link, url: "//evil.example/x" })).toBe(false);
    expect(isDownloadLink({ url: link.url })).toBe(false);
    // Only the file route of one artifact, not another path under the prefix.
    expect(isDownloadLink({ ...link, url: "/api/downloads/../admin/releases?x=1" })).toBe(false);
    expect(isDownloadLink({ ...link, url: `/api/downloads/${ARTIFACT}/link?x=1` })).toBe(false);
    expect(isDownloadLink({ ...link, url: "/api/downloads/not-a-uuid/file?x=1" })).toBe(false);
  });

  it("recognises the not-entitled refusal", () => {
    expect(isNotEntitled(new HubError(403, NOT_ENTITLED))).toBe(true);
    expect(isNotEntitled(new HubError(401, NOT_ENTITLED))).toBe(false);
    expect(isNotEntitled(new HubError(401, "not signed in"))).toBe(false);
    // A CSRF refusal is a 403 too, but not a reason to buy a licence.
    expect(isNotEntitled(new HubError(403, "missing or invalid CSRF token"))).toBe(false);
    expect(isNotEntitled(new Error("x"))).toBe(false);
  });
});

describe("calls", () => {
  it("lists with a GET without CSRF", async () => {
    const f = respond(200, { releases: [release] });
    expect(await listDownloads(f.impl)).toEqual({ releases: [release] });
    expect(f.calls).toEqual([{ url: "/api/downloads", method: "GET", csrf: null }]);
  });

  it("surfaces a 403 as a not-entitled HubError", async () => {
    const f = respond(403, { error: NOT_ENTITLED });
    const err = await rejection(listDownloads(f.impl));
    expect(isNotEntitled(err)).toBe(true);
    expect((err as HubError).message).toBe(NOT_ENTITLED);
  });

  it("requests a link with CSRF, by encoded id", async () => {
    const f = respond(200, link);
    expect(await requestDownloadLink(ARTIFACT, f.impl)).toEqual(link);
    expect(f.calls).toEqual([
      { url: `/api/downloads/${ARTIFACT}/link`, method: "POST", csrf: "csrf-1" },
    ]);
    const missing = respond(404, { error: "no such download" });
    const err = await rejection(requestDownloadLink("x/y", missing.impl));
    expect((err as HubError).status).toBe(404);
    expect(missing.calls[0].url).toBe("/api/downloads/x%2Fy/link");
    const elsewhere = respond(200, { ...link, url: "https://evil.example/" });
    expect(await rejection(requestDownloadLink(ARTIFACT, elsewhere.impl))).toBeInstanceOf(Error);
  });
});
