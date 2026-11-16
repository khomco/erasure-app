import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { LicenseInfo } from "@/api/types";
import { LicenceBanner } from "./LicenceBanner";

function render(client: QueryClient): string {
  return renderToStaticMarkup(
    createElement(QueryClientProvider, { client }, createElement(LicenceBanner)),
  );
}

/** Render the banner with `info` (if any) already in the query cache. */
function renderWith(info?: LicenseInfo): string {
  const client = new QueryClient();
  if (info) client.setQueryData(["license"], info);
  return render(client);
}

const UNLICENSED_MESSAGE = "No licence installed.";

function info(over: Partial<LicenseInfo>): LicenseInfo {
  return {
    status: { status: "unlicensed" },
    operator_message: UNLICENSED_MESSAGE,
    customer: null,
    not_after: null,
    paid_through: null,
    renewal_overdue: null,
    instance_public_key_id: null,
    new_jobs_refused: null,
    simulated_backend: true,
    fingerprint: "fp",
    ...over,
  };
}

describe("LicenceBanner", () => {
  it("shows evaluation mode with the server's message and no button or link", () => {
    const html = renderWith(info({}));
    expect(html).toContain('role="status"');
    expect(html).toContain("Evaluation mode — no licence installed");
    expect(html).toContain(UNLICENSED_MESSAGE);
    expect(html).not.toMatch(/<button|<a /);
  });

  it("renders a refusal in danger tone with the server's text and no button or link", () => {
    const refusal =
      "This station has no licence, so it cannot start new jobs. Install a licence to start them.";
    const html = renderWith(info({ simulated_backend: false, new_jobs_refused: refusal }));
    expect(html).toContain("bg-rose-950");
    expect(html).toContain("New jobs are refused");
    expect(html).toContain(refusal);
    expect(html).not.toMatch(/<button|<a /);
  });

  it("renders a lapsed licence in danger tone with no button or link", () => {
    const html = renderWith(
      info({
        status: { status: "expired", not_after: "2026-11-01T00:00:00Z" },
        operator_message: "Licence expired on 2026-11-01.",
      }),
    );
    expect(html).toContain("bg-rose-950");
    expect(html).toContain("Licence expired on 2026-11-01.");
    expect(html).not.toMatch(/<button|<a /);
  });

  it("renders a simulated station's lease fault as a warning that does not claim evaluation", () => {
    const html = renderWith(
      info({
        status: { status: "lease_state_unavailable", detail: "disk full" },
        operator_message: "Licence state could not be read or saved (disk full).",
      }),
    );
    expect(html).toContain("Licence state needs attention");
    expect(html).toContain("new jobs still start");
    expect(html).not.toContain("bg-rose-950");
    expect(html).not.toMatch(/evaluation/);
  });

  it("renders nothing for a valid licence with 31 days left", () => {
    expect(
      renderWith(
        info({
          status: { status: "valid", days_remaining: 31 },
          operator_message: "Licensed - unlimited, 31 days left",
          not_after: "2026-12-09T00:00:00Z",
        }),
      ),
    ).toBe("");
  });

  it("renders nothing while the station has not answered", () => {
    expect(renderWith()).toBe("");
  });

  it("says the licence status is unknown when the route failed", () => {
    // retryOnMount: false keeps the seeded error instead of refetching.
    const client = new QueryClient({ defaultOptions: { queries: { retryOnMount: false } } });
    client
      .getQueryCache()
      .build(client, { queryKey: ["license"] })
      .setState({ status: "error", error: new Error("502"), fetchStatus: "idle" });
    const html = render(client);
    expect(html).toContain('role="status"');
    expect(html).toContain("Licence status unknown");
    expect(html).not.toMatch(/<button|<a /);
  });

  it("says the licence status is unknown when the route failed after an earlier answer", () => {
    const client = new QueryClient();
    client.setQueryData(
      ["license"],
      info({
        status: { status: "valid", days_remaining: 31 },
        operator_message: "Licensed - unlimited, 31 days left",
        not_after: "2026-12-09T00:00:00Z",
      }),
    );
    const query = client.getQueryCache().find({ queryKey: ["license"] })!;
    query.setState({ ...query.state, status: "error", error: new Error("502"), fetchStatus: "idle" });
    expect(query.state.data).toBeDefined();
    expect(render(client)).toContain("Licence status unknown");
  });
});
