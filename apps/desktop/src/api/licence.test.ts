import { describe, expect, it } from "vitest";
import {
  EXPIRY_FINAL_DAYS,
  EXPIRY_NOTICE_DAYS,
  EXPIRY_URGENT_DAYS,
  LICENCE_UNKNOWN_BANNER,
  licenceBanner,
} from "./licence";
import type { LapsePolicy, LeaseStatus, LicenseInfo } from "./types";

function info(status: LeaseStatus, over: Partial<LicenseInfo> = {}): LicenseInfo {
  return {
    status,
    operator_message: `server message for ${status.status}`,
    customer: { id: "c1", name: "Acme ITAD" },
    not_after: "2026-12-08T00:00:00Z",
    instance_public_key_id: "k1",
    lapse_policy: "evaluation",
    fingerprint: "fp",
    ...over,
  };
}

function valid(days: number, lapse_policy: LapsePolicy | null = "evaluation", not_after: string | null = "2026-12-08T00:00:00Z") {
  return licenceBanner(
    info({ status: "valid", days_remaining: days }, { lapse_policy, not_after }),
  );
}

describe("licenceBanner", () => {
  it("names the thresholds 30, 7 and 1 days", () => {
    expect([EXPIRY_NOTICE_DAYS, EXPIRY_URGENT_DAYS, EXPIRY_FINAL_DAYS]).toEqual([30, 7, 1]);
  });

  it("shows evaluation mode, in warning tone, on an unlicensed station", () => {
    const view = licenceBanner(
      info(
        { status: "unlicensed" },
        { customer: null, not_after: null, instance_public_key_id: null, lapse_policy: null },
      ),
    );
    expect(view).toEqual({
      tone: "warning",
      heading: "Evaluation mode — no licence installed",
      lines: ["server message for unlicensed"],
    });
  });

  const lapsed: LeaseStatus[] = [
    { status: "expired", not_after: "2026-11-01T00:00:00Z" },
    { status: "not_yet_valid", not_before: "2026-12-01T00:00:00Z" },
    { status: "quota_exhausted", used: 10, allowed: 10 },
    { status: "wrong_machine", expected: "aaa", actual: "bbb" },
    { status: "clock_rollback", watermark: "2026-11-08T00:00:00Z", observed: "2026-01-01T00:00:00Z" },
    { status: "lease_state_unavailable", detail: "disk full" },
  ];
  for (const status of lapsed) {
    it(`shows the server's message in danger tone when ${status.status}`, () => {
      expect(licenceBanner(info(status))).toEqual({
        tone: "danger",
        heading: "Licensed signing unavailable — certificates are marked evaluation",
        lines: [`server message for ${status.status}`],
      });
    });
  }

  it("shows nothing with 31 days left", () => {
    expect(valid(31)).toBeNull();
  });

  it("warns from 30 days, naming the expiry date and days left", () => {
    for (const days of [30, 8]) {
      const view = valid(days);
      expect(view?.tone).toBe("warning");
      expect(view?.heading).toBe(`Licence expires in ${days} days, on 2026-12-08 UTC`);
      expect(view?.lines).toEqual([
        "When it expires, erasure continues, but certificates will be marked evaluation until a renewed licence is installed.",
      ]);
    }
  });

  it("says renew now from 7 days, still in warning tone", () => {
    expect(valid(7)).toMatchObject({
      tone: "warning",
      heading: "Licence expires in 7 days, on 2026-12-08 UTC — renew now",
    });
    expect(valid(2)).toMatchObject({
      tone: "warning",
      heading: "Licence expires in 2 days, on 2026-12-08 UTC — renew now",
    });
  });

  it("turns to danger tone at 1 day, reading in 1 day and within a day", () => {
    expect(valid(1)).toMatchObject({
      tone: "danger",
      heading: "Licence expires in 1 day, on 2026-12-08 UTC — renew now",
    });
    expect(valid(0)).toMatchObject({
      tone: "danger",
      heading: "Licence expires within a day, on 2026-12-08 UTC — renew now",
    });
  });

  it("says new jobs will be refused under the stop lapse policy", () => {
    expect(valid(7, "stop")?.lines).toEqual([
      "This licence's lapse policy is stop: new jobs will be refused when it expires. Renew and install the new licence before then.",
    ]);
  });

  it("treats a null lapse policy as evaluation", () => {
    expect(valid(7, null)?.lines[0]).toMatch(/erasure continues/);
  });

  it("uses days left only when not_after is null", () => {
    expect(valid(8, "evaluation", null)?.heading).toBe("Licence expires in 8 days");
    expect(valid(0, "evaluation", null)?.heading).toBe("Licence expires within a day — renew now");
  });

  it("has an unknown-status banner telling the operator the station could not report", () => {
    expect(LICENCE_UNKNOWN_BANNER.tone).toBe("warning");
    expect(LICENCE_UNKNOWN_BANNER.heading).toBe("Licence status unknown");
    expect(LICENCE_UNKNOWN_BANNER.lines.join(" ")).toMatch(/could not report its licence/);
  });
});
