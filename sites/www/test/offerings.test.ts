// The build-time catalog read for /pricing/ (offerings-and-payments story 5):
// the strict shape check, every failure the build must stop on, and the
// labels. No network: every fetch here is a fake.
import { describe, expect, it } from "vitest";
import {
  formatPrice,
  intervalLabel,
  loadOfferings,
  parseCatalog,
  quotaLabel,
  scopeLabel,
  seatsLabel,
  termLabel,
  trialHref,
} from "../src/lib/offerings";
import { parseHubOrigin } from "../src/lib/lead-submit";

const HUB = "http://127.0.0.1:7900";
const URL_ = `${HUB}/api/offerings`;

const annual = () => ({
  id: "11111111-1111-4111-8111-111111111111",
  name: "Station annual",
  description: "One station.",
  price_minor: 120000,
  currency: "USD",
  interval: "annual",
  trial: false,
  grants: {
    scope: "machine",
    seats: 1,
    quota_erasures: null,
    features: ["hub_sync"],
    term_months: 12,
  },
  renewal_notice: {
    headline: "Renews automatically every year at 1,200.00 USD until you cancel.",
    detail: "Detail.",
  },
});

const oneOff = () => ({
  ...annual(),
  id: "22222222-2222-4222-8222-222222222222",
  name: "Site pack",
  interval: "one_off",
  price_minor: 500000,
  grants: { scope: "site", seats: 5, quota_erasures: 10000, features: [], term_months: 24 },
  renewal_notice: null,
});

const trial = () => ({
  ...oneOff(),
  id: "33333333-3333-4333-8333-333333333333",
  name: "Free trial",
  price_minor: 0,
  trial: true,
  grants: { scope: "site", seats: 1, quota_erasures: 25, features: [], term_months: 1 },
});

/** The catalog with offering 1 (a one-off) changed by `edit`. */
function withSecond(edit: (o: Record<string, any>) => void) {
  const o: Record<string, any> = oneOff();
  edit(o);
  return { offerings: [annual(), o] };
}

