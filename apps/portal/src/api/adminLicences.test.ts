import { beforeEach, describe, expect, it } from "vitest";
import { HubError, resetCsrfCache } from "./auth";
import {
  adminLicenceDownloadPath,
  expiryAfter,
  fetchLicence,
  inspectLicence,
  isAdminLicence,
  isInspected,
  isLicenceDetail,
  isLicenseBody,
  issueBody,
  issueLicence,
  licenceHolderText,
  licenceStatusText,
  listOrganizationLicences,
  quotaText,
  revokeLicence,
  scopeText,
  type AdminLicence,
  type IssueInput,
  type LicenseBody,
  type Verification,
} from "./adminLicences";

const ORG = "0f8fad5b-d9cb-469f-a165-70867728950e";
const KEY = "ed25519:q83vEjRWeJA0MlR4kKvN7w";

const vendorLicence: AdminLicence = {
  id: "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
  organization: { id: ORG, name: "Acme Recyclers" },
  source: "vendor",
  station: null,
  entitlement_id: null,
  instance_key_id: KEY,
  root_key_id: "ed25519:AAAAAAAAAAAAAAAAAAAAAA",
  not_after: "2026-12-08T10:00:00Z",
  issued_at: "2026-11-08T10:00:00Z",
  status: "active",
  superseded_at: null,
  revoked_at: null,
  issue_reason: "30-day evaluation",
  revocation_reason: null,
  lapse_policy: "evaluation",
};

const stationLicence: AdminLicence = {
  ...vendorLicence,
  id: "a1b2c3d4-0000-4000-8000-000000000001",
  source: "station",
  station: { id: "7c9e6679-7425-40de-944b-e07fc1f90ae7", station_id: "bench-1", label: "Bench 1" },
  entitlement_id: "5b0d8b4e-6f1e-4c3a-9d2e-1f6c2a7b8c90",
  issue_reason: null,
};

const body: LicenseBody = {
  license_format_version: 1,
  license_id: vendorLicence.id,
  root_key_id: vendorLicence.root_key_id,
  instance_public_key_id: KEY,
  entitlements: {
    customer_id: ORG,
    customer_name: "Acme Recyclers",
    quota: { kind: "unlimited" },
    scope: { kind: "machine", fingerprint: "mf1:abc" },
    not_before: "2026-11-08T10:00:00Z",
    not_after: "2026-12-08T10:00:00Z",
    features: ["hub_sync"],
    allowed_methods: { kind: "all" },
  },
  issued_at: "2026-11-08T10:00:00Z",
};

const verified: Verification = {
  status: "verified",
  root_key_id: vendorLicence.root_key_id,
  detail: null,
};

interface Call {
  url: string;
  method: string;
  body: unknown;
  csrf: string | null;
}

