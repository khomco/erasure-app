/**
 * The consent wording of the early-access and contact forms (lead-capture
 * story 1). This file is the only copy on the site: `LeadForm.astro` renders
 * the statement and the privacy sentence as the consent label, and the form
 * script submits `consentText(kind)`, the label's whole text.
 *
 * The hub stores that text with each submission as proof of consent and
 * accepts only its own copy of it (`crates/wipe-hub/src/leads.rs`, pinned to
 * this file by a unit test). Changing a word here therefore needs the same
 * change in the hub; until both ship, the hub answers "the consent text has
 * changed".
 */
export type LeadKind = "early-access" | "contact";

export const CONSENT_STATEMENTS = {
  "early-access": "I agree that these details are used only to follow up on early access.",
  contact: "I agree that these details are used only to reply to this message.",
} as const satisfies Record<LeadKind, string>;

/** Follows each statement; "privacy notice" links to `/privacy/`. */
export const PRIVACY_SENTENCE = "See the privacy notice.";
/** The linked words inside `PRIVACY_SENTENCE`. */
export const PRIVACY_LINK_TEXT = "privacy notice";

/** The consent label's whole text, exactly as the hub stores it. */
export function consentText(kind: LeadKind): string {
  return `${CONSENT_STATEMENTS[kind]} ${PRIVACY_SENTENCE}`;
}
