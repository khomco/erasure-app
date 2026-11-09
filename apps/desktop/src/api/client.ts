import type {
  Capabilities,
  ConcurrencePolicy,
  DestructMethod,
  DestructionManifest,
  Device,
  EnclosureCatalog,
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

/** Identifiers scanned (bar code or typed) from a drive's labels. */
export interface ScannedIdentifiers {
  asset_tag: string | null;
  serial: string | null;
}

/** What the operator attests to; mirrors `handlers::AttestationFields`. */
export interface AttestationFields {
  method: DestructMethod;
  equipment_id: string;
  location: string;
  operator: OperatorRef;
  full_name: string;
  title: string;
  /** The operator ticked "I attest" against the previewed statement. */
  attest: boolean;
  notes: string | null;
}

/**
 * Record an existing Job for destruction. Omit `identifiers` to have the
 * server read them from the device (`source: device`); send them to record
 * scanned identifiers (`source: scanned`). The client never names the source.
 */
export interface RecordForDestructionRequest extends AttestationFields {
  identifiers?: ScannedIdentifiers | null;
}

/** Record a scanned group of drives; one new Job per drive. */
export interface RecordScannedRequest extends AttestationFields {
  classification: JobSpec["classification"];
  drives: ScannedIdentifiers[];
}

export interface StatementPreviewRequest {
  method: DestructMethod;
  equipment_id: string;
  location: string;
  full_name: string;
  title: string;
}

export interface CreateManifestRequest {
  assembled_by: OperatorRef;
  job_ids: string[];
  note?: string | null;
}

/** Who signs a completion or concurrence; mirrors `handlers::SignerFields`. */
export interface SignerFields {
  operator: OperatorRef;
  full_name: string;
  title: string;
  /** The signer ticked "I attest" against the previewed statement. */
  attest: boolean;
}

/** Mirrors `handlers::CompletionRequest`. */
export interface CompletionRequest extends SignerFields {
  /** The time the previewed statement names; null lets the server use now. */
  completed_at: string | null;
  remnants_inspected: boolean;
  equipment_cycle_completed: boolean;
  equipment_id: string;
  criterion: string;
  video_ref: string | null;
}

export interface CompletionPreviewRequest {
  completed_at?: string | null;
  equipment_id: string;
  criterion: string;
  video_ref: string | null;
}

export interface ConcurrencePreviewRequest {
  full_name: string;
  title: string;
}

/** The concurrence kind comes from the run, never from this request. */
export type ConcurrenceRequest = SignerFields;

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
  enclosureCatalog: () => jsonFetch<EnclosureCatalog>("/api/enclosure-catalog"),
  // Dry-run: resolve an unsaved draft against the devices attached right now,
  // so the builder preview can show live occupancy without re-implementing
  // the matching rules in TypeScript.
  resolveBayTopology: (topology: BayTopology) =>
    jsonFetch<ResolvedBayTopology>("/api/bay-topology/resolve", {
      method: "POST",
      body: JSON.stringify(topology),
    }),
  // Simulated hot-plug. The routes are always mounted but return 404 whenever
  // the host attached no simulator — on real hardware and in the mock-backed
  // desktop shell alike — and identify mode falls back to the operator
  // physically inserting drives.
  simAttach: (device_id?: string) =>
    jsonFetch<Device>("/api/sim/devices/attach", {
      method: "POST",
      body: JSON.stringify({ device_id: device_id ?? null }),
    }),
  simDetach: (device_id: string) =>
    jsonFetch<Device>("/api/sim/devices/detach", {
      method: "POST",
      body: JSON.stringify({ device_id }),
    }),
  simDetached: () => jsonFetch<Device[]>("/api/sim/devices"),
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
  recordForDestruction: (id: string, body: RecordForDestructionRequest) =>
    jsonFetch<{ ok: boolean }>(`/api/jobs/${id}/record-for-destruction`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  recordScanned: (body: RecordScannedRequest) =>
    jsonFetch<{ job_ids: string[] }>("/api/destruction/records", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  // The statement is rendered only in Rust; the UI shows it verbatim.
  previewStatement: (body: StatementPreviewRequest) =>
    jsonFetch<{ statement: string }>("/api/destruction/statement", {
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
  // Destruction runs (ADR-0011): completion, then any required concurrence.
  // Statements are rendered only in Rust; the UI shows them verbatim.
  completeRun: (id: string, body: CompletionRequest) =>
    jsonFetch<DestructionManifest>(`/api/manifests/${id}/completion`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  concurRun: (id: string, body: ConcurrenceRequest) =>
    jsonFetch<DestructionManifest>(`/api/manifests/${id}/concurrence`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  previewCompletion: (id: string, body: CompletionPreviewRequest) =>
    jsonFetch<{ statement: string; completed_at: string }>(
      `/api/manifests/${id}/completion/statement`,
      { method: "POST", body: JSON.stringify(body) },
    ),
  previewConcurrence: (id: string, body: ConcurrencePreviewRequest) =>
    jsonFetch<{ statement: string }>(`/api/manifests/${id}/concurrence/statement`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  destructionPolicy: () =>
    jsonFetch<{ concurrence_policy: ConcurrencePolicy }>("/api/destruction/policy"),
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
