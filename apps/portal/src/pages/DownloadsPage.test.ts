import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterContextProvider,
  createMemoryHistory,
  createRootRoute,
  createRouter,
} from "@tanstack/react-router";
import { describe, expect, it } from "vitest";
import type { ReleaseDownload } from "@/api/adminReleases";
import type { DownloadRelease } from "@/api/downloads";
import { ChecksumNote, NotEntitled, ReleaseCard, releaseHeading } from "./DownloadsPage";
import { downloadedWhen, DownloadsTable } from "./AdminReleasePage";

/** `node` inside a memory router, so its links render. */
function withRouter(node: ReturnType<typeof createElement>) {
  const rootRoute = createRootRoute();
  const router = createRouter({
    routeTree: rootRoute,
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  return createElement(RouterContextProvider, { router, children: node });
}

function text(html: string): string {
  return html.replace(/<[^>]+>/g, " ").replace(/&#x27;/g, "'").replace(/\s+/g, " ");
}

const SHA = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

const release: DownloadRelease = {
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  version: "0.2.0-rc.1",
  notes: "First build.",
  channel: "preview",
  published_at: "2026-11-08T10:00:00Z",
  artifacts: [
    {
      id: "7c9e6679-7425-40de-944b-e07fc1f90ae7",
      platform: "linux-x86_64",
      filename: "wipestation",
      size_bytes: 1536,
      sha256: SHA,
    },
  ],
};

/** `ReleaseCard` inside a query client, as the page renders it. */
function renderCard(r: DownloadRelease): string {
  return renderToStaticMarkup(
    createElement(
      QueryClientProvider,
      { client: new QueryClient() },
      createElement(ReleaseCard, { release: r }),
    ),
  );
}

describe("downloads page", () => {
  it("heads each release with its version, channel and publication date", () => {
    expect(releaseHeading(release)).toBe("0.2.0-rc.1 · Preview · published 2026-11-08");
    expect(releaseHeading({ ...release, channel: "stable" })).toMatch(/· Stable ·/);
  });

  it("shows each file's platform, size and full checksum with a Download button", () => {
    const html = renderCard(release);
    expect(html).toContain("linux-x86_64");
    expect(html).toContain("wipestation");
    expect(html).toContain("1.5 KiB (1,536 bytes)");
    expect(html).toContain(SHA);
    expect(html).toContain(">Download</button>");
    // Each button names its file; the action column has a hidden header.
    expect(html).toContain('aria-label="Download wipestation for linux-x86_64"');
    expect(html).toContain('<span class="sr-only">Download</span>');
    expect(html).toContain("First build.");
  });

  it("explains how to check a checksum", () => {
    expect(renderToStaticMarkup(createElement(ChecksumNote))).toContain("sha256sum");
  });

  it("tells an organization without a current entitlement, with a link to buy", () => {
    const html = renderToStaticMarkup(withRouter(createElement(NotEntitled)));
    expect(text(html)).toContain(
      "Your organization has no current entitlement, so it cannot download releases.",
    );
    expect(html).toMatch(/<a [^>]*href="\/buy"[^>]*>Buy a licence<\/a>/);
    // No file is listed.
    expect(html).not.toContain(">Download</button>");
    expect(html).not.toContain("<table");
  });
});

const download: ReleaseDownload = {
  id: "9b2e7c1a-3d4f-4a5b-8c6d-7e8f9a0b1c2d",
  artifact_id: "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  platform: "linux-x86_64",
  filename: "wipestation",
  user_id: "1c2d3e4f-5a6b-4c7d-8e9f-0a1b2c3d4e5f",
  user_email: "ada@example.com",
  organization_id: "2d3e4f5a-6b7c-4d8e-9f0a-1b2c3d4e5f6a",
  organization_name: "Acme Recyclers",
  downloaded_at: "2026-11-08T12:34:56Z",
};

function renderTable(downloads: ReleaseDownload[]): string {
  return renderToStaticMarkup(withRouter(createElement(DownloadsTable, { downloads })));
}

describe("admin release downloads", () => {
  it("formats when a download happened", () => {
    expect(downloadedWhen("2026-11-08T12:34:56Z")).toBe("2026-11-08 12:34 UTC");
  });

  it("shows when, who, which organization and which platform", () => {
    const html = renderTable([download]);
    const t = text(html);
    expect(t).toContain("2026-11-08 12:34 UTC");
    expect(t).toContain("ada@example.com");
    expect(t).toContain("linux-x86_64");
    expect(html).toMatch(
      new RegExp(
        `<a [^>]*href="/admin/organizations/${download.organization_id}"[^>]*>Acme Recyclers</a>`,
      ),
    );
    expect(t).not.toContain("Removed user");
  });

  it("shows a removed user's id in place of an email", () => {
    const t = text(renderTable([{ ...download, user_email: null }]));
    expect(t).toContain(`Removed user ${download.user_id}`);
    expect(t).not.toContain("ada@example.com");
    expect(t).toContain("Acme Recyclers");
  });

  it("says when no one has downloaded a release", () => {
    expect(renderToStaticMarkup(createElement(DownloadsTable, { downloads: [] }))).toContain(
      "No one has downloaded this release yet.",
    );
  });
});
