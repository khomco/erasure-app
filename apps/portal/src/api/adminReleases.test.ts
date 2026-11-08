import { beforeEach, describe, expect, it } from "vitest";
import { HubError, resetCsrfCache } from "./auth";
import {
  ARTIFACT_MAX_BYTES,
  CHANNELS,
  createRelease,
  fetchRelease,
  fileProblem,
  formatSize,
  isFileName,
  isPlatformLabel,
  listReleaseDownloads,
  listReleases,
  publishRelease,
  removeArtifact,
  uploadArtifact,
  withArtifact,
  withdrawRelease,
  type Release,
  type ReleaseArtifact,
  type ReleaseDownload,
} from "./adminReleases";

const SHA = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

const artifact: ReleaseArtifact = {
  id: "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  platform: "linux-x86_64",
  filename: "wipestation",
  size_bytes: 3,
  sha256: SHA,
  created_at: "2026-11-08T10:01:00Z",
};

const release: Release = {
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  version: "0.2.0",
  notes: "First build.",
  channel: "stable",
  status: "draft",
  created_at: "2026-11-08T10:00:00Z",
  published_at: null,
  withdrawn_at: null,
  artifacts: [],
};

interface Call {
  url: string;
  method: string;
  body: unknown;
  contentType: string | null;
  csrf: string | null;
}

/** A fake hub, as in `adminOfferings.test.ts`, that also records raw bodies. */
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
    calls.push({
      url,
      method,
      body: typeof init?.body === "string" ? JSON.parse(init.body) : init?.body,
      contentType: sent["Content-Type"] ?? null,
      csrf: sent["X-CSRF-Token"] ?? null,
    });
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

describe("pure helpers", () => {
  it("mirrors the hub's channels", () => {
    expect(CHANNELS).toEqual(["stable", "preview"]);
    expect(ARTIFACT_MAX_BYTES).toBe(1073741824);
  });

  it("checks platform labels as the hub does", () => {
    for (const ok of ["linux-x86_64", "a", "windows.arm64", "0", "a".repeat(64)]) {
      expect(isPlatformLabel(ok), ok).toBe(true);
    }
    for (const bad of ["", "Linux", "-x", ".x", "linux x86", "linux/x86", "a".repeat(65)]) {
      expect(isPlatformLabel(bad), bad).toBe(false);
    }
  });

  it("checks file names as the hub does", () => {
    for (const ok of ["wipestation", "wipestation-0.2.0.tar.gz", "Setup+x64.msi", "-a"]) {
      expect(isFileName(ok), ok).toBe(true);
    }
    for (const bad of ["", ".hidden", "..", "a/b", "a b", "naïve", "a".repeat(129)]) {
      expect(isFileName(bad), bad).toBe(false);
    }
  });

  it("says why a file cannot be uploaded", () => {
    expect(fileProblem({ name: "wipestation", size: 10 })).toBeNull();
    expect(fileProblem({ name: "wipestation", size: 0 })).toBe("The file is empty.");
    expect(fileProblem({ name: "w", size: ARTIFACT_MAX_BYTES + 1 })).toMatch(/1 GiB/);
    expect(fileProblem({ name: "my build", size: 1 })).toMatch(/Rename the file/);
  });

  it("formats sizes with the exact byte count", () => {
    expect(formatSize(1)).toBe("1 byte");
    expect(formatSize(1023)).toBe("1,023 bytes");
    expect(formatSize(1536)).toBe("1.5 KiB (1,536 bytes)");
    expect(formatSize(250_000_000)).toBe("238 MiB (250,000,000 bytes)");
    expect(formatSize(ARTIFACT_MAX_BYTES)).toBe("1.0 GiB (1,073,741,824 bytes)");
  });

  it("adds an artifact in the hub's platform order", () => {
    const mac = { ...artifact, id: "b", platform: "macos-aarch64" };
    const win = { ...artifact, id: "c", platform: "windows-x86_64" };
    const r = withArtifact(withArtifact(withArtifact(release, win), artifact), mac);
    expect(r.artifacts.map((a) => a.platform)).toEqual([
      "linux-x86_64",
      "macos-aarch64",
      "windows-x86_64",
    ]);
    expect(release.artifacts).toEqual([]);
  });
});

