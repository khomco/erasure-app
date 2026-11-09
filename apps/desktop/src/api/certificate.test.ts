import { describe, expect, it } from "vitest";

import type {
  AttestationSigner,
  CertDestruction,
  Device,
  JobActivity,
  SignedCertificate,
} from "./types";
import {
  concurrenceLabel,
  deviceSummary,
  identifierSourceLabel,
  licenceIsWarning,
  licencePill,
  licenceSummary,
  recordedForDestruction,
} from "./certificate";

const signer: AttestationSigner = {
  operator: { id: "op-2", display_name: "Bo", email: "bo@example.com" },
  full_name: "Bo Chen",
  title: "Site supervisor",
  identity_assurance: { level: "self_asserted" },
};

function record(overrides: Partial<CertDestruction> = {}): CertDestruction {
  return {
    manifest_id: "run-1",
    destruction_event_id: "ev-1",
    technique: "shred",
    standard_label: "NIST SP 800-88 Destroy",
    nist_destroy: true,
    completion: {} as CertDestruction["completion"],
    required_concurrence: null,
    concurrence: null,
    ...overrides,
  };
}

function cert(device: Device | undefined, activities: JobActivity[] = []): SignedCertificate {
  return {
    certificate: { device, activities, destruction: record() },
  } as unknown as SignedCertificate;
}

describe("identifierSourceLabel", () => {
  it("says where the identifiers came from", () => {
    expect(identifierSourceLabel("scanned")).toBe("Scanned from the drive's labels");
    expect(identifierSourceLabel("device")).toBe("Read from the device");
  });
});

describe("concurrenceLabel", () => {
  it("names a witness as a witness and a reviewer as a reviewer", () => {
    const at = "2026-11-08T10:00:00Z";
    const base = { manifest_id: "run-1", at, signer, statement: "…" };
    expect(
      concurrenceLabel(
        record({
          required_concurrence: "witness",
          concurrence: { ...base, kind: "witness" },
        }),
      ),
    ).toBe("Witnessed by Bo Chen, Site supervisor");
    expect(
      concurrenceLabel(
        record({ required_concurrence: "review", concurrence: { ...base, kind: "review" } }),
      ),
    ).toBe("Reviewed by Bo Chen, Site supervisor");
  });

  it("says when none was required, and when a required one is missing", () => {
    expect(concurrenceLabel(record())).toBe("None required by the site's policy");
    expect(concurrenceLabel(record({ required_concurrence: "review" }))).toBe(
      "Missing: a review was required",
    );
  });
});

describe("deviceSummary", () => {
  it("shows the device read from the drive", () => {
    const device = {
      vendor: "Samsung",
      model: "990 PRO",
      serial: "S6AB",
      capacity_bytes: 2_000_000_000_000,
    } as Device;
    const s = deviceSummary(cert(device));
    expect(s.title).toBe("Samsung 990 PRO");
    expect(s.detail.startsWith("S6AB · ")).toBe(true);
  });

  it("says plainly that nothing was read from a never-erased drive", () => {
    expect(deviceSummary(cert(undefined)).title).toBe("Not read from the device");
  });
});

describe("recordedForDestruction", () => {
  it("finds the attestation on the activity the record names", () => {
    const attestation = { statement: "I attest" } as never;
    const activities = [
      { type: "destruction", id: "ev-0" },
      { type: "destruction", id: "ev-1", attestation },
    ] as unknown as JobActivity[];
    expect(recordedForDestruction(cert(undefined, activities))).toBe(attestation);
    expect(recordedForDestruction(cert(undefined, []))).toBeUndefined();
  });
});

