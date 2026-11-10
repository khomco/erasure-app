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
import type { CatalogOffering } from "@/api/orders";
import { SignInPrompt } from "./AccountPage";
import { OfferingCard } from "./BuyPage";
import { safeNext } from "./SignInPage";

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
  trial: false,
  grants: { scope: "machine", seats: 1, quota_erasures: null, features: [], term_months: 12 },
  renewal_notice: notice,
};

const trial: CatalogOffering = {
  ...annual,
  name: "Free trial",
  description: "Try it on one bench.",
  price_minor: 0,
  interval: "one_off",
  trial: true,
  grants: { scope: "site", seats: 1, quota_erasures: 25, features: [], term_months: 1 },
  renewal_notice: null,
};

function render(offering: CatalogOffering, canBuy = true): string {
  return renderToStaticMarkup(
    createElement(
      QueryClientProvider,
      { client: new QueryClient() },
      createElement(OfferingCard, { offering, canBuy }),
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

  it("shows a priced offering's price and a Buy button", () => {
    const t = text(render(annual));
    expect(t).toContain("1,200.00 USD (annual)");
    expect(t).toContain("Buy");
    expect(t).not.toContain("Free trial");
  });

  it("shows a free trial as free, with a Start free trial button and no price or notice", () => {
    const t = text(render(trial));
    expect(t).toContain("Free trial");
    expect(t).toContain("Start free trial");
    expect(t).toContain("25 erasures");
    expect(t).not.toContain("USD");
    expect(t).not.toContain("Renews automatically");
    expect(t).not.toMatch(/\bBuy\b/);
  });

  it("offers no button to someone who cannot buy", () => {
    expect(text(render(trial, false))).not.toContain("Start free trial");
    expect(text(render(annual, false))).not.toMatch(/\bBuy\b/);
  });

  it("shows no notice for a one-off offering", () => {
    const html = render({ ...annual, interval: "one_off", renewal_notice: null });
    expect(html).not.toContain("<strong");
    expect(text(html)).not.toContain("Renews automatically");
    expect(text(html)).not.toContain("Cancel any time");
  });
});

describe("the buy page signed out", () => {
  it("links to sign-in and back to /buy, so a trial link from the site survives sign-in", () => {
    const router = createRouter({
      routeTree: createRootRoute(),
      history: createMemoryHistory({ initialEntries: ["/buy"] }),
    });
    const html = renderToStaticMarkup(
      createElement(RouterContextProvider, {
        router,
        children: createElement(SignInPrompt, { next: "/buy" }),
      }),
    );
    const href = /href="([^"]*)"/.exec(html)![1].replace(/&amp;/g, "&");
    const url = new URL(href, "http://portal.test");
    expect(url.pathname).toBe("/sign-in");
    expect(url.searchParams.get("next")).toBe("/buy");
    expect(safeNext(url.searchParams.get("next") ?? undefined)).toBe("/buy");
  });
});
