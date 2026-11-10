import { describe, expect, it } from "vitest";
import type { EnrollmentAttempt, EnrollmentView } from "./types";
import {
  countdownText,
  enrollmentPanel,
  initialHubAddress,
  requestErrorText,
} from "./enrollment";

const view = (over: Partial<EnrollmentView> = {}): EnrollmentView => ({
  saved_hub_origin: null,
  can_enroll: true,
  cannot_enroll_reason: null,
  attempt: null,
  ...over,
});

const pending = (seconds_left: number): EnrollmentAttempt => ({
  state: "pending",
  hub_origin: "https://hub.example",
  user_code: "BCDF-GHJK",
  verification_uri: "https://hub.example/activate",
  verification_uri_complete: "https://hub.example/activate?code=BCDF-GHJK",
  seconds_left,
  interval: 5,
});

describe("countdownText", () => {
  it("formats m:ss and floors at 0:00", () => {
    expect(countdownText(600)).toBe("10:00");
    expect(countdownText(65)).toBe("1:05");
    expect(countdownText(9)).toBe("0:09");
    expect(countdownText(0)).toBe("0:00");
    expect(countdownText(-3)).toBe("0:00");
  });
});

describe("enrollmentPanel", () => {
  it("shows the form when nothing is pending", () => {
    expect(enrollmentPanel(view())).toEqual({
      kind: "form",
      heading: "Register with platform",
      countdown: null,
    });
  });

  it("shows only the reason when the station cannot enroll", () => {
    expect(
      enrollmentPanel(view({ can_enroll: false, cannot_enroll_reason: "no root" })).kind,
    ).toBe("unavailable");
  });

  it("counts down a pending attempt", () => {
    expect(enrollmentPanel(view({ attempt: pending(125) }))).toEqual({
      kind: "pending",
      heading: "Waiting for approval",
      countdown: "2:05",
    });
  });

  it("says Licensed once installed", () => {
    const p = enrollmentPanel(
      view({
        attempt: {
          state: "installed",
          hub_origin: "https://hub.example",
          licence_id: "lic-1",
          customer_name: "Acme",
          not_after: "2027-11-10T00:00:00Z",
          operator_message: "Licensed - unlimited",
          saved_to: null,
          message: "m",
        },
      }),
    );
    expect(p).toEqual({ kind: "installed", heading: "Licensed", countdown: null });
  });

  it.each([
    ["denied", "Registration denied"],
    ["expired", "The code expired"],
    ["unreachable", "The platform could not be reached"],
    ["refused", "The platform refused this registration"],
    ["failed", "Registration failed"],
  ] as const)("names an ended %s attempt", (state, heading) => {
    expect(
      enrollmentPanel(view({ attempt: { state, hub_origin: "h", message: "m" } })),
    ).toEqual({ kind: "ended", heading, countdown: null });
  });
});

describe("initialHubAddress", () => {
  it("prefers the last attempt's address, then the saved one", () => {
    expect(initialHubAddress(view())).toBe("");
    expect(initialHubAddress(view({ saved_hub_origin: "https://saved" }))).toBe("https://saved");
    expect(
      initialHubAddress(
        view({
          saved_hub_origin: "https://saved",
          attempt: { state: "denied", hub_origin: "https://tried", message: "m" },
        }),
      ),
    ).toBe("https://tried");
  });
});

describe("requestErrorText", () => {
  it("reads the station's error out of a rejected request", () => {
    expect(
      requestErrorText(new Error('400 Bad Request: {"error":"not a hub address"}')),
    ).toBe("not a hub address");
    expect(requestErrorText(new Error("Failed to fetch"))).toBe("Failed to fetch");
    expect(requestErrorText(new Error("500 Internal: oops"))).toBe("500 Internal: oops");
  });
});
