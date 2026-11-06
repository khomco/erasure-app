import { useQuery } from "@tanstack/react-query";
import { fetchHealth, type HealthReport } from "@/api/health";

export function HomePage() {
  const health = useQuery({
    queryKey: ["health"],
    queryFn: () => fetchHealth(),
    refetchInterval: 10_000,
    // Report a down hub at once rather than after React Query's retries.
    retry: false,
  });

  return (
    <section className="space-y-4">
      <h1 className="text-xl font-semibold">Portal</h1>
      <div className="rounded-md border border-slate-800 bg-slate-900 p-4">
        <h2 className="text-sm font-medium text-slate-400">Hub status</h2>
        {health.isPending && <p className="mt-2 text-sm text-slate-400">Checking…</p>}
        {health.isError && (
          <p className="mt-2 text-sm text-rose-400">
            Hub unreachable: {health.error instanceof Error ? health.error.message : "unknown error"}
          </p>
        )}
        {!health.isError && health.data && <HealthDetails report={health.data} />}
      </div>
    </section>
  );
}

function HealthDetails({ report }: { report: HealthReport }) {
  const ok = report.status === "ok";
  return (
    <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
      <dt className="text-slate-400">Status</dt>
      <dd className={ok ? "text-emerald-400" : "text-amber-400"}>{report.status}</dd>
      <dt className="text-slate-400">Database</dt>
      <dd>{report.database}</dd>
      <dt className="text-slate-400">Mail</dt>
      <dd>{report.mail}</dd>
    </dl>
  );
}