describe("parseCatalog", () => {
  it("accepts a valid catalog and keeps only the fields the page uses", () => {
    const list = parseCatalog({ offerings: [annual(), oneOff()] });
    expect(list).toHaveLength(2);
    expect(list[0].renewal_notice).toEqual({
      headline: "Renews automatically every year at 1,200.00 USD until you cancel.",
    });
    expect(list[1]).toMatchObject({ interval: "one_off", renewal_notice: null });
    expect(parseCatalog({ offerings: [] })).toEqual([]);
  });

  it("accepts a free trial: price 0, one-off, a counted quota", () => {
    const [t] = parseCatalog({ offerings: [trial()] });
    expect(t).toMatchObject({ trial: true, price_minor: 0, interval: "one_off", renewal_notice: null });
    expect(t.grants.quota_erasures).toBe(25);
    expect(parseCatalog({ offerings: [annual()] })[0].trial).toBe(false);
  });

  it.each([
    ["trial", (o: Record<string, any>) => delete o.trial],
    ["trial", (o: Record<string, any>) => (o.trial = "yes")],
    ["price_minor", (o: Record<string, any>) => (o.price_minor = 100)],
    [
      "interval",
      (o: Record<string, any>) => {
        o.interval = "annual";
        o.renewal_notice = { headline: "x" };
      },
    ],
    ["grants.quota_erasures", (o: Record<string, any>) => (o.grants.quota_erasures = null)],
  ])("rejects a free trial with a bad %s", (field, edit) => {
    const o: Record<string, any> = trial();
    edit(o);
    expect(() => parseCatalog({ offerings: [o] })).toThrow(`offerings[0].${field}: `);
  });

  it.each([
    ["no object", null, /^offerings: /],
    ["no offerings", {}, /^offerings: /],
    ["offerings not an array", { offerings: {} }, /^offerings: /],
    ["an offering that is not an object", { offerings: [annual(), "x"] }, /^offerings\[1\]: not an object/],
  ])("rejects a body with %s", (_, body, msg) => {
    expect(() => parseCatalog(body)).toThrow(msg);
  });

  const bad: [string, (o: Record<string, any>) => void][] = [
    ["offerings[1].id", (o) => (o.id = 7)],
    ["offerings[1].name", (o) => delete o.name],
    ["offerings[1].description", (o) => (o.description = null)],
    ["offerings[1].price_minor", (o) => (o.price_minor = 0)],
    ["offerings[1].price_minor", (o) => (o.price_minor = 12.5)],
    ["offerings[1].price_minor", (o) => (o.price_minor = "100")],
    ["offerings[1].price_minor", (o) => (o.price_minor = 2 ** 53)],
    ["offerings[1].currency", (o) => (o.currency = "usd")],
    ["offerings[1].currency", (o) => (o.currency = "USDT")],
    ["offerings[1].interval", (o) => (o.interval = "monthly")],
    ["offerings[1].grants", (o) => (o.grants = null)],
    ["offerings[1].grants.scope", (o) => (o.grants.scope = "fleet")],
    ["offerings[1].grants.seats", (o) => (o.grants.seats = 0)],
    ["offerings[1].grants.seats", (o) => (o.grants.seats = 1.5)],
    ["offerings[1].grants.quota_erasures", (o) => (o.grants.quota_erasures = 0)],
    ["offerings[1].grants.quota_erasures", (o) => delete o.grants.quota_erasures],
    ["offerings[1].grants.features", (o) => (o.grants.features = [1])],
    ["offerings[1].grants.features", (o) => delete o.grants.features],
    ["offerings[1].grants.term_months", (o) => (o.grants.term_months = 0)],
    ["offerings[1].renewal_notice", (o) => (o.renewal_notice = { headline: "x" })],
    [
      "offerings[1].renewal_notice",
      (o) => {
        o.interval = "annual";
      },
    ],
    [
      "offerings[1].renewal_notice",
      (o) => {
        o.interval = "annual";
        o.renewal_notice = { detail: "no headline" };
      },
    ],
  ];

  it.each(bad)("names %s", (field, edit) => {
    let err: Error | undefined;
    try {
      parseCatalog(withSecond(edit));
    } catch (e) {
      err = e as Error;
    }
    expect(err?.message).toMatch(new RegExp(`^${field.replace(/[[\].]/g, "\\$&")}`));
  });
});

/** A fake fetch answering `status` with `body` (a string is sent as is). */
const answering =
  (status: number, body: unknown): typeof fetch =>
  async () =>
    new Response(typeof body === "string" ? body : JSON.stringify(body), {
      status,
      headers: { "content-type": "application/json" },
    });

const PREFIX = `pricing: could not read ${URL_} (PRICING_HUB_ORIGIN): `;

