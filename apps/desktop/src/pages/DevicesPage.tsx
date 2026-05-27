import { useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Cpu, HardDrive, Database, AlertTriangle, ChevronRight, UserCircle2, X } from "lucide-react";

import { api, classNames, formatBytes } from "@/api/client";
import { useOperator } from "@/operator/context";
import type { Classification, Device, Intent } from "@/api/types";

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

export function DevicesPage() {
  const devices = useQuery({ queryKey: ["devices"], queryFn: api.devices });
  const [selected, setSelected] = useState<Device | null>(null);

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
        <h2 className="text-lg font-semibold">Attached devices</h2>
        <span className="text-xs text-slate-500">{list.length} found</span>
      </div>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {list.map((d) => (
          <button
            key={d.id}
            onClick={() => setSelected(d)}
            className="card text-left transition hover:border-indigo-500/40 hover:bg-slate-900/80"
          >
            <div className="flex items-start justify-between">
              <div className="flex items-start gap-3">
                {deviceIcon(d)}
                <div>
                  <div className="font-medium">{d.model}</div>
                  <div className="text-xs text-slate-400">
                    {d.vendor} · {d.serial}
                  </div>
                </div>
              </div>
              <ChevronRight className="h-4 w-4 text-slate-500" />
            </div>
            <div className="mt-3 flex items-center gap-2">
              <span className="pill">{d.media_type}</span>
              <span className="pill">{d.bus}</span>
              <span className="pill">{formatBytes(d.capacity_bytes)}</span>
            </div>
            <div className="mt-2 font-mono text-[11px] text-slate-500">{d.path}</div>
          </button>
        ))}
      </div>
      {selected && (
        <EraseWizard device={selected} onClose={() => setSelected(null)} />
      )}
    </div>
  );
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
