import { useQuery } from "@tanstack/react-query";

import { api, classNames } from "@/api/client";
import { LICENCE_UNKNOWN_BANNER, licenceBanner } from "@/api/licence";

/**
 * A persistent, non-dismissible bar on every bench page while the station
 * refuses new jobs, is unlicensed, its licence is not valid, or it expires
 * within 30 days (SPEC CAP-3, ADR-0013 §1). It informs only: no button, link
 * or prompt. The station refuses starts itself; this bar only says so.
 */
export function LicenceBanner() {
  const license = useQuery({
    queryKey: ["license"],
    queryFn: api.license,
    // `GET /api/license` is a pure read (no watermark move, no save), so
    // polling is safe; once a minute is enough for a days-granular expiry.
    refetchInterval: 60_000,
    // Overrides the app-wide `false`: a station left open overnight should re-read on return.
    refetchOnWindowFocus: true,
  });
  // Silence would read as "licensed", so an unreadable licence says so.
  const view = license.isError
    ? LICENCE_UNKNOWN_BANNER
    : license.data
      ? licenceBanner(license.data)
      : null;
  if (!view) return null;
  const danger = view.tone === "danger";
  return (
    <div
      role="status"
      className={classNames(
        "w-full border-b px-4 py-2 text-sm",
        danger
          ? "border-rose-700 bg-rose-950 text-rose-200"
          : "border-amber-700 bg-amber-950 text-amber-200",
      )}
    >
      <p className={classNames("font-semibold", danger ? "text-rose-100" : "text-amber-100")}>
        {view.heading}
      </p>
      {view.lines.map((line) => (
        <p key={line}>{line}</p>
      ))}
    </div>
  );
}
