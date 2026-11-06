// Mirrors `wipe_hub::health::HealthReport` (crates/wipe-hub/src/health.rs).
export type HealthStatus = "ok" | "degraded";
export type ComponentStatus = "up" | "down";

export interface HealthReport {
  status: HealthStatus;
  database: ComponentStatus;
  mail: ComponentStatus;
}

/**
 * `GET /api/health`. The hub answers 200 when everything is up and 503 with
 * the same body when degraded, so both statuses return the parsed report.
 * A network error or a body that is not a health report throws.
 */
export async function fetchHealth(fetchImpl: typeof fetch = fetch): Promise<HealthReport> {
  const res = await fetchImpl("/api/health", { headers: { Accept: "application/json" } });
  let body: unknown;
  try {
    body = await res.json();
  } catch {
    throw new Error(`hub health returned a non-JSON response (HTTP ${res.status})`);
  }
  if (!isHealthReport(body)) {
    throw new Error(`hub health returned an unexpected body (HTTP ${res.status})`);
  }
  return body;
}

function isHealthReport(v: unknown): v is HealthReport {
  if (typeof v !== "object" || v === null) return false;
  const r = v as Record<string, unknown>;
  return (
    (r.status === "ok" || r.status === "degraded") &&
    (r.database === "up" || r.database === "down") &&
    (r.mail === "up" || r.mail === "down")
  );
}
