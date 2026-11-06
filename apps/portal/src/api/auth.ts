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

/** An error answered by the hub; `message` is its `{"error": …}` text. */
export class HubError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "HubError";
    this.status = status;
  }
}

type Fetch = typeof fetch;

async function request(
  fetchImpl: Fetch,
  method: "GET" | "POST",
  path: string,
  body?: unknown,
): Promise<Response> {
  const headers: Record<string, string> = { Accept: "application/json" };
  const init: RequestInit = { method, headers, credentials: "same-origin" };
  if (body !== undefined) {
    headers["Content-Type"] = "application/json";
    init.body = JSON.stringify(body);
  }
  return fetchImpl(path, init);
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
  throw new HubError(res.status, message);
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

/** `POST /api/auth/sign-in`. A wrong email or password is a 401 `HubError`. */
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
