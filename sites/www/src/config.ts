/**
 * Site-wide settings. Brand and origin are placeholders until the vendor
 * decides (SPEC open question); `SITE_URL` overrides the origin at build time.
 */
import { parseHubOrigin } from "./lib/lead-submit";

export const PLACEHOLDER_ORIGIN = "https://wipestation.example";

export const site = {
  name: "Wipestation",
  /**
   * Analytics hook (ADR-0006 "Not decided here"). Off by default: no vendor is
   * chosen and no consent banner exists. When enabled, `scriptSrc` is rendered
   * as a single deferred script; a third-party origin will still fail the
   * build-time site check, so a tracker must be self-hosted on the site origin.
   */
  analytics: {
    enabled: false,
    scriptSrc: "",
  },
  /**
   * Whether `/privacy/` is the full privacy notice rather than the placeholder.
   * The placeholder promises the forms will not open before the notice is
   * published, so `astro build` refuses `PUBLIC_HUB_ORIGIN` while this is
   * `false` (`src/integrations/site-check.ts`). Publishing the notice is legal
   * copy and a vendor decision: set it with the new page, never on its own.
   */
  privacyNoticePublished: false,
} as const;

/**
 * The hub the early-access and contact forms post to, from
 * `PUBLIC_HUB_ORIGIN` (lead-capture story 1). `""` when unset: the forms stay
 * closed and ship no script. Read at build time.
 */
export function hubOrigin(): string {
  return parseHubOrigin(import.meta.env.PUBLIC_HUB_ORIGIN);
}