/** A fake hub, as in `adminOfferings.test.ts`. */
function respond(status: number, reply?: unknown) {
  const calls: Call[] = [];
  const impl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const method = init?.method ?? "GET";
    if (url === "/api/auth/csrf" && method === "GET") {
      return new Response(JSON.stringify({ csrf_token: "csrf-1" }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    const sent = (init?.headers ?? {}) as Record<string, string>;
    calls.push({
      url,
      method,
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
      csrf: sent["X-CSRF-Token"] ?? null,
    });
    return new Response(reply === undefined ? null : JSON.stringify(reply), {
      status,
      headers: { "Content-Type": "application/json" },
    });
  }) as typeof fetch;
  return { impl, calls };
}

beforeEach(() => resetCsrfCache());

describe("guards", () => {
  it("accept the hub's shapes", () => {
    expect(isAdminLicence(vendorLicence)).toBe(true);
    expect(isAdminLicence(stationLicence)).toBe(true);
    expect(
      isAdminLicence({
        ...stationLicence,
        status: "revoked",
        revoked_at: "2026-11-09T00:00:00Z",
        revocation_reason: "fraud",
      }),
    ).toBe(true);
    expect(isLicenseBody(body)).toBe(true);
    expect(
      isLicenseBody({
        ...body,
        entitlements: {
          ...body.entitlements,
          quota: { kind: "count", erasures: 5 },
          scope: { kind: "site", site_id: "Floor 2" },
          machine_binding: "mf1:x",
        },
      }),
    ).toBe(true);
    expect(isAdminLicence({ ...vendorLicence, lapse_policy: "stop" })).toBe(true);
    expect(isLicenseBody({ ...body, lapse_policy: "stop" })).toBe(true);
    expect(isLicenseBody({ ...body, lapse_policy: "evaluation" })).toBe(true);
    expect(isLicenceDetail({ licence: vendorLicence, body, verification: verified })).toBe(true);
    expect(isInspected({ body, verification: verified, licence: null })).toBe(true);
  });

  it("reject malformed ones", () => {
    expect(isAdminLicence({ ...vendorLicence, source: "cli" })).toBe(false);
    expect(isAdminLicence({ ...vendorLicence, status: "cancelled" })).toBe(false);
    expect(isAdminLicence({ ...vendorLicence, revocation_reason: undefined })).toBe(false);
    expect(isAdminLicence({ ...stationLicence, station: { id: 1 } })).toBe(false);
    expect(isLicenseBody({ ...body, entitlements: { ...body.entitlements, quota: { kind: "count" } } })).toBe(
      false,
    );
    expect(isLicenseBody({ ...body, license_id: 7 })).toBe(false);
    expect(isAdminLicence({ ...vendorLicence, lapse_policy: undefined })).toBe(false);
    expect(isAdminLicence({ ...vendorLicence, lapse_policy: "sometimes" })).toBe(false);
    expect(isLicenseBody({ ...body, lapse_policy: "sometimes" })).toBe(false);
    expect(
      isInspected({ body, verification: { ...verified, status: "maybe" }, licence: null }),
    ).toBe(false);
  });
});

describe("issueBody", () => {
  const input: IssueInput = {
    instance_key_id: ` ${KEY} `,
    scope: "machine",
    station_id: " bench-1 ",
    site: "Floor 2",
    quota_erasures: null,
    features: ["hub_sync"],
    days: 30,
    reason: "  evaluation ",
  };

  it("trims, keeps the scope's own field and drops the other", () => {
    expect(issueBody(input)).toEqual({
      instance_key_id: KEY,
      scope: "machine",
      station_id: "bench-1",
      quota_erasures: null,
      features: ["hub_sync"],
      days: 30,
      reason: "evaluation",
    });
    expect(issueBody({ ...input, scope: "site", site: "  " })).toEqual({
      instance_key_id: KEY,
      scope: "site",
      quota_erasures: null,
      features: ["hub_sync"],
      days: 30,
      reason: "evaluation",
    });
  });
});

describe("text", () => {
  it("states status, holder, quota and scope", () => {
    expect(licenceStatusText(vendorLicence)).toBe("Active");
    expect(
      licenceStatusText({ ...vendorLicence, status: "revoked", revoked_at: "2026-11-09T01:00:00Z" }),
    ).toBe("Revoked on 2026-11-09");
    expect(
      licenceStatusText({
        ...vendorLicence,
        status: "superseded",
        superseded_at: "2026-11-10T01:00:00Z",
      }),
    ).toBe("Superseded on 2026-11-10");
    expect(licenceHolderText(vendorLicence)).toBe("Vendor-issued");
    expect(licenceHolderText(stationLicence)).toBe("Bench 1 (bench-1)");
    expect(quotaText({ kind: "unlimited" })).toBe("Unlimited erasures");
    expect(quotaText({ kind: "count", erasures: 5000 })).toBe("5,000 erasures");
    expect(scopeText({ kind: "machine", fingerprint: "mf1:abc" })).toBe("Machine mf1:abc");
    expect(scopeText({ kind: "site", site_id: "Floor 2" })).toBe("Site Floor 2");
    expect(expiryAfter(30, new Date("2026-11-08T10:00:00Z"))).toBe("2026-12-08");
  });
});

describe("calls", () => {
  it("lists an organization's licences", async () => {
    const f = respond(200, { licences: [vendorLicence, stationLicence] });
    expect(await listOrganizationLicences(ORG, f.impl)).toEqual([vendorLicence, stationLicence]);
    expect(f.calls).toEqual([
      { url: `/api/admin/organizations/${ORG}/licences`, method: "GET", body: undefined, csrf: null },
    ]);
  });

  it("issues with the trimmed body and CSRF", async () => {
    const f = respond(201, { licence: vendorLicence });
    expect(
      await issueLicence(
        ORG,
        {
          instance_key_id: KEY,
          scope: "site",
          quota_erasures: 10,
          features: [],
          days: 7,
          reason: " trial ",
        },
        f.impl,
      ),
    ).toEqual(vendorLicence);
    expect(f.calls).toEqual([
      {
        url: `/api/admin/organizations/${ORG}/licences`,
        method: "POST",
        body: {
          instance_key_id: KEY,
          scope: "site",
          quota_erasures: 10,
          features: [],
          days: 7,
          reason: "trial",
        },
        csrf: "csrf-1",
      },
    ]);
  });

  it("views a licence", async () => {
    const detail = { licence: vendorLicence, body, verification: verified };
    const f = respond(200, detail);
    expect(await fetchLicence(vendorLicence.id, f.impl)).toEqual(detail);
    expect(f.calls[0].url).toBe(`/api/admin/licences/${vendorLicence.id}`);
  });

  it("inspects a pasted file with a POST", async () => {
    const reply = {
      body,
      verification: { ...verified, status: "failed", detail: "licence was tampered with: x" },
      licence: null,
    };
    const f = respond(200, reply);
    expect(await inspectLicence("{…}", f.impl)).toEqual(reply);
    expect(f.calls).toEqual([
      { url: "/api/admin/licences/inspect", method: "POST", body: { document: "{…}" }, csrf: "csrf-1" },
    ]);
  });

  it("revokes with the trimmed reason and surfaces a second revocation", async () => {
    const revoked = {
      ...vendorLicence,
      status: "revoked",
      revoked_at: "2026-11-09T00:00:00Z",
      revocation_reason: "fraud",
    };
    const f = respond(200, { licence: revoked });
    expect(await revokeLicence(vendorLicence.id, " fraud ", f.impl)).toEqual(revoked);
    expect(f.calls).toEqual([
      {
        url: `/api/admin/licences/${vendorLicence.id}/revoke`,
        method: "POST",
        body: { reason: "fraud" },
        csrf: "csrf-1",
      },
    ]);
    const message = "this licence is already revoked";
    const e = await revokeLicence(vendorLicence.id, "x", respond(409, { error: message }).impl).then(
      () => undefined,
      (err: unknown) => err,
    );
    expect(e).toBeInstanceOf(HubError);
    expect((e as HubError).status).toBe(409);
    expect((e as HubError).message).toBe(message);
  });

  it("downloads from the admin path", () => {
    expect(adminLicenceDownloadPath(vendorLicence.id)).toBe(
      `/api/admin/licences/${vendorLicence.id}/download`,
    );
  });
});
