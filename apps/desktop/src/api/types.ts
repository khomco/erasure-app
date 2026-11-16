// Mirrors the serde-serialized shapes from wipe-common. Keep in sync.
//
// Per ADR-0001 (v0.2): the outer `Job` is the outcome-bearing unit that
// composes typed activities; the v0.1 `Job` (one attempted erasure) is
// now `ErasureEvent`.

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

/** Mirrors `wipe_common::DestructMethod`; order is `DestructMethod::ALL`. */
export type DestructMethod =
  | "shred"
  | "disintegrate"
  | "incinerate"
  | "pulverize"
  | "melt"
  | "crush"
  | "punch"
  | "degauss"
  | "drill";

export type Method =
  | { kind: "nvme_sanitize_block_erase"; ause: boolean; no_deallocate: boolean }
  | { kind: "nvme_sanitize_crypto_erase"; ause: boolean; no_deallocate: boolean }
  | { kind: "nvme_sanitize_overwrite"; ause: boolean; no_deallocate: boolean; pattern_u32: number }
  | { kind: "ata_secure_erase"; enhanced: boolean }
  | { kind: "block_overwrite"; pattern: unknown; passes: number }
  | { kind: "opal_revert" }
  | { kind: "destroy"; method: DestructMethod };

/** Inner state of one ErasureEvent (one wipe attempt). */
export type ErasureEventStateLabel =
  | "queued"
  | "probing"
  | "unfreezing"
  | "confirming"
  | "running"
  | "completed"
  | "failed"
  | "aborted";

export interface ErasureEventStateWire {
  state: ErasureEventStateLabel;
}

/** Outer Job state — the Asset's terminal disposition machine. */
export type JobStateLabel =
  | "queued"
  | "in_progress"
  // Recorded for destruction (ADR-0011); the server still accepts the old
  // name `pending_co_sign` on input but always sends this one.
  | "pending_destruction"
  | "erased"
  | "destroyed"
  | "quarantined"
  | "aborted";

export interface JobStateWire {
  state: JobStateLabel;
}

export type AssetDisposition = "erased" | "destroyed" | "quarantined";

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
  operator: OperatorRef;
  asset_tag: string | null;
  site_label: string | null;
  ticket_ref: string | null;
  work_order_ref: string | null;
  customer_ref: string | null;
  contract_ref: string | null;
  sanitization_profile_ref: string | null;
}

