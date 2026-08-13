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
import { useEventStream } from "@/api/ws";
import { BayMap, type BayCellData } from "@/bench/BayMap";
import {
  ATTENTION_KINDS,
  deriveSlotStatus,
  SLOT_TONE,
  type SlotStatus,
  type SlotStatusKind,
} from "@/bench/slotStatus";
import type { Classification, Device, Intent, Job } from "@/api/types";

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

// Slot status derivation lives in @/bench/slotStatus so the card grid and the
// bay map cannot disagree about what colour a drive is.

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

  const bays = useQuery({
    queryKey: ["bay-topology"],
    queryFn: api.bayTopology,
    // Geometry is static config; only the occupancy moves, and that only
    // when a drive is physically inserted or pulled.
    refetchInterval: 5000,
  });

  // Model artwork (ADR-0004). Static per station; a failure here degrades to
  // generic outlines rather than to a missing bay map.
  const catalog = useQuery({
    queryKey: ["enclosure-catalog"],
    queryFn: api.enclosureCatalog,
    staleTime: Infinity,
  });

  const [selected, setSelected] = useState<Device | null>(null);
  const [view, setView] = useState<"bays" | "cards">("bays");
  const [focusedBayId, setFocusedBayId] = useState<string | null>(null);

  const devicesById = useMemo(() => {
    const m = new Map<string, Device>();
    for (const d of devices.data ?? []) m.set(d.id, d);
    return m;
  }, [devices.data]);

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
  const resolved = bays.data ?? null;

  const statuses = list.map((d) => deriveSlotStatus(deviceToLatestJob.get(d.id)));
  const attention = statuses.filter((s) =>
    ATTENTION_KINDS.includes(s.kind),
  ).length;

  const onBaySelect = (cell: BayCellData) => {
    setFocusedBayId(cell.bay.id);
    // An empty bay has nothing to act on; clicking a populated one opens the
    // same wizard the card grid does.
    if (cell.device && cell.status.kind === "empty") return;
    if (cell.device && cell.status.kind === "idle") setSelected(cell.device);
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div className="flex items-baseline gap-3">
          <h2 className="text-lg font-semibold">Bench</h2>
          {resolved && (
            <span className="text-xs text-slate-500">
              {resolved.topology.label}
            </span>
          )}
        </div>
        <div className="flex items-center gap-3">
          {attention > 0 && (
            <span className="pill pill-warning">
              <AlertTriangle className="h-3 w-3" />
              {attention} need{attention === 1 ? "s" : ""} attention
            </span>
          )}
          <span className="text-xs text-slate-500">
            {list.length} device{list.length === 1 ? "" : "s"} attached
          </span>
          <ViewToggle view={view} onChange={setView} />
        </div>
      </div>

      {view === "bays" ? (
        bays.isLoading ? (
          <div className="text-slate-400">Reading bay layout…</div>
        ) : bays.isError ? (
          <div className="card border-rose-700/60 text-xs text-rose-300">
            Could not read the bay topology — falling back to cards.
          </div>
        ) : resolved ? (
          <div className="space-y-3">
            {resolved.topology.generated && (
              <div className="rounded-md border border-amber-600/40 bg-amber-500/5 px-3 py-2 text-xs text-amber-200/90">
                <span className="font-semibold">Bench not configured.</span>{" "}
                These positions are enumeration order, not physical bays. Start
                the station with <code>--bay-profile</code> or{" "}
                <code>--bay-topology</code> to mirror the real hardware.
              </div>
            )}
            <BayMap
              resolved={resolved}
              devicesById={devicesById}
              jobsByDeviceId={deviceToLatestJob}
              deriveStatus={deriveSlotStatus}
              onSelect={onBaySelect}
              selectedBayId={focusedBayId}
              catalog={catalog.data ?? null}
            />
            <BayLegend />
            {resolved.unplaced_devices.length > 0 && (
              <div className="rounded-md border border-amber-600/40 bg-amber-500/5 px-3 py-2 text-xs text-amber-200/90">
                <span className="font-semibold">
                  {resolved.unplaced_devices.length} attached device
                  {resolved.unplaced_devices.length === 1 ? "" : "s"} not on the
                  map.
                </span>{" "}
                No bay claimed{" "}
                {resolved.unplaced_devices
                  .map((id) => devicesById.get(id)?.model ?? id)
                  .join(", ")}
                . Switch to Cards to act on them.
              </div>
            )}
          </div>
        ) : null
      ) : (
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
      )}

      {selected && (
        <EraseWizard device={selected} onClose={() => setSelected(null)} />
      )}
    </div>
  );
}

function ViewToggle({
  view,
  onChange,
}: {
  view: "bays" | "cards";
  onChange: (v: "bays" | "cards") => void;
}) {
  return (
    <div className="flex overflow-hidden rounded-md border border-slate-700 text-xs">
      {(["bays", "cards"] as const).map((v) => (
        <button
          key={v}
          onClick={() => onChange(v)}
          className={classNames(
            "px-2.5 py-1 capitalize transition-colors",
            view === v
              ? "bg-slate-700 text-slate-100"
              : "bg-transparent text-slate-400 hover:bg-slate-800",
          )}
        >
          {v === "bays" ? "Bay map" : "Cards"}
        </button>
      ))}
    </div>
  );
}

/** Colour key. A bay map is only useful if the colours are unambiguous, and
 *  `empty` vs `idle` is the pair most worth spelling out. */
function BayLegend() {
  const shown: SlotStatusKind[] = [
    "empty",
    "idle",
    "wiping",
    "erased",
    "failed",
    "pending_co_sign",
    "destroyed",
    "quarantined",
  ];
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5 text-[11px] text-slate-400">
      {shown.map((kind) => (
        <span key={kind} className="flex items-center gap-1.5">
          <span
            className="inline-block h-2.5 w-2.5 rounded-sm"
            style={{
              backgroundColor: SLOT_TONE[kind].accent,
              outline: `1px solid ${SLOT_TONE[kind].stroke}`,
            }}
          />
          {SLOT_TONE[kind].label}
        </span>
      ))}
      <span className="text-slate-600">
        empty = nothing plugged in · idle = drive present, no Job
      </span>
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
    // A card is built from a device, so `empty` never reaches the grid — it
    // only exists for the bay map, where a Bay outlives whatever was in it.
    case "empty":
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
    case "empty":
      return { icon: <HardDrive className="h-3 w-3" />, label: "empty" };
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
      // `erasure` is null while the Job is in progress but has not appended
      // its first ErasureEvent — show a busy bar at 0% rather than inventing
      // an event shape.
      const progress = status.erasure?.progress ?? null;
      const pct = Math.round((progress?.fraction ?? 0) * 100);
      const stage = progress?.stage ?? "starting…";
      const eta = progress?.eta_seconds;
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
