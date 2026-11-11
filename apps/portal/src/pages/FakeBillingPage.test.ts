import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { Entitlement } from "@/api/orders";
import { Subscription } from "./FakeBillingPage";

const renewing: Entitlement = {
  id: "5b0d8b4e-6f1e-4c3a-9d2e-1f6c2a7b8c90",
  order_id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  offering_name: "Station — annual, 1 seat",
  interval: "annual",
  grants: { scope: "machine", seats: 1, quota_erasures: null, features: [], term_months: 12 },
  term_start: "2026-11-08T10:00:00Z",
  term_end: "2027-11-08T10:00:00Z",
  status: "active",
  renews: true,
  current: true,
  cancelled_at: null,
  refunded_at: null,
  credits: null,
};

function render(canManage: boolean): string {
  return renderToStaticMarkup(
    createElement(
      QueryClientProvider,
      { client: new QueryClient() },
      createElement(Subscription, { entitlement: renewing, canManage }),
    ),
  );
}

describe("Subscription on /billing/fake", () => {
  it("shows a member the subscription without the owner's test actions", () => {
    const html = render(false);
    expect(html).toContain("Station — annual, 1 seat");
    expect(html).not.toContain("Renew now (test)");
    expect(html).not.toContain("Cancel (test)");
  });

  it("shows an owner both test actions", () => {
    const html = render(true);
    expect(html).toContain("Station — annual, 1 seat");
    expect(html).toContain("Renew now (test)");
    expect(html).toContain("Cancel (test)");
  });
});
