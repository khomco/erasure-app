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
