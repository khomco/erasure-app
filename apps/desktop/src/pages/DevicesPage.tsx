import { useMemo, useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  CheckCircle2,
  ChevronRight,
  CircleOff,
  Cpu,
  Database,
  FileSignature,
  HardDrive,
  Loader2,
  Trash2,
  Unplug,
  UserCircle2,
  X,
} from "lucide-react";

import { api, classNames, formatBytes } from "@/api/client";
import { useOperator } from "@/operator/context";
import { latestErasure, useEventStream } from "@/api/ws";
import type {
  Classification,
  Device,
  ErasureEvent,
  Intent,
  Job,
  JobStateLabel,
} from "@/api/types";

function deviceIcon(d: Device) {
  switch (d.media_type) {
    case "ssd_nvme":
    case "ssd_sata":
      return <Cpu className="h-5 w-5 text-emerald-400" />;
    case "hdd_magnetic":
      return <Database className="h-5 w-5 text-amber-400" />;
    default:
      return <HardDrive className="h-5 w-5 text-slate-400" />;
  }
}

/**
 * Per-device card state. The page joins `/api/devices` against
 * `/api/jobs` by `device_id` and derives a single status per slot —
 * the at-a-glance signal that lets an operator walk the bench and
 * know what's done, what's still running, and what needs attention
 * without clicking through.
 */
type SlotStatus =
  | { kind: "idle" }
  | { kind: "wiping"; job: Job; erasure: ErasureEvent }
  | { kind: "erased"; job: Job }
  | { kind: "failed"; job: Job; erasure: ErasureEvent }
  | { kind: "pending_co_sign"; job: Job }
  | { kind: "destroyed"; job: Job }
  | { kind: "quarantined"; job: Job }
  | { kind: "aborted"; job: Job };

function deriveSlotStatus(job: Job | undefined): SlotStatus {
  if (!job) return { kind: "idle" };
  const state: JobStateLabel = job.state.state;
  const erasure = latestErasure(job);
  switch (state) {
    case "queued":
    case "in_progress":
      if (erasure && erasure.state.state === "failed") {
        return { kind: "failed", job, erasure };
      }
      if (erasure) {
        return { kind: "wiping", job, erasure };
      }
      // Job is queued/in_progress with no erasure yet — treat as wiping
      // with no progress data so the operator sees a busy indicator.
      return {
        kind: "wiping",
        job,
        erasure: {
          // synthetic stub so renderers have a shape; never persisted
          id: "",
          device_snapshot: {} as Device,
          capabilities_snapshot: {} as never,
          spec: {} as never,
          resolved_method: null,
          state: { state: "queued" },
          progress: null,
          events: [],
          created_at: job.created_at,
          started_at: null,
          ended_at: null,
          station_id: null,
        },
      };
    case "erased":
      return { kind: "erased", job };
    case "pending_co_sign":
      return { kind: "pending_co_sign", job };
    case "destroyed":
      return { kind: "destroyed", job };
    case "quarantined":
      return { kind: "quarantined", job };
    case "aborted":
      return { kind: "aborted", job };
  }
}

export function DevicesPage() {
  const devices = useQuery({
    queryKey: ["devices"],
    queryFn: api.devices,
    refetchInterval: 4000,
  });
  const jobs = useQuery({
    queryKey: ["jobs"],
    queryFn: api.jobs,
    refetchInterval: 1500,
  });
  const qc = useQueryClient();
  // Live broadcast → re-fetch jobs immediately rather than waiting for poll.
  useEventStream((env) => {
    if (env.type === "job_broadcast") {
      qc.invalidateQueries({ queryKey: ["jobs"] });
    }
  });

  const [selected, setSelected] = useState<Device | null>(null);

  const deviceToLatestJob = useMemo(() => {
    const map = new Map<string, Job>();
    for (const job of jobs.data ?? []) {
      const did = job.spec.device_id;
      const existing = map.get(did);
      if (!existing || job.created_at > existing.created_at) {
        map.set(did, job);
      }
    }
    return map;
  }, [jobs.data]);

  if (devices.isLoading) {
    return <div className="text-slate-400">Probing attached storage…</div>;
  }
  if (devices.isError) {
    return (
      <div className="card border-rose-700/60">
        <div className="flex items-center gap-2 text-rose-300">
          <AlertTriangle className="h-4 w-4" />
          Could not enumerate devices.
        </div>
        <pre className="mt-2 text-xs text-rose-200/70">
          {(devices.error as Error).message}
        </pre>
      </div>
    );
  }
  const list = devices.data ?? [];
  return (
    <div className="space-y-4">
      <div className="flex items-baseline justify-between">
        <h2 className="text-lg font-semibold">Bench</h2>
        <span className="text-xs text-slate-500">
          {list.length} device{list.length === 1 ? "" : "s"} attached
        </span>
      </div>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {list.map((d) => (
          <DeviceCard
            key={d.id}
            device={d}
            status={deriveSlotStatus(deviceToLatestJob.get(d.id))}
            onStart={() => setSelected(d)}
          />
        ))}
      </div>
      {selected && (
        <EraseWizard device={selected} onClose={() => setSelected(null)} />
      )}
    </div>
  );
}

