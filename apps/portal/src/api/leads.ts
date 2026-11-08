// Mirrors `POST /api/leads/unsubscribe` in crates/wipe-hub/src/lead_routes.rs
// (lead-capture story 1). Change both together. The marketing site, not the
// portal, posts sales leads (`sites/www/src/lib/lead-submit.ts`).

import { ensureOk, isObject, json, request, type Fetch } from "./auth";

/**
 * `POST /api/leads/unsubscribe` with the token from a sales-lead
 * confirmation email. Resolves once the sales lead is unsubscribed, also
 * when it already was; a malformed or unknown token is a 404 `HubError`.
 */
export async function unsubscribeLead(token: string, fetchImpl: Fetch = fetch): Promise<void> {
  const res = await ensureOk(
    await request(fetchImpl, "POST", "/api/leads/unsubscribe", { token }),
  );
  await json(
    res,
    (v): v is { unsubscribed: true } => isObject(v) && v.unsubscribed === true,
    "unsubscribe",
  );
}
