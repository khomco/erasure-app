import { useState } from "react";
import { Link, useParams } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ActivitySquare,
  AlertTriangle,
  ArrowLeft,
  CheckCircle2,
  ShieldCheck,
  Trash2,
  XCircle,
} from "lucide-react";

import { api, classNames, formatBytes } from "@/api/client";
import { latestErasure, useJobLiveState } from "@/api/ws";
import { identityAssuranceLabel, techniqueName, techniqueStandardLabel } from "@/api/destruction";
import { toRecordRequest, type RecordForm } from "@/api/recordForDestruction";
import {
  AttestationInputs,
  readyToRecord,
  StatementReview,
  type ReviewedStatement,
} from "@/components/AttestationFields";
import { useOperator } from "@/operator/context";
import type {
  Classification,
  ErasureEvent,
  Job,
  JobActivity,
  JobUpdate,
} from "@/api/types";

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
  return <JobBody job={live} onAbort={() => abort.mutate()} aborting={abort.isPending} />;
}

function JobBody({
  job,
  onAbort,
  aborting,
}: {
  job: Job;
  onAbort: () => void;
  aborting: boolean;
}) {
  const erasure = latestErasure(job);
  const device = erasure?.device_snapshot;
  const devices = useQuery({ queryKey: ["devices"], queryFn: api.devices });
  // The serial a device-sourced record would store, as far as the UI knows:
  // the latest erasure snapshot's, else the attached device's.
  const knownSerial =
    device?.serial?.trim() ||
    devices.data?.find((d) => d.id === job.spec.device_id)?.serial?.trim() ||
    null;
  const pct = Math.round((erasure?.progress?.fraction ?? 0) * 100);
  const outerTerminal =
    job.state.state === "erased" ||
    job.state.state === "destroyed" ||
    job.state.state === "quarantined" ||
    job.state.state === "aborted";
  const showCert =
    job.state.state === "erased" ||
    job.state.state === "destroyed" ||
    job.state.state === "pending_destruction";

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
              {device ? `${device.vendor} ${device.model}` : job.spec.device_id}
            </h2>
            <p className="mt-0.5 text-xs text-slate-400">
              {device
                ? `${device.serial} · ${formatBytes(device.capacity_bytes)} · ${device.path}`
                : "no device snapshot yet (queued)"}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <span className="pill">{job.spec.classification}</span>
            <span className="pill">{job.spec.intent}</span>
            <OuterStatePill state={job.state.state} />
          </div>
        </div>

        {erasure?.progress && !outerTerminal && (
          <div className="mt-4">
            <div className="h-3 overflow-hidden rounded-full bg-slate-800">
              <div
                className={classNames(
                  "h-full rounded-full transition-all",
                  erasure.state.state === "failed"
                    ? "bg-rose-500"
                    : erasure.state.state === "completed"
                      ? "bg-emerald-500"
                      : "bg-indigo-500"
                )}
                style={{ width: `${pct}%` }}
              />
            </div>
            <div className="mt-1 flex items-center justify-between text-xs text-slate-400">
              <span>{erasure.progress.stage}</span>
              <span>
                {pct}% ·{" "}
                {erasure.progress.eta_seconds != null
                  ? `${erasure.progress.eta_seconds}s remaining`
                  : ""}
              </span>
            </div>
          </div>
        )}

        <div className="mt-4 flex items-center justify-end gap-2">
          {!outerTerminal && job.state.state !== "pending_destruction" && (
            <button
              className="btn btn-danger"
              onClick={onAbort}
              disabled={aborting}
            >
              {aborting ? "Aborting…" : "Abort"}
            </button>
          )}
          {(job.state.state === "queued" || job.state.state === "in_progress") && (
            <RecordForDestructionButton
              jobId={job.id}
              classification={job.spec.classification}
              knownSerial={knownSerial}
              assetTag={job.spec.asset_tag}
            />
          )}
          {showCert && (
            <Link
              to="/certs/$jobId"
              params={{ jobId: job.id }}
              className="btn btn-primary"
            >
              <ShieldCheck className="h-4 w-4" /> View certificate
            </Link>
          )}
          {job.state.state === "pending_destruction" && (
            <Link to="/manifests" className="btn btn-secondary">
              <ActivitySquare className="h-4 w-4" /> Manifest co-sign
            </Link>
          )}
        </div>
      </div>

      <ActivityTimeline activities={job.activities} />
    </div>
  );
}

