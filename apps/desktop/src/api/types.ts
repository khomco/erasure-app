// Mirrors the serde-serialized shapes from wipe-common. Keep in sync.

export type Classification = "low" | "moderate" | "high";
export type Intent = "reuse" | "recycle" | "destroy";
export type Category = "clear" | "purge" | "destroy";
export type MediaType =
  | "hdd_magnetic"
  | "ssd_sata"
  | "ssd_nvme"
  | "emmc"
  | "ufs"
  | "usb_flash"
  | "optical"
  | "tape"
  | "unknown";
export type BusType = "sata" | "nvme" | "scsi" | "sas" | "usb" | "mmc" | "unknown";
export type SedStatus = "none" | "supported_not_provisioned" | "provisioned" | "locked";
export type StationRole = "lead" | "member" | "console" | "hub";

export interface Device {
  id: string;
  vendor: string;
  model: string;
  serial: string;
  wwn: string | null;
  capacity_bytes: number;
  media_type: MediaType;
  bus: BusType;
  firmware: string | null;
  removable: boolean;
  block_size: number;
  path: string;
}

export interface NvmeSanitizeCaps {
  block_erase: boolean;
  overwrite: boolean;
  crypto_erase: boolean;
  ndi_inhibited: boolean;
  nodmmas: number;
  estimated_block_erase_secs: number | null;
  estimated_crypto_erase_secs: number | null;
  estimated_overwrite_secs: number | null;
}

export interface AtaSecurityCaps {
  supported: boolean;
  enhanced_supported: boolean;
  estimated_minutes: number | null;
  enhanced_estimated_minutes: number | null;
  frozen: boolean;
}

export interface Capabilities {
  ata_security: AtaSecurityCaps | null;
  nvme_sanitize: NvmeSanitizeCaps | null;
  trim: boolean;
  crypto_erase_supported: boolean;
  sed: SedStatus;
  hpa_present: boolean;
  dco_present: boolean;
  frozen: boolean;
}

export type Method =
  | { kind: "nvme_sanitize_block_erase"; ause: boolean; no_deallocate: boolean }
  | { kind: "nvme_sanitize_crypto_erase"; ause: boolean; no_deallocate: boolean }
  | { kind: "nvme_sanitize_overwrite"; ause: boolean; no_deallocate: boolean; pattern_u32: number }
  | { kind: "ata_secure_erase"; enhanced: boolean }
  | { kind: "block_overwrite"; pattern: unknown; passes: number }
  | { kind: "opal_revert" }
  | { kind: "destroy"; method: string };

export type JobStateLabel =
  | "queued"
  | "probing"
  | "confirming"
  | "unfreezing"
  | "running"
  | "verifying"
  | "generating_cert"
  | "signing"
  | "completed"
  | "failed"
  | "aborted";

export interface JobStateWire {
  state: JobStateLabel;
}

export interface Progress {
  fraction: number;
  eta_seconds: number | null;
  stage: string;
  bytes_processed: number | null;
  bytes_total: number | null;
}

export interface OperatorRef {
  id: string;
  display_name: string;
  email: string;
}

export interface JobSpec {
  device_id: string;
  classification: Classification;
  intent: Intent;
  method: Method | null;
  verify: boolean;
  verify_samples: number;
  operator: OperatorRef;
  asset_tag: string | null;
  site_label: string | null;
  ticket_ref: string | null;
}

export interface CommandEvidence {
  interface: string;
  opcode: number | null;
  action: number | null;
  raw_cdb: string | null;
  status: number | null;
  sense: string | null;
  log_page: string | null;
  duration_ms: number;
  note: string | null;
}

export interface SampleResult {
  offset_bytes: number;
  size_bytes: number;
  sha256_hex: string;
  entropy_bits_per_byte: number;
  passed: boolean;
}

export interface VerificationReport {
  method: "sampled_pattern" | "sampled_entropy" | "full_readback";
  sample_count: number;
  bytes_sampled: number;
  samples: SampleResult[];
  all_passed: boolean;
}

export type JobUpdateKind =
  | { kind: "state_changed"; from: JobStateWire; to: JobStateWire }
  | { kind: "progress"; fraction: number; eta_seconds: number | null; stage: string; bytes_processed: number | null; bytes_total: number | null }
  | { kind: "command_issued" } & CommandEvidence
  | { kind: "command_result" } & CommandEvidence
  | { kind: "verification" } & VerificationReport
  | { kind: "warning"; code: string; message: string }
  | { kind: "failed"; reason: string };

export interface JobUpdate {
  at: string;
  event: JobUpdateKind;
}

export interface Job {
  id: string;
  device_snapshot: Device;
  capabilities_snapshot: Capabilities;
  spec: JobSpec;
  resolved_method: Method | null;
  state: JobStateWire;
  progress: Progress | null;
  events: JobUpdate[];
  created_at: string;
  started_at: string | null;
  ended_at: string | null;
  verification: VerificationReport | null;
  certificate_id: string | null;
}

export interface StationInfo {
  id: string;
  hostname: string;
  role: StationRole;
  version: string;
  api_port: number;
  started_at: string;
  active_jobs: number;
  last_seen: string | null;
}

export interface SignedCertificate {
  certificate: {
    "@context": string;
    type: string;
    id: string;
    cert_format_version: number;
    issuer: { tool_name: string; tool_version: string; public_key_id: string };
    issued_at: string;
    job_id: string;
    operator: OperatorRef;
    spec: {
      classification: Classification;
      intent: Intent;
      asset_tag: string | null;
      ticket_ref: string | null;
      site_label: string | null;
    };
    device: Device;
    capabilities_snapshot: Capabilities;
    sanitization: {
      category: Category;
      method: Method;
      method_human: string;
      standard_refs: Array<{ standard: string; section: string }>;
    };
    evidence: {
      command_evidence: CommandEvidence[];
      verification: VerificationReport | null;
      started_at: string;
      ended_at: string;
      duration_seconds: number;
      events: JobUpdate[];
    };
    validation: {
      validated: boolean;
      media_class: string;
      validation_ref: string | null;
      validation_expires: string | null;
    };
    media_status: { operational: boolean; damaged: boolean; notes: string | null };
  };
  signature: {
    algorithm: string;
    public_key_id: string;
    canonical_sha256_hex: string;
    signature_b64: string;
  };
}

export interface PublicKeyResponse {
  public_key_id: string;
  public_key_b64: string;
  algorithm: string;
}