describe("loadOfferings", () => {
  it("fetches <origin>/api/offerings and returns the checked offerings", async () => {
    const seen: string[] = [];
    const fetchImpl: typeof fetch = async (input) => {
      seen.push(String(input));
      return new Response(JSON.stringify({ offerings: [annual(), oneOff()] }), { status: 200 });
    };
    const list = await loadOfferings(HUB, { fetchImpl });
    expect(seen).toEqual([URL_]);
    expect(list.map((o) => o.name)).toEqual(["Station annual", "Site pack"]);
  });

  it("fails on a refused connection, with the cause", async () => {
    const fetchImpl: typeof fetch = async () => {
      const cause = Object.assign(new Error("connect ECONNREFUSED 127.0.0.1:7900"), {
        code: "ECONNREFUSED",
      });
      throw new TypeError("fetch failed", { cause });
    };
    await expect(loadOfferings(HUB, { fetchImpl })).rejects.toThrow(
      `${PREFIX}fetch failed: connect ECONNREFUSED 127.0.0.1:7900`,
    );
  });

  it("fails on a timeout, naming it", async () => {
    const fetchImpl: typeof fetch = (_, init) =>
      new Promise((_resolve, reject) => {
        init?.signal?.addEventListener("abort", () =>
          reject(new DOMException("This operation was aborted", "AbortError")),
        );
      });
    await expect(loadOfferings(HUB, { fetchImpl, timeoutMs: 20 })).rejects.toThrow(
      `${PREFIX}timed out after 20 ms`,
    );
  });

  it("fails on a timeout while reading the body", async () => {
    const fetchImpl: typeof fetch = async (_, init) => {
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(new TextEncoder().encode('{"offerings": ['));
          init?.signal?.addEventListener("abort", () =>
            controller.error(new DOMException("This operation was aborted", "AbortError")),
          );
        },
      });
      return new Response(stream, { status: 200 });
    };
    await expect(loadOfferings(HUB, { fetchImpl, timeoutMs: 20 })).rejects.toThrow(
      `${PREFIX}timed out after 20 ms`,
    );
  });

  it.each([401, 404, 500])("fails on HTTP %i, naming the status", async (status) => {
    await expect(
      loadOfferings(HUB, { fetchImpl: answering(status, { error: "no" }) }),
    ).rejects.toThrow(`${PREFIX}HTTP status ${status}`);
  });

  it("fails on a body that is not JSON", async () => {
    await expect(
      loadOfferings(HUB, { fetchImpl: answering(200, "<html>hub</html>") }),
    ).rejects.toThrow(new RegExp(`^${PREFIX.replace(/[.()[\]]/g, "\\$&")}the body is not JSON`));
  });

  it("fails on a malformed offering, naming its index and field", async () => {
    await expect(
      loadOfferings(HUB, {
        fetchImpl: answering(200, withSecond((o) => (o.currency = "usd"))),
      }),
    ).rejects.toThrow(`${PREFIX}offerings[1].currency`);
    await expect(
      loadOfferings(HUB, { fetchImpl: answering(200, { items: [] }) }),
    ).rejects.toThrow(`${PREFIX}offerings: `);
  });
});

describe("labels", () => {
  it("links a free trial to the portal's /buy", () => {
    expect(trialHref(HUB)).toBe(`${HUB}/buy`);
  });

  it("formats prices like the portal, with no floats", () => {
    expect(formatPrice(120000, "USD")).toBe("1,200.00 USD");
    expect(formatPrice(5, "USD")).toBe("0.05 USD");
    expect(formatPrice(123456789, "EUR")).toBe("1,234,567.89 EUR");
    expect(formatPrice(1200000, "JPY")).toBe("1,200,000 JPY");
    expect(formatPrice(900719925474099, "USD")).toBe("9,007,199,254,740.99 USD");
  });

  it("labels interval, seats, scope, quota and term", () => {
    expect(intervalLabel("annual")).toBe("per year");
    expect(intervalLabel("one_off")).toBe("one-off payment");
    expect(seatsLabel(1)).toBe("1 seat");
    expect(seatsLabel(5)).toBe("5 seats");
    expect(seatsLabel(1500)).toBe("1,500 seats");
    expect(scopeLabel("machine")).toBe("Machine licence");
    expect(scopeLabel("site")).toBe("Site licence");
    expect(quotaLabel(null)).toBe("Unlimited erasures");
    expect(quotaLabel(1)).toBe("1 erasure");
    expect(quotaLabel(10000)).toBe("10,000 erasures");
    expect(termLabel(12)).toBe("12-month term");
  });
});

describe("parseHubOrigin for PRICING_HUB_ORIGIN", () => {
  it("accepts a bare origin", () => {
    expect(parseHubOrigin("http://127.0.0.1:7900/", "PRICING_HUB_ORIGIN")).toBe(HUB);
    expect(parseHubOrigin(undefined, "PRICING_HUB_ORIGIN")).toBe("");
  });

  it.each(["localhost:7900", "http://h/path", "ftp://h"])("names the variable for %s", (v) => {
    expect(() => parseHubOrigin(v, "PRICING_HUB_ORIGIN")).toThrow(
      `PRICING_HUB_ORIGIN ${JSON.stringify(v)} must be an http(s) origin`,
    );
  });

  it("still names PUBLIC_HUB_ORIGIN by default", () => {
    expect(() => parseHubOrigin("ftp://h")).toThrow(/^PUBLIC_HUB_ORIGIN /);
  });
});