function DeviceCard({
  device,
  status,
  onStart,
}: {
  device: Device;
  status: SlotStatus;
  onStart: () => void;
}) {
  const tone = toneFor(status);
  return (
    <div
      className={classNames(
        "card relative transition",
        tone.border,
        tone.bg
      )}
    >
      <div className="flex items-start justify-between">
        <div className="flex items-start gap-3">
          {deviceIcon(device)}
          <div>
            <div className="font-medium">{device.model}</div>
            <div className="text-xs text-slate-400">
              {device.vendor} · {device.serial}
            </div>
          </div>
        </div>
        <StatusBadge status={status} />
      </div>
      <div className="mt-3 flex items-center gap-2">
        <span className="pill">{device.media_type}</span>
        <span className="pill">{device.bus}</span>
        <span className="pill">{formatBytes(device.capacity_bytes)}</span>
      </div>
      <div className="mt-2 font-mono text-[11px] text-slate-500">{device.path}</div>

      <StatusBody status={status} />

      <div className="mt-3 flex items-center justify-end gap-2">
        <Actions status={status} device={device} onStart={onStart} />
      </div>
    </div>
  );
}

interface Tone {
  border: string;
  bg: string;
  pill: string;
}

function toneFor(status: SlotStatus): Tone {
  switch (status.kind) {
    case "idle":
      return { border: "", bg: "", pill: "" };
    case "wiping":
      return {
        border: "border-indigo-500/60",
        bg: "bg-indigo-500/5",
        pill: "pill-info",
      };
    case "erased":
      return {
        border: "border-emerald-500/60",
        bg: "bg-emerald-500/5",
        pill: "pill-success",
      };
    case "failed":
      return {
        border: "border-amber-500/60",
        bg: "bg-amber-500/5",
        pill: "pill-warning",
      };
    case "pending_co_sign":
      return {
        border: "border-indigo-400/60",
        bg: "bg-indigo-400/5",
        pill: "pill-info",
      };
    case "destroyed":
      return {
        border: "border-orange-500/60",
        bg: "bg-orange-500/5",
        pill: "pill-warning",
      };
    case "quarantined":
      return {
        border: "border-rose-500/60",
        bg: "bg-rose-500/5",
        pill: "pill-danger",
      };
    case "aborted":
      return { border: "border-slate-700", bg: "", pill: "" };
  }
}

function StatusBadge({ status }: { status: SlotStatus }) {
  const tone = toneFor(status);
  const { icon, label } = badgeContent(status);
  return (
    <span className={classNames("pill", tone.pill)}>
      {icon}
      {label}
    </span>
  );
}

function badgeContent(status: SlotStatus): { icon: React.ReactNode; label: string } {
  switch (status.kind) {
    case "idle":
      return { icon: <HardDrive className="h-3 w-3" />, label: "idle" };
    case "wiping":
      return {
        icon: <Loader2 className="h-3 w-3 animate-spin" />,
        label: "wiping",
      };
    case "erased":
      return { icon: <CheckCircle2 className="h-3 w-3" />, label: "erased" };
    case "failed":
      return {
        icon: <AlertTriangle className="h-3 w-3" />,
        label: "needs attention",
      };
    case "pending_co_sign":
      return {
        icon: <FileSignature className="h-3 w-3" />,
        label: "pending co-sign",
      };
    case "destroyed":
      return { icon: <Trash2 className="h-3 w-3" />, label: "destroyed" };
    case "quarantined":
      return {
        icon: <AlertTriangle className="h-3 w-3" />,
        label: "quarantined",
      };
    case "aborted":
      return { icon: <CircleOff className="h-3 w-3" />, label: "aborted" };
  }
}

