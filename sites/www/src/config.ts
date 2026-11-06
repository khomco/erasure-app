/**
 * Site-wide settings. Brand and origin are placeholders until the vendor
 * decides (SPEC open question); `SITE_URL` overrides the origin at build time.
 */
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
} as const;
