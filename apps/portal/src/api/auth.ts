// Mirrors the `/api/auth/*` shapes in crates/wipe-hub/src/auth.rs (sign-up,
// sign-in, verification, sessions, magic links, password reset and password
// change), `POST /api/account/profile` in crates/wipe-hub/src/team_routes.rs
// and `wipe_hub::accounts::{Account, User, Organization, Role}`
// (crates/wipe-hub/src/accounts.rs). Change both together. `request`,
// `ensureOk`, `json` and the type guards are shared with `./team`.

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

/** A 429's `Retry-After` as "N minute(s)", rounded up; `null` for anything else. */
export function waitFor(error: unknown): string | null {
  if (error instanceof HubError && error.status === 429 && error.retryAfterSecs !== null) {
    const minutes = Math.max(1, Math.ceil(error.retryAfterSecs / 60));
    return `${minutes} minute${minutes === 1 ? "" : "s"}`;
  }
  return null;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : "Something went wrong";
}

/**
 * What the sign-in form shows for an error: a sign-in lockout (429) says
 * how long to wait, rounded up to whole minutes; anything else is the
 * error's own message.
 */
export function signInErrorMessage(error: unknown): string {
  const wait = waitFor(error);
  return wait ? `Too many failed sign-in attempts. Try again in ${wait}.` : messageOf(error);
}

/**
 * What a form that makes the hub send email (sign-up, resend verification,
 * magic link, password reset) shows for an error: the email-send limit (429)
 * says how long to wait, rounded up to whole minutes; anything else is the
 * error's own message.
 */
export function emailRequestErrorMessage(error: unknown): string {
  const wait = waitFor(error);
  return wait ? `Too many emails requested. Try again in ${wait}.` : messageOf(error);
}

export type Fetch = typeof fetch;

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

/** A body sent as is (a file upload), with its content type. */
export interface RawBody {
  body: Blob;
  contentType: string;
}

/**
 * A request to the hub. `body` is sent as JSON; a `RawBody` is sent as is
 * (the release upload sends a file as `application/octet-stream`). Every
 * POST carries the CSRF token.
 */
export async function request(
  fetchImpl: Fetch,
  method: "GET" | "POST",
  path: string,
  body?: unknown,
  raw?: RawBody,
): Promise<Response> {
  const send = async (): Promise<Response> => {
    const headers: Record<string, string> = { Accept: "application/json" };
    const init: RequestInit = { method, headers, credentials: "same-origin" };
    if (raw !== undefined) {
      headers["Content-Type"] = raw.contentType;
      init.body = raw.body;
    } else if (body !== undefined) {
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
export async function ensureOk(res: Response): Promise<Response> {
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

export async function json<T>(res: Response, guard: (v: unknown) => v is T, what: string): Promise<T> {
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

/**
 * `POST /api/auth/magic-link` (202): email a sign-in link to `email` if an
 * account has it. The hub answers the same whether or not one does.
 */
export async function requestMagicLink(email: string, fetchImpl: Fetch = fetch): Promise<void> {
  await ensureOk(await request(fetchImpl, "POST", "/api/auth/magic-link", { email }));
}

/**
 * `POST /api/auth/magic-link/sign-in` with the token from the emailed link:
 * signs in and returns the account. A used, expired or unknown link is a 400
 * `HubError`.
 */
export async function signInWithMagicLink(token: string, fetchImpl: Fetch = fetch): Promise<Account> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/auth/magic-link/sign-in", { token }),
  );
  return json(res, isAccount, "magic-link sign-in");
}

/**
 * `POST /api/auth/password-reset` (202): email a password reset link to
 * `email` if an account has it. The hub answers the same whether or not one
 * does.
 */
export async function requestPasswordReset(email: string, fetchImpl: Fetch = fetch): Promise<void> {
  await ensureOk(await request(fetchImpl, "POST", "/api/auth/password-reset", { email }));
}

/**
 * `POST /api/auth/password-reset/complete`: set a new password with the
 * token from the emailed link. Every session of the account ends, this
 * browser's included: the user then signs in with the new password.
 */
export async function resetPassword(
  token: string,
  password: string,
  fetchImpl: Fetch = fetch,
): Promise<void> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/auth/password-reset/complete", { token, password }),
  );
  await json(
    res,
    (v): v is { password_reset: true } => isObject(v) && v.password_reset === true,
    "password reset",
  );
}

/**
 * `POST /api/auth/password`: change the signed-in user's password. Every
 * other session of the account ends; this browser gets a new session and
 * stays signed in. A wrong current password is a 400 `HubError`; too many
 * wrong ones lock like sign-in (429, see `signInErrorMessage`).
 */
export async function changePassword(
  currentPassword: string,
  newPassword: string,
  fetchImpl: Fetch = fetch,
): Promise<Account> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/auth/password", {
      current_password: currentPassword,
      new_password: newPassword,
    }),
  );
  return json(res, isAccount, "password change");
}

/** `POST /api/account/profile`: set the signed-in user's name. */
export async function updateProfile(name: string, fetchImpl: Fetch = fetch): Promise<Account> {
  const res = await ensureOk(await request(fetchImpl, "POST", "/api/account/profile", { name }));
  return json(res, isAccount, "profile");
}

export function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null;
}

export function isUser(v: unknown): v is User {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.email === "string" &&
    typeof v.name === "string" &&
    (v.role === "owner" || v.role === "member" || v.role === "vendor_admin") &&
    typeof v.email_verified === "boolean"
  );
}

export function isOrganization(v: unknown): v is Organization {
  return isObject(v) && typeof v.id === "string" && typeof v.name === "string";
}

export function isAccount(v: unknown): v is Account {
  return isObject(v) && isUser(v.user) && isOrganization(v.organization);
}

function isSignUpResult(v: unknown): v is SignUpResult {
  return isObject(v) && isAccount(v) && typeof v.verification_email_sent === "boolean";
}