function StatusBody({ status }: { status: SlotStatus }) {
  switch (status.kind) {
    case "idle":
      return null;
    case "wiping": {
      const pct = Math.round((status.erasure.progress?.fraction ?? 0) * 100);
      const stage = status.erasure.progress?.stage ?? "starting…";
      const eta = status.erasure.progress?.eta_seconds;
      return (
        <div className="mt-3">
          <div className="h-2 overflow-hidden rounded-full bg-slate-800">
            <div
              className="h-full rounded-full bg-indigo-500 transition-all"
              style={{ width: `${pct}%` }}
            />
          </div>
          <div className="mt-1 flex items-center justify-between text-[10px] text-slate-400">
            <span>{stage}</span>
            <span>
              {pct}%{eta != null ? ` · ${eta}s left` : ""}
            </span>
          </div>
        </div>
      );
    }
    case "erased":
      return (
        <div className="mt-3 flex items-center gap-2 text-xs text-emerald-300">
          <Unplug className="h-3.5 w-3.5" />
          <span className="font-medium">Safe to disconnect</span>
        </div>
      );
    case "failed":
      return (
        <div className="mt-3 text-xs text-amber-300">
          Erasure attempt failed — operator action required.
        </div>
      );
    case "pending_co_sign":
      return (
        <div className="mt-3 text-xs text-indigo-300">
          Awaiting supervisor co-sign on a destruction manifest.
        </div>
      );
    case "destroyed":
      return (
        <div className="mt-3 text-xs text-orange-300">
          Marked destroyed — drive should not be on the bench.
        </div>
      );
    case "quarantined":
      return (
        <div className="mt-3 text-xs text-rose-300">
          Quarantined — set aside for review.
        </div>
      );
    case "aborted":
      return (
        <div className="mt-3 text-xs text-slate-400">Previous Job aborted.</div>
      );
  }
}

function Actions({
  status,
  device: _device,
  onStart,
}: {
  status: SlotStatus;
  device: Device;
  onStart: () => void;
}) {
  switch (status.kind) {
    case "idle":
      return (
        <button className="btn btn-primary" onClick={onStart}>
          Start <ChevronRight className="h-4 w-4" />
        </button>
      );
    case "wiping":
    case "failed":
    case "pending_co_sign":
      return (
        <Link
          to="/jobs/$jobId"
          params={{ jobId: status.job.id }}
          className="btn btn-secondary"
        >
          Open Job <ChevronRight className="h-4 w-4" />
        </Link>
      );
    case "erased":
      return (
        <>
          <button className="btn btn-ghost" onClick={onStart} title="Re-wipe this drive">
            Start new
          </button>
          <Link
            to="/certs/$jobId"
            params={{ jobId: status.job.id }}
            className="btn btn-primary"
          >
            View cert
          </Link>
        </>
      );
    case "destroyed":
    case "quarantined":
    case "aborted":
      return (
        <>
          <button className="btn btn-ghost" onClick={onStart} title="Re-wipe this drive">
            Start new
          </button>
          <Link
            to="/jobs/$jobId"
            params={{ jobId: status.job.id }}
            className="btn btn-secondary"
          >
            Open Job
          </Link>
        </>
      );
  }
}

