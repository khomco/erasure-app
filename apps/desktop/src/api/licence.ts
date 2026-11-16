/**
 * How the bench reads `GET /api/license` into the station-wide licence
 * banner (SPEC CAP-3, ADR-0013 §1). Pure, so the wording is tested without
 * rendering. The licence verdict (`status`) and whether new jobs are
 * refused (`new_jobs_refused`) are the server's: nothing here re-evaluates
 * the licence.
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
    "This station could not report its licence, so new jobs may be refused or their certificates marked evaluation.",
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

/** What expiry will mean for new jobs on this station. */
function expiryConsequence(info: LicenseInfo, date: string | null): string {
  if (info.simulated_backend) {
    return "This station's backend is simulated: when the licence expires, jobs still run, with certificates marked evaluation, until a renewed licence is installed.";
  }
  const from = date ? `From ${date}` : "When it expires";
  return `${from}, new jobs will be refused until a renewed licence is installed. Renew and install it before then.`;
}

/** The line a simulated station adds to a status that does not cover jobs. */
const SIMULATED_EVALUATION_LINE =
  "This station's backend is simulated: jobs still run, and their certificates are marked evaluation.";

/**
 * The banner for this licence report, or `null` when there is nothing to
 * say: a valid licence with more than `EXPIRY_NOTICE_DAYS` left.
 *
 * When the server says new jobs are refused, a danger banner shows its
 * refusal text verbatim. Otherwise any non-valid status shows the server's
 * `operator_message` with its consequence. A valid licence warns from
 * `EXPIRY_NOTICE_DAYS` on, says "renew now" from `EXPIRY_URGENT_DAYS`, and
 * turns to danger tone from `EXPIRY_FINAL_DAYS`. A valid licence whose
 * renewal payment is overdue (`renewal_overdue`, real backends only) says
 * so, whatever the days left, with the same tone rule.
 */
export function licenceBanner(info: LicenseInfo): LicenceBannerView | null {
  if (info.new_jobs_refused) {
    return {
      tone: "danger",
      heading: "New jobs are refused",
      lines: [info.new_jobs_refused],
    };
  }
  const s = info.status;
  if (s.status !== "valid") {
    if (s.status === "unlicensed") {
      return {
        tone: "warning",
        heading: "Evaluation mode — no licence installed",
        lines: [info.operator_message, SIMULATED_EVALUATION_LINE],
      };
    }
    // A lease fault that refuses nothing: on an `Unlimited` licence the
    // station's decision stays licensed, so do not claim evaluation.
    if (s.status === "lease_state_unavailable" || !info.simulated_backend) {
      return {
        tone: "warning",
        heading: "Licence state needs attention",
        lines: [
          info.operator_message,
          info.simulated_backend
            ? "This station's backend is simulated: new jobs still start. Fix the problem named above."
            : "New jobs still start under this licence. Fix the problem named above.",
        ],
      };
    }
    return {
      tone: "danger",
      heading: "Licence not in force — certificates are marked evaluation",
      lines: [info.operator_message, SIMULATED_EVALUATION_LINE],
    };
  }
  const date = expiryDate(info.not_after);
  // Inside a renewal grace whose paid term has passed (ADR-0013 §4): the
  // server's line says since when and from when new jobs are refused.
  if (info.renewal_overdue) {
    return {
      tone: s.days_remaining <= EXPIRY_FINAL_DAYS ? "danger" : "warning",
      heading: "Renewal payment overdue",
      lines: [info.renewal_overdue, expiryConsequence(info, date)],
    };
  }
  if (s.days_remaining > EXPIRY_NOTICE_DAYS) return null;
  return {
    tone: s.days_remaining <= EXPIRY_FINAL_DAYS ? "danger" : "warning",
    heading: expiryHeading(s.days_remaining, date),
    lines: [expiryConsequence(info, date)],
  };
}