describe("release calls", () => {
  it("lists with a GET without CSRF", async () => {
    const f = respond(200, { releases: [{ ...release, artifacts: [artifact] }] });
    expect(await listReleases(f.impl)).toEqual({ releases: [{ ...release, artifacts: [artifact] }] });
    expect(f.calls).toEqual([
      { url: "/api/admin/releases", method: "GET", body: undefined, contentType: null, csrf: null },
    ]);
  });

  it("rejects an unknown status or a malformed checksum in the hub's answer", async () => {
    const bad = respond(200, { releases: [{ ...release, status: "archived" }] });
    const err = await rejection(listReleases(bad.impl));
    expect((err as Error).message).toBe("release list returned an unexpected body (HTTP 200)");
    const bad2 = respond(200, {
      release: { ...release, artifacts: [{ ...artifact, sha256: SHA.toUpperCase() }] },
    });
    expect(await rejection(fetchRelease(release.id, bad2.impl))).toBeInstanceOf(Error);
  });

  it("fetches one by encoded id", async () => {
    const f = respond(200, { release });
    expect(await fetchRelease(release.id, f.impl)).toEqual(release);
    const missing = respond(404, { error: "no such release" });
    const err = await rejection(fetchRelease("x/y", missing.impl));
    expect((err as HubError).status).toBe(404);
    expect(missing.calls[0].url).toBe("/api/admin/releases/x%2Fy");
  });

  it("creates with CSRF; a used version is a 409 HubError", async () => {
    const input = { version: "0.2.0", notes: "First build.", channel: "stable" as const };
    const f = respond(201, { release });
    expect(await createRelease(input, f.impl)).toEqual(release);
    expect(f.calls).toEqual([
      {
        url: "/api/admin/releases",
        method: "POST",
        body: input,
        contentType: "application/json",
        csrf: "csrf-1",
      },
    ]);
    const taken = respond(409, { error: "a release with this version already exists" });
    const err = await rejection(createRelease(input, taken.impl));
    expect(err).toBeInstanceOf(HubError);
    expect((err as HubError).status).toBe(409);
  });

  it("uploads the file itself as an octet stream with CSRF", async () => {
    const f = respond(201, { artifact });
    const file = new Blob([new Uint8Array([97, 98, 99])]);
    expect(await uploadArtifact(release.id, "linux-x86_64", file, "wipe station+1", f.impl)).toEqual(
      artifact,
    );
    expect(f.calls).toHaveLength(1);
    const call = f.calls[0];
    expect(call.url).toBe(
      `/api/admin/releases/${release.id}/artifacts?platform=linux-x86_64&filename=wipe+station%2B1`,
    );
    expect(new URL(call.url, "http://x").searchParams.get("filename")).toBe("wipe station+1");
    expect(call.method).toBe("POST");
    expect(call.contentType).toBe("application/octet-stream");
    expect(call.csrf).toBe("csrf-1");
    expect(call.body).toBe(file);
    const tooBig = respond(413, { error: "the file is larger than the hub's upload limit (1 GiB)" });
    const err = await rejection(uploadArtifact(release.id, "linux-x86_64", file, "w", tooBig.impl));
    expect((err as HubError).status).toBe(413);
  });

  it("removes, publishes and withdraws with CSRF", async () => {
    const r = respond(200, { release });
    expect(await removeArtifact(release.id, artifact.id, r.impl)).toEqual(release);
    expect(r.calls[0].url).toBe(
      `/api/admin/releases/${release.id}/artifacts/${artifact.id}/remove`,
    );
    expect(r.calls[0].csrf).toBe("csrf-1");
    const published = {
      ...release,
      status: "published" as const,
      published_at: "2026-11-08T11:00:00Z",
      artifacts: [artifact],
    };
    const p = respond(200, { release: published });
    expect(await publishRelease(release.id, p.impl)).toEqual(published);
    expect(p.calls[0]).toMatchObject({
      url: `/api/admin/releases/${release.id}/publish`,
      method: "POST",
      csrf: "csrf-1",
    });
    const withdrawn = { ...published, status: "withdrawn" as const, withdrawn_at: "2026-11-09T11:00:00Z" };
    const w = respond(200, { release: withdrawn });
    expect(await withdrawRelease(release.id, w.impl)).toEqual(withdrawn);
    expect(w.calls[0]).toMatchObject({
      url: `/api/admin/releases/${release.id}/withdraw`,
      method: "POST",
      csrf: "csrf-1",
    });
  });

  it("lists a release's downloads with a GET and checks the answer", async () => {
    const download: ReleaseDownload = {
      id: "9b2e7c1a-3d4f-4a5b-8c6d-7e8f9a0b1c2d",
      artifact_id: artifact.id,
      platform: "linux-x86_64",
      filename: "wipestation",
      user_id: "1c2d3e4f-5a6b-4c7d-8e9f-0a1b2c3d4e5f",
      user_email: "ada@example.com",
      organization_id: "2d3e4f5a-6b7c-4d8e-9f0a-1b2c3d4e5f6a",
      organization_name: "Acme Recyclers",
      downloaded_at: "2026-11-08T12:00:00Z",
    };
    const f = respond(200, { downloads: [download] });
    expect(await listReleaseDownloads(release.id, f.impl)).toEqual([download]);
    expect(f.calls).toEqual([
      {
        url: `/api/admin/releases/${release.id}/downloads`,
        method: "GET",
        body: undefined,
        contentType: null,
        csrf: null,
      },
    ]);
    // A removed user has no email any more.
    const removed = respond(200, { downloads: [{ ...download, user_email: null }] });
    expect(await listReleaseDownloads(release.id, removed.impl)).toEqual([
      { ...download, user_email: null },
    ]);
    const bad = respond(200, { downloads: [{ ...download, user_email: 7 }] });
    expect(await rejection(listReleaseDownloads(release.id, bad.impl))).toBeInstanceOf(Error);
    const noId = respond(200, { downloads: [{ ...download, user_id: null }] });
    expect(await rejection(listReleaseDownloads(release.id, noId.impl))).toBeInstanceOf(Error);
    const missing = respond(404, { error: "no such release" });
    const err = await rejection(listReleaseDownloads("x/y", missing.impl));
    expect((err as HubError).status).toBe(404);
    expect(missing.calls[0].url).toBe("/api/admin/releases/x%2Fy/downloads");
  });
});