function EraseWizard({ device, onClose }: { device: Device; onClose: () => void }) {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { operator } = useOperator();
  const caps = useQuery({
    queryKey: ["caps", device.id],
    queryFn: () => api.deviceCapabilities(device.id),
  });
  const [classification, setClassification] = useState<Classification>("high");
  const [intent, setIntent] = useState<Intent>("reuse");
  const [assetTag, setAssetTag] = useState("");
  const [ticket, setTicket] = useState("");

  const create = useMutation({
    mutationFn: async () => {
      if (!operator) {
        throw new Error("operator not signed in");
      }
      const created = await api.createJob({
        device_id: device.id,
        classification,
        intent,
        operator,
        asset_tag: assetTag || null,
        site_label: null,
        ticket_ref: ticket || null,
      });
      await api.startJob(created.job_id);
      return created.job_id;
    },
    onSuccess: async (job_id) => {
      await qc.invalidateQueries({ queryKey: ["jobs"] });
      onClose();
      navigate({ to: "/jobs/$jobId", params: { jobId: job_id } });
    },
  });

  return (
    <div className="fixed inset-0 z-30 flex items-end justify-center bg-slate-950/70 p-4 sm:items-center">
      <div className="w-full max-w-lg space-y-4 rounded-xl border border-slate-700 bg-slate-900 p-5 shadow-2xl">
        <div className="flex items-start justify-between">
          <div>
            <h3 className="text-base font-semibold">Sanitize device</h3>
            <p className="mt-0.5 text-xs text-slate-400">
              {device.vendor} {device.model} · {device.serial} ·{" "}
              {formatBytes(device.capacity_bytes)}
            </p>
          </div>
          <button onClick={onClose} className="btn btn-ghost p-1">
            <X className="h-4 w-4" />
          </button>
        </div>

        {operator && (
          <div className="flex items-center gap-2 rounded-md border border-slate-800 bg-slate-950/40 px-3 py-2 text-xs">
            <UserCircle2 className="h-4 w-4 text-indigo-400" />
            <span className="text-slate-300">
              Running as <span className="font-medium">{operator.display_name}</span>
            </span>
            <span className="font-mono text-[10px] text-slate-500">
              &lt;{operator.email}&gt;
            </span>
            <span className="ml-auto text-[10px] uppercase tracking-wide text-slate-500">
              stamped on cert
            </span>
          </div>
        )}

        <div className="grid grid-cols-2 gap-3">
          <div>
            <div className="label">Classification (FIPS 199)</div>
            <div className="flex gap-1">
              {(["low", "moderate", "high"] as const).map((c) => (
                <button
                  key={c}
                  onClick={() => setClassification(c)}
                  className={classNames(
                    "btn flex-1 text-xs",
                    classification === c
                      ? "bg-indigo-500 text-white"
                      : "bg-slate-800 text-slate-300 hover:bg-slate-700"
                  )}
                >
                  {c}
                </button>
              ))}
            </div>
          </div>
          <div>
            <div className="label">Intent</div>
            <div className="flex gap-1">
              {(["reuse", "recycle", "destroy"] as const).map((c) => (
                <button
                  key={c}
                  onClick={() => setIntent(c)}
                  className={classNames(
                    "btn flex-1 text-xs",
                    intent === c
                      ? "bg-indigo-500 text-white"
                      : "bg-slate-800 text-slate-300 hover:bg-slate-700"
                  )}
                >
                  {c}
                </button>
              ))}
            </div>
          </div>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <div className="label">Asset tag</div>
            <input
              className="field"
              value={assetTag}
              onChange={(e) => setAssetTag(e.target.value)}
              placeholder="optional"
            />
          </div>
          <div>
            <div className="label">Ticket / RMA</div>
            <input
              className="field"
              value={ticket}
              onChange={(e) => setTicket(e.target.value)}
              placeholder="optional"
            />
          </div>
        </div>

        {caps.data && (
          <div className="rounded-md border border-slate-800 bg-slate-950/50 p-3 text-xs">
            <div className="mb-1 font-semibold text-slate-300">Detected capabilities</div>
            <ul className="space-y-0.5 text-slate-400">
              {caps.data.nvme_sanitize && (
                <li>
                  NVMe Sanitize:{" "}
                  {caps.data.nvme_sanitize.block_erase ? "BlockErase " : ""}
                  {caps.data.nvme_sanitize.crypto_erase ? "CryptoErase " : ""}
                  {caps.data.nvme_sanitize.overwrite ? "Overwrite " : ""}
                </li>
              )}
              {caps.data.ata_security?.supported && (
                <li>
                  ATA Security:{" "}
                  {caps.data.ata_security.enhanced_supported
                    ? "Enhanced supported"
                    : "Basic only"}
                  {caps.data.ata_security.frozen ? " (frozen)" : ""}
                </li>
              )}
              <li>SED: {caps.data.sed}</li>
            </ul>
          </div>
        )}

        {create.isError && (
          <div className="rounded-md bg-rose-950/40 p-2 text-xs text-rose-300">
            {(create.error as Error).message}
          </div>
        )}

        <div className="flex justify-end gap-2 pt-2">
          <button className="btn btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn btn-primary"
            onClick={() => create.mutate()}
            disabled={create.isPending || !operator}
          >
            {create.isPending ? "Starting…" : "Begin sanitization"}
          </button>
        </div>
      </div>
    </div>
  );
}
