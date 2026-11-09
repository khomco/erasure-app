import { describe, expect, it } from "vitest";

import type { Classification } from "./types";
import {
  DESTRUCTION_TECHNIQUES,
  identityAssuranceLabel,
  isNistDestroy,
  techniqueCaution,
  techniqueName,
  techniqueStandardLabel,
} from "./destruction";

describe("destruction techniques", () => {
  it("mirrors DestructMethod::ALL", () => {
    expect(DESTRUCTION_TECHNIQUES).toEqual([
      "shred",
      "disintegrate",
      "incinerate",
      "pulverize",
      "melt",
      "crush",
      "punch",
      "degauss",
      "drill",
    ]);
  });

  it("labels the five NIST Destroy techniques and the rest honestly", () => {
    for (const t of ["shred", "disintegrate", "incinerate", "pulverize", "melt"] as const) {
      expect(isNistDestroy(t)).toBe(true);
      expect(techniqueStandardLabel(t)).toBe("NIST SP 800-88 Destroy");
    }
    for (const t of ["crush", "punch", "degauss", "drill"] as const) {
      expect(isNistDestroy(t)).toBe(false);
      expect(techniqueStandardLabel(t)).toBe("physical damage — not NIST Destroy");
    }
  });

  it("names techniques as the Rust human_name does", () => {
    expect(techniqueName("shred")).toBe("Shred");
    expect(techniqueName("degauss")).toBe("Degauss");
  });

  it("cautions on shred and pulverize above Low only", () => {
    const caution =
      "NIST SP 800-88 Rev. 2: shred and pulverize should be avoided for anything but the lowest security categories.";
    for (const t of DESTRUCTION_TECHNIQUES) {
      expect(techniqueCaution(t, "low")).toBeNull();
      for (const c of ["moderate", "high"] as Classification[]) {
        expect(techniqueCaution(t, c)).toBe(
          t === "shred" || t === "pulverize" ? caution : null,
        );
      }
    }
  });

  it("labels identity assurance", () => {
    expect(identityAssuranceLabel({ level: "self_asserted" })).toBe("Self-asserted");
    expect(
      identityAssuranceLabel({ level: "verified", issuer: "https://idp.example.com", subject: "u-42" }),
    ).toBe("Verified by https://idp.example.com");
  });
});
