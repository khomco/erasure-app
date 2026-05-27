import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, FileSignature, Loader2, UserCheck } from "lucide-react";

import { api, classNames } from "@/api/client";
import { useOperator } from "@/operator/context";
import type { DestructionManifest, Job, OperatorRef } from "@/api/types";

export function ManifestsPage() {
  const jobs = useQuery({
    queryKey: ["jobs"],
    queryFn: api.jobs,
    refetchInterval: 2000,
  });
  const manifests = useQuery({
    queryKey: ["manifests"],
    queryFn: api.manifests,
    refetchInterval: 2000,
  });

  const pendingJobs = useMemo(
    () =>
      (jobs.data ?? []).filter(
        (j) => j.state.state === "pending_co_sign" && !j.manifest_id
      ),
    [jobs.data]
  );

  return (
    <div className="space-y-5">
      <div>
        <h2 className="text-lg font-semibold">Destruction manifests</h2>
        <p className="text-xs text-slate-400">
          Batched supervisor co-sign for Jobs in <span className="font-mono">pending_co_sign</span>.
          Tier-1: local sync co-sign at the lead station.
        </p>
      </div>

      <PendingJobsCard jobs={pendingJobs} loading={jobs.isLoading} />

      <ManifestList manifests={manifests.data ?? []} loading={manifests.isLoading} />
    </div>
  );
}

