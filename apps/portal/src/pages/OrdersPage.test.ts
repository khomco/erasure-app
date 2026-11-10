import { describe, expect, it } from "vitest";
import type { Order } from "@/api/orders";
import { PROVIDER_LABELS, orderPriceText } from "./OrdersPage";

const paid: Pick<Order, "provider" | "price_minor" | "currency" | "interval"> = {
  provider: "stripe",
  price_minor: 120000,
  currency: "USD",
  interval: "annual",
};

describe("order prices", () => {
  it("show a paid order's price and interval", () => {
    expect(orderPriceText(paid)).toBe("1,200.00 USD annual");
    expect(orderPriceText(paid, { bracketed: true })).toBe("1,200.00 USD (annual)");
  });

  it("show a free trial as Free trial, with no price", () => {
    const trial = { ...paid, provider: "trial" as const, price_minor: 0, interval: "one_off" as const };
    expect(orderPriceText(trial)).toBe("Free trial");
    expect(orderPriceText(trial, { bracketed: true })).toBe("Free trial");
  });

  it("name a trial's provider as no payment", () => {
    expect(PROVIDER_LABELS.trial).toBe("None (free trial)");
  });
});