function OuterStatePill({ state }: { state: Job["state"]["state"] }) {
  const tone =
    state === "erased"
      ? "pill-success"
      : state === "destroyed"
        ? "pill-warning"
        : state === "quarantined"
          ? "pill-danger"
          : state === "aborted"
            ? ""
            : "pill-info";
  return <span className={classNames("pill", tone)}>{state.replace(/_/g, " ")}</span>;
}

function RecordForDestructionButton({
  jobId,
  classification,
  knownSerial,
  assetTag,
}: {
  jobId: string;
  classification: Classification;
  knownSerial: string | null;
  assetTag: string | null;
}) {
  const { operator } = useOperator();
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [form, setForm] = useState<RecordForm>(() => ({
    method: "shred",
    equipment_id: "",
    location: "",
    full_name: operator?.display_name ?? "",
    title: "",
    notes: "",
    identifiers: { from: "device" },
  }));
  const [reviewed, setReviewed] = useState<ReviewedStatement | null>(null);
  const [attested, setAttested] = useState(false);

  const record = useMutation({
    mutationFn: () => {
      if (!operator) throw new Error("operator not signed in");
      return api.recordForDestruction(jobId, toRecordRequest(form, operator, attested));
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["job", jobId] });
      setOpen(false);
      setAttested(false);
    },
  });

  const ids = form.identifiers;
  return (
    <>
      <button className="btn btn-warning" onClick={() => setOpen(true)}>
        <Trash2 className="h-4 w-4" /> Record for destruction
      </button>
      {open && (
        <div className="fixed inset-0 z-30 flex items-center justify-center bg-slate-950/70 p-4">
          <div className="max-h-full w-full max-w-lg space-y-4 overflow-auto rounded-xl border border-slate-700 bg-slate-900 p-5 shadow-2xl">
            <div>
              <h3 className="text-base font-semibold">Record for destruction</h3>
              <p className="mt-1 text-xs text-slate-400">
                You attest that this drive is being placed into the destruction process
                now. The Job becomes <strong>pending destruction</strong>; it is not
                destroyed until the destruction run is completed on a manifest.
              </p>
            </div>
            <AttestationInputs form={form} onChange={setForm} classification={classification} />
            <div className="space-y-2">
              <div className="label">Identifiers</div>
              <div className="flex gap-4 text-sm">
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    checked={ids.from === "device"}
                    onChange={() => setForm({ ...form, identifiers: { from: "device" } })}
                  />
                  From the device
                </label>
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    checked={ids.from === "scanned"}
                    onChange={() =>
                      setForm({
                        ...form,
                        identifiers: { from: "scanned", asset_tag: "", serial: "" },
                      })
                    }
                  />
                  Scanned
                </label>
              </div>
              {ids.from === "device" ? (
                knownSerial ? (
                  <div className="rounded-md border border-slate-800 p-2 text-xs text-slate-300">
                    <div className="mb-1 text-slate-500">Will be recorded (read from the device):</div>
                    <div>
                      Serial: <span className="font-mono">{knownSerial}</span>
                    </div>
                    <div>
                      Asset tag: <span className="font-mono">{assetTag?.trim() || "—"}</span>
                    </div>
                  </div>
                ) : (
                  <p className="rounded-md bg-amber-950/40 p-2 text-xs text-amber-300">
                    This drive cannot be read, so its serial is unknown. Choose Scanned and
                    scan the drive&apos;s asset tag or serial.
                  </p>
                )
              ) : (
                <div className="grid grid-cols-2 gap-2">
                  <input
                    className="field"
                    placeholder="Asset tag"
                    value={ids.asset_tag}
                    onChange={(e) =>
                      setForm({ ...form, identifiers: { ...ids, asset_tag: e.target.value } })
                    }
                  />
                  <input
                    className="field"
                    placeholder="Serial"
                    value={ids.serial}
                    onChange={(e) =>
                      setForm({ ...form, identifiers: { ...ids, serial: e.target.value } })
                    }
                  />
                </div>
              )}
            </div>
            <StatementReview
              form={form}
              reviewed={reviewed}
              onReviewed={setReviewed}
              attested={attested}
              onAttested={setAttested}
            />
            {record.isError && (
              <div className="rounded-md bg-rose-950/40 p-2 text-xs text-rose-300">
                {(record.error as Error).message}
              </div>
            )}
            <div className="flex justify-end gap-2 pt-2">
              <button className="btn btn-ghost" onClick={() => setOpen(false)}>
                Cancel
              </button>
              <button
                className="btn btn-warning"
                onClick={() => record.mutate()}
                disabled={
                  record.isPending || !operator || !readyToRecord(form, reviewed, attested)
                }
              >
                {record.isPending ? "Recording…" : "Record for destruction"}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

function ActivityTimeline({ activities }: { activities: JobActivity[] }) {
  if (activities.length === 0) {
    return (
      <div className="card text-sm text-slate-500">
        No activities yet — Job is queued.
      </div>
    );
  }
  return (
    <div className="space-y-3">
      <h3 className="text-sm font-semibold uppercase tracking-wide text-slate-400">
        Activity chain ({activities.length})
      </h3>
      <div className="space-y-3">
        {activities.map((a, i) => (
          <ActivityCard key={activityKey(a)} activity={a} index={i} />
        ))}
      </div>
    </div>
  );
}

function activityKey(a: JobActivity): string {
  // All variants carry an `id`.
  return (a as { id: string }).id;
}

function ActivityCard({ activity, index }: { activity: JobActivity; index: number }) {
  switch (activity.type) {
    case "erasure":
      return <ErasureCard erasure={activity as unknown as ErasureEvent} index={index} />;
    case "verification":
      return (
        <div className="card">
          <ActivityHeader
            icon={<CheckCircle2 className="h-4 w-4 text-emerald-400" />}
            title={`Verification #${index + 1}`}
            at={activity.at}
          />
          <div className="mt-2 text-sm">
            Result:{" "}
            <span
              className={
                activity.report.all_passed ? "text-emerald-300" : "text-rose-300"
              }
            >
              {activity.report.all_passed ? "all passed" : "failures present"}
            </span>
            <div className="text-xs text-slate-400">
              {activity.report.sample_count} samples,{" "}
              {formatBytes(activity.report.bytes_sampled)} sampled · against
              erasure{" "}
              <span className="font-mono text-[10px]">
                {activity.erasure_event_id.slice(0, 8)}
              </span>
            </div>
          </div>
        </div>
      );
    case "destruction":
      return (
        <div className="card border-amber-700/40">
          <ActivityHeader
            icon={<Trash2 className="h-4 w-4 text-amber-400" />}
            title={`Destruction #${index + 1}`}
            at={activity.at}
          />
          <div className="mt-2 space-y-1 text-sm">
            <div>
              Technique: {techniqueName(activity.method)}{" "}
              <span className="text-xs text-slate-400">
                ({techniqueStandardLabel(activity.method)})
              </span>
            </div>
            <div className="text-xs text-slate-400">
              Operator: {activity.operator.display_name}
            </div>
            {activity.attestation && (
              <>
                <div className="text-xs text-slate-400">
                  Recorded for destruction by {activity.attestation.signer.full_name},{" "}
                  {activity.attestation.signer.title} · equipment{" "}
                  {activity.attestation.equipment_id} · {activity.attestation.location}
                </div>
                <div className="text-xs text-slate-400">
                  Identifiers ({activity.attestation.identifiers.source}): asset tag{" "}
                  {activity.attestation.identifiers.asset_tag ?? "—"} · serial{" "}
                  {activity.attestation.identifiers.serial ?? "—"}
                </div>
                <div className="text-xs text-slate-400">
                  Identity:{" "}
                  {identityAssuranceLabel(activity.attestation.signer.identity_assurance)}
                </div>
                <blockquote className="rounded-md border border-slate-800 p-2 text-xs text-slate-300">
                  {activity.attestation.statement}
                </blockquote>
              </>
            )}
            {activity.notes && (
              <div className="text-xs text-slate-400">Notes: {activity.notes}</div>
            )}
            {activity.manifest_ref && (
              <div className="text-xs text-slate-500">
                Manifest:{" "}
                <span className="font-mono">{activity.manifest_ref.slice(0, 8)}</span>
              </div>
            )}
          </div>
        </div>
      );
    case "diagnostic":
      return (
        <div className="card">
          <ActivityHeader
            icon={<AlertTriangle className="h-4 w-4 text-yellow-400" />}
            title={`Diagnostic #${index + 1}`}
            at={activity.at}
          />
          <ul className="mt-2 space-y-1 text-xs text-slate-400">
            {activity.findings.map((f, i) => (
              <li key={i}>
                <span className="font-mono">{f.code}</span> ({f.severity}) — {f.message}
              </li>
            ))}
          </ul>
        </div>
      );
    case "health_check":
      return (
        <div className="card">
          <ActivityHeader
            icon={<ActivitySquare className="h-4 w-4 text-sky-400" />}
            title={`HealthCheck #${index + 1}`}
            at={activity.at}
          />
        </div>
      );
  }
}

function ActivityHeader({
  icon,
  title,
  at,
}: {
  icon: React.ReactNode;
  title: string;
  at: string;
}) {
  return (
    <div className="flex items-center justify-between">
      <div className="flex items-center gap-2">
        {icon}
        <span className="font-medium">{title}</span>
      </div>
      <span className="font-mono text-[10px] text-slate-500">
        {new Date(at).toLocaleString()}
      </span>
    </div>
  );
}

function ErasureCard({ erasure, index }: { erasure: ErasureEvent; index: number }) {
  const pct = Math.round((erasure.progress?.fraction ?? 0) * 100);
  return (
    <div className="card">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          {erasure.state.state === "completed" ? (
            <CheckCircle2 className="h-4 w-4 text-emerald-400" />
          ) : erasure.state.state === "failed" ? (
            <XCircle className="h-4 w-4 text-rose-400" />
          ) : (
            <ActivitySquare className="h-4 w-4 text-indigo-400" />
          )}
          <span className="font-medium">Erasure #{index + 1}</span>
          <span className="pill">{erasure.state.state}</span>
        </div>
        <span className="font-mono text-[10px] text-slate-500">
          {erasure.created_at && new Date(erasure.created_at).toLocaleString()}
        </span>
      </div>
      <div className="mt-2 grid grid-cols-1 gap-3 lg:grid-cols-2">
        <div>
          <div className="label">Resolved method</div>
          {erasure.resolved_method ? (
            <div className="font-mono text-sm">{erasure.resolved_method.kind}</div>
          ) : (
            <span className="text-slate-500">pending</span>
          )}
        </div>
        {erasure.progress && (
          <div>
            <div className="label">Progress</div>
            <div className="h-2 overflow-hidden rounded-full bg-slate-800">
              <div
                className={classNames(
                  "h-full rounded-full transition-all",
                  erasure.state.state === "failed" ? "bg-rose-500" : "bg-indigo-500"
                )}
                style={{ width: `${pct}%` }}
              />
            </div>
            <div className="mt-0.5 text-[10px] text-slate-500">
              {pct}% · {erasure.progress.stage}
            </div>
          </div>
        )}
      </div>
      {erasure.events.length > 0 && (
        <details className="mt-3">
          <summary className="cursor-pointer text-xs text-slate-400 hover:text-slate-200">
            Event log ({erasure.events.length})
          </summary>
          <div className="mt-2 max-h-56 overflow-auto rounded-md border border-slate-800 bg-slate-950/60">
            {erasure.events.slice().reverse().map((e, i) => (
              <UpdateRow event={e} key={i} />
            ))}
          </div>
        </details>
      )}
    </div>
  );
}

function UpdateRow({ event }: { event: JobUpdate }) {
  const kind = event.event.kind;
  let label: string = kind;
  let detail: React.ReactNode = null;
  switch (kind) {
    case "state_changed":
      label = `state → ${event.event.to.state}`;
      break;
    case "progress":
      label = `progress ${Math.round(event.event.fraction * 100)}%`;
      detail = <span className="text-slate-500">{event.event.stage}</span>;
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
