import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, FileSignature, Loader2, UserCheck } from "lucide-react";

import { api, classNames } from "@/api/client";
import {
  canPreviewCompletion,
  canPreviewConcurrence,
  commonEquipmentId,
  completionPreviewKey,
  concurrencePreviewKey,
  readyToComplete,
  readyToConcur,
  runAnchorId,
  runDriveLabels,
  toLocalInput,
  concurrenceSignerProblem,
  runStatusLabel,
  toCompletionPreviewRequest,
  toCompletionRequest,
  toConcurrencePreviewRequest,
  toConcurrenceRequest,
  type CompletionForm,
  type ConcurrenceForm,
  type ReviewedCompletion,
  type ReviewedConcurrence,
} from "@/api/destructionRuns";
import { useOperator } from "@/operator/context";
import type { ConcurrencePolicy, DestructionManifest, Job } from "@/api/types";
import { assemblableJobs, offeredSelection } from "./manifestAssembly";

const POLICY_TEXT: Record<ConcurrencePolicy, string> = {
  none: "No concurrence: completing a run destroys its Jobs.",
  review: "Review required: after completion, a second person reviews the records and evidence.",
  witness: "Witness required: after completion, a second person who witnessed the destruction concurs.",
};

/**
 * Destruction runs (ADR-0011): assemble Jobs recorded for destruction into a
 * run, confirm its completion, and give the concurrence the station's policy
 * requires. Route `/manifests`; a run is a `DestructionManifest`.
 */
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
  const policy = useQuery({ queryKey: ["destruction-policy"], queryFn: api.destructionPolicy });

  const pendingJobs = useMemo(
    () => assemblableJobs(jobs.data ?? [], manifests.data ?? []),
    [jobs.data, manifests.data]
  );

  return (
    <div className="space-y-5">
      <div>
        <h2 className="text-lg font-semibold">Destruction runs</h2>
        <p className="text-xs text-slate-400">
          Group Jobs recorded for destruction into a run, then confirm the run's completion.
          Its Jobs stay pending destruction until then.
        </p>
        {policy.data && (
          <p className="mt-1 text-xs text-slate-300">
            Station policy: {POLICY_TEXT[policy.data.concurrence_policy]}
          </p>
        )}
      </div>

      <PendingJobsCard
        jobs={pendingJobs}
        loading={jobs.isLoading || manifests.isLoading}
      />

      <RunList
        manifests={manifests.data ?? []}
        jobs={jobs.data ?? []}
        loading={manifests.isLoading}
      />
    </div>
  );
}

