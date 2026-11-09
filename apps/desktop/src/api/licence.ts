/**
 * How the bench reads `GET /api/license` into the station-wide licence
 * banner (SPEC CAP-3, ADR-0012 §6). Pure, so the wording is tested without
 * rendering. The licence verdict (`status`) is the server's: nothing here
 * re-evaluates the licence.
 */
import type { LicenseInfo } from "./types";

/** A valid licence starts warning at this many days left. */
export const EXPIRY_NOTICE_DAYS = 30;
/** At or below this many days left the warning says "renew now". */
export const EXPIRY_URGENT_DAYS = 7;
/** At or below this many days left the warning turns to danger tone. */
export const EXPIRY_FINAL_DAYS = 1;

export type LicenceBannerView = {
  tone: "warning" | "danger";
  heading: string;
  lines: string[];
};

/** Shown when the station could not report its licence at all. */
export const LICENCE_UNKNOWN_BANNER: LicenceBannerView = {
  tone: "warning",
  heading: "Licence status unknown",
  lines: [
    "This station could not report its licence, so its certificates may be marked evaluation.",
  ],
};

/** The UTC calendar date of an RFC 3339 `Z` timestamp, labelled as UTC. */
function expiryDate(notAfter: string | null): string | null {
  if (!notAfter) return null;
  const m = /^\d{4}-\d{2}-\d{2}/.exec(notAfter);
  return m ? `${m[0]} UTC` : null;
}

/**
 * `days` is whole days left, rounded down by the server, so `0` means
 * under 24 hours and `1` under 48: never "today"/"tomorrow", which would
 * not match a calendar date.
 */
function expiryHeading(days: number, date: string | null): string {
  const on = date ? `, on ${date}` : "";
  const when =
    days <= 0 ? "within a day" : days === 1 ? "in 1 day" : `in ${days} days`;
  return days <= EXPIRY_URGENT_DAYS
    ? `Licence expires ${when}${on} — renew now`
    : `Licence expires ${when}${on}`;
}

function lapsePolicyLine(info: LicenseInfo): string {
  return info.lapse_policy === "stop"
    ? "This licence's lapse policy is stop: new jobs will be refused when it expires. Renew and install the new licence before then."
    : "When it expires, erasure continues, but certificates will be marked evaluation until a renewed licence is installed.";
}

/**
 * The banner for this licence report, or `null` when there is nothing to
 * say: a valid licence with more than `EXPIRY_NOTICE_DAYS` left.
 *
 * Any non-valid status shows the server's `operator_message` verbatim. A
 * valid licence warns from `EXPIRY_NOTICE_DAYS` on, says "renew now" from
 * `EXPIRY_URGENT_DAYS`, and turns to danger tone from `EXPIRY_FINAL_DAYS`.
 */
export function licenceBanner(info: LicenseInfo): LicenceBannerView | null {
  const s = info.status;
  if (s.status !== "valid") {
    return s.status === "unlicensed"
      ? {
          tone: "warning",
          heading: "Evaluation mode — no licence installed",
          lines: [info.operator_message],
        }
      : {
          tone: "danger",
          heading: "Licensed signing unavailable — certificates are marked evaluation",
          lines: [info.operator_message],
        };
  }
  if (s.days_remaining > EXPIRY_NOTICE_DAYS) return null;
  return {
    tone: s.days_remaining <= EXPIRY_FINAL_DAYS ? "danger" : "warning",
    heading: expiryHeading(s.days_remaining, expiryDate(info.not_after)),
    lines: [lapsePolicyLine(info)],
  };
}
