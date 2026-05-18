import { useQuery } from "@tanstack/react-query";
import { ShieldCheck, Cpu } from "lucide-react";

import { api } from "@/api/client";

export function FleetPage() {
  const station = useQuery({ queryKey: ["station"], queryFn: api.station });
  const peers = useQuery({
    queryKey: ["peers"],
    queryFn: api.fleetPeers,
    refetchInterval: 2000,
  });
  const lead = useQuery({
    queryKey: ["lead"],
    queryFn: api.fleetLead,
    refetchInterval: 2000,
  });

  const all = [station.data, ...(peers.data ?? [])].filter(Boolean) as Array<
    NonNullable<typeof station.data>
  >;

  return (
    <div className="space-y-4">
      <h2 className="text-lg font-semibold">Fleet</h2>
      <div className="card">
        <div className="flex items-center gap-3">
          <ShieldCheck className="h-5 w-5 text-emerald-400" />
          <div>
            <div className="text-sm">
              Lead station:{" "}
              <span className="font-mono">{lead.data?.lead ?? "—"}</span>
            </div>
            <div className="text-xs text-slate-400">
              {lead.data?.is_lead ? "This station is the lead." : "Lead is elsewhere."}
            </div>
          </div>
        </div>
      </div>

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {all.map((s) => (
          <div key={s.id} className="card">
            <div className="flex items-start gap-3">
              <Cpu className="h-5 w-5 text-indigo-400" />
              <div className="min-w-0">
                <div className="font-medium truncate">{s.hostname}</div>
                <div className="text-xs text-slate-400 font-mono truncate">
                  {s.id}
                </div>
                <div className="mt-1 flex flex-wrap gap-1">
                  <span className="pill">{s.role}</span>
                  <span className="pill">v{s.version}</span>
                  <span className="pill">port {s.api_port}</span>
                  <span className="pill">{s.active_jobs} jobs</span>
                </div>
              </div>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
