import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  CheckCircle2,
  CheckSquare,
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
import { ScanGroup, ScanInput } from "@/components/ScanInput";
import { useOperator } from "@/operator/context";
import { useEventStream } from "@/api/ws";
import { BayMap, makeBayLookup, type BayCellData } from "@/bench/BayMap";
import {
  EMPTY_SELECTION,
  batchTargets,
  bayCellsInOrder,
  canStart,
  pruneSelection,
  runBatch,
  selectAllIdle,
  toggleBay,
  type BatchResult,
  type BatchSelection,
  type BatchTarget,
} from "@/bench/batchSelection";
import {
  ATTENTION_KINDS,
  deriveSlotStatus,
  SLOT_TONE,
  type SlotStatus,
  type SlotStatusKind,
} from "@/bench/slotStatus";
import type { Classification, Device, Intent, Job, OperatorRef } from "@/api/types";

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
  // Batch start (ROADMAP O1.4): select mode ticks idle bays instead of
  // opening the single-drive wizard; the selection is ad hoc, never stored.
  const [selectMode, setSelectMode] = useState(false);
  const [batch, setBatch] = useState<BatchSelection>(EMPTY_SELECTION);
  const [batchOpen, setBatchOpen] = useState(false);
  const { operator } = useOperator();

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

  // Every bay as the map draws it, in bay-map order — from the server's
  // resolved occupancy only (ADR-0002).
  const cells = useMemo(() => {
    if (!bays.data) return [];
    const lookup = makeBayLookup(bays.data, devicesById, deviceToLatestJob, deriveSlotStatus);
    return bayCellsInOrder(bays.data.topology, lookup);
  }, [bays.data, devicesById, deviceToLatestJob]);

  // A refetch of devices, jobs or topology can make a ticked bay stale (drive
  // pulled or swapped, a Job started on it); drop it before it can be started.
  useEffect(() => {
    setBatch((prev) => pruneSelection(prev, cells));
  }, [cells]);

  const targets = useMemo(() => batchTargets(batch, cells), [batch, cells]);

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

  const leaveSelectMode = () => {
    setSelectMode(false);
    setBatch(EMPTY_SELECTION);
  };

  const onBaySelect = (cell: BayCellData) => {
    if (selectMode) {
      setBatch((prev) => toggleBay(prev, cell));
      return;
    }
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
          {view === "bays" && resolved && (
            <button
              className={classNames(
                "btn text-xs",
                selectMode ? "btn-secondary" : "btn-ghost",
              )}
              onClick={() => (selectMode ? leaveSelectMode() : setSelectMode(true))}
              aria-pressed={selectMode}
            >
              <CheckSquare className="h-3.5 w-3.5" />
              {selectMode ? "Done selecting" : "Select bays"}
            </button>
          )}
          <ViewToggle
            view={view}
            onChange={(v) => {
              if (v !== "bays") leaveSelectMode();
              setView(v);
            }}
          />
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
            {selectMode && (
              <SelectionBar
                count={targets.length}
                operator={operator}
                onSelectAll={() => setBatch((prev) => selectAllIdle(prev, cells))}
                onClear={() => setBatch(EMPTY_SELECTION)}
                onStart={() => setBatchOpen(true)}
                canOpen={canStart({ targets, operator, running: false })}
              />
            )}
            <BayMap
              resolved={resolved}
              devicesById={devicesById}
              jobsByDeviceId={deviceToLatestJob}
              deriveStatus={deriveSlotStatus}
              onSelect={onBaySelect}
              selectedBayId={selectMode ? null : focusedBayId}
              selectedBayIds={selectMode ? new Set(batch.keys()) : undefined}
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

      {batchOpen && (
        <BatchWizard
          targets={targets}
          onClose={(ran) => {
            setBatchOpen(false);
            // Cancelling before Begin keeps the ticks so the operator can add
            // a bay; only a finished run clears the selection.
            if (ran) leaveSelectMode();
          }}
        />
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
    "pending_destruction",
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
    case "pending_destruction":
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
    case "pending_destruction":
      return {
        icon: <FileSignature className="h-3 w-3" />,
        label: "pending destruction",
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
    case "pending_destruction":
      return (
        <div className="mt-3 text-xs text-indigo-300">
          Recorded for destruction; pending destruction on a manifest.
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
    case "pending_destruction":
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

        {operator && <OperatorChip operator={operator} />}

        <SpecChoices
          classification={classification}
          onClassification={setClassification}
          intent={intent}
          onIntent={setIntent}
        />

        {/* A scanner's Enter in Asset tag moves to Ticket/RMA, never to Begin. */}
        <ScanGroup className="grid grid-cols-2 gap-3">
          <div>
            <div className="label">Asset tag</div>
            <ScanInput
              className="field"
              value={assetTag}
              onChange={(e) => setAssetTag(e.target.value)}
              placeholder="optional"
            />
          </div>
          <TicketField value={ticket} onChange={setTicket} />
        </ScanGroup>

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

// --- spec controls shared by the single-drive and batch wizards ------------

function OperatorChip({ operator }: { operator: OperatorRef }) {
  return (
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
  );
}

function SpecChoices({
  classification,
  onClassification,
  intent,
  onIntent,
}: {
  classification: Classification;
  onClassification: (c: Classification) => void;
  intent: Intent;
  onIntent: (i: Intent) => void;
}) {
  return (
    <div className="grid grid-cols-2 gap-3">
      <div>
        <div className="label">Classification (FIPS 199)</div>
        <div className="flex gap-1">
          {(["low", "moderate", "high"] as const).map((c) => (
            <button
              key={c}
              onClick={() => onClassification(c)}
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
              onClick={() => onIntent(c)}
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
  );
}

function TicketField({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return (
    <div>
      <div className="label">Ticket / RMA</div>
      <ScanInput
        className="field"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="optional"
      />
    </div>
  );
}

// --- batch start (ROADMAP O1.4) --------------------------------------------

function SelectionBar({
  count,
  operator,
  canOpen,
  onSelectAll,
  onClear,
  onStart,
}: {
  count: number;
  operator: OperatorRef | null;
  canOpen: boolean;
  onSelectAll: () => void;
  onClear: () => void;
  onStart: () => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2 rounded-md border border-sky-700/50 bg-sky-500/5 px-3 py-2 text-xs">
      <span className="font-medium text-sky-200">{count} selected</span>
      <span className="text-slate-500">Click idle bays to add or remove them.</span>
      {!operator && <span className="text-amber-300">Sign in to start Jobs.</span>}
      <div className="ml-auto flex items-center gap-2">
        <button className="btn btn-ghost text-xs" onClick={onSelectAll}>
          Select all idle
        </button>
        <button className="btn btn-ghost text-xs" onClick={onClear} disabled={count === 0}>
          Clear
        </button>
        <button className="btn btn-primary text-xs" onClick={onStart} disabled={!canOpen}>
          Start {count}… <ChevronRight className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}

/**
 * One shared spec, one Job per drive. Rows follow the live selection until
 * Begin, so a drive pulled meanwhile drops out; from Begin on, the rows are
 * the snapshot that was sent and then each drive's outcome.
 */
function BatchWizard({
  targets,
  onClose,
}: {
  targets: BatchTarget[];
  /** `ran` is true once Begin has been pressed. */
  onClose: (ran: boolean) => void;
}) {
  const qc = useQueryClient();
  const { operator } = useOperator();
  const [classification, setClassification] = useState<Classification>("high");
  const [intent, setIntent] = useState<Intent>("reuse");
  const [ticket, setTicket] = useState("");
  const [assetTags, setAssetTags] = useState<ReadonlyMap<string, string>>(new Map());
  const [running, setRunning] = useState(false);
  const [sent, setSent] = useState<BatchTarget[] | null>(null);
  const [results, setResults] = useState<BatchResult[] | null>(null);
  // State updates are not synchronous; this stops a double click from
  // starting a second run before `running` re-renders the button disabled.
  const inFlight = useRef(false);

  const begin = async () => {
    if (inFlight.current || !operator) return;
    if (!canStart({ targets, operator, running })) return;
    inFlight.current = true;
    setRunning(true);
    const snapshot = targets;
    setSent(snapshot);
    try {
      const out = await runBatch(
        snapshot,
        { classification, intent, operator, ticket },
        assetTags,
        api,
      );
      setResults(out);
    } finally {
      setRunning(false);
      await qc.invalidateQueries({ queryKey: ["jobs"] });
    }
  };

  const rows = sent ?? targets;
  const done = results !== null;
  const close = () => onClose(sent !== null);
  const started = results?.filter((r) => r.outcome.kind === "started").length ?? 0;

  return (
    <div className="fixed inset-0 z-30 flex items-end justify-center bg-slate-950/70 p-4 sm:items-center">
      <div className="flex max-h-[90vh] w-full max-w-2xl flex-col space-y-4 rounded-xl border border-slate-700 bg-slate-900 p-5 shadow-2xl">
        <div className="flex items-start justify-between">
          <div>
            <h3 className="text-base font-semibold">
              {done ? "Batch results" : `Sanitize ${rows.length} drive${rows.length === 1 ? "" : "s"}`}
            </h3>
            <p className="mt-0.5 text-xs text-slate-400">
              {done
                ? `${started} of ${results.length} started`
                : "One Job per drive, with the spec below. Asset tags are per drive."}
            </p>
          </div>
          <button onClick={close} className="btn btn-ghost p-1" disabled={running}>
            <X className="h-4 w-4" />
          </button>
        </div>

        {!done && operator && <OperatorChip operator={operator} />}

        {!done && (
          <SpecChoices
            classification={classification}
            onClassification={setClassification}
            intent={intent}
            onIntent={setIntent}
          />
        )}

        {/* One scan group: Ticket/RMA → row 1's asset tag → … → row N's. */}
        <ScanGroup className="flex min-h-0 flex-col space-y-4">
          {!done && (
            <div className="grid grid-cols-2 gap-3">
              <TicketField value={ticket} onChange={setTicket} />
            </div>
          )}

          <div className="min-h-0 overflow-y-auto rounded-md border border-slate-800">
            <table className="w-full text-xs">
              <thead className="bg-slate-950/50 text-left text-slate-400">
                <tr>
                  <th className="px-3 py-1.5 font-medium">Bay</th>
                  <th className="px-3 py-1.5 font-medium">Drive</th>
                  <th className="px-3 py-1.5 font-medium">{done ? "Result" : "Asset tag"}</th>
                </tr>
              </thead>
              <tbody>
                {done
                  ? results.map((r) => <BatchResultRow key={r.target.bayId} result={r} />)
                  : rows.map((t) => (
                      <tr key={t.bayId} className="border-t border-slate-800">
                        <td className="px-3 py-1.5 font-mono">{t.bayLabel}</td>
                        <td className="px-3 py-1.5">
                          <div>{t.device.model}</div>
                          <div className="font-mono text-[10px] text-slate-500">{t.device.serial}</div>
                        </td>
                        <td className="px-3 py-1.5">
                          <ScanInput
                            className="field"
                            value={assetTags.get(t.device.id) ?? ""}
                            disabled={running}
                            onChange={(e) => {
                              const next = new Map(assetTags);
                              next.set(t.device.id, e.target.value);
                              setAssetTags(next);
                            }}
                            placeholder="optional"
                            aria-label={`Asset tag for bay ${t.bayLabel}`}
                          />
                        </td>
                      </tr>
                    ))}
                {!done && rows.length === 0 && (
                  <tr>
                    <td colSpan={3} className="px-3 py-3 text-center text-slate-500">
                      No idle drives left in the selection.
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        </ScanGroup>

        <div className="flex justify-end gap-2 pt-2">
          {done ? (
            <button className="btn btn-primary" onClick={close}>
              Close
            </button>
          ) : (
            <>
              <button className="btn btn-ghost" onClick={close} disabled={running}>
                Cancel
              </button>
              <button
                className="btn btn-primary"
                onClick={begin}
                disabled={!canStart({ targets: rows, operator, running })}
              >
                {running ? "Starting…" : `Begin sanitization (${rows.length})`}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function BatchResultRow({ result }: { result: BatchResult }) {
  const { target, outcome } = result;
  return (
    <tr className="border-t border-slate-800 align-top">
      <td className="px-3 py-1.5 font-mono">{target.bayLabel}</td>
      <td className="px-3 py-1.5">
        <div>{target.device.model}</div>
        <div className="font-mono text-[10px] text-slate-500">{target.device.serial}</div>
      </td>
      <td className="px-3 py-1.5">
        {outcome.kind === "started" && (
          <span className="flex items-center gap-2">
            <span className="pill pill-info">started</span>
            <Link to="/jobs/$jobId" params={{ jobId: outcome.jobId }} className="text-indigo-300 hover:underline">
              Open Job
            </Link>
          </span>
        )}
        {outcome.kind === "not_started" && (
          <div className="space-y-1">
            <span className="flex items-center gap-2">
              <span className="pill pill-warning">created, not started</span>
              <Link to="/jobs/$jobId" params={{ jobId: outcome.jobId }} className="text-indigo-300 hover:underline">
                Open Job
              </Link>
            </span>
            <div className="text-rose-300">{outcome.error}</div>
          </div>
        )}
        {outcome.kind === "not_created" && (
          <div className="space-y-1">
            <span className="pill pill-danger">not created</span>
            <div className="text-rose-300">{outcome.error}</div>
          </div>
        )}
      </td>
    </tr>
  );
}
