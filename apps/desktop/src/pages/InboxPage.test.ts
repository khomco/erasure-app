import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";

import type { DestructionManifest, Job, JobStateLabel } from "@/api/types";
import { buildInbox } from "./inbox";
import { HrefRowLink, InboxList, InboxView } from "./InboxPage";

// TanStack `Link` needs a router context, so rows render through the
// `href`-based `HrefRowLink`; the in-app link takes its target from the same
// `inboxTarget` that `inboxHref` reads.
const render = (jobs: Job[], runs: DestructionManifest[]) =>
  renderToStaticMarkup(createElement(InboxList, { entries: buildInbox(jobs, runs), RowLink: HrefRowLink }));

function job(id: string, state: JobStateLabel, erasureState?: string, created_at = "2026-11-09T10:00:00Z"): Job {
  return {
    id,
    spec: { device_id: `dev-${id}`, asset_tag: `AT-${id}` },
    state: { state },
    activities: erasureState
      ? [
          {
            type: "erasure",
            id: `${id}-e`,
            state: { state: erasureState },
            events: [{ at: "t", event: { kind: "failed", reason: "medium error" } }],
          },
        ]
      : [],
    manifest_id: null,
    created_at,
  } as unknown as Job;
}

const run = (id: string, job_ids: string[]): DestructionManifest =>
  ({
    id,
    job_ids,
    state: "pending",
    created_at: "2026-11-09T10:00:00Z",
    required_concurrence: null,
  }) as unknown as DestructionManifest;

const hrefs = (html: string) => [...html.matchAll(/href="([^"]*)"/g)].map((m) => m[1]);

describe("InboxList", () => {
  it("renders the empty state when nothing waits", () => {
    const html = render([], []);
    expect(html).toContain("Nothing needs you");
    expect(hrefs(html)).toEqual([]);
  });

  it("links a failed erasure, an unassembled Job and a pending run to where each is resolved", () => {
    const html = render(
      [
        job("f1", "in_progress", "failed"),
        job("p1", "pending_destruction"),
        job("r1a", "pending_destruction"),
      ],
      [run("run1", ["r1a"])],
    );
    expect(hrefs(html)).toEqual(["/jobs/f1", "/manifests", "/manifests#run-run1"]);
    expect(html).toContain("asset tag AT-f1");
    expect(html).toContain("medium error");
    expect(html).toContain("Pending completion");
    expect(html).not.toContain("Nothing needs you");
  });

  it("titles each group with its count, in group order", () => {
    const html = render(
      [job("f1", "in_progress", "failed"), job("f2", "queued", "failed"), job("q1", "quarantined")],
      [],
    );
    const failedAt = html.indexOf("Failed erasure (2)");
    const quarantinedAt = html.indexOf("Quarantined (1)");
    expect(failedAt).toBeGreaterThanOrEqual(0);
    expect(quarantinedAt).toBeGreaterThan(failedAt);
    expect(hrefs(html)).toEqual(["/jobs/f1", "/jobs/f2", "/jobs/q1"]);
  });

  it("shows a failed verification's sample count on its row", () => {
    const j = job("v1", "in_progress", "completed");
    j.activities.push({
      type: "verification",
      id: "v1-v",
      erasure_event_id: "v1-e",
      report: {
        sample_count: 8,
        samples: [false, false, true, true, true, true, true, true].map((passed) => ({ passed })),
        all_passed: false,
      },
    } as never);
    const html = render([j], []);
    expect(html).toContain("Failed erasure (1)");
    expect(html).toContain("Verification failed: 2 of 8 samples failed");
    expect(hrefs(html)).toEqual(["/jobs/v1"]);
  });

  it("shows a failed list's error verbatim instead of the list", () => {
    const html = renderToStaticMarkup(
      createElement(InboxView, {
        jobs: { data: [job("f1", "in_progress", "failed")], error: null },
        manifests: { data: undefined, error: new Error("500 Internal Server Error: manifests down") },
        RowLink: HrefRowLink,
      }),
    );
    expect(html).toContain("500 Internal Server Error: manifests down");
    expect(hrefs(html)).toEqual([]);
  });

  it("shows the awaiting-concurrence label on a run", () => {
    const html = renderToStaticMarkup(
      createElement(InboxView, {
        jobs: { data: [job("a", "pending_destruction")], error: null },
        manifests: {
          data: [{ ...run("r1", ["a"]), state: "awaiting_concurrence", required_concurrence: "witness" }],
          error: null,
        },
        RowLink: HrefRowLink,
      }),
    );
    expect(html).toContain("Awaiting witness");
    expect(hrefs(html)).toEqual(["/manifests#run-r1"]);
  });

  it("renders the in-app router links from inboxTarget, hash included", async () => {
    const entries = buildInbox(
      [job("f1", "in_progress", "failed"), job("p1", "pending_destruction"), job("r1a", "pending_destruction")],
      [run("run1", ["r1a"])],
    );
    const rootRoute = createRootRoute({ component: () => createElement(InboxList, { entries }) });
    const router = createRouter({
      routeTree: rootRoute,
      history: createMemoryHistory({ initialEntries: ["/inbox"] }),
    });
    await router.load();
    const html = renderToStaticMarkup(createElement(RouterProvider, { router } as never));
    expect(hrefs(html)).toEqual(["/jobs/f1", "/manifests", "/manifests#run-run1"]);
  });
});