export interface ErasureEventSpec {
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
  | { kind: "state_changed"; from: ErasureEventStateWire; to: ErasureEventStateWire }
  | {
      kind: "progress";
      fraction: number;
      eta_seconds: number | null;
      stage: string;
      bytes_processed: number | null;
      bytes_total: number | null;
    }
  | ({ kind: "command_issued" } & CommandEvidence)
  | ({ kind: "command_result" } & CommandEvidence)
  | { kind: "warning"; code: string; message: string }
  | { kind: "failed"; reason: string };

export interface JobUpdate {
  at: string;
  event: JobUpdateKind;
}

/** One attempted wipe of one device. Several may exist inside one Job. */
export interface ErasureEvent {
  id: string;
  device_snapshot: Device;
  capabilities_snapshot: Capabilities;
  spec: ErasureEventSpec;
  resolved_method: Method | null;
  state: ErasureEventStateWire;
  progress: Progress | null;
  events: JobUpdate[];
  created_at: string;
  started_at: string | null;
  ended_at: string | null;
  station_id: string | null;
}

export interface DiagnosticFinding {
  code: string;
  severity: "info" | "warning" | "critical";
  message: string;
}

export interface DiagnosticEvent {
  id: string;
  device_id: string;
  at: string;
  findings: DiagnosticFinding[];
  station_id: string | null;
}

export interface HealthCheckEvent {
  id: string;
  device_id: string;
  at: string;
  attributes: unknown;
  station_id: string | null;
}

export interface VerificationEvent {
  id: string;
  erasure_event_id: string;
  device_id: string;
  at: string;
  report: VerificationReport;
  station_id: string | null;
}

export interface DestructionEvent {
  id: string;
  device_id: string;
  at: string;
  method: DestructMethod;
  operator: OperatorRef;
  supervisor: OperatorRef | null;
  manifest_ref: string | null;
  photo_refs: string[];
  notes: string | null;
  station_id: string | null;
  /** Omitted (not null) when absent, so pre-ADR-0011 certs keep their bytes. */
  attestation?: DestructionAttestation;
}

// ---- Destruction records (ADR-0011; mirrors wipe_common::destruction) ----

export type IdentityAssurance =
  | { level: "self_asserted" }
  | { level: "verified"; issuer: string; subject: string };

export interface AttestationSigner {
  operator: OperatorRef;
  full_name: string;
  title: string;
  identity_assurance: IdentityAssurance;
}

export type IdentifierSource = "scanned" | "device";

export interface RecordedIdentifiers {
  asset_tag: string | null;
  serial: string | null;
  source: IdentifierSource;
}

/** "Recorded for destruction". The technique is the event's `method`. */
export interface DestructionAttestation {
  at: string;
  equipment_id: string;
  location: string;
  identifiers: RecordedIdentifiers;
  signer: AttestationSigner;
  statement: string;
}

export interface DestructionVerification {
  remnants_inspected: boolean;
  equipment_cycle_completed: boolean;
  equipment_id: string;
  criterion: string;
}

/** "Destruction completed", for one destruction run (manifest). */
export interface DestructionCompletion {
  completed_at: string;
  verification: DestructionVerification;
  video_ref: string | null;
  signer: AttestationSigner;
  statement: string;
}

export type ConcurrenceKind = "review" | "witness";

export type ConcurrencePolicy = "none" | "review" | "witness";

/**
 * The lapse policy carried in a signed licence (ADR-0012). Still parsed,
 * but ignored by stations (ADR-0013 §5).
 */
export type LapsePolicy = "evaluation" | "stop";

/** Mirror of `wipe_license::LeaseStatus`, tagged on `status`. Times are RFC 3339. */
export type LeaseStatus =
  | { status: "valid"; remaining_erasures?: number; days_remaining: number }
  | { status: "expired"; not_after: string }
  | { status: "not_yet_valid"; not_before: string }
  | { status: "quota_exhausted"; used: number; allowed: number }
  | { status: "wrong_machine"; expected: string; actual: string }
  | { status: "clock_rollback"; watermark: string; observed: string }
  | { status: "unlicensed" }
  | { status: "lease_state_unavailable"; detail: string };

export interface LicenseCustomer {
  id: string;
  name: string;
}

/**
 * Mirror of `wipe_server::licence_fetch::HubCheck`: the station's latest
 * licence check with the platform (ADR-0008 §3). `checked_at` is RFC 3339.
 */
export interface HubCheck {
  checked_at: string;
  ok: boolean;
  message: string;
}

/** `GET /api/license`. Licence fields are null on an unlicensed station. */
export interface LicenseInfo {
  status: LeaseStatus;
  operator_message: string;
  customer: LicenseCustomer | null;
  not_after: string | null;
  instance_public_key_id: string | null;
  /**
   * Why the station's next start would be refused (ADR-0013 §1), in the
   * refusal's own words; null when it would not be, and always on a
   * simulated backend.
   */
  new_jobs_refused: string | null;
  /** The backend only simulates devices: nothing is refused (ADR-0013 §2). */
  simulated_backend: boolean;
  fingerprint: string;
  /** Absent until the station has checked with a saved platform address. */
  hub_check?: HubCheck;
}

/**
 * Mirror of `wipe_server::enrollment::EnrollmentAttempt`, tagged on `state`
 * (ADR-0008, registering with the platform from the bench). `seconds_left`
 * is counted by the station on a monotonic clock, never the browser's.
 */
export type EnrollmentAttempt =
  | {
      state: "pending";
      hub_origin: string;
      user_code: string;
      verification_uri: string;
      verification_uri_complete: string;
      seconds_left: number;
      interval: number;
    }
  | {
      state: "installed";
      hub_origin: string;
      licence_id: string;
      customer_name: string;
      not_after: string;
      operator_message: string;
      saved_to: string | null;
      message: string;
    }
  | {
      state: "denied" | "expired" | "unreachable" | "refused" | "failed";
      hub_origin: string;
      message: string;
    };

/** `GET /api/enrollment`: mirror of `wipe_server::enrollment::EnrollmentView`. */
export interface EnrollmentView {
  saved_hub_origin: string | null;
  can_enroll: boolean;
  cannot_enroll_reason: string | null;
  attempt: EnrollmentAttempt | null;
}

/**
 * Mirror of `wipe_license::Entitlements`. `quota`, `scope` and
 * `allowed_methods` are tagged enums on the Rust side; the UI reads none of
 * them, so they stay `unknown` rather than guessing their spellings here.
 */
export interface LicenseEntitlements {
  customer_id: string;
  customer_name: string;
  quota: unknown;
  scope: unknown;
  not_before: string;
  not_after: string;
  features: string[];
  allowed_methods: unknown;
  machine_binding?: string;
}

/** Mirror of `wipe_license::LicenseBody`. `issued_at` is RFC 3339. */
export interface LicenseBody {
  license_format_version: number;
  license_id: string;
  root_key_id: string;
  instance_public_key_id: string;
  entitlements: LicenseEntitlements;
  issued_at: string;
  /** Omitted when `evaluation` (ADR-0012). Parsed but ignored by stations (ADR-0013 §5). */
  lapse_policy?: LapsePolicy;
}

/** Mirror of `wipe_license::LicenseSignature` (the vendor root's signature). */
export interface LicenseSignature {
  algorithm: string;
  public_key_id: string;
  canonical_sha256_hex: string;
  signature_b64: string;
}

/** Mirror of `wipe_license::LicenseCertificate`. */
export interface LicenseCertificate {
  body: LicenseBody;
  signature: LicenseSignature;
}

/**
 * Mirror of `wipe_license::AttestationChain`, carried on a licensed
 * certificate (ADR-0005 §1). Opaque JSON on the Rust certificate side, so
 * read it defensively; it is verified only offline, by `verify-cert`.
 */
export interface AttestationChain {
  license: LicenseCertificate;
  root_key_id: string;
}

export interface Concurrence {
  kind: ConcurrenceKind;
  manifest_id: string;
  at: string;
  signer: AttestationSigner;
  statement: string;
}

export type JobActivity =
  | { type: "diagnostic"; id: string; device_id: string; at: string; findings: DiagnosticFinding[]; station_id: string | null }
  | { type: "health_check"; id: string; device_id: string; at: string; attributes: unknown; station_id: string | null }
  | ({ type: "erasure" } & ErasureEvent)
  | ({ type: "verification" } & VerificationEvent)
  | ({ type: "destruction" } & DestructionEvent);

/**
 * Mirror of `wipe_common::JobLicence`: the licence decision a Job's start
 * recorded (ADR-0013 §1). Its certificate is marked from it. Not signed.
 */
export interface JobLicence {
  evaluation: boolean;
  /** The licence attestation chain; absent on an evaluation decision. */
  attestation?: AttestationChain;
  license_id?: string;
  credits_reserved: number;
  decided_at: string;
}

export interface Job {
  id: string;
  spec: JobSpec;
  state: JobStateWire;
  activities: JobActivity[];
  manifest_id: string | null;
  certificate_id: string | null;
  /** Absent until the Job's start has decided. */
  licence?: JobLicence;
  created_at: string;
  started_at: string | null;
  ended_at: string | null;
}

/**
 * A destruction run's state: `pending` (assembled, awaiting completion) →
 * `awaiting_concurrence` (only when one is required) → `completed`.
 * `rejected` is reserved; nothing sets it. The server reads the pre-ADR-0011
 * `signed` as `completed` but never sends it.
 */
export type ManifestStateLabel = "pending" | "awaiting_concurrence" | "completed" | "rejected";

/** A destruction run (mirrors `wipe_common::DestructionManifest`). */
export interface DestructionManifest {
  id: string;
  created_at: string;
  assembled_by: OperatorRef;
  job_ids: string[];
  state: ManifestStateLabel;
  note: string | null;
  /** The concurrence the station's policy required when the run was assembled. */
  required_concurrence: ConcurrenceKind | null;
  completion: DestructionCompletion | null;
  concurrence: Concurrence | null;
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

export interface CoSignatureBlock {
  signature: {
    algorithm: string;
    public_key_id: string;
    canonical_sha256_hex: string;
    signature_b64: string;
  };
  role: "supervisor" | "auditor";
  manifest_ref: string | null;
  signer: OperatorRef;
  signed_at: string;
}

/**
 * How a drive was destroyed, signed inside a v3 `destroyed` certificate
 * (mirrors `wipe_cert::DestructionRecord`). The "recorded for destruction"
 * attestation is on the Destruction activity `destruction_event_id` names.
 */
export interface CertDestruction {
  manifest_id: string;
  destruction_event_id: string;
  technique: DestructMethod;
  standard_label: string;
  nist_destroy: boolean;
  caution?: string;
  completion: DestructionCompletion;
  required_concurrence: ConcurrenceKind | null;
  concurrence: Concurrence | null;
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
      customer_ref?: string | null;
      work_order_ref?: string | null;
      contract_ref?: string | null;
    };
    /** Absent (v3) for a drive that was never erased: nothing was read from it. */
    device?: Device;
    capabilities_snapshot?: Capabilities;
    disposition: AssetDisposition;
    sanitization: {
      /** Absent only for destruction that is not NIST Destroy (v3). */
      category?: Category;
      method: Method;
      method_human: string;
      standard_refs: Array<{ standard: string; section: string }>;
    };
    /** Set on a v3 `destroyed` certificate and only there. */
    destruction?: CertDestruction;
    activities: JobActivity[];
    started_at: string;
    ended_at: string;
    duration_seconds: number;
    validation: {
      validated: boolean;
      media_class: string;
      validation_ref: string | null;
      validation_expires: string | null;
    };
    media_status: { operational: boolean; damaged: boolean; notes: string | null };
    audit_verification_ref?: { id: string; at: string | null } | null;
    /**
     * Signed marker of an Evaluation certificate (ADR-0005 §5). Always
     * serialized by current stations; treat a missing key as `false`.
     */
    evaluation: boolean;
  };
  signature: {
    algorithm: string;
    public_key_id: string;
    canonical_sha256_hex: string;
    signature_b64: string;
  };
  /** Pre-v3 supervisor co-signatures; never on a v3 certificate. */
  co_signatures?: CoSignatureBlock[];
  /** The licence chain of a licensed station; absent on an evaluation certificate. */
  attestation?: AttestationChain;
}

