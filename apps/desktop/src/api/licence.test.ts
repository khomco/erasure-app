import { describe, expect, it } from "vitest";
import {
  EXPIRY_FINAL_DAYS,
  EXPIRY_NOTICE_DAYS,
  EXPIRY_URGENT_DAYS,
  LICENCE_UNKNOWN_BANNER,
  licenceBanner,
} from "./licence";
import type { LeaseStatus, LicenseInfo } from "./types";

function info(status: LeaseStatus, over: Partial<LicenseInfo> = {}): LicenseInfo {
  return {
    status,
    operator_message: `server message for ${status.status}`,
    customer: { id: "c1", name: "Acme ITAD" },
    not_after: "2026-12-08T00:00:00Z",
    instance_public_key_id: "k1",
    new_jobs_refused: null,
    simulated_backend: false,
    fingerprint: "fp",
    ...over,
  };
}

function valid(
  days: number,
  over: Partial<LicenseInfo> = {},
) {
  return licenceBanner(info({ status: "valid", days_remaining: days }, over));
}

const REAL_EXPIRY_LINE =
  "From 2026-12-08 UTC, new jobs will be refused until a renewed licence is installed. Renew and install it before then.";

describe("licenceBanner", () => {
  it("names the thresholds 30, 7 and 1 days", () => {
    expect([EXPIRY_NOTICE_DAYS, EXPIRY_URGENT_DAYS, EXPIRY_FINAL_DAYS]).toEqual([30, 7, 1]);
  });

  it("shows the server's refusal text in danger tone when new jobs are refused", () => {
    const refusal =
      "This station's licence expired on 2026-11-01. Install the renewed licence to start new jobs.";
    for (const status of [
      { status: "unlicensed" },
      { status: "expired", not_after: "2026-11-01T00:00:00Z" },
      { status: "quota_exhausted", used: 10, allowed: 10 },
    ] as LeaseStatus[]) {
      expect(licenceBanner(info(status, { new_jobs_refused: refusal }))).toEqual({
        tone: "danger",
        heading: "New jobs are refused",
        lines: [refusal],
      });
    }
  });

  it("shows evaluation mode, in warning tone, on an unlicensed simulated station", () => {
    const view = licenceBanner(
      info(
        { status: "unlicensed" },
        {
          customer: null,
          not_after: null,
          instance_public_key_id: null,
          simulated_backend: true,
        },
      ),
    );
    expect(view).toEqual({
      tone: "warning",
      heading: "Evaluation mode — no licence installed",
      lines: [
        "server message for unlicensed",
        "This station's backend is simulated: jobs still run, and their certificates are marked evaluation.",
      ],
    });
  });

  const lapsed: LeaseStatus[] = [
    { status: "expired", not_after: "2026-11-01T00:00:00Z" },
    { status: "not_yet_valid", not_before: "2026-12-01T00:00:00Z" },
    { status: "quota_exhausted", used: 10, allowed: 10 },
    { status: "wrong_machine", expected: "aaa", actual: "bbb" },
    { status: "clock_rollback", watermark: "2026-11-08T00:00:00Z", observed: "2026-01-01T00:00:00Z" },
  ];
  for (const status of lapsed) {
    it(`on a simulated station, shows the server's message in danger tone when ${status.status}`, () => {
      expect(licenceBanner(info(status, { simulated_backend: true }))).toEqual({
        tone: "danger",
        heading: "Licence not in force — certificates are marked evaluation",
        lines: [
          `server message for ${status.status}`,
          "This station's backend is simulated: jobs still run, and their certificates are marked evaluation.",
        ],
      });
    });
  }

  it("on a simulated station, a lease fault claims neither licensed nor evaluation", () => {
    expect(
      licenceBanner(
        info({ status: "lease_state_unavailable", detail: "disk full" }, { simulated_backend: true }),
      ),
    ).toEqual({
      tone: "warning",
      heading: "Licence state needs attention",
      lines: [
        "server message for lease_state_unavailable",
        "This station's backend is simulated: new jobs still start. Fix the problem named above.",
      ],
    });
  });

  it("says jobs still start when a real station's lease fault refuses nothing", () => {
    expect(
      licenceBanner(info({ status: "lease_state_unavailable", detail: "disk full" })),
    ).toEqual({
      tone: "warning",
      heading: "Licence state needs attention",
      lines: [
        "server message for lease_state_unavailable",
        "New jobs still start under this licence. Fix the problem named above.",
      ],
    });
  });

  it("shows nothing with 31 days left", () => {
    expect(valid(31)).toBeNull();
  });

  it("warns from 30 days, naming the expiry date and that new jobs will then be refused", () => {
    for (const days of [30, 8]) {
      const view = valid(days);
      expect(view?.tone).toBe("warning");
      expect(view?.heading).toBe(`Licence expires in ${days} days, on 2026-12-08 UTC`);
      expect(view?.lines).toEqual([REAL_EXPIRY_LINE]);
    }
  });

  it("says jobs still run with evaluation certificates on a simulated station", () => {
    expect(valid(8, { simulated_backend: true })?.lines).toEqual([
      "This station's backend is simulated: when the licence expires, jobs still run, with certificates marked evaluation, until a renewed licence is installed.",
    ]);
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

  it("uses days left only when not_after is null", () => {
    expect(valid(8, { not_after: null })?.heading).toBe("Licence expires in 8 days");
    expect(valid(0, { not_after: null })?.heading).toBe("Licence expires within a day — renew now");
    expect(valid(8, { not_after: null })?.lines[0]).toMatch(/^When it expires, new jobs will be refused/);
  });

  it("has an unknown-status banner saying jobs may be refused or marked evaluation", () => {
    expect(LICENCE_UNKNOWN_BANNER.tone).toBe("warning");
    expect(LICENCE_UNKNOWN_BANNER.heading).toBe("Licence status unknown");
    const text = LICENCE_UNKNOWN_BANNER.lines.join(" ");
    expect(text).toMatch(/could not report its licence/);
    expect(text).toMatch(/new jobs may be refused or their certificates marked evaluation/);
  });
});
