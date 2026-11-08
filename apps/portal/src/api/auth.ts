// Mirrors the `/api/auth/*` shapes in crates/wipe-hub/src/auth.rs and
// `wipe_hub::accounts::{Account, User, Organization, Role}`
// (crates/wipe-hub/src/accounts.rs). Change both together.

export type Role = "owner" | "member" | "vendor_admin";

export interface User {
  id: string;
  email: string;
  name: string;
  role: Role;
  email_verified: boolean;
}

export interface Organization {
  id: string;
  name: string;
}

export interface Account {
  user: User;
  organization: Organization;
}

export interface SignUpInput {
  email: string;
  password: string;
  name: string;
  organization_name: string;
}

export interface SignUpResult extends Account {
  verification_email_sent: boolean;
}

/**
 * An error answered by the hub; `message` is its `{"error": …}` text.
 * `retryAfterSecs` is the hub's `Retry-After` (whole seconds), when sent.
 */
export class HubError extends Error {
  readonly status: number;
  readonly retryAfterSecs: number | null;
  constructor(status: number, message: string, retryAfterSecs: number | null = null) {
    super(message);
    this.name = "HubError";
    this.status = status;
    this.retryAfterSecs = retryAfterSecs;
  }
}

/** `Retry-After` as whole seconds; `null` when absent or not delay-seconds. */
function retryAfter(res: Response): number | null {
  const v = res.headers.get("Retry-After")?.trim();
  return v && /^\d+$/.test(v) ? Number(v) : null;
}

/**
 * What the sign-in form shows for an error: a sign-in lockout (429) says
 * how long to wait, rounded up to whole minutes; anything else is the
 * error's own message.
 */
export function signInErrorMessage(error: unknown): string {
  if (error instanceof HubError && error.status === 429 && error.retryAfterSecs !== null) {
    const minutes = Math.max(1, Math.ceil(error.retryAfterSecs / 60));
    return `Too many failed sign-in attempts. Try again in ${minutes} minute${minutes === 1 ? "" : "s"}.`;
  }
  return error instanceof Error ? error.message : "Something went wrong";
}

type Fetch = typeof fetch;

/** The hub's 403 text for a missing or mismatched CSRF token (crates/wipe-hub/src/csrf.rs). */
export const CSRF_ERROR = "missing or invalid CSRF token";

let cachedCsrf: Promise<string> | null = null;

/**
 * The double-submit CSRF token from `GET /api/auth/csrf`, fetched once and
 * cached for the page's life. The hub sets the matching cookie; every POST
 * echoes the token in `X-CSRF-Token`.
 */
function csrfToken(fetchImpl: Fetch): Promise<string> {
  if (cachedCsrf === null) {
    const pending = fetchCsrf(fetchImpl);
    cachedCsrf = pending;
    // A failed fetch is not cached: the next POST tries again.
    pending.catch(() => {
      if (cachedCsrf === pending) cachedCsrf = null;
    });
  }
  return cachedCsrf;
}

async function fetchCsrf(fetchImpl: Fetch): Promise<string> {
  const res = await ensureOk(
    await fetchImpl("/api/auth/csrf", {
      method: "GET",
      headers: { Accept: "application/json" },
      credentials: "same-origin",
    }),
  );
  const body = await json(
    res,
    (v): v is { csrf_token: string } => isObject(v) && typeof v.csrf_token === "string",
    "csrf",
  );
  return body.csrf_token;
}

/** Test only: forget the cached CSRF token. */
export function resetCsrfCache(): void {
  cachedCsrf = null;
}

async function isCsrfRejection(res: Response): Promise<boolean> {
  if (res.status !== 403) return false;
  try {
    const body: unknown = await res.clone().json();
    return isObject(body) && body.error === CSRF_ERROR;
  } catch {
    return false;
  }
}

