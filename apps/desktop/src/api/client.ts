import type {
  Capabilities,
  DestructMethod,
  DestructionManifest,
  Device,
  Job,
  JobSpec,
  OperatorRef,
  PublicKeyResponse,
  BayTopology,
  ResolvedBayTopology,
  StoreStatus,
  SignedCertificate,
  StationInfo,
} from "./types";

const BASE = ""; // Vite proxy handles /api → backend in dev; empty in Tauri.

async function jsonFetch<T>(url: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(`${BASE}${url}`, {
    headers: { "Content-Type": "application/json" },
    ...init,
  });
  if (!resp.ok) {
    let detail = await resp.text();
    try {
      detail = JSON.stringify(JSON.parse(detail));
    } catch {
      // leave as text
    }
    throw new Error(`${resp.status} ${resp.statusText}: ${detail}`);
  }
  return resp.json() as Promise<T>;
}

export interface CreateJobRequest {
  device_id: string;
  classification: JobSpec["classification"];
  intent: JobSpec["intent"];
  operator: OperatorRef;
  asset_tag?: string | null;
  site_label?: string | null;
  ticket_ref?: string | null;
  work_order_ref?: string | null;
  customer_ref?: string | null;
  contract_ref?: string | null;
  sanitization_profile_ref?: string | null;
}

export interface EscalateRequest {
  method: DestructMethod;
  operator: OperatorRef;
  notes?: string | null;
}

export interface CreateManifestRequest {
  assembled_by: OperatorRef;
  job_ids: string[];
  note?: string | null;
}

export const api = {
  health: () => jsonFetch<{ ok: boolean }>("/api/health"),
  station: () => jsonFetch<StationInfo>("/api/station"),
  publicKey: () => jsonFetch<PublicKeyResponse>("/api/public_key"),
  fleetPeers: () => jsonFetch<StationInfo[]>("/api/fleet/peers"),
  fleetLead: () => jsonFetch<{ lead: string | null; is_lead: boolean }>("/api/fleet/lead"),
  devices: () => jsonFetch<Device[]>("/api/devices"),
  bayTopology: () => jsonFetch<ResolvedBayTopology>("/api/bay-topology"),
  bayTopologyConfig: () => jsonFetch<BayTopology>("/api/bay-topology/config"),
  bayTopologyStore: () => jsonFetch<StoreStatus>("/api/bay-topology/store"),
  acknowledgeEphemeralStore: () =>
    jsonFetch<StoreStatus>("/api/bay-topology/store/acknowledge", { method: "POST" }),
  saveBayTopology: (topology: BayTopology) =>
    jsonFetch<BayTopology>("/api/bay-topology", {
      method: "PUT",
      body: JSON.stringify(topology),
    }),
  deviceCapabilities: (id: string) =>
    jsonFetch<Capabilities>(`/api/devices/${encodeURIComponent(id)}/capabilities`),
  jobs: () => jsonFetch<Job[]>("/api/jobs"),
  job: (id: string) => jsonFetch<Job>(`/api/jobs/${id}`),
  createJob: (spec: CreateJobRequest) =>
    jsonFetch<{ job_id: string }>("/api/jobs", {
      method: "POST",
      body: JSON.stringify(spec),
    }),
  startJob: (id: string) =>
    jsonFetch<{ ok: boolean }>(`/api/jobs/${id}/start`, { method: "POST" }),
  abortJob: (id: string) =>
    jsonFetch<{ ok: boolean }>(`/api/jobs/${id}/abort`, { method: "POST" }),
  escalateToDestroy: (id: string, body: EscalateRequest) =>
    jsonFetch<{ ok: boolean }>(`/api/jobs/${id}/escalate-to-destroy`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  certificate: (id: string) =>
    jsonFetch<SignedCertificate>(`/api/jobs/${id}/certificate`),
  manifests: () => jsonFetch<DestructionManifest[]>("/api/manifests"),
  manifest: (id: string) => jsonFetch<DestructionManifest>(`/api/manifests/${id}`),
  createManifest: (body: CreateManifestRequest) =>
    jsonFetch<DestructionManifest>("/api/manifests", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  cosignManifest: (id: string, supervisor: OperatorRef) =>
    jsonFetch<DestructionManifest>(`/api/manifests/${id}/cosign`, {
      method: "POST",
      body: JSON.stringify({ supervisor }),
    }),
};

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 ? 2 : v < 100 ? 1 : 0)} ${units[i]}`;
}

export function classNames(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}
