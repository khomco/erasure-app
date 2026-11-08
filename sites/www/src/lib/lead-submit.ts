/**
 * Posting the early-access and contact forms to the hub (lead-capture
 * story 1). Only builds given `PUBLIC_HUB_ORIGIN` ship this as a script;
 * without it the forms stay closed and the site ships no JavaScript.
 *
 * The hub is on another origin and keeps its double-submit CSRF check
 * (ADR-0007 §3), so a submission fetches `GET /api/auth/csrf` and then posts
 * `POST /api/leads`, both with `credentials: "include"`; the hub answers the
 * site's origin with credentialed CORS. A stale token (403 with the hub's
 * CSRF message) is refetched and the post retried once.
 *
 * Mirrors `wipe_hub::leads::LeadRequest` (crates/wipe-hub/src/leads.rs);
 * change both together.
 */
import { consentText, type LeadKind } from "../data/consent";

/** The hub's 403 text for a missing or mismatched CSRF token (crates/wipe-hub/src/csrf.rs). */
export const CSRF_ERROR = "missing or invalid CSRF token";

/**
 * `PUBLIC_HUB_ORIGIN` as an origin (`scheme://host[:port]`), or `""` when it
 * is unset or blank (the forms stay closed). Anything but a bare http(s)
 * origin throws, so a typo fails the build instead of shipping a form that
 * posts nowhere.
 */
export function parseHubOrigin(value: string | undefined | null): string {
  const v = (value ?? "").trim();
  if (v === "") return "";
  let url: URL;
  try {
    url = new URL(v);
  } catch {
    throw new Error(`PUBLIC_HUB_ORIGIN ${JSON.stringify(v)} is not a URL`);
  }
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username !== "" ||
    url.password !== "" ||
    url.pathname !== "/" ||
    url.search !== "" ||
    url.hash !== "" ||
    v.endsWith("?") ||
    v.endsWith("#")
  ) {
    throw new Error(
      `PUBLIC_HUB_ORIGIN ${JSON.stringify(v)} must be an http(s) origin such as http://localhost:7900`,
    );
  }
  return url.origin;
}

/** The JSON body of `POST /api/leads`. */
export interface LeadBody {
  kind: LeadKind;
  name: string;
  email: string;
  company: string;
  role?: string;
  segment?: string;
  message: string;
  consent: boolean;
  consent_text: string;
  website: string;
}

function field(data: FormData, name: string): string {
  const v = data.get(name);
  return typeof v === "string" ? v : "";
}

/**
 * The request body for a form's data. The consent text is the site's own
 * copy (`src/data/consent.ts`), never read back from the page.
 */
export function leadBody(kind: LeadKind, data: FormData): LeadBody {
  const body: LeadBody = {
    kind,
    name: field(data, "name"),
    email: field(data, "email"),
    company: field(data, "company"),
    // `FormData` gives a textarea CRLF line breaks; the hub counts characters
    // against the same 5000 the browser's `maxlength` allows, so send LF.
    message: field(data, "message").replace(/\r\n/g, "\n"),
    consent: data.get("consent") !== null,
    consent_text: consentText(kind),
    website: field(data, "website"),
  };
  if (kind === "early-access") {
    body.role = field(data, "role");
    body.segment = field(data, "segment");
  }
  return body;
}

/** What the form shows after a submission. */
export interface SubmitOutcome {
  ok: boolean;
  message: string;
}

/**
 * Success copy. It never says an email was sent: the hub sends the
 * confirmation after answering, and never to an address that unsubscribed.
 */
export const SUCCESS: Record<LeadKind, string> = {
  "early-access": "Thank you, you are on the early-access list. We will reply by email.",
  contact: "Thank you, we have your message. We will reply by email.",
};
export const NETWORK_ERROR =
  "The form could not reach our server. Check your connection and try again.";

/** Shown for a 429 whose body carries no `error` text. */
export const TOO_MANY = "Too many requests.";

