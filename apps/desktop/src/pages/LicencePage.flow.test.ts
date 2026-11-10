// @vitest-environment happy-dom
//
// The Licence page's wiring (station-enrollment story 4), mounted under a
// QueryClientProvider with `fetch` stubbed: the pending re-read, the licence
// re-read after an install, Start again, and a station refusal of a start.
import { createElement } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { EnrollmentAttempt, EnrollmentView, LicenseInfo } from "@/api/types";
import { LicencePage } from "./LicencePage";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

type Reply = { status?: number; body: unknown };
/** Answers per `METHOD path`, in order; the last one repeats. */
type Routes = Record<string, Reply[]>;

let root: Root | null = null;
let host: HTMLDivElement | null = null;
const calls: string[] = [];
const bodies: Record<string, unknown[]> = {};

function stubFetch(routes: Routes) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      const key = `${init?.method ?? "GET"} ${url}`;
      calls.push(key);
      (bodies[key] ??= []).push(init?.body ? JSON.parse(String(init.body)) : null);
      const queue = routes[key];
      if (!queue || queue.length === 0) throw new Error(`unexpected ${key}`);
      const reply = queue.length > 1 ? queue.shift()! : queue[0];
      return new Response(JSON.stringify(reply.body), {
        status: reply.status ?? 200,
        headers: { "Content-Type": "application/json" },
      });
    }),
  );
}

