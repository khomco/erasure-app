import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";

import type { Job, JobStateLabel } from "@/api/types";
import { HrefJobLink, JobsList, JobsView } from "./JobsPage";

// TanStack `Link` needs a router context, so rows render through `HrefJobLink`.
const render = (data: Job[] | undefined, error: unknown = null) =>
  renderToStaticMarkup(createElement(JobsView, { jobs: { data, error }, RowLink: HrefJobLink }));

function job(id: string, state: JobStateLabel, created_at = "2026-11-09T10:00:00Z", ended_at: string | null = null): Job {
  return {
    id,
    spec: { device_id: `dev-${id}`, classification: "clear", intent: "reuse" },
    state: { state },
    activities: [
      {
        type: "erasure",
        id: `${id}-e`,
        state: { state: state === "in_progress" ? "running" : "completed" },
        events: [],
        device_snapshot: {
          vendor: "Acme",
          model: `Model-${id}`,
          serial: `SN-${id}`,
          capacity_bytes: 1_000_000_000,
        },
        progress: { fraction: 0.42, eta_seconds: 3600, stage: "overwrite", bytes_processed: null, bytes_total: null },
      },
    ],
    manifest_id: null,
    certificate_id: null,
    created_at,
    started_at: null,
    ended_at,
  } as unknown as Job;
}

const hrefs = (html: string) => [...html.matchAll(/href="([^"]*)"/g)].map((m) => m[1]);
const headingAt = (html: string, text: string) => {
  const at = html.indexOf(text);
  expect(at, text).toBeGreaterThanOrEqual(0);
  return at;
};

describe("JobsView", () => {
  it("heads Active, Queued, Finished with counts, in order, each Job under its section", () => {
    const html = render([
      job("e1", "erased", "2026-11-09T09:00:00Z", "2026-11-09T11:00:00Z"),
      job("w1", "in_progress", "2026-11-09T10:00:00Z"),
      job("q1", "queued"),
      job("e2", "erased", "2026-11-09T09:00:00Z", "2026-11-09T11:30:00Z"),
      job("w2", "in_progress", "2026-11-09T10:01:00Z"),
      job("e3", "erased", "2026-11-09T09:00:00Z", "2026-11-09T10:30:00Z"),
    ]);
    const active = headingAt(html, "Active · 2");
    const queued = headingAt(html, "Queued · 1");
    const finished = headingAt(html, "Finished · 3");
    expect(active).toBeLessThan(queued);
    expect(queued).toBeLessThan(finished);
    expect(hrefs(html)).toEqual(["/jobs/w1", "/jobs/w2", "/jobs/q1", "/jobs/e2", "/jobs/e1", "/jobs/e3"]);
    const between = (from: number, to: number) => html.slice(from, to);
    expect(between(active, queued)).toContain('href="/jobs/w1"');
    expect(between(queued, finished)).toContain('href="/jobs/q1"');
    expect(html.slice(finished)).toContain('href="/jobs/e1"');
    expect(html).toContain("Acme Model-w1");
    expect(html).toContain("42% · overwrite");
    expect(html).not.toContain("None");
    expect(html).not.toContain("No jobs yet");
  });

  it("groups a Job by its current state: queued under Queued, started under Active", () => {
    expect(render([job("a", "queued")])).toContain("Queued · 1");
    const html = render([job("a", "in_progress")]);
    expect(html).toContain("Active · 1");
    expect(html).toContain("Queued · 0");
  });

  it("shows None under an empty section", () => {
    const html = render([job("e1", "erased")]);
    const active = headingAt(html, "Active · 0");
    const queued = headingAt(html, "Queued · 0");
    const finished = headingAt(html, "Finished · 1");
    expect(html.slice(active, queued)).toContain("None");
    expect(html.slice(queued, finished)).toContain("None");
    expect(html.slice(finished)).not.toContain("None");
  });

  it("shows the empty state, with no sections, for no Jobs", () => {
    const html = render([]);
    expect(html).toContain("No jobs yet. Start one from the Devices page.");
    expect(html).not.toContain("Active");
  });

  it("shows a load error verbatim, with neither sections nor the empty state", () => {
    const html = render(undefined, new Error("GET /api/jobs failed: 503"));
    expect(html).toContain("GET /api/jobs failed: 503");
    expect(html).not.toContain("No jobs yet");
    expect(html).not.toContain("Active");
    // An error after an earlier answer still wins over the cached list.
    const cached = render([job("w1", "in_progress")], new Error("connection refused"));
    expect(cached).toContain("connection refused");
    expect(cached).not.toContain("Active");
  });

  it("shows Loading jobs… before the first answer", () => {
    expect(render(undefined)).toContain("Loading jobs…");
  });

  it("never renders an ETA or time remaining", () => {
    const html = render([job("w1", "in_progress"), job("q1", "queued")]);
    expect(html).not.toMatch(/ETA|remaining|3600|1h|60 ?min/i);
  });

  it("renders the in-app router links to each Job, in section order", async () => {
    const jobs = [job("e1", "erased"), job("q1", "queued"), job("w1", "in_progress")];
    const rootRoute = createRootRoute({ component: () => createElement(JobsList, { jobs }) });
    const router = createRouter({
      routeTree: rootRoute,
      history: createMemoryHistory({ initialEntries: ["/jobs"] }),
    });
    await router.load();
    const html = renderToStaticMarkup(createElement(RouterProvider, { router } as never));
    expect(hrefs(html)).toEqual(["/jobs/w1", "/jobs/q1", "/jobs/e1"]);
  });
});
