// Mirrors an owner's enrollment routes in crates/wipe-hub/src/enrollment_routes.rs
// (`POST /api/enrollments/{lookup,approve,deny}`) and the serde shape
// `wipe_hub::enrollments::PendingEnrollment` (crates/wipe-hub/src/enrollments.rs).
// Change both together.
//
// A station shows a user code (`XXXX-XXXX`) and sends its owner to `/activate`
// (station-enrollment story 2, ADR-0008 §1). The code travels in request
// bodies only, never in an API URL, and each action sends it again.

import { ensureOk, isObject, json, request, waitFor, type Fetch } from "./auth";
import { isLicence, isStation, type Registered } from "./stations";

/**
 * A station waiting for an owner's approval: what is asking, from where and
 * when. Timestamps are RFC 3339 UTC.
 */
export interface PendingEnrollment {
  /** `XXXX-XXXX`. */
  user_code: string;
  /** The bench's `--station-id`. */
  station_id: string;
  label: string | null;
  instance_key_id: string;
  /** The network address the request came from, when the hub saw one. */
  requested_from: string | null;
  requested_at: string;
  expires_at: string;
}

const isNullableString = (v: unknown): v is string | null => v === null || typeof v === "string";

export function isPendingEnrollment(v: unknown): v is PendingEnrollment {
  return (
    isObject(v) &&
    typeof v.user_code === "string" &&
    typeof v.station_id === "string" &&
    isNullableString(v.label) &&
    typeof v.instance_key_id === "string" &&
    isNullableString(v.requested_from) &&
    typeof v.requested_at === "string" &&
    typeof v.expires_at === "string"
  );
}

const isLookup = (v: unknown): v is { enrollment: PendingEnrollment } =>
  isObject(v) && isPendingEnrollment(v.enrollment);

const isRegistered = (v: unknown): v is Registered =>
  isObject(v) && isStation(v.station) && isLicence(v.licence);

/**
 * What the activation page shows for a hub error: too many wrong codes (429)
 * says how long to wait, rounded up to whole minutes; anything else is the
 * hub's own message.
 */
export function enrollmentErrorMessage(error: unknown): string {
  const wait = waitFor(error);
  if (wait) return `Too many wrong codes. Try again in ${wait}.`;
  return error instanceof Error ? error.message : "Something went wrong";
}

/** `POST /api/enrollments/lookup` (owner): the pending enrollment `code` names. */
export async function lookupEnrollment(
  code: string,
  fetchImpl: Fetch = fetch,
): Promise<PendingEnrollment> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/enrollments/lookup", { code: code.trim() }),
  );
  return (await json(res, isLookup, "enrollment lookup")).enrollment;
}

/**
 * `POST /api/enrollments/approve` (owner, 201): registers the station against
 * `entitlementId`'s seat and issues its licence, which the station then
 * collects by itself. The `POST /api/stations` shape.
 */
export async function approveEnrollment(
  code: string,
  entitlementId: string,
  fetchImpl: Fetch = fetch,
): Promise<Registered> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/enrollments/approve", {
      code: code.trim(),
      entitlement_id: entitlementId,
    }),
  );
  return json(res, isRegistered, "enrollment approval");
}

/** `POST /api/enrollments/deny` (owner): the station's next poll is refused. */
export async function denyEnrollment(code: string, fetchImpl: Fetch = fetch): Promise<void> {
  await ensureOk(await request(fetchImpl, "POST", "/api/enrollments/deny", { code: code.trim() }));
}