export interface PublicKeyResponse {
  public_key_id: string;
  public_key_b64: string;
  algorithm: string;
}

// ---------------------------------------------------------------------------
// Bench topology (ADR-0002)
//
// The station declares its physical drive bays; the server resolves each bay
// to a device and serves the result at GET /api/bay-topology. The frontend is
// a renderer — it never re-implements the binding rules.
// ---------------------------------------------------------------------------

export type EnclosureKind =
  | "rackmount"
  | "duplicator"
  | "dock"
  | "nvme_carrier"
  | "usb_caddy"
  | "internal";

/** Wire values are the sizes an operator would say out loud. */
export type BayFormFactor = "3.5in" | "2.5in" | "m2" | "u2" | "other";

export type TrayOrientation = "horizontal" | "vertical";

export type BayBinding =
  | { by: "ses_slot"; slot: number; enclosure?: string | null }
  | { by: "path"; path: string }
  | { by: "serial"; serial: string }
  | { by: "wwn"; wwn: string }
  | { by: "device_id"; device_id: string }
  | { by: "unbound" };

export interface Bay {
  id: string;
  /** What is silkscreened on the hardware — never our array index. */
  label: string;
  row: number;
  col: number;
  form_factor?: BayFormFactor | null;
  binding: BayBinding;
  disabled: boolean;
  note?: string | null;
}

