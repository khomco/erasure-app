import { Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { CheckCircle2, CircleOff, Loader2, XCircle } from "lucide-react";

import { api, classNames, formatBytes } from "@/api/client";
import type { Job, JobStateLabel } from "@/api/types";
import { useEventStream } from "@/api/ws";

function StateIcon({ state }: { state: JobStateLabel }) {
  if (state === "completed")
    return <CheckCircle2 className="h-4 w-4 text-emerald-400" />;
  if (state === "failed") return <XCircle className="h-4 w-4 text-rose-400" />;
  if (state === "aborted")
    return <CircleOff className="h-4 w-4 text-slate-400" />;
  return <Loader2 className="h-4 w-4 animate-spin text-indigo-400" />;
}

export function JobsPage() {
  const jobs = useQuery({
    queryKey: ["jobs"],
    queryFn: api.jobs,
    refetchInterval: 1500,
  });
  // Pulse the list whenever a job event comes in.
  useEventStream(() => {});

  if (jobs.isLoading) return <div className="text-slate-400">Loading jobs…</div>;
  const list = (jobs.data ?? []).slice().reverse();
  if (list.length === 0) {
    return (
      <div className="card text-slate-400">
        No jobs yet. Start one from the Devices page.
      </div>
    );
  }
  return (
    <div className="space-y-2">
      <h2 className="text-lg font-semibold">Jobs</h2>
      <div className="space-y-2">
        {list.map((j) => (
          <JobRow key={j.id} job={j} />
        ))}
      </div>
    </div>
  );
}

function JobRow({ job }: { job: Job }) {
  return (
    <Link
      to="/jobs/$jobId"
      params={{ jobId: job.id }}
      className="block transition hover:border-indigo-500/40"
    >
      <div className="card flex items-center justify-between gap-4">
        <div className="flex min-w-0 items-center gap-3">
          <StateIcon state={job.state.state} />
          <div className="min-w-0">
            <div className="truncate font-medium">
              {job.device_snapshot.vendor} {job.device_snapshot.model}
            </div>
            <div className="truncate text-xs text-slate-400">
              {job.device_snapshot.serial} · {formatBytes(job.device_snapshot.capacity_bytes)} ·{" "}
              {job.spec.classification}/{job.spec.intent}
            </div>
          </div>
        </div>
        <div className="flex items-center gap-3">
          {job.progress && job.state.state !== "completed" && (
            <div className="w-32">
              <div className="h-1.5 overflow-hidden rounded-full bg-slate-800">
                <div
                  className={classNames(
                    "h-full rounded-full transition-all",
                    job.state.state === "failed" ? "bg-rose-500" : "bg-indigo-500"
                  )}
                  style={{ width: `${Math.round(job.progress.fraction * 100)}%` }}
                />
              </div>
              <div className="mt-0.5 text-[10px] text-slate-500">
                {Math.round(job.progress.fraction * 100)}% · {job.progress.stage}
              </div>
            </div>
          )}
          <span className="pill">{job.state.state}</span>
        </div>
      </div>
    </Link>
  );
}
