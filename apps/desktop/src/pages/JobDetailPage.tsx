import { Link, useParams } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, ShieldCheck } from "lucide-react";

import { api, classNames, formatBytes } from "@/api/client";
import { useJobLiveState } from "@/api/ws";
import type { JobUpdate } from "@/api/types";

export function JobDetailPage() {
  const { jobId } = useParams({ from: "/jobs/$jobId" });
  const qc = useQueryClient();
  const job = useQuery({
    queryKey: ["job", jobId],
    queryFn: () => api.job(jobId),
    refetchInterval: 1500,
  });
  const live = useJobLiveState(job.data);

  const abort = useMutation({
    mutationFn: () => api.abortJob(jobId),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["job", jobId] }),
  });

  if (!live) return <div className="text-slate-400">Loading job…</div>;

  const pct = Math.round((live.progress?.fraction ?? 0) * 100);
  const terminal = ["completed", "failed", "aborted"].includes(live.state.state);

  return (
    <div className="space-y-4">
      <Link
        to="/jobs"
        className="inline-flex items-center gap-1 text-xs text-slate-400 hover:text-slate-200"
      >
        <ArrowLeft className="h-3 w-3" /> All jobs
      </Link>

      <div className="card">
        <div className="flex items-start justify-between">
          <div>
            <h2 className="text-lg font-semibold">
              {live.device_snapshot.vendor} {live.device_snapshot.model}
            </h2>
            <p className="mt-0.5 text-xs text-slate-400">
              {live.device_snapshot.serial} ·{" "}
              {formatBytes(live.device_snapshot.capacity_bytes)} ·{" "}
              {live.device_snapshot.path}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <span className="pill">{live.spec.classification}</span>
            <span className="pill">{live.spec.intent}</span>
            <span
              className={classNames(
                "pill",
                live.state.state === "completed"
                  ? "pill-success"
                  : live.state.state === "failed"
                    ? "pill-danger"
                    : "pill-info"
              )}
            >
              {live.state.state}
            </span>
          </div>
        </div>

        <div className="mt-4">
          <div className="h-3 overflow-hidden rounded-full bg-slate-800">
            <div
              className={classNames(
                "h-full rounded-full transition-all",
                live.state.state === "failed"
                  ? "bg-rose-500"
                  : live.state.state === "completed"
                    ? "bg-emerald-500"
                    : "bg-indigo-500"
              )}
              style={{ width: `${pct}%` }}
            />
          </div>
          <div className="mt-1 flex items-center justify-between text-xs text-slate-400">
            <span>{live.progress?.stage ?? "queued"}</span>
            <span>
              {pct}% ·{" "}
              {live.progress?.eta_seconds != null
                ? `${live.progress.eta_seconds}s remaining`
                : ""}
            </span>
          </div>
        </div>

        <div className="mt-4 flex items-center justify-end gap-2">
          {!terminal && (
            <button
              className="btn btn-danger"
              onClick={() => abort.mutate()}
              disabled={abort.isPending}
            >
              {abort.isPending ? "Aborting…" : "Abort"}
            </button>
          )}
          {live.state.state === "completed" && (
            <Link
              to="/certs/$jobId"
              params={{ jobId }}
              className="btn btn-primary"
            >
              <ShieldCheck className="h-4 w-4" /> View certificate
            </Link>
          )}
        </div>
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <div className="card">
          <h3 className="mb-2 text-sm font-semibold uppercase tracking-wide text-slate-400">
            Resolved method
          </h3>
          {live.resolved_method ? (
            <div>
              <div className="font-mono text-sm">{live.resolved_method.kind}</div>
              <pre className="mt-2 overflow-x-auto rounded-md bg-slate-950/60 p-3 text-[11px] text-slate-300">
                {JSON.stringify(live.resolved_method, null, 2)}
              </pre>
            </div>
          ) : (
            <span className="text-slate-500">pending</span>
          )}
        </div>

        <div className="card">
          <h3 className="mb-2 text-sm font-semibold uppercase tracking-wide text-slate-400">
            Verification
          </h3>
          {live.verification ? (
            <div className="space-y-1 text-sm">
              <div>
                Result:{" "}
                <span
                  className={
                    live.verification.all_passed
                      ? "text-emerald-300"
                      : "text-rose-300"
                  }
                >
                  {live.verification.all_passed ? "all passed" : "failures present"}
                </span>
              </div>
              <div className="text-xs text-slate-400">
                {live.verification.sample_count} samples,{" "}
                {formatBytes(live.verification.bytes_sampled)} sampled
              </div>
            </div>
          ) : (
            <span className="text-slate-500">not yet run</span>
          )}
        </div>
      </div>

      <div className="card">
        <h3 className="mb-2 text-sm font-semibold uppercase tracking-wide text-slate-400">
          Event log ({live.events.length})
        </h3>
        <div className="max-h-72 overflow-auto rounded-md border border-slate-800 bg-slate-950/60">
          {live.events
            .slice()
            .reverse()
            .map((e, i) => (
              <EventRow event={e} key={i} />
            ))}
        </div>
      </div>
    </div>
  );
}

function EventRow({ event }: { event: JobUpdate }) {
  const kind = event.event.kind;
  let label: string = kind;
  let detail: React.ReactNode = null;
  switch (kind) {
    case "state_changed":
      label = `state → ${event.event.to.state}`;
      break;
    case "progress":
      label = `progress ${Math.round(event.event.fraction * 100)}%`;
      detail = (
        <span className="text-slate-500">
          {event.event.stage}
        </span>
      );
      break;
    case "command_issued":
    case "command_result": {
      const ev = event.event as unknown as {
        interface: string;
        opcode: number | null;
        action: number | null;
        note: string | null;
      };
      label = `${kind}: ${ev.interface}`;
      detail = (
        <span className="font-mono text-[10px] text-slate-500">
          {ev.opcode != null && `op=0x${ev.opcode.toString(16).padStart(2, "0")} `}
          {ev.action != null && `act=0x${ev.action.toString(16).padStart(2, "0")} `}
          {ev.note}
        </span>
      );
      break;
    }
    case "verification":
      label = `verification: ${(event.event as any).all_passed ? "passed" : "failed"}`;
      break;
    case "failed":
      label = `failed: ${(event.event as { reason: string }).reason}`;
      break;
  }
  return (
    <div className="flex items-baseline justify-between border-b border-slate-900 px-3 py-1.5 text-xs last:border-0">
      <div className="flex items-baseline gap-2">
        <span className="font-mono text-[10px] text-slate-500">
          {new Date(event.at).toLocaleTimeString()}
        </span>
        <span>{label}</span>
      </div>
      <div>{detail}</div>
    </div>
  );
}
