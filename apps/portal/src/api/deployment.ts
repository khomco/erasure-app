// Mirrors `wipe_hub::deployment::DeploymentReport`
// (crates/wipe-hub/src/deployment.rs). The hub decides whether it is a
// development hub; the portal only renders `development` and `reasons`.
export interface DeploymentReport {
  development: boolean;
  payment_provider: string;
  /** `file` or `none` today; a future custody kind adds a value. */
  vendor_root: string;
  /** Machine codes: `fake_payment_provider`, `file_vendor_root`. */
  reasons: string[];
}

/** The banner's first line. */
export const BANNER_HEADING = "Development hub — not for production use.";

/** One sentence per reason code the hub reports. */
const REASON_LINES = new Map<string, string>([
  ["fake_payment_provider", "Payments are simulated by the test payment provider; no money moves."],
  [
    "file_vendor_root",
    "The vendor root is held in a file, so licences signed here are for testing only.",
  ],
]);

/**
 * `GET /api/deployment`. A network error, a non-JSON body or a body that is
 * not a deployment report throws, and the caller shows no banner.
 */
export async function fetchDeployment(fetchImpl: typeof fetch = fetch): Promise<DeploymentReport> {
  const res = await fetchImpl("/api/deployment", { headers: { Accept: "application/json" } });
  let body: unknown;
  try {
    body = await res.json();
  } catch {
    throw new Error(`hub deployment returned a non-JSON response (HTTP ${res.status})`);
  }
  if (!res.ok || !isDeploymentReport(body)) {
    throw new Error(`hub deployment returned an unexpected body (HTTP ${res.status})`);
  }
  return body;
}

function isDeploymentReport(v: unknown): v is DeploymentReport {
  if (typeof v !== "object" || v === null) return false;
  const r = v as Record<string, unknown>;
  return (
    typeof r.development === "boolean" &&
    typeof r.payment_provider === "string" &&
    typeof r.vendor_root === "string" &&
    Array.isArray(r.reasons) &&
    r.reasons.every((x) => typeof x === "string")
  );
}

/**
 * The banner's reason lines: one fixed sentence per known reason, in the
 * hub's order. Unknown reasons are skipped; a non-development hub gives none.
 */
export function bannerLines(report: DeploymentReport): string[] {
  if (!report.development) return [];
  return report.reasons.flatMap((r) => {
    const line = REASON_LINES.get(r);
    return line === undefined ? [] : [line];
  });
}
