import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { Offering } from "@/api/adminOfferings";
import { OfferingForm, offeringBody, type OfferingFields } from "./OfferingForm";

const priced: Offering = {
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  name: "Station — annual, 1 seat",
  description: "One station.",
  price_minor: 120000,
  currency: "USD",
  interval: "annual",
  trial: false,
  grants: { scope: "machine", seats: 1, quota_erasures: null, features: [], term_months: 12 },
  credit_target_reserve: null,
  status: "draft",
  created_at: "2026-11-10T10:00:00Z",
  updated_at: "2026-11-10T10:00:00Z",
  published_at: null,
  retired_at: null,
};

const trial: Offering = {
  ...priced,
  name: "Free trial",
  price_minor: 0,
  interval: "one_off",
  trial: true,
  grants: { scope: "site", seats: 1, quota_erasures: 25, features: [], term_months: 1 },
  credit_target_reserve: 50,
};

function render(initial?: Offering, termsLocked = false): string {
  return renderToStaticMarkup(
    createElement(OfferingForm, {
      initial,
      termsLocked,
      submitLabel: "Save",
      pending: false,
      error: null,
      onSubmit: () => {},
    }),
  );
}

/** The `<input>` or `<select>` tag named `name`. */
function tag(html: string, name: string): string {
  const m = new RegExp(`<(input|select)[^>]*name="${name}"[^>]*>`).exec(html);
  if (!m) throw new Error(`no field ${name}`);
  return m[0];
}

describe("the offering form", () => {
  it("leaves Free trial unticked for a new or priced offering", () => {
    for (const html of [render(), render(priced)]) {
      expect(tag(html, "trial")).not.toContain('checked=""');
      expect(tag(html, "price")).not.toContain('disabled=""');
      expect(html).toContain('<option value="annual"');
    }
  });

  it("fixes a trial's price at 0, its interval at one-off and requires a quota", () => {
    const html = render(trial);
    expect(tag(html, "trial")).toContain('checked=""');
    const price = tag(html, "price");
    expect(price).toContain('disabled=""');
    expect(price).toContain('value="0"');
    expect(html).not.toContain('<option value="annual"');
    expect(html).toContain('<option value="one_off"');
    expect(tag(html, "unlimited")).toContain('disabled=""');
    expect(tag(html, "unlimited")).not.toContain('checked=""');
    expect(tag(html, "quota")).toContain('value="25"');
    expect(html).toContain("Each organization can take it once.");
  });

  it("locks a published trial's flag with its other terms", () => {
    const html = render({ ...trial, status: "published" }, true);
    expect(html).toContain("free-trial setting cannot change");
    expect(html).toMatch(/<fieldset disabled=""/);
    expect(tag(html, "trial")).toContain('checked=""');
  });
});

describe("the body the form sends", () => {
  const fields: OfferingFields = {
    name: "Free trial",
    description: "Try it.",
    currency: "usd",
    price: "1200.00",
    interval: "annual",
    trial: true,
    scope: "site",
    seats: "1",
    unlimited: false,
    quota: "25",
    features: [],
    term: "1",
  };

  it("sends a trial as free, one-off and counted, whatever price and interval were left", () => {
    expect(offeringBody(fields)).toEqual({
      body: {
        name: "Free trial",
        description: "Try it.",
        price_minor: 0,
        currency: "USD",
        interval: "one_off",
        trial: true,
        grants: { scope: "site", seats: 1, quota_erasures: 25, features: [], term_months: 1 },
      },
    });
  });

  it("ignores a stale Unlimited tick on a trial", () => {
    const built = offeringBody({ ...fields, unlimited: true });
    expect("body" in built && built.body.grants.quota_erasures).toBe(25);
  });

  it("keeps a locked published trial's terms and trial flag", () => {
    const published = { ...trial, status: "published" as const };
    const built = offeringBody(
      { ...fields, name: "Renamed", trial: false, price: "5", interval: "annual" },
      published,
    );
    expect(built).toEqual({
      body: {
        name: "Renamed",
        description: "Try it.",
        price_minor: 0,
        currency: "USD",
        interval: "one_off",
        trial: true,
        grants: trial.grants,
        credit_target_reserve: 50,
      },
    });
  });

  it("sends the credit target reserve only with a counted quota", () => {
    const pooled = { ...fields, trial: false, interval: "one_off" as const, quota: "500" };
    expect(offeringBody({ ...pooled, reserve: "80" })).toMatchObject({
      body: { credit_target_reserve: 80 },
    });
    // Blank leaves the hub's default.
    const blank = offeringBody({ ...pooled, reserve: " " });
    expect("body" in blank && "credit_target_reserve" in blank.body).toBe(false);
    // Unlimited never sends one.
    const unlimited = offeringBody({ ...pooled, unlimited: true, reserve: "80" });
    expect("body" in unlimited && "credit_target_reserve" in unlimited.body).toBe(false);
    expect(offeringBody({ ...pooled, reserve: "1.5" })).toEqual({
      error: "The credit target reserve is a whole number.",
    });
  });

  it("shows the target reserve field only with a counted quota", () => {
    expect(render(trial)).toContain('name="reserve"');
    expect(render(trial)).toContain('value="50"');
    expect(render(priced)).not.toContain('name="reserve"');
  });

  it("keeps a priced offering's price and Unlimited", () => {
    const built = offeringBody({ ...fields, trial: false, unlimited: true, term: "12" });
    expect(built).toMatchObject({
      body: { price_minor: 120000, interval: "annual", trial: false },
    });
    expect("body" in built && built.body.grants.quota_erasures).toBeNull();
  });
});