describe("licenceSummary", () => {
  const chain = {
    root_key_id: "root-1",
    license: {
      body: {
        license_format_version: 1,
        license_id: "lic-1",
        root_key_id: "root-1",
        instance_public_key_id: "key-1",
        entitlements: {
          customer_id: "c-1",
          customer_name: "Acme",
          quota: { kind: "unlimited" },
          scope: { kind: "site", site_id: "s-1" },
          not_before: "2026-01-01T00:00:00Z",
          not_after: "2027-01-01T00:00:00Z",
          features: [],
          allowed_methods: { kind: "all" },
        },
        issued_at: "2026-01-01T00:00:00Z",
      },
      signature: {
        algorithm: "ed25519",
        public_key_id: "root-1",
        canonical_sha256_hex: "00",
        signature_b64: "AA==",
      },
    },
  };

  function signed(evaluation: boolean | undefined, attestation?: unknown): SignedCertificate {
    const certificate = evaluation === undefined ? {} : { evaluation };
    return (
      attestation === undefined ? { certificate } : { certificate, attestation }
    ) as unknown as SignedCertificate;
  }

  it("reads an evaluation certificate from the signed marker", () => {
    const s = licenceSummary(signed(true));
    expect(s.kind).toBe("evaluation");
    expect(s.title).toBe("Evaluation certificate");
    expect(s.detail).toContain("without a valid licence");
    expect(s.detail).toContain("still valid");
  });

  it("names the licensed customer without claiming the chain was checked", () => {
    const s = licenceSummary(signed(false, chain));
    expect(s.kind).toBe("licensed");
    expect(s.title).toBe("Licensed to Acme (c-1)");
    expect(s.detail).toContain("lic-1");
    expect(s.detail).toContain("does not check");
    expect(s.detail).toContain("--vendor-root-b64");
  });

  it("warns when an evaluation certificate carries a licence", () => {
    const s = licenceSummary(signed(true, chain));
    expect(s.kind).toBe("inconsistent");
    expect(s.detail).toContain("Warning");
    expect(s.detail).toContain("evaluation");
  });

  it("reads evaluation with a null attestation as evaluation, not inconsistent", () => {
    expect(licenceSummary(signed(true, null)).kind).toBe("evaluation");
  });

  it("warns on an evaluation certificate with a malformed chain, naming no customer", () => {
    const s = licenceSummary(signed(true, { license: { body: {} } }));
    expect(s.kind).toBe("inconsistent");
    expect(s.detail).not.toContain("naming");
  });

  it("says a pre-licensing certificate has no licence and no evaluation mark", () => {
    const s = licenceSummary(signed(false));
    expect(s.kind).toBe("unmarked");
    expect(s.title).toBe("No licence attached");
    expect(s.detail).toContain("does not mark itself evaluation");
    expect(licenceSummary(signed(false, null)).kind).toBe("unmarked");
  });

  it("reports a malformed chain as unreadable without throwing", () => {
    for (const bad of [
      {},
      "chain",
      42,
      [],
      { license: null },
      { license: { body: { entitlements: { customer_name: 7 } } } },
    ]) {
      const s = licenceSummary(signed(false, bad));
      expect(s.kind).toBe("unreadable");
      expect(s.detail).toContain("--vendor-root-b64");
    }
    expect(() => licenceSummary(null as unknown as SignedCertificate)).not.toThrow();
  });

  it("treats a certificate without the evaluation key as not evaluation", () => {
    expect(licenceSummary(signed(undefined)).kind).toBe("unmarked");
    expect(licenceSummary(signed(undefined, chain)).kind).toBe("licensed");
  });
});

describe("licencePill", () => {
  it("shows Evaluation in warning tone for an evaluation certificate", () => {
    expect(licencePill("evaluation")).toEqual({ label: "Evaluation", tone: "pill-warning" });
    expect(licenceIsWarning("evaluation")).toBe(true);
  });

  it("shows Evaluation in warning tone when an evaluation certificate carries a licence", () => {
    expect(licencePill("inconsistent")).toEqual({ label: "Evaluation", tone: "pill-warning" });
    expect(licenceIsWarning("inconsistent")).toBe(true);
  });

  it("shows Licence unreadable in warning tone for an unreadable chain", () => {
    expect(licencePill("unreadable")).toEqual({
      label: "Licence unreadable",
      tone: "pill-warning",
    });
    expect(licenceIsWarning("unreadable")).toBe(true);
  });

  it("shows Licensed in neutral info tone, since the chain is not checked here", () => {
    expect(licencePill("licensed")).toEqual({ label: "Licensed", tone: "pill-info" });
    expect(licenceIsWarning("licensed")).toBe(false);
  });

  it("shows no pill for an unmarked certificate", () => {
    expect(licencePill("unmarked")).toBeNull();
    expect(licenceIsWarning("unmarked")).toBe(false);
  });
});