export type BayOrder = "row_major" | "column_major";
export type BayOrigin = "top_left" | "top_right" | "bottom_left" | "bottom_right";

/** How a bank's labels were generated, so an editor can round-trip them. */
export interface NumberingRun {
  order: BayOrder;
  origin: BayOrigin;
  label_start: number;
}

export interface Bank {
  id: string;
  label?: string | null;
  rows: number;
  cols: number;
  form_factor: BayFormFactor;
  orientation: TrayOrientation;
  numbering?: NumberingRun | null;
  bays: Bay[];
}

export interface Enclosure {
  id: string;
  label: string;
  kind: EnclosureKind;
  /** Catalog model this enclosure was expanded from (ADR-0004), if any.
   *  Advisory: the banks below remain the truth about the layout. */
  model_ref?: string | null;
  banks: Bank[];
  note?: string | null;
}

export interface BayTopology {
  schema_version: number;
  label: string;
  /** True when the station had no config and this layout was invented for
   *  display only — the UI must say so. */
  generated: boolean;
  auto_fill_unbound: boolean;
  /** Bumped on every save; a stale value is rejected with 409. */
  revision: number;
  enclosures: Enclosure[];
}

export type ProblemSeverity = "error" | "warning";

export interface TopologyProblem {
  severity: ProblemSeverity;
  code: string;
  message: string;
  enclosure_id?: string | null;
  bank_id?: string | null;
  bay_id?: string | null;
}