beforeEach(() => {
  vi.useFakeTimers();
  calls.length = 0;
  for (const k of Object.keys(bodies)) delete bodies[k];
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

/** Let fetches resolve and React re-render, moving fake time by `ms`. */
async function settle(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}

async function mount() {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  act(() =>
    root!.render(createElement(QueryClientProvider, { client: qc }, createElement(LicencePage))),
  );
  await settle();
}

function button(text: string): HTMLButtonElement {
  const found = [...host!.querySelectorAll("button")].find((b) => b.textContent === text);
  if (!found) throw new Error(`no ${text} button in: ${host!.textContent}`);
  return found;
}

function count(key: string) {
  return calls.filter((c) => c === key).length;
}

const unlicensed: LicenseInfo = {
  status: { status: "unlicensed" },
  operator_message: "Unlicensed",
  customer: null,
  not_after: null,
  instance_public_key_id: "ed25519:k",
  lapse_policy: null,
  fingerprint: "fp",
};

const licensed: LicenseInfo = {
  status: { status: "valid", days_remaining: 365 },
  operator_message: "Licensed - unlimited, 365 days left",
  customer: { id: "org-1", name: "Acme Recycling" },
  not_after: "2027-11-10T00:00:00Z",
  instance_public_key_id: "ed25519:k",
  lapse_policy: "evaluation",
  fingerprint: "fp",
};

const view = (attempt: EnrollmentAttempt | null, over: Partial<EnrollmentView> = {}) => ({
  saved_hub_origin: null,
  can_enroll: true,
  cannot_enroll_reason: null,
  attempt,
  ...over,
});

const pending: EnrollmentAttempt = {
  state: "pending",
  hub_origin: "https://hub.example",
  user_code: "BCDF-GHJK",
  verification_uri: "https://hub.example/activate",
  verification_uri_complete: "https://hub.example/activate?code=BCDF-GHJK",
  seconds_left: 590,
  interval: 5,
};

const installed: EnrollmentAttempt = {
  state: "installed",
  hub_origin: "https://hub.example",
  licence_id: "lic-1",
  customer_name: "Acme Recycling",
  not_after: "2027-11-10T00:00:00Z",
  operator_message: "Licensed - unlimited, 365 days left",
  saved_to: "/cfg/enrolled-licence.json",
  message: "This station holds the licence until it restarts.",
};

describe("LicencePage", () => {
  it("re-reads a pending attempt until it is Licensed, then re-reads the licence", async () => {
    stubFetch({
      "GET /api/license": [{ body: unlicensed }, { body: licensed }],
      "GET /api/enrollment": [{ body: view(pending) }, { body: view(installed) }],
    });
    await mount();
    expect(host!.textContent).toContain("BCDF-GHJK");
    expect(host!.querySelector('svg[aria-label="QR code of the approval link"]')).not.toBeNull();
    expect(count("GET /api/license")).toBe(1);

    await settle(2100);
    expect(count("GET /api/enrollment")).toBe(2);
    expect(host!.querySelector("h3")!.textContent).toBe("Licensed");
    expect(host!.textContent).toContain("This station holds the licence until it restarts.");
    expect(count("GET /api/license")).toBe(2);
    expect(host!.textContent).toContain("Acme Recycling");

    // Installed: no more polling.
    await settle(5000);
    expect(count("GET /api/enrollment")).toBe(2);
  });

  it("keeps a pending code on screen when a re-read fails", async () => {
    stubFetch({
      "GET /api/license": [{ body: unlicensed }],
      "GET /api/enrollment": [{ body: view(pending) }, { status: 500, body: { error: "down" } }],
    });
    await mount();
    await settle(2100);
    expect(count("GET /api/enrollment")).toBe(2);
    expect(host!.textContent).toContain("BCDF-GHJK");
    expect(host!.textContent).not.toContain("could not report its registration state");
  });

  it("Start again cancels and shows the form with the address kept", async () => {
    const denied: EnrollmentAttempt = {
      state: "denied",
      hub_origin: "https://hub.example",
      message: "An owner denied this registration.",
    };
    stubFetch({
      "GET /api/license": [{ body: unlicensed }],
      "GET /api/enrollment": [{ body: view(denied) }],
      "POST /api/enrollment/cancel": [{ body: view(null) }],
    });
    await mount();
    expect(host!.textContent).toContain("Registration denied");
    act(() => button("Start again").click());
    await settle();
    expect(count("POST /api/enrollment/cancel")).toBe(1);
    const input = host!.querySelector<HTMLInputElement>("#hub-origin")!;
    expect(input).not.toBeNull();
    expect(input.value).toBe("https://hub.example");
  });

  it("shows a started attempt from Register's answer, then re-reads it while pending", async () => {
    stubFetch({
      "GET /api/license": [{ body: unlicensed }],
      "GET /api/enrollment": [
        { body: view(null, { saved_hub_origin: "https://hub.example" }) },
        { body: view(pending) },
      ],
      "POST /api/enrollment": [{ body: view(pending) }],
    });
    await mount();
    const labelInput = host!.querySelector<HTMLInputElement>("#station-label")!;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    act(() => {
      setter.call(labelInput, "  Bench 3  ");
      labelInput.dispatchEvent(new Event("input", { bubbles: true }));
    });
    act(() => button("Register").click());
    await settle();
    expect(bodies["POST /api/enrollment"][0]).toEqual({
      hub_origin: "https://hub.example",
      label: "Bench 3",
    });
    // Shown from the POST's answer, with no further read.
    expect(count("GET /api/enrollment")).toBe(1);
    expect(host!.textContent).toContain("BCDF-GHJK");
    expect(host!.querySelector('svg[aria-label="QR code of the approval link"]')).not.toBeNull();

    await settle(2100);
    expect(count("GET /api/enrollment")).toBe(2);
    expect(host!.textContent).toContain("BCDF-GHJK");
  });

  it("shows the station's error when it refuses a start", async () => {
    stubFetch({
      "GET /api/license": [{ body: unlicensed }],
      "GET /api/enrollment": [{ body: view(null, { saved_hub_origin: "https://hub.example" }) }],
      "POST /api/enrollment": [
        { status: 400, body: { error: "The hub address must be https." } },
      ],
    });
    await mount();
    expect(host!.querySelector<HTMLInputElement>("#hub-origin")!.value).toBe(
      "https://hub.example",
    );
    act(() => button("Register").click());
    await settle();
    expect(count("POST /api/enrollment")).toBe(1);
    expect(bodies["POST /api/enrollment"][0]).toEqual({
      hub_origin: "https://hub.example",
      label: null,
    });
    expect(host!.textContent).toContain("The hub address must be https.");
    expect(host!.querySelector("form")).not.toBeNull();
  });

  it("shows a failed Start again request's error", async () => {
    const expired: EnrollmentAttempt = {
      state: "expired",
      hub_origin: "https://hub.example",
      message: "The code expired.",
    };
    stubFetch({
      "GET /api/license": [{ body: unlicensed }],
      "GET /api/enrollment": [{ body: view(expired) }],
      "POST /api/enrollment/cancel": [{ status: 500, body: { error: "station busy" } }],
    });
    await mount();
    act(() => button("Start again").click());
    await settle();
    expect(host!.textContent).toContain("station busy");
    expect(host!.textContent).toContain("The code expired");
  });
});
