import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { Entitlement } from "@/api/orders";
import { creditPoolText, entitlementState, SeatsSummary } from "./SeatsPage";

const base: Entitlement = {
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

/** The markup's text with tags removed. */
function text(html: string): string {
  return html.replace(/<[^>]+>/g, "");
}

describe("entitlementState", () => {
  it("says a refunded entitlement is refunded, whatever its term", () => {
    expect(
      entitlementState({
        ...base,
        status: "refunded",
        renews: false,
        current: false,
        refunded_at: "2027-01-01T00:00:00Z",
      }),
    ).toBe("Refunded");
  });

  it("says a lapsed term ended", () => {
    expect(entitlementState({ ...base, renews: false, current: false })).toBe(
      "Ended on 2027-11-08",
    );
  });

  it("says a cancelled, still current entitlement ends at its term end", () => {
    expect(
      entitlementState({
        ...base,
        status: "cancelled",
        renews: false,
        cancelled_at: "2027-06-01T00:00:00Z",
      }),
    ).toBe("Cancelled; ends on 2027-11-08");
  });

  it("says a renewing entitlement renews at its term end", () => {
    expect(entitlementState(base)).toBe("Renews on 2027-11-08");
  });

  it("says an active entitlement that will not renew ends", () => {
    expect(entitlementState({ ...base, renews: false })).toBe(
      "Ends on 2027-11-08, not renewing",
    );
  });
});

describe("the seats summary", () => {
  it("shows one available seat for one current 1-seat entitlement", () => {
    const html = renderToStaticMarkup(
      createElement(SeatsSummary, { seats: { entitlements: [base], available_seats: 1 } }),
    );
    expect(html).toContain(`<span class="font-semibold">1</span>`);
    expect(text(html)).toBe("1 available licence seat");
  });

  it("pluralises other counts", () => {
    const html = renderToStaticMarkup(
      createElement(SeatsSummary, { seats: { entitlements: [], available_seats: 0 } }),
    );
    expect(text(html)).toBe("0 available licence seats");
  });
});

describe("credit pool", () => {
  it("shows what is available of what was bought, granted and returned", () => {
    expect(
      creditPoolText({
        ...base,
        credits: { purchased: 5000, granted: 80, returned: 30, available: 4950, target_reserve: 50 },
      }),
    ).toBe("4950 of 5000 available · 80 granted · 30 returned");
    expect(creditPoolText(base)).toBe("Unlimited");
  });
});