/** Where configuration goes, and whether it survives a reboot (ADR-0003). */
export type StoreTier = "local_file" | "control_plane" | "ephemeral";

export interface StoreStatus {
  tier: StoreTier;
  survives_reboot: boolean;
  location: string;
  /** Tier 3: nowhere to persist and nobody has said what to do about it. */
  needs_operator_decision: boolean;
  detail: string;
}

export interface BayOccupancy {
  bay_id: string;
  device_id: string;
}

export interface ResolvedBayTopology {
  topology: BayTopology;
  occupancy: BayOccupancy[];
  /** Devices the station can see that no bay claimed. */
  unplaced_devices: string[];
}

// ---------------------------------------------------------------------------
// Enclosure model catalog (ADR-0004)
//
// Served from GET /api/enclosure-catalog rather than bundled into the UI, so a
// site-local overlay takes effect without a frontend rebuild.
// ---------------------------------------------------------------------------

export interface BankSpec {
  label?: string | null;
  rows: number;
  cols: number;
  form_factor: BayFormFactor;
  orientation: TrayOrientation;
  order: BayOrder;
  origin: BayOrigin;
  label_start: number;
}

export interface ModelSpec {
  banks: BankSpec[];
  connectors?: string[];
  notes?: string | null;
}

/** Absent means "we have not verified this", which must not render as "no". */
export interface ModelCapabilities {
  locate_led: boolean;
  per_bay_power: boolean;
  hotswap_notify: boolean;
  ses_slot_addressing: boolean;
}

export interface EnclosureModel {
  id: string;
  vendor: string;
  product: string;
  aliases?: string[];
  kind: EnclosureKind;
  spec: ModelSpec;
  /** Key into the shell registry. Absent means the generic shell, labelled
   *  as generic — a supported outcome, not a gap. */
  art?: string | null;
  capabilities?: ModelCapabilities | null;
  verified_by?: string | null;
}

export interface EnclosureCatalog {
  schema_version: number;
  models: EnclosureModel[];
}