async function request(
  fetchImpl: Fetch,
  method: "GET" | "POST",
  path: string,
  body?: unknown,
): Promise<Response> {
  const send = async (): Promise<Response> => {
    const headers: Record<string, string> = { Accept: "application/json" };
    const init: RequestInit = { method, headers, credentials: "same-origin" };
    if (body !== undefined) {
      headers["Content-Type"] = "application/json";
      init.body = JSON.stringify(body);
    }
    if (method === "POST") headers["X-CSRF-Token"] = await csrfToken(fetchImpl);
    return fetchImpl(path, init);
  };
  const res = await send();
  if (method !== "POST" || !(await isCsrfRejection(res))) return res;
  // The cached token went stale (e.g. the cookie expired): refetch and retry once.
  resetCsrfCache();
  return send();
}

/** Throws a `HubError` carrying the hub's `error` message for a non-2xx. */
async function ensureOk(res: Response): Promise<Response> {
  if (res.ok) return res;
  let message = `request failed (HTTP ${res.status})`;
  try {
    const body: unknown = await res.json();
    if (isObject(body) && typeof body.error === "string") message = body.error;
  } catch {
    // Not JSON: keep the generic message.
  }
  throw new HubError(res.status, message, retryAfter(res));
}

async function json<T>(res: Response, guard: (v: unknown) => v is T, what: string): Promise<T> {
  let body: unknown;
  try {
    body = await res.json();
  } catch {
    throw new Error(`${what} returned a non-JSON response (HTTP ${res.status})`);
  }
  if (!guard(body)) throw new Error(`${what} returned an unexpected body (HTTP ${res.status})`);
  return body;
}

/** `POST /api/auth/sign-up` (201). Also signs the new owner in. */
export async function signUp(input: SignUpInput, fetchImpl: Fetch = fetch): Promise<SignUpResult> {
  const res = await ensureOk(await request(fetchImpl, "POST", "/api/auth/sign-up", input));
  return json(res, isSignUpResult, "sign-up");
}

/**
 * `POST /api/auth/sign-in`. A wrong email or password is a 401 `HubError`;
 * too many failures is a 429 `HubError` with the hub's message and its
 * `Retry-After` in `retryAfterSecs` (see `signInErrorMessage`).
 */
export async function signIn(
  email: string,
  password: string,
  fetchImpl: Fetch = fetch,
): Promise<Account> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/auth/sign-in", { email, password }),
  );
  return json(res, isAccount, "sign-in");
}

/** `POST /api/auth/sign-out` (204). */
export async function signOut(fetchImpl: Fetch = fetch): Promise<void> {
  await ensureOk(await request(fetchImpl, "POST", "/api/auth/sign-out"));
}

/** `POST /api/auth/sign-out-everywhere` (204): end every session of this user. */
export async function signOutEverywhere(fetchImpl: Fetch = fetch): Promise<void> {
  await ensureOk(await request(fetchImpl, "POST", "/api/auth/sign-out-everywhere"));
}

/** `GET /api/auth/me`: the signed-in account, or `null` when signed out (401). */
export async function fetchMe(fetchImpl: Fetch = fetch): Promise<Account | null> {
  const res = await request(fetchImpl, "GET", "/api/auth/me");
  if (res.status === 401) return null;
  await ensureOk(res);
  return json(res, isAccount, "me");
}

/** `POST /api/auth/verify-email` with the token from the emailed link. */
export async function verifyEmail(token: string, fetchImpl: Fetch = fetch): Promise<void> {
  await ensureOk(await request(fetchImpl, "POST", "/api/auth/verify-email", { token }));
}

/** `POST /api/auth/verification-email` (202): email a new verification link. */
export async function resendVerification(fetchImpl: Fetch = fetch): Promise<void> {
  await ensureOk(await request(fetchImpl, "POST", "/api/auth/verification-email"));
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null;
}

function isUser(v: unknown): v is User {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.email === "string" &&
    typeof v.name === "string" &&
    (v.role === "owner" || v.role === "member" || v.role === "vendor_admin") &&
    typeof v.email_verified === "boolean"
  );
}

function isOrganization(v: unknown): v is Organization {
  return isObject(v) && typeof v.id === "string" && typeof v.name === "string";
}

function isAccount(v: unknown): v is Account {
  return isObject(v) && isUser(v.user) && isOrganization(v.organization);
}

function isSignUpResult(v: unknown): v is SignUpResult {
  return isObject(v) && isAccount(v) && typeof v.verification_email_sent === "boolean";
}
