/**
 * How the Licence page reads `GET /api/enrollment` into its Register with
 * platform panel (ADR-0008, station-enrollment story 4). Pure, so every
 * state's wording is tested without rendering. The outcome is the station's:
 * nothing here decides whether a licence is valid.
 */
import type { EnrollmentAttempt, EnrollmentView } from "./types";

/** How often the page re-reads a pending attempt. */
export const PENDING_REFETCH_MS = 2000;

export type EnrollmentPanelKind =
  /** The station cannot enroll (no vendor root, or a run-only key): the reason, no form. */
  | "unavailable"
  /** No attempt: the hub address form. */
  | "form"
  /** Waiting for an owner to approve the code. */
  | "pending"
  /** Approved, verified and installed: Register again. */
  | "installed"
  /** Denied, expired, unreachable, refused or failed: start again. */
  | "ended";

export interface EnrollmentPanel {
  kind: EnrollmentPanelKind;
  heading: string;
  /** `m:ss` until the code expires; pending only. */
  countdown: string | null;
}

/** `seconds` as `m:ss`, floored at `0:00`. */
export function countdownText(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** The heading for an ended attempt, naming what happened. */
export function endedHeading(state: EnrollmentAttempt["state"]): string {
  switch (state) {
    case "denied":
      return "Registration denied";
    case "expired":
      return "The code expired";
    case "unreachable":
      return "The platform could not be reached";
    case "refused":
      return "The platform refused this registration";
    case "failed":
      return "Registration failed";
    case "pending":
      return "Waiting for approval";
    case "installed":
      return "Licensed";
  }
}

export function enrollmentPanel(view: EnrollmentView): EnrollmentPanel {
  const attempt = view.attempt;
  if (attempt?.state === "pending") {
    return {
      kind: "pending",
      heading: "Waiting for approval",
      countdown: countdownText(attempt.seconds_left),
    };
  }
  if (attempt?.state === "installed") {
    return { kind: "installed", heading: "Licensed", countdown: null };
  }
  if (attempt) {
    return { kind: "ended", heading: endedHeading(attempt.state), countdown: null };
  }
  if (!view.can_enroll) {
    return { kind: "unavailable", heading: "Registration unavailable", countdown: null };
  }
  return { kind: "form", heading: "Register with platform", countdown: null };
}

/** The address the form starts with: the last attempt's, else the saved one. */
export function initialHubAddress(view: EnrollmentView): string {
  return view.attempt?.hub_origin ?? view.saved_hub_origin ?? "";
}

/**
 * The station's `error` text from a rejected enrollment request (Register,
 * Cancel or Start again; the client's message is `"<status> <reason>: <body>"`),
 * else the message as is.
 */
export function requestErrorText(err: unknown): string {
  const message = err instanceof Error ? err.message : String(err);
  const body = message.indexOf(": ");
  if (body >= 0) {
    try {
      const parsed = JSON.parse(message.slice(body + 2)) as { error?: unknown };
      if (typeof parsed.error === "string") return parsed.error;
    } catch {
      // not JSON: fall through
    }
  }
  return message;
}
