import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { EnrollmentAttempt, EnrollmentView, LicenseInfo } from "@/api/types";
import { EnrollmentPanelView, LicenceStatus } from "./LicencePage";

const noop = () => {};

function render(view: EnrollmentView, over: { address?: string; error?: string | null } = {}) {
  return renderToStaticMarkup(
    createElement(EnrollmentPanelView, {
      view,
      address: over.address ?? "",
      label: "",
      busy: false,
      error: over.error ?? null,
      onAddress: noop,
      onLabel: noop,
      onRegister: noop,
      onCancel: noop,
      onStartAgain: noop,
    }),
  );
}

const view = (attempt: EnrollmentAttempt | null, over: Partial<EnrollmentView> = {}) => ({
  saved_hub_origin: null,
  can_enroll: true,
  cannot_enroll_reason: null,
  attempt,
  ...over,
});

describe("EnrollmentPanelView", () => {
  it("offers the address form, prefilled", () => {
    const html = render(view(null, { saved_hub_origin: "https://hub.example" }), {
      address: "https://hub.example",
    });
    expect(html).toContain("Register with platform");
    expect(html).toContain('value="https://hub.example"');
    expect(html).toContain("Label (optional)");
    expect(html).toContain(">Register</button>");
  });

  it("shows a refusal from the station under the form", () => {
    const html = render(view(null), { address: "http://hub.example", error: "not a hub address" });
    expect(html).toContain("not a hub address");
  });

  it("shows the reason and no form when the station cannot enroll", () => {
    const html = render(
      view(null, { can_enroll: false, cannot_enroll_reason: "This station trusts no vendor root." }),
    );
    expect(html).toContain("This station trusts no vendor root.");
    expect(html).not.toContain("<form");
    expect(html).not.toContain("<input");
  });

  it("shows the code, the link, a QR code and the countdown while pending", () => {
    const html = render(
      view({
        state: "pending",
        hub_origin: "https://hub.example",
        user_code: "BCDF-GHJK",
        verification_uri: "https://hub.example/activate",
        verification_uri_complete: "https://hub.example/activate?code=BCDF-GHJK",
        seconds_left: 543,
        interval: 5,
      }),
    );
    expect(html).toContain("BCDF-GHJK");
    expect(html).toContain("https://hub.example/activate<");
    expect(html).toContain('href="https://hub.example/activate?code=BCDF-GHJK"');
    expect(html).toMatch(/<svg[^>]*aria-label="QR code of the approval link"/);
    expect(html).toMatch(/<path d="M\d/);
    expect(html).toContain("9:03");
    expect(html).toContain(">Cancel</button>");
    expect(html).not.toContain("<form");
  });

  it("says Licensed with the station's messages once installed", () => {
    const html = render(
      view({
        state: "installed",
        hub_origin: "https://hub.example",
        licence_id: "lic-1",
        customer_name: "Acme",
        not_after: "2027-11-10T00:00:00Z",
        operator_message: "Licensed - unlimited",
        saved_to: "/etc/ws/enrolled-licence.json",
        message: "This station holds the licence until it restarts.",
      }),
    );
    expect(html).toContain("Licensed");
    expect(html).toContain("Licensed - unlimited");
    expect(html).toContain("This station holds the licence until it restarts.");
    expect(html).toContain(">Register again</button>");
    expect(html).not.toContain("Start again");
    expect(html).not.toContain("<form");
  });

  it("shows a failed Cancel or Start again request's error under the panel", () => {
    const html = render(
      view({ state: "denied", hub_origin: "https://hub.example", message: "denied" }),
      { error: "the station did not answer" },
    );
    expect(html).toContain("the station did not answer");
    expect(html).toContain(">Start again</button>");
  });

  it.each([
    ["denied", "Registration denied"],
    ["expired", "The code expired"],
    ["unreachable", "The platform could not be reached"],
    ["refused", "The platform refused this registration"],
    ["failed", "Registration failed"],
  ] as const)("names a %s outcome, shows the message and offers Start again", (state, heading) => {
    const html = render(
      view({ state, hub_origin: "https://hub.example", message: `server says ${state}` }),
    );
    expect(html).toContain(heading);
    expect(html).toContain(`server says ${state}`);
    expect(html).toContain(">Start again</button>");
    expect(html).not.toContain("<form");
  });
});

describe("LicenceStatus", () => {
  const info: LicenseInfo = {
    status: { status: "valid", days_remaining: 300 },
    operator_message: "Licensed - unlimited, 300 days left",
    customer: { id: "c", name: "Acme Recycling" },
    not_after: "2027-09-06T12:00:00Z",
    instance_public_key_id: "ed25519:k",
    new_jobs_refused: null,
    simulated_backend: true,
    fingerprint: "fp",
  };

  it("shows the message, customer and expiry date", () => {
    const html = renderToStaticMarkup(createElement(LicenceStatus, { info }));
    expect(html).toContain("Licensed - unlimited, 300 days left");
    expect(html).toContain("Acme Recycling");
    expect(html).toContain("2027-09-06 UTC");
  });

  it("shows no platform check before the station has made one", () => {
    const html = renderToStaticMarkup(createElement(LicenceStatus, { info }));
    expect(html).not.toContain("Last check with the platform");
  });

  it("shows a successful platform check in local time", () => {
    const checked_at = "2026-11-10T09:30:00Z";
    const html = renderToStaticMarkup(
      createElement(LicenceStatus, {
        info: {
          ...info,
          hub_check: {
            checked_at,
            ok: true,
            message: "Up to date: the platform at https://hub.example has no newer licence.",
          },
        },
      }),
    );
    expect(html).toContain(
      `Last check with the platform: ${new Date(checked_at).toLocaleString()} — Up to date`,
    );
    expect(html).toContain("text-slate-400");
    expect(html).not.toContain("text-amber-200");
  });

  it("shows a failed platform check, which leaves the licence in force", () => {
    const html = renderToStaticMarkup(
      createElement(LicenceStatus, {
        info: {
          ...info,
          hub_check: {
            checked_at: "2026-11-10T09:30:00Z",
            ok: false,
            message:
              "Could not reach the platform at https://hub.example (timed out). This station's " +
              "current licence still applies.",
          },
        },
      }),
    );
    expect(html).toContain("Last check with the platform:");
    expect(html).toContain("Could not reach the platform at https://hub.example (timed out).");
    expect(html).toContain("current licence still applies.");
    expect(html).toContain("text-amber-200");
    // The licence itself is still shown as it was.
    expect(html).toContain("Licensed - unlimited, 300 days left");
  });

  it("says so when the licence cannot be read", () => {
    const html = renderToStaticMarkup(createElement(LicenceStatus, { error: true }));
    expect(html).toContain("could not report its licence");
  });
});
