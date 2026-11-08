import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { DeploymentReport } from "@/api/deployment";
import { DevelopmentBanner } from "./DevelopmentBanner";

/** Render the banner with `report` (if any) already in the query cache. */
function render(report?: DeploymentReport): string {
  const client = new QueryClient();
  if (report) client.setQueryData(["deployment"], report);
  return renderToStaticMarkup(
    createElement(QueryClientProvider, { client }, createElement(DevelopmentBanner)),
  );
}

describe("DevelopmentBanner", () => {
  it("shows the heading and one line per reason on a development hub", () => {
    const html = render({
      development: true,
      payment_provider: "fake",
      vendor_root: "file",
      reasons: ["fake_payment_provider", "file_vendor_root"],
    });
    expect(html).toContain('role="status"');
    expect(html).toContain("Development hub — not for production use.");
    expect(html).toContain("Payments are simulated by the test payment provider; no money moves.");
    expect(html).toContain(
      "The vendor root is held in a file, so licences signed here are for testing only.",
    );
    expect(html).not.toMatch(/<button/);
  });

  it("renders nothing when the hub is not a development hub", () => {
    expect(
      render({ development: false, payment_provider: "stripe", vendor_root: "none", reasons: [] }),
    ).toBe("");
  });

  it("renders nothing while the hub has not answered", () => {
    expect(render()).toBe("");
  });
});
