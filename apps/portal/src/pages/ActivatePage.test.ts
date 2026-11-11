import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  RouterContextProvider,
  createMemoryHistory,
  createRootRoute,
  createRouter,
} from "@tanstack/react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import { HubError, type Account, type Role } from "@/api/auth";
import type { PendingEnrollment } from "@/api/enrollments";
import type { Entitlement } from "@/api/orders";
import type { Licence, Station } from "@/api/stations";
import {
  activatePath,
  ActivatePage,
  ActivateSignIn,
  ApprovedPanel,
  CodeEntryForm,
  DeniedPanel,
  EnrollmentDetails,
  NotOwnerPanel,
  SeatPicker,
  utcMinute,
} from "./ActivatePage";
import { safeNext, signInSearch } from "./SignInPage";
import { ActivateLink, seatChoices } from "./StationsPage";

/** `node` inside a memory router, so its links render. */
function withRouter(node: ReturnType<typeof createElement>) {
  const rootRoute = createRootRoute();
  const router = createRouter({
    routeTree: rootRoute,
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  return createElement(RouterContextProvider, { router, children: node });
}

function render(node: ReturnType<typeof createElement>): string {
  return renderToStaticMarkup(withRouter(node));
}

function text(html: string): string {
  return html
    .replace(/<[^>]+>/g, " ")
    .replace(/&#x27;/g, "'")
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ");
}

/** Every `href` in the markup. */
function hrefs(html: string): string[] {
  return [...html.matchAll(/href="([^"]*)"/g)].map((m) => m[1].replace(/&amp;/g, "&"));
}

const KEY = "ed25519:q83vEjRWeJA0MlR4kKvN7w";

const enrollment: PendingEnrollment = {
  user_code: "BCDF-GHJK",
  station_id: "bench-1",
  label: "Bench 1",
  instance_key_id: KEY,
  requested_from: "203.0.113.7",
  requested_at: "2026-11-09T10:02:00Z",
  expires_at: "2026-11-09T10:12:00Z",
};

const machine: Entitlement = {
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

const site: Entitlement = {
  ...machine,
  id: "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
  offering_name: "Site — 2 seats",
  interval: "one_off",
  grants: { scope: "site", seats: 2, quota_erasures: 5000, features: [], term_months: 24 },
  renews: false,
};

const licence: Licence = {
  id: "a1b2c3d4-0000-4000-8000-000000000001",
  station_id: "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  entitlement_id: machine.id,
  instance_key_id: KEY,
  root_key_id: "ed25519:AAAAAAAAAAAAAAAAAAAAAA",
  not_after: "2027-11-08T10:00:00Z",
  issued_at: "2026-11-09T10:03:00Z",
  status: "active",
  superseded_at: null,
  revoked_at: null,
};

const station: Station = {
  id: licence.station_id,
  entitlement_id: machine.id,
  offering_name: machine.offering_name,
  scope: "machine",
  instance_key_id: KEY,
  station_id: "bench-1",
  label: "Bench 1",
  site: null,
  registered_at: "2026-11-09T10:03:00Z",
  released_at: null,
  licence,
  renewal_available: false,
  credit_balance: null,
  credit_balance_reported_at: null,
  credits_granted: 0,
};

describe("activate page", () => {
  it("names its own path with the code", () => {
    expect(activatePath()).toBe("/activate");
    expect(activatePath("  ")).toBe("/activate");
    expect(activatePath("BCDF-GHJK")).toBe("/activate?code=BCDF-GHJK");
    expect(activatePath("a b&c")).toBe("/activate?code=a%20b%26c");
  });

  it("shows times to the minute in UTC", () => {
    expect(utcMinute("2026-11-09T10:02:59Z")).toBe("2026-11-09 10:02 UTC");
  });

  it("signed out, links to sign-in and back here with the code", () => {
    for (const [code, back] of [
      ["BCDF-GHJK", "/activate?code=BCDF-GHJK"],
      [undefined, "/activate"],
    ] as const) {
      const html = render(createElement(ActivateSignIn, { code }));
      const [href] = hrefs(html);
      const url = new URL(href, "http://portal.test");
      expect(url.pathname).toBe("/sign-in");
      expect(url.searchParams.get("next")).toBe(back);
      expect(safeNext(url.searchParams.get("next") ?? undefined)).toBe(back);
      expect(text(html)).toMatch(/Sign in as an owner of your organization/);
    }
  });

  it("tells anyone but an owner who can approve", () => {
    const html = renderToStaticMarkup(createElement(NotOwnerPanel));
    expect(html).toContain('role="alert"');
    expect(text(html)).toMatch(/Only an organization owner can approve or deny/);
    expect(html).not.toContain("<form");
    expect(html).not.toContain("<button");
  });

  it("prefills the code field and shows hub errors, a lockout in minutes", () => {
    const entry = (error: unknown) =>
      renderToStaticMarkup(
        createElement(CodeEntryForm, {
          code: "BCDF-GHJK",
          onCodeChange: () => {},
          onSubmit: () => {},
          pending: false,
          error,
        }),
      );
    const clean = entry(null);
    expect(clean).toContain('name="code"');
    expect(clean).toContain('value="BCDF-GHJK"');
    expect(clean).toContain(">Look up</button>");
    expect(clean).not.toContain('role="alert"');

    const missing = entry(new HubError(404, "no station is waiting with this code"));
    expect(text(missing)).toContain("no station is waiting with this code");
    const locked = entry(new HubError(429, "too many wrong enrollment codes", 900));
    expect(text(locked)).toContain("Too many wrong codes. Try again in 15 minutes.");
  });

  it("shows what is asking: station id, label, key id, address and time", () => {
    const t = text(renderToStaticMarkup(createElement(EnrollmentDetails, { enrollment })));
    for (const want of [
      "BCDF-GHJK",
      "bench-1",
      "Bench 1",
      KEY,
      "203.0.113.7",
      "2026-11-09 10:02 UTC",
      "2026-11-09 10:12 UTC",
    ]) {
      expect(t).toContain(want);
    }
    const bare = text(
      renderToStaticMarkup(
        createElement(EnrollmentDetails, {
          enrollment: { ...enrollment, label: null, requested_from: null },
        }),
      ),
    );
    expect(bare).toContain("none");
    expect(bare).toContain("unknown address");
  });

  it("offers the current entitlements, those without a free seat disabled", () => {
    const choices = seatChoices([machine, site], [station]);
    const html = render(
      createElement(SeatPicker, { choices, value: site.id, onChange: () => {} }),
    );
    expect(html).toContain('name="entitlement_id"');
    expect(html).toMatch(new RegExp(`value="${machine.id}" disabled`));
    expect(html).not.toMatch(new RegExp(`value="${site.id}" disabled`));
    expect(text(html)).toContain("0 of 1 free");
    expect(text(html)).toContain("2 of 2 free");
  });

  it("says when no seat is free and links to buy one", () => {
    const html = render(
      createElement(SeatPicker, {
        choices: seatChoices([machine], [station]),
        value: "",
        onChange: () => {},
      }),
    );
    expect(text(html)).toMatch(/No licence seat is free/);
    expect(hrefs(html)).toContain("/buy");
    expect(html).not.toContain("<select");
    const none = render(createElement(SeatPicker, { choices: [], value: "", onChange: () => {} }));
    expect(text(none)).toMatch(/No licence seat is free/);
  });

  it("approved: names the station and its licence expiry, collected by the station itself", () => {
    const html = render(createElement(ApprovedPanel, { registered: { station, licence, credit_grant: null } }));
    const t = text(html);
    expect(t).toContain("Approved. Bench 1 (bench-1) is registered, licensed to 2027-11-08.");
    expect(t).toMatch(/collects its licence by itself/);
    expect(hrefs(html)).toContain("/stations");
  });

  it("denied: says nothing was registered", () => {
    const t = text(renderToStaticMarkup(createElement(DeniedPanel, { enrollment })));
    expect(t).toMatch(/Denied\. Station bench-1 was not registered/);
  });

  it("is linked from the owner's stations page", () => {
    expect(hrefs(render(createElement(ActivateLink)))).toContain("/activate");
  });
});

describe("safeNext", () => {
  it("follows a same-origin path", () => {
    for (const next of ["/", "/activate", "/activate?code=BCDF-GHJK", "/stations#x"]) {
      expect(safeNext(next)).toBe(next);
    }
  });

  it("falls back to the home page for anything else", () => {
    for (const next of [
      undefined,
      "",
      "activate",
      "//evil.example/activate",
      "/\\evil.example",
      "https://evil.example/",
      "javascript:alert(1)",
      "/\t/evil.example",
      "/\n/evil.example",
      " /activate",
    ]) {
      expect(safeNext(next), JSON.stringify(next)).toBe("/");
    }
  });
});

/** The whole page, with the signed-in account (`null`: signed out) cached. */
function renderPage(account: Account | null, code?: string): string {
  const client = new QueryClient();
  client.setQueryData(["me"], account);
  return render(
    createElement(QueryClientProvider, { client }, createElement(ActivatePage, { code })),
  );
}

function account(role: Role): Account {
  return {
    user: { id: "u1", email: "a@b.co", name: "A", role, email_verified: true },
    organization: { id: "o1", name: "Acme Recyclers" },
  };
}

describe("ActivatePage picks the panel for who is signed in", () => {
  it("signed out: the sign-in link back here with the code, no code field", () => {
    const html = renderPage(null, "BCDF-GHJK");
    const signIn = hrefs(html).find((h) => h.startsWith("/sign-in"));
    expect(signIn).toBeDefined();
    expect(new URL(signIn!, "http://portal.test").searchParams.get("next")).toBe(
      "/activate?code=BCDF-GHJK",
    );
    expect(html).not.toContain('name="code"');
  });

  it("a member or vendor admin: who can approve, and no code entry to send from", () => {
    for (const role of ["member", "vendor_admin"] as const) {
      const html = renderPage(account(role), "BCDF-GHJK");
      expect(text(html), role).toMatch(/Only an organization owner can approve or deny/);
      expect(html, role).not.toContain('name="code"');
      expect(html, role).not.toContain("<form");
    }
  });

  it("an owner: the code field, prefilled from the link", () => {
    const html = renderPage(account("owner"), "BCDF-GHJK");
    expect(html).toContain('name="code"');
    expect(html).toContain('value="BCDF-GHJK"');
    expect(text(html)).not.toMatch(/Only an organization owner/);
    expect(renderPage(account("owner"))).toContain('value=""');
  });
});

describe("signInSearch", () => {
  it("keeps a string next and drops anything else", () => {
    expect(signInSearch({ next: "/activate?code=BCDF-GHJK" })).toEqual({
      next: "/activate?code=BCDF-GHJK",
    });
    expect(signInSearch({ next: 7 })).toEqual({});
    expect(signInSearch({})).toEqual({});
  });
});