function PendingJobsCard({ jobs, loading }: { jobs: Job[]; loading: boolean }) {
  const { operator } = useOperator();
  const qc = useQueryClient();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [note, setNote] = useState("");

  const create = useMutation({
    mutationFn: () => {
      if (!operator) throw new Error("operator not signed in");
      return api.createManifest({
        assembled_by: operator,
        job_ids: Array.from(selected),
        note: note || null,
      });
    },
    onSuccess: () => {
      setSelected(new Set());
      setNote("");
      qc.invalidateQueries({ queryKey: ["manifests"] });
      qc.invalidateQueries({ queryKey: ["jobs"] });
    },
  });

  const toggle = (id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  if (loading) {
    return (
      <div className="card flex items-center gap-2 text-slate-400">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading pending jobs…
      </div>
    );
  }
  if (jobs.length === 0) {
    return (
      <div className="card text-sm text-slate-400">
        No Jobs awaiting manifest assignment. (Escalate a Job to destroy to see one
        here.)
      </div>
    );
  }
  return (
    <div className="card">
      <h3 className="mb-2 text-sm font-semibold uppercase tracking-wide text-slate-400">
        Pending Jobs ({jobs.length})
      </h3>
      <div className="space-y-1">
        {jobs.map((j) => (
          <label
            key={j.id}
            className="flex items-center gap-3 rounded-md border border-slate-800 bg-slate-950/30 px-3 py-2 text-sm"
          >
            <input
              type="checkbox"
              checked={selected.has(j.id)}
              onChange={() => toggle(j.id)}
            />
            <div className="flex-1">
              <div className="font-mono text-xs">{j.id.slice(0, 8)}</div>
              <div className="text-xs text-slate-400">
                {j.spec.device_id} · {j.spec.asset_tag ?? "no asset tag"} ·{" "}
                {j.spec.classification}/{j.spec.intent}
              </div>
            </div>
            <FileSignature className="h-4 w-4 text-indigo-300" />
          </label>
        ))}
      </div>
      <div className="mt-3 flex items-end gap-2">
        <div className="flex-1">
          <div className="label">Note (shredder run id, vendor pickup, …)</div>
          <input
            className="field"
            value={note}
            onChange={(e) => setNote(e.target.value)}
            placeholder="optional"
          />
        </div>
        <button
          className="btn btn-primary"
          disabled={selected.size === 0 || create.isPending || !operator}
          onClick={() => create.mutate()}
        >
          {create.isPending ? "Creating…" : `Create manifest (${selected.size})`}
        </button>
      </div>
      {create.isError && (
        <div className="mt-2 rounded-md bg-rose-950/40 p-2 text-xs text-rose-300">
          {(create.error as Error).message}
        </div>
      )}
    </div>
  );
}

function ManifestList({
  manifests,
  loading,
}: {
  manifests: DestructionManifest[];
  loading: boolean;
}) {
  if (loading) {
    return (
      <div className="card flex items-center gap-2 text-slate-400">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading manifests…
      </div>
    );
  }
  if (manifests.length === 0) {
    return (
      <div className="card text-sm text-slate-400">No manifests yet.</div>
    );
  }
  const sorted = manifests
    .slice()
    .sort((a, b) => (a.created_at < b.created_at ? 1 : -1));
  return (
    <div className="space-y-3">
      <h3 className="text-sm font-semibold uppercase tracking-wide text-slate-400">
        Manifests ({manifests.length})
      </h3>
      {sorted.map((m) => (
        <ManifestCard key={m.id} manifest={m} />
      ))}
    </div>
  );
}

function ManifestCard({ manifest }: { manifest: DestructionManifest }) {
  const { operator } = useOperator();
  const qc = useQueryClient();
  const cosign = useMutation({
    mutationFn: (supervisor: OperatorRef) => api.cosignManifest(manifest.id, supervisor),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["manifests"] });
      qc.invalidateQueries({ queryKey: ["jobs"] });
    },
  });

  return (
    <div
      className={classNames(
        "card",
        manifest.state === "signed" && "border-emerald-700/40"
      )}
    >
      <div className="flex items-start justify-between">
        <div>
          <div className="flex items-center gap-2">
            {manifest.state === "signed" ? (
              <CheckCircle2 className="h-4 w-4 text-emerald-400" />
            ) : (
              <FileSignature className="h-4 w-4 text-indigo-300" />
            )}
            <span className="font-mono text-xs">{manifest.id.slice(0, 8)}</span>
            <span
              className={classNames(
                "pill",
                manifest.state === "signed"
                  ? "pill-success"
                  : manifest.state === "rejected"
                    ? "pill-danger"
                    : "pill-info"
              )}
            >
              {manifest.state}
            </span>
          </div>
          <div className="mt-1 text-xs text-slate-400">
            {manifest.job_ids.length} Job{manifest.job_ids.length === 1 ? "" : "s"} ·
            assembled by {manifest.assembled_by.display_name} ·{" "}
            {new Date(manifest.created_at).toLocaleString()}
          </div>
          {manifest.note && (
            <div className="mt-1 text-xs text-slate-500">{manifest.note}</div>
          )}
          {manifest.supervisor && (
            <div className="mt-1 flex items-center gap-1 text-xs text-emerald-300">
              <UserCheck className="h-3 w-3" />
              Co-signed by {manifest.supervisor.display_name}{" "}
              {manifest.signed_at &&
                `at ${new Date(manifest.signed_at).toLocaleString()}`}
            </div>
          )}
        </div>
        {manifest.state === "pending" && operator && (
          <button
            className="btn btn-primary"
            onClick={() => cosign.mutate(operator)}
            disabled={cosign.isPending}
            title="Co-sign as the active operator (Tier-1: real per-supervisor keys land with auth in v0.2 #5)"
          >
            <UserCheck className="h-4 w-4" />{" "}
            {cosign.isPending ? "Signing…" : "Supervisor co-sign"}
          </button>
        )}
      </div>
      {cosign.isError && (
        <div className="mt-2 rounded-md bg-rose-950/40 p-2 text-xs text-rose-300">
          {(cosign.error as Error).message}
        </div>
      )}
      <details className="mt-2">
        <summary className="cursor-pointer text-xs text-slate-400 hover:text-slate-200">
          Job IDs in manifest
        </summary>
        <ul className="mt-1 space-y-0.5 text-xs">
          {manifest.job_ids.map((jid) => (
            <li key={jid} className="font-mono text-slate-500">
              {jid}
            </li>
          ))}
        </ul>
      </details>
    </div>
  );
}
