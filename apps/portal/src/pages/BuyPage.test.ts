import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { CatalogOffering } from "@/api/orders";
import { OfferingCard } from "./BuyPage";

const notice = {
  headline: "Renews automatically every year at 1,200.00 USD until you cancel.",
  detail:
    "Your first year runs to 8 November 2027. Cancel any time from Billing in your account: " +
    "your current year still runs to its end, and you will not be charged again. " +
    "We email you before each renewal.",
};

const annual: CatalogOffering = {
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  name: "Station — annual, 1 seat",
  description: "One station.",
  price_minor: 120000,
  currency: "USD",
  interval: "annual",
  grants: { scope: "machine", seats: 1, quota_erasures: null, features: [], term_months: 12 },
  renewal_notice: notice,
};

function render(offering: CatalogOffering): string {
  return renderToStaticMarkup(
    createElement(
      QueryClientProvider,
      { client: new QueryClient() },
      createElement(OfferingCard, { offering, canBuy: true }),
    ),
  );
}

/** The markup's text with tags removed and entities decoded. */
function text(html: string): string {
  return html
    .replace(/<[^>]+>/g, "")
    .replace(/&#x27;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, "&");
}

describe("the buy page", () => {
  it("shows an annual offering's renewal notice verbatim, headline in bold", () => {
    const html = render(annual);
    expect(html).toContain(`<strong class="font-semibold text-slate-100">${notice.headline}</strong>`);
    expect(text(html)).toContain(`${notice.headline} ${notice.detail}`);
  });

  it("shows no notice for a one-off offering", () => {
    const html = render({ ...annual, interval: "one_off", renewal_notice: null });
    expect(html).not.toContain("<strong");
    expect(text(html)).not.toContain("Renews automatically");
    expect(text(html)).not.toContain("Cancel any time");
  });
});