/** The hub's error text as a sentence: first letter capitalised, a final full stop. */
export function sentence(text: string): string {
  const t = text.trim();
  if (t === "") return t;
  const s = t[0].toUpperCase() + t.slice(1);
  return /[.!?]$/.test(s) ? s : `${s}.`;
}

/** A 429's `Retry-After` as "Try again in N minute(s).", rounded up. */
export function tryAgainIn(retryAfter: string | null): string {
  const v = retryAfter?.trim();
  if (!v || !/^\d+$/.test(v)) return "Try again later.";
  const minutes = Math.max(1, Math.ceil(Number(v) / 60));
  return `Try again in ${minutes} minute${minutes === 1 ? "" : "s"}.`;
}

async function errorText(res: Response): Promise<string | null> {
  try {
    const body: unknown = await res.clone().json();
    if (typeof body === "object" && body !== null && "error" in body) {
      const e = (body as { error: unknown }).error;
      if (typeof e === "string") return e;
    }
  } catch {
    // Not JSON.
  }
  return null;
}

async function fetchCsrf(hubOrigin: string, fetchImpl: typeof fetch): Promise<string> {
  const res = await fetchImpl(`${hubOrigin}/api/auth/csrf`, {
    method: "GET",
    credentials: "include",
    headers: { Accept: "application/json" },
  });
  if (!res.ok) throw new Error(`csrf: HTTP ${res.status}`);
  const body: unknown = await res.json();
  const token = (body as { csrf_token?: unknown } | null)?.csrf_token;
  if (typeof token !== "string") throw new Error("csrf: unexpected body");
  return token;
}

/** Post one submission and say what happened, in words for the visitor. */
export async function submitLead(
  hubOrigin: string,
  body: LeadBody,
  fetchImpl: typeof fetch = fetch,
): Promise<SubmitOutcome> {
  const post = async (): Promise<Response> => {
    const token = await fetchCsrf(hubOrigin, fetchImpl);
    return fetchImpl(`${hubOrigin}/api/leads`, {
      method: "POST",
      credentials: "include",
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
        "X-CSRF-Token": token,
      },
      body: JSON.stringify(body),
    });
  };
  let res: Response;
  try {
    res = await post();
    if (res.status === 403 && (await errorText(res)) === CSRF_ERROR) res = await post();
  } catch {
    return { ok: false, message: NETWORK_ERROR };
  }
  if (res.status === 202) return { ok: true, message: SUCCESS[body.kind] };
  if (res.status === 400) {
    const e = await errorText(res);
    return { ok: false, message: e ? `Please check the form: ${e}.` : "Please check the form." };
  }
  if (res.status === 429) {
    // The per-IP limit and the email-send limit say different things, so
    // show the hub's own words.
    const e = await errorText(res);
    return {
      ok: false,
      message: `${e ? sentence(e) : TOO_MANY} ${tryAgainIn(res.headers.get("Retry-After"))}`,
    };
  }
  return { ok: false, message: `Something went wrong (HTTP ${res.status}). Try again later.` };
}

/** Wire every open lead form on the page: enable it and post on submit. */
export function initLeadForms(root: ParentNode = document): void {
  for (const form of root.querySelectorAll<HTMLFormElement>("form[data-lead-form][data-hub-origin]")) {
    const kind = form.dataset.leadForm as LeadKind;
    const hubOrigin = form.dataset.hubOrigin ?? "";
    const button = form.querySelector<HTMLButtonElement>('button[type="submit"]');
    const status = form.querySelector<HTMLElement>("[data-lead-status]");
    if (!button || !status || !hubOrigin) continue;
    button.disabled = false;
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      button.disabled = true;
      status.textContent = "Sending…";
      void submitLead(hubOrigin, leadBody(kind, new FormData(form)))
        .then((outcome) => {
          status.textContent = outcome.message;
          if (outcome.ok) form.reset();
        })
        .finally(() => {
          button.disabled = false;
        });
    });
  }
}
