// @vitest-environment happy-dom
//
// `initLeadForms` in a DOM (lead-capture story 1). The script is the only
// thing between a visitor and a native submit that would leak their details
// to the static host, so: it enables only open forms, always calls
// `preventDefault`, posts to the hub, shows the outcome and resets the form
// only on success.
import { afterEach, describe, expect, it, vi } from "vitest";
import { consentText } from "../src/data/consent";
import { SUCCESS, initLeadForms } from "../src/lib/lead-submit";

const HUB = "http://localhost:7900";

/** The open early-access form's markup, as `LeadForm.astro` renders it. */
function openForm(hubOrigin: string | null): HTMLFormElement {
  const hub = hubOrigin === null ? "" : ` data-hub-origin="${hubOrigin}"`;
  document.body.innerHTML = `
    <form data-lead-form="early-access"${hub}>
      <input name="name" />
      <input name="email" />
      <input name="company" />
      <input name="role" />
      <select name="segment"><option value="" selected>Choose one</option><option value="itad">ITAD</option></select>
      <textarea name="message"></textarea>
      <input name="website" />
      <input name="consent" type="checkbox" />
      <button type="submit" disabled>Join early access</button>
      <p data-lead-status role="status" aria-live="polite"></p>
    </form>`;
  const form = document.querySelector("form")!;
  // Typed by the visitor (values, not attributes, so a reset clears them).
  const typed: Record<string, string> = {
    name: "Ada Lovelace",
    email: "ada@example.com",
    company: "Engines Ltd",
    role: "Ops lead",
    segment: "itad",
  };
  for (const [name, value] of Object.entries(typed)) {
    form.querySelector<HTMLInputElement | HTMLSelectElement>(`[name="${name}"]`)!.value = value;
  }
  form.querySelector<HTMLInputElement>('[name="consent"]')!.checked = true;
  return form;
}

interface Posted {
  url: string;
  body: Record<string, unknown>;
}

/** Stub `fetch`: the CSRF endpoint answers a token; posts get `status`. */
function stubHub(status: number, error?: string): Posted[] {
  const posts: Posted[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/api/auth/csrf")) {
        return new Response(JSON.stringify({ csrf_token: "t" }), { status: 200 });
      }
      posts.push({ url, body: JSON.parse(String(init?.body)) });
      return new Response(error === undefined ? "{}" : JSON.stringify({ error }), { status });
    }),
  );
  return posts;
}

function submit(form: HTMLFormElement): Event {
  const event = new Event("submit", { bubbles: true, cancelable: true });
  form.dispatchEvent(event);
  return event;
}

async function settled(form: HTMLFormElement): Promise<string> {
  const status = form.querySelector<HTMLElement>("[data-lead-status]")!;
  await vi.waitFor(() => {
    expect(status.textContent).not.toBe("Sending…");
    expect(status.textContent).not.toBe("");
  });
  return status.textContent ?? "";
}

const button = (form: HTMLFormElement) => form.querySelector("button")!;
const nameField = (form: HTMLFormElement) => form.querySelector<HTMLInputElement>('[name="name"]')!;

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

describe("initLeadForms", () => {
  it("enables an open form, prevents the native submit and posts to the hub", async () => {
    const form = openForm(HUB);
    const posts = stubHub(202);
    initLeadForms();
    expect(button(form).disabled).toBe(false);

    const event = submit(form);
    expect(event.defaultPrevented).toBe(true);
    expect(await settled(form)).toBe(SUCCESS["early-access"]);
    expect(posts).toHaveLength(1);
    expect(posts[0].url).toBe(`${HUB}/api/leads`);
    expect(posts[0].body).toMatchObject({
      kind: "early-access",
      name: "Ada Lovelace",
      email: "ada@example.com",
      segment: "itad",
      consent: true,
      consent_text: consentText("early-access"),
      website: "",
    });
    // Reset on success.
    expect(nameField(form).value).toBe("");
    expect(button(form).disabled).toBe(false);
  });

  it("keeps what was typed when the hub refuses", async () => {
    const form = openForm(HUB);
    stubHub(400, "email is not a valid email address");
    initLeadForms();
    expect(submit(form).defaultPrevented).toBe(true);
    expect(await settled(form)).toBe("Please check the form: email is not a valid email address.");
    expect(nameField(form).value).toBe("Ada Lovelace");
    expect(button(form).disabled).toBe(false);
  });

  it("leaves a closed form untouched", () => {
    const form = openForm(null);
    const fetchSpy = vi.fn();
    vi.stubGlobal("fetch", fetchSpy);
    initLeadForms();
    expect(button(form).disabled).toBe(true);
    const event = submit(form);
    // No listener: nothing prevented, nothing posted (the disabled button is
    // what keeps a closed form from submitting).
    expect(event.defaultPrevented).toBe(false);
    expect(fetchSpy).not.toHaveBeenCalled();
  });
});