function PendingJobsCard({
  jobs,
  loading,
}: {
  jobs: Job[];
  loading: boolean;
}) {
  const { operator } = useOperator();
  const qc = useQueryClient();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [note, setNote] = useState("");
  const submitIds = useMemo(() => offeredSelection(selected, jobs), [selected, jobs]);

  const create = useMutation({
    mutationFn: () => {
      if (!operator) throw new Error("operator not signed in");
      return api.createManifest({
        assembled_by: operator,
        job_ids: submitIds,
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
        No Jobs waiting for a destruction run. (Record a Job for destruction to see one
        here.)
      </div>
    );
  }
  return (
    <div className="card">
      <h3 className="mb-2 text-sm font-semibold uppercase tracking-wide text-slate-400">
        Pending destruction ({jobs.length})
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
          disabled={submitIds.length === 0 || create.isPending || !operator}
          onClick={() => create.mutate()}
        >
          {create.isPending ? "Creating…" : `Create run (${submitIds.length})`}
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

function RunList({
  manifests,
  jobs,
  loading,
}: {
  manifests: DestructionManifest[];
  jobs: Job[];
  loading: boolean;
}) {
  if (loading) {
    return (
      <div className="card flex items-center gap-2 text-slate-400">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading destruction runs…
      </div>
    );
  }
  if (manifests.length === 0) {
    return <div className="card text-sm text-slate-400">No destruction runs yet.</div>;
  }
  const sorted = manifests
    .slice()
    .sort((a, b) => (a.created_at < b.created_at ? 1 : -1));
  return (
    <div className="space-y-3">
      <h3 className="text-sm font-semibold uppercase tracking-wide text-slate-400">
        Destruction runs ({manifests.length})
      </h3>
      {sorted.map((m) => (
        <RunCard key={m.id} run={m} jobs={jobs.filter((j) => m.job_ids.includes(j.id))} />
      ))}
    </div>
  );
}

function RunCard({ run, jobs }: { run: DestructionManifest; jobs: Job[] }) {
  const done = run.state === "completed";
  return (
    <div id={runAnchorId(run.id)} className={classNames("card", done && "border-emerald-700/40")}>
      <div className="flex items-center gap-2">
        {done ? (
          <CheckCircle2 className="h-4 w-4 text-emerald-400" />
        ) : (
          <FileSignature className="h-4 w-4 text-indigo-300" />
        )}
        <span className="font-mono text-xs">{run.id.slice(0, 8)}</span>
        <span
          className={classNames(
            "pill",
            done ? "pill-success" : run.state === "rejected" ? "pill-danger" : "pill-info"
          )}
        >
          {runStatusLabel(run)}
        </span>
      </div>
      <div className="mt-1 text-xs text-slate-400">
        {run.job_ids.length} Job{run.job_ids.length === 1 ? "" : "s"} · assembled by{" "}
        {run.assembled_by.display_name} · {new Date(run.created_at).toLocaleString()}
        {run.required_concurrence && ` · needs a ${run.required_concurrence}`}
      </div>
      {run.note && <div className="mt-1 text-xs text-slate-500">{run.note}</div>}

      {run.completion && (
        <RecordedStatement
          title={`Completion — ${run.completion.signer.full_name}, ${run.completion.signer.title}`}
          statement={run.completion.statement}
        />
      )}
      {run.concurrence && (
        <RecordedStatement
          title={`${run.concurrence.kind === "witness" ? "Witness" : "Review"} — ${run.concurrence.signer.full_name}, ${run.concurrence.signer.title}`}
          statement={run.concurrence.statement}
        />
      )}

      {run.state === "pending" && <CompletionCard run={run} jobs={jobs} />}
      {run.state === "awaiting_concurrence" && <ConcurrenceCard run={run} jobs={jobs} />}

      <details className="mt-2">
        <summary className="cursor-pointer text-xs text-slate-400 hover:text-slate-200">
          Job IDs in this run
        </summary>
        <ul className="mt-1 space-y-0.5 text-xs">
          {run.job_ids.map((jid) => (
            <li key={jid} className="font-mono text-slate-500">
              {jid}
            </li>
          ))}
        </ul>
      </details>
    </div>
  );
}

function RecordedStatement({ title, statement }: { title: string; statement: string }) {
  return (
    <div className="mt-2">
      <div className="flex items-center gap-1 text-xs text-emerald-300">
        <UserCheck className="h-3 w-3" /> {title}
      </div>
      <blockquote className="mt-1 rounded-md border border-slate-800 bg-slate-950/40 p-2 text-xs text-slate-300">
        {statement}
      </blockquote>
    </div>
  );
}

/** The drives a statement's "device(s) listed above" refers to. */
function DriveList({ run, jobs }: { run: DestructionManifest; jobs: Job[] }) {
  return (
    <div>
      <div className="label">Devices in this run ({run.job_ids.length})</div>
      <ul className="list-disc pl-5 text-xs text-slate-300">
        {runDriveLabels(run.job_ids, jobs).map((label, i) => (
          <li key={run.job_ids[i]} className="font-mono">
            {label}
          </li>
        ))}
      </ul>
    </div>
  );
}

function ErrorNote({ error }: { error: unknown }) {
  return (
    <div className="rounded-md bg-rose-950/40 p-2 text-xs text-rose-300">
      {(error as Error).message}
    </div>
  );
}

function TextInput({
  label,
  value,
  placeholder,
  onChange,
}: {
  label: string;
  value: string;
  placeholder?: string;
  onChange: (v: string) => void;
}) {
  return (
    <label className="block">
      <div className="label">{label}</div>
      <input
        className="field"
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
      />
    </label>
  );
}

/** A statement the server rendered, shown verbatim, with the "I attest" box. */
function AttestBox({
  statement,
  attested,
  onAttested,
}: {
  statement: string;
  attested: boolean;
  onAttested: (v: boolean) => void;
}) {
  return (
    <>
      <blockquote className="rounded-md border border-slate-700 bg-slate-950/60 p-3 text-xs text-slate-200">
        {statement}
      </blockquote>
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" checked={attested} onChange={(e) => onAttested(e.target.checked)} />
        I attest
      </label>
    </>
  );
}

function CompletionCard({ run, jobs }: { run: DestructionManifest; jobs: Job[] }) {
  const { operator } = useOperator();
  const qc = useQueryClient();
  const [form, setForm] = useState<CompletionForm>(() => ({
    completed_at_local: toLocalInput(new Date()),
    remnants_inspected: false,
    equipment_cycle_completed: false,
    equipment_id: commonEquipmentId(jobs) ?? "",
    criterion: "",
    video_ref: "",
    full_name: operator?.display_name ?? "",
    title: "",
    attested: false,
  }));
  const [reviewed, setReviewed] = useState<ReviewedCompletion | null>(null);
  const update = (patch: Partial<CompletionForm>) => setForm((f) => ({ ...f, ...patch }));
  // Prefill the equipment once the member Jobs have loaded, if still blank.
  const recordedEquipment = commonEquipmentId(jobs);
  useEffect(() => {
    if (recordedEquipment) {
      setForm((f) => (f.equipment_id === "" ? { ...f, equipment_id: recordedEquipment } : f));
    }
  }, [recordedEquipment]);

  const preview = useMutation({
    // The form sent is the mutation variable, so the stored key is the one
    // the statement was rendered from even if the operator edits mid-flight.
    mutationFn: (f: CompletionForm) => api.previewCompletion(run.id, toCompletionPreviewRequest(f)),
    onSuccess: (r, f) => {
      update({ attested: false });
      setReviewed({ key: completionPreviewKey(f), statement: r.statement, completed_at: r.completed_at });
    },
  });
  const complete = useMutation({
    mutationFn: () => {
      if (!operator) throw new Error("operator not signed in");
      if (!reviewed) throw new Error("review the statement first");
      return api.completeRun(run.id, toCompletionRequest(form, operator, reviewed));
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["manifests"] });
      qc.invalidateQueries({ queryKey: ["jobs"] });
    },
  });
  const current = reviewed && reviewed.key === completionPreviewKey(form) ? reviewed : null;

  return (
    <div className="mt-3 space-y-3 rounded-md border border-slate-800 p-3">
      <h4 className="text-sm font-semibold">Confirm destruction completed</h4>
      <div className="space-y-1 text-sm">
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={form.remnants_inspected}
            onChange={(e) => update({ remnants_inspected: e.target.checked })}
          />
          Remnants inspected
        </label>
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={form.equipment_cycle_completed}
            onChange={(e) => update({ equipment_cycle_completed: e.target.checked })}
          />
          Equipment cycle completed
        </label>
      </div>
      <div className="grid grid-cols-2 gap-2">
        <label className="block">
          <div className="label">Destruction completed at (local time)</div>
          <input
            className="field"
            type="datetime-local"
            step={1}
            value={form.completed_at_local}
            onChange={(e) => update({ completed_at_local: e.target.value })}
          />
        </label>
        <div />
        <TextInput label="Equipment ID" value={form.equipment_id} placeholder="e.g. SHR-2" onChange={(v) => update({ equipment_id: v })} />
        <TextInput label="Criterion met" value={form.criterion} placeholder="e.g. particles ≤ 2 mm" onChange={(v) => update({ criterion: v })} />
        <TextInput label="Video reference" value={form.video_ref} placeholder="optional — e.g. CAM-3 14:02" onChange={(v) => update({ video_ref: v })} />
        <div />
        <TextInput label="Full name" value={form.full_name} placeholder="as you sign it" onChange={(v) => update({ full_name: v })} />
        <TextInput label="Title" value={form.title} placeholder="e.g. Bench lead" onChange={(v) => update({ title: v })} />
      </div>
      <DriveList run={run} jobs={jobs} />
      <button
        type="button"
        className="btn btn-secondary"
        onClick={() => preview.mutate(form)}
        disabled={!canPreviewCompletion(form) || preview.isPending}
      >
        {preview.isPending ? "Loading statement…" : "Review statement"}
      </button>
      {preview.isError && <ErrorNote error={preview.error} />}
      {current && (
        <AttestBox statement={current.statement} attested={form.attested} onAttested={(v) => update({ attested: v })} />
      )}
      <button
        className="btn btn-primary"
        disabled={!operator || !readyToComplete(form, current) || complete.isPending}
        onClick={() => complete.mutate()}
      >
        {complete.isPending ? "Recording…" : "Confirm completion"}
      </button>
      {complete.isError && <ErrorNote error={complete.error} />}
    </div>
  );
}

function ConcurrenceCard({ run, jobs }: { run: DestructionManifest; jobs: Job[] }) {
  const { operator } = useOperator();
  const qc = useQueryClient();
  const [form, setForm] = useState<ConcurrenceForm>({ full_name: "", title: "", attested: false });
  const [reviewed, setReviewed] = useState<ReviewedConcurrence | null>(null);
  const update = (patch: Partial<ConcurrenceForm>) => setForm((f) => ({ ...f, ...patch }));
  const witness = run.required_concurrence === "witness";

  const preview = useMutation({
    mutationFn: (f: ConcurrenceForm) => api.previewConcurrence(run.id, toConcurrencePreviewRequest(f)),
    onSuccess: (r, f) => {
      update({ attested: false });
      setReviewed({ key: concurrencePreviewKey(f), statement: r.statement });
    },
  });
  const concur = useMutation({
    mutationFn: () => {
      if (!operator) throw new Error("operator not signed in");
      return api.concurRun(run.id, toConcurrenceRequest(form, operator));
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["manifests"] });
      qc.invalidateQueries({ queryKey: ["jobs"] });
    },
  });
  const current = reviewed && reviewed.key === concurrencePreviewKey(form) ? reviewed : null;
  const completionSigner = run.completion?.signer ?? null;
  const signerProblem = concurrenceSignerProblem(form, operator, completionSigner);

  return (
    <div className="mt-3 space-y-3 rounded-md border border-slate-800 p-3">
      <h4 className="text-sm font-semibold">{witness ? "Witness concurrence" : "Review concurrence"}</h4>
      <p className="text-xs text-slate-400">
        {witness
          ? "Given by someone who witnessed the destruction."
          : "Given by someone who reviewed the destruction records and verification evidence."}{" "}
        It must be someone other than {run.completion?.signer.full_name ?? "the person who confirmed completion"}:
        sign in as the concurring operator first.
      </p>
      <div className="grid grid-cols-2 gap-2">
        <TextInput label="Full name" value={form.full_name} placeholder="as you sign it" onChange={(v) => update({ full_name: v })} />
        <TextInput label="Title" value={form.title} placeholder="e.g. Quality lead" onChange={(v) => update({ title: v })} />
      </div>
      <DriveList run={run} jobs={jobs} />
      <button
        type="button"
        className="btn btn-secondary"
        onClick={() => preview.mutate(form)}
        disabled={!canPreviewConcurrence(form) || preview.isPending}
      >
        {preview.isPending ? "Loading statement…" : "Review statement"}
      </button>
      {preview.isError && <ErrorNote error={preview.error} />}
      {current && (
        <AttestBox statement={current.statement} attested={form.attested} onAttested={(v) => update({ attested: v })} />
      )}
      <button
        className="btn btn-primary"
        disabled={
          !operator || !readyToConcur(form, current, operator, completionSigner) || concur.isPending
        }
        onClick={() => concur.mutate()}
      >
        {concur.isPending ? "Recording…" : "Concur"}
      </button>
      {signerProblem && <div className="text-xs text-amber-300">{signerProblem}</div>}
      {concur.isError && <ErrorNote error={concur.error} />}
    </div>
  );
}
