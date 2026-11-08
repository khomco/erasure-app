import { useQuery } from "@tanstack/react-query";
import { BANNER_HEADING, bannerLines, fetchDeployment } from "./api/deployment";

/**
 * A persistent, non-dismissible bar on every portal page while the hub
 * reports itself a development hub (`GET /api/deployment`, CAP-3). If the
 * hub cannot say, nothing is shown: no guessed banner.
 */
export function DevelopmentBanner() {
  const deployment = useQuery({
    queryKey: ["deployment"],
    queryFn: () => fetchDeployment(),
    // Fixed for one hub process, but the tab may outlive it (a hub restarted
    // with another provider or root) or load before it (a dev server up
    // first), so keep re-reading it.
    staleTime: 30_000,
    refetchOnWindowFocus: true,
    refetchInterval: 60_000,
  });
  const report = deployment.data;
  if (!report?.development) return null;
  return (
    <div
      role="status"
      className="w-full border-b border-amber-700 bg-amber-950 px-6 py-2 text-sm text-amber-200"
    >
      <p className="font-semibold text-amber-100">{BANNER_HEADING}</p>
      {bannerLines(report).map((line) => (
        <p key={line}>{line}</p>
      ))}
    </div>
  );
}
