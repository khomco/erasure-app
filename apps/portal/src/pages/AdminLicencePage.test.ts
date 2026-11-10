import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  RouterContextProvider,
  createMemoryHistory,
  createRootRoute,
  createRouter,
} from "@tanstack/react-router";
import { describe, expect, it } from "vitest";
import type { AdminLicence, LicenseBody, Verification } from "@/api/adminLicences";
import {
  LicenceBodyView,
  LicenceRecord,
  RevocationNote,
  verificationText,
} from "./AdminLicencePage";
import { fitsInspection } from "./AdminLicenceInspectPage";
import { IssueLicenceNote, LicenceTable, OrganizationView } from "./AdminOrganizationPage";

/** `node` inside a router, so its links render. */
function withRouter(node: ReturnType<typeof createElement>) {
  const rootRoute = createRootRoute();
  const router = createRouter({
    routeTree: rootRoute,
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  return createElement(RouterContextProvider, { router, children: node });
}

const licence: AdminLicence = {
  id: "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
  organization: { id: "0f8fad5b-d9cb-469f-a165-70867728950e", name: "Acme Recyclers" },
  source: "vendor",
  station: null,
  entitlement_id: null,
  instance_key_id: "ed25519:q83vEjRWeJA0MlR4kKvN7w",
  root_key_id: "ed25519:AAAAAAAAAAAAAAAAAAAAAA",
  not_after: "2026-12-08T10:00:00Z",
  issued_at: "2026-11-08T10:00:00Z",
  status: "active",
  superseded_at: null,
  revoked_at: null,
  issue_reason: "evaluation",
  revocation_reason: null,
};

const body: LicenseBody = {
  license_format_version: 1,
  license_id: licence.id,
  root_key_id: licence.root_key_id,
  instance_public_key_id: licence.instance_key_id,
  entitlements: {
    customer_id: licence.organization.id,
    customer_name: "Acme Recyclers",
    quota: { kind: "count", erasures: 5000 },
    scope: { kind: "site", site_id: "Floor 2" },
    not_before: "2026-11-08T10:00:00Z",
    not_after: "2026-12-08T10:00:00Z",
    features: ["hub_sync"],
    allowed_methods: { kind: "all" },
  },
  issued_at: "2026-11-08T10:00:00Z",
};

function text(html: string): string {
  return html.replace(/<[^>]+>/g, " ").replace(/&#x27;/g, "'").replace(/\s+/g, " ");
}

describe("verification", () => {
  it("states each outcome", () => {
    const root = licence.root_key_id;
    expect(verificationText({ status: "verified", root_key_id: root, detail: null })).toBe(
      `Verified: signed by this hub's vendor root (${root}).`,
    );
    expect(
      verificationText({
        status: "failed",
        root_key_id: root,
        detail: "licence was tampered with: license payload was modified after signing",
      }),
    ).toContain("Does not verify against this hub's vendor root: licence was tampered with");
    expect(verificationText({ status: "no_root", root_key_id: null, detail: null })).toBe(
      "Not checked: this hub has no vendor root configured.",
    );
  });

  it("shows the decoded body with its verification", () => {
    const failed: Verification = { status: "failed", root_key_id: null, detail: "x" };
    const t = text(renderToStaticMarkup(createElement(LicenceBodyView, { body, verification: failed })));
    expect(t).toContain("Does not verify");
    expect(t).toContain("Acme Recyclers");
    expect(t).toContain("Site Floor 2");
    expect(t).toContain("5,000 erasures");
    expect(t).toContain("hub_sync");
  });

  it("never shows a lapse policy, even one an old file carries (retired, ADR-0013 §5)", () => {
    const verified: Verification = { status: "verified", root_key_id: null, detail: null };
    const t = text(
      renderToStaticMarkup(
        createElement(LicenceBodyView, { body: { ...body, lapse_policy: "stop" }, verification: verified }),
      ),
    );
    expect(t).not.toMatch(/lapse/i);
    expect(t).not.toContain("Stop new jobs");
    const r = text(renderToStaticMarkup(withRouter(createElement(LicenceRecord, { licence }))));
    expect(r).not.toMatch(/lapse/i);
  });
});

describe("plain statements", () => {
  it("says revocation is hub-only and the file keeps verifying offline until it expires", () => {
    const t = text(renderToStaticMarkup(createElement(RevocationNote, { source: "station" })));
    expect(t).toContain("recorded in this hub only");
    expect(t).toContain("Stations are not told");
    expect(t).toContain("keeps verifying it offline");
    expect(t).toContain("until its expiry date");
    expect(t).toContain("not the reason");
    expect(t).toContain("register its key again");
  });

  it("does not promise a customer-side effect for a vendor-issued licence", () => {
    const t = text(renderToStaticMarkup(createElement(RevocationNote, { source: "vendor" })));
    expect(t).toContain("recorded in this hub only");
    expect(t).toContain("never sees it in the portal");
    expect(t).not.toContain("not the reason");
  });

  it("says an issued licence uses no seat", () => {
    const t = text(renderToStaticMarkup(createElement(IssueLicenceNote)));
    expect(t).toContain("uses no seat and no entitlement");
    expect(t).toContain("does not appear on the customer's Stations page");
  });

  it("caps a pasted file at 64 KiB", () => {
    expect(fitsInspection("x".repeat(64 * 1024))).toBe(true);
    expect(fitsInspection("x".repeat(64 * 1024 + 1))).toBe(false);
    // Counted in UTF-8 bytes, as the hub counts.
    expect(fitsInspection("é".repeat(32 * 1024 + 1))).toBe(false);
  });
});

describe("licence rows", () => {
  const station: AdminLicence = {
    ...licence,
    id: "a1b2c3d4-0000-4000-8000-000000000001",
    source: "station",
    station: { id: "7c9e6679-7425-40de-944b-e07fc1f90ae7", station_id: "bench-1", label: "Bench 1" },
    status: "revoked",
    revoked_at: "2026-11-20T08:00:00Z",
    revocation_reason: "fraud",
    issue_reason: null,
  };

  it("link each licence and state its holder and status", () => {
    const html = renderToStaticMarkup(
      withRouter(createElement(LicenceTable, { licences: [licence, station] })),
    );
    expect(html).toContain(`href="/admin/licences/${licence.id}"`);
    expect(html).toContain(`href="/admin/licences/${station.id}"`);
    const t = text(html);
    expect(t).toContain("Vendor-issued");
    expect(t).toContain("Active");
    expect(t).toContain("Bench 1 (bench-1)");
    expect(t).toContain("Revoked on 2026-11-20");
    expect(t).toContain("2026-12-08");
  });

  it("say none when there are none", () => {
    expect(text(renderToStaticMarkup(createElement(LicenceTable, { licences: [] })))).toContain(
      "None.",
    );
  });
});

describe("admin organization page", () => {
  it("shows no lapse policy (retired, ADR-0013 §5)", () => {
    const detail = {
      organization: { id: licence.organization.id, name: "Acme Recyclers", created_at: "2026-11-01T00:00:00Z" },
      users: [],
    };
    const t = text(renderToStaticMarkup(withRouter(createElement(OrganizationView, { detail }))));
    expect(t).toContain("Acme Recyclers");
    expect(t).not.toMatch(/lapse/i);
    expect(t).not.toContain("Change to");
  });
});
