//! Job — the outcome-bearing unit of processing one Asset to a terminal
//! disposition (ADR-0001).
//!
//! A `Job` composes one or more typed activities (Diagnostic, HealthCheck,
//! ErasureEvent, Verification, Destruction) and produces one signed
//! `Certificate` covering the full evidence chain. Retries, method
//! fallback, and the worst-case path where erasure fails and the Asset
//! must be physically destroyed are all first-class parts of the audit
//! story — not separate records that an auditor has to compose by hand.
//!
//! Before ADR-0001 (v0.1) the type called `Job` in this crate meant
//! "one attempted erasure of one device." That type is now
//! `ErasureEvent`; the *new* `Job` is the outer composition.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    DeviceId, ErasureEvent, Intent, OperatorRef, StationId, VerificationReport,
};

/// What was asked for at Job creation: target Asset, classification,
/// intent, operator, optional WorkOrder/ticket/site references.
///
/// In Simple mode, `asset_tag` is freeform and `work_order_ref` / `customer_ref`
/// are typically `None`. In Enterprise mode (v0.2 #3) those reference fields
/// are populated and the classification is inherited from the WorkOrder →
/// Contract → Default policy chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    /// Primary device for this Job. A Job is scoped to one Asset; one
    /// Asset has one primary device snapshot at intake.
    pub device_id: DeviceId,
    pub classification: crate::Classification,
    pub intent: Intent,
    pub operator: OperatorRef,

    /// Freeform asset tag (Simple mode), or the Asset's tag (Enterprise mode).
    pub asset_tag: Option<String>,
    pub site_label: Option<String>,
    pub ticket_ref: Option<String>,

    // Enterprise-mode optional references. Populated by the integration tier
    // when an upstream ERP record drives this Job.
    pub work_order_ref: Option<String>,
    pub customer_ref: Option<String>,
    pub contract_ref: Option<String>,
    pub sanitization_profile_ref: Option<String>,
}

/// Outer Job state machine: reflects the Asset's terminal disposition,
/// not any single attempt's progress.
///
/// `PendingDestruction` is the audit-honest state for the Destroy path
/// (ADR-0011): the operator has recorded the Asset for destruction with a
/// `DestructionAttestation`, and the Job waits for destruction to be
/// completed on a `DestructionManifest`. Nothing in this state claims the
/// Asset is destroyed. Serialized `pending_destruction`; the pre-ADR-0011
/// name `pending_co_sign` is still accepted on input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobState {
    Queued,
    InProgress,
    /// Recorded for destruction; awaiting destruction on a DestructionManifest.
    #[serde(alias = "pending_co_sign")]
    PendingDestruction,
    Erased,
    Destroyed,
    Quarantined,
    Aborted,
}

impl JobState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Erased | Self::Destroyed | Self::Quarantined | Self::Aborted
        )
    }
}

/// The Asset's resolved disposition, recorded on the signed cert.
/// `disposition` is explicit on the cert so an auditor doesn't have to
/// re-derive it from the activity chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetDisposition {
    Erased,
    Destroyed,
    Quarantined,
}

impl JobState {
    pub fn disposition(self) -> Option<AssetDisposition> {
        match self {
            Self::Erased => Some(AssetDisposition::Erased),
            Self::Destroyed => Some(AssetDisposition::Destroyed),
            Self::Quarantined => Some(AssetDisposition::Quarantined),
            _ => None,
        }
    }
}

/// Pre-flight diagnostics on an Asset's device(s). Schema-only in v0.2;
/// runner does not emit these yet (deferred to SanitizationProfile-driven
/// policy in v0.2 #3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticEvent {
    pub id: Uuid,
    pub device_id: DeviceId,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub findings: Vec<DiagnosticFinding>,
    pub station_id: Option<StationId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticFinding {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Critical,
}

/// SMART / NVMe health snapshot. Schema-only in v0.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckEvent {
    pub id: Uuid,
    pub device_id: DeviceId,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// Raw health attributes captured from the device. Implementation
    /// details deferred; the schema slot is here for forward compatibility.
    pub attributes: serde_json::Value,
    pub station_id: Option<StationId>,
}

/// Verification — the per-erasure sampled-read check (entropy or pattern).
///
/// This is "verification #1" in the project's three-verification model:
/// fast, sample-of-drive, runs immediately after a wipe attempt. The
/// R2v3 audit-sample forensic verification is a separate entity, not
/// modelled here; the cert leaves a loose reference slot for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationEvent {
    pub id: Uuid,
    /// Which ErasureEvent this verification ran against.
    pub erasure_event_id: Uuid,
    pub device_id: DeviceId,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub report: VerificationReport,
    pub station_id: Option<StationId>,
}

/// Physical destruction chain-of-custody record. Populated when an Asset
/// can no longer be erased and must be physically destroyed.
///
/// v0.2 ships the schema + Tier-1 local-sync supervisor co-sign. The
/// full async DocuSign-style supervisor flow (Tier-2 cloud) is deferred.
/// Photo references are optional URIs; the full chain-of-custody UX
/// (photo capture, evidence handling) is v0.3 #12.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestructionEvent {
    pub id: Uuid,
    pub device_id: DeviceId,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub method: crate::DestructMethod,
    /// Operator who physically processed the destruction.
    pub operator: OperatorRef,
    /// Optional supervisor identity if co-signed in-line (Tier 1 local
    /// flow). For batched manifest co-sign, the supervisor is recorded
    /// on the DestructionManifest and the cert carries a supervisor
    /// signature via `manifest_ref`.
    pub supervisor: Option<OperatorRef>,
    /// Manifest this DestructionEvent was rolled into for batched co-sign,
    /// when applicable.
    pub manifest_ref: Option<Uuid>,
    /// Optional photo/video evidence URIs. Schema-only in v0.2.
    #[serde(default)]
    pub photo_refs: Vec<String>,
    pub notes: Option<String>,
    pub station_id: Option<StationId>,
    /// The operator's "recorded for destruction" attestation (ADR-0011 §1).
    /// Omitted when absent, so a v2 certificate signed before this field
    /// existed re-canonicalizes to the same bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<crate::DestructionAttestation>,
}

/// One typed activity composing the Job's evidence chain.
///
/// A Job carries `activities: Vec<JobActivity>` in event order. The cert
/// serialises the same list. The variants correspond to the typed events
/// in ADR-0001 § Decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobActivity {
    Diagnostic(DiagnosticEvent),
    HealthCheck(HealthCheckEvent),
    Erasure(ErasureEvent),
    Verification(VerificationEvent),
    Destruction(DestructionEvent),
}

impl JobActivity {
    pub fn id(&self) -> Uuid {
        match self {
            Self::Diagnostic(e) => e.id,
            Self::HealthCheck(e) => e.id,
            Self::Erasure(e) => e.id,
            Self::Verification(e) => e.id,
            Self::Destruction(e) => e.id,
        }
    }
}

/// The outer, outcome-bearing unit. Processes one Asset (or freeform
/// `Device + asset_tag` in Simple mode) to a terminal disposition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    pub spec: JobSpec,
    pub state: JobState,
    /// Typed activities in event order. The cert composes these.
    pub activities: Vec<JobActivity>,
    /// Manifest this Job is rolled into when in `PendingDestruction`.
    pub manifest_id: Option<Uuid>,
    /// Set once the outer Job's cert has been generated and signed.
    pub certificate_id: Option<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
}

impl Job {
    pub fn new(spec: JobSpec) -> Self {
        Self {
            id: Uuid::new_v4(),
            spec,
            state: JobState::Queued,
            activities: Vec::new(),
            manifest_id: None,
            certificate_id: None,
            created_at: OffsetDateTime::now_utc(),
            started_at: None,
            ended_at: None,
        }
    }

    /// Most recent ErasureEvent on this Job, if any. The runner uses this
    /// to decide retry/fallback policy.
    pub fn latest_erasure(&self) -> Option<&ErasureEvent> {
        self.activities.iter().rev().find_map(|a| match a {
            JobActivity::Erasure(e) => Some(e),
            _ => None,
        })
    }

    /// All ErasureEvents in this Job, in event order.
    pub fn erasures(&self) -> impl Iterator<Item = &ErasureEvent> {
        self.activities.iter().filter_map(|a| match a {
            JobActivity::Erasure(e) => Some(e),
            _ => None,
        })
    }
}

/// A manifest grouping N pending Destroy Jobs for one supervisor co-sign
/// action. Distinct from the operator-UX `Batch` concept in CONTEXT §5;
/// the manifest is auditor-facing and matches paper-shredder convention.
///
/// Tier-1 ships local-sync co-sign at the lead station (supervisor walks
/// over with YubiKey/PIV). Tier-2 cloud will add async DocuSign-style
/// remote co-sign — the schema is the same; only the notification +
/// signing UX differs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestructionManifest {
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Operator who assembled the manifest (typically the floor lead).
    pub assembled_by: OperatorRef,
    /// Jobs included in this manifest. Each is in `PendingDestruction` until
    /// the manifest is signed.
    pub job_ids: Vec<Uuid>,
    pub state: ManifestState,
    /// Free-form note attached at assembly (e.g. shredder run id, vendor pickup).
    pub note: Option<String>,
    /// Populated when a supervisor cosigns. Multi-supervisor (two-person
    /// rule) is a future extension; v0.2 records the single supervisor.
    pub supervisor: Option<OperatorRef>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub signed_at: Option<OffsetDateTime>,
    /// "Destruction completed" for this run (ADR-0011 §1, second step).
    #[serde(default)]
    pub completion: Option<crate::DestructionCompletion>,
    /// Optional concurrence with this run (ADR-0011 §3).
    #[serde(default)]
    pub concurrence: Option<crate::Concurrence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestState {
    Pending,
    Signed,
    Rejected,
}

impl DestructionManifest {
    pub fn new(assembled_by: OperatorRef, job_ids: Vec<Uuid>, note: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            created_at: OffsetDateTime::now_utc(),
            assembled_by,
            job_ids,
            state: ManifestState::Pending,
            note,
            supervisor: None,
            signed_at: None,
            completion: None,
            concurrence: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event_json() -> serde_json::Value {
        json!({
            "id": "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f",
            "device_id": "dev-1",
            "at": "2026-11-08T10:00:00Z",
            "method": "shred",
            "operator": {"id": "op-1", "display_name": "Ana", "email": "ana@example.com"},
            "supervisor": null,
            "manifest_ref": null,
            "photo_refs": [],
            "notes": null,
            "station_id": null
        })
    }

    /// v2 certificates embed `DestructionEvent`; one signed before the
    /// `attestation` field existed must re-canonicalize to the same bytes.
    #[test]
    fn an_event_without_an_attestation_keeps_its_old_shape() {
        let old = event_json();
        let e: DestructionEvent = serde_json::from_value(old.clone()).unwrap();
        assert!(e.attestation.is_none());
        let again = serde_json::to_value(&e).unwrap();
        assert!(again.get("attestation").is_none());
        assert_eq!(again, old);
    }

    #[test]
    fn the_old_pending_co_sign_name_still_deserializes() {
        let s: JobState = serde_json::from_value(json!({"state": "pending_co_sign"})).unwrap();
        assert_eq!(s, JobState::PendingDestruction);
        assert_eq!(
            serde_json::to_value(s).unwrap(),
            json!({"state": "pending_destruction"})
        );
        let s: JobState = serde_json::from_value(json!({"state": "pending_destruction"})).unwrap();
        assert_eq!(s, JobState::PendingDestruction);
    }

    #[test]
    fn new_techniques_round_trip_on_an_event() {
        let mut v = event_json();
        v["method"] = json!("crush");
        let e: DestructionEvent = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(e.method, crate::DestructMethod::Crush);
        assert_eq!(serde_json::to_value(&e).unwrap(), v);
    }

    #[test]
    fn a_manifest_with_completion_and_concurrence_round_trips() {
        use crate::{
            AttestationSigner, Concurrence, ConcurrenceKind, DestructionCompletion,
            DestructionVerification,
        };
        let op = OperatorRef {
            id: "op-1".into(),
            display_name: "Ana".into(),
            email: "ana@example.com".into(),
        };
        let signer =
            AttestationSigner::self_asserted(op.clone(), "Ana Ruiz", "Bench lead").unwrap();
        let at = OffsetDateTime::from_unix_timestamp(1_794_000_000).unwrap();
        let mut m = DestructionManifest::new(op, vec![Uuid::new_v4()], None);
        m.completion = Some(
            DestructionCompletion::new(
                at,
                DestructionVerification {
                    remnants_inspected: true,
                    equipment_cycle_completed: true,
                    equipment_id: "SHR-2".into(),
                    criterion: "platters will not engage".into(),
                },
                Some("CAM-4"),
                signer.clone(),
            )
            .unwrap(),
        );
        m.concurrence = Some(Concurrence::new(ConcurrenceKind::Witness, m.id, at, signer).unwrap());
        let back: DestructionManifest =
            serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(back.completion, m.completion);
        assert_eq!(back.concurrence, m.concurrence);
    }

    #[test]
    fn a_manifest_stored_before_completion_existed_still_loads() {
        let m = DestructionManifest::new(
            OperatorRef {
                id: "op-1".into(),
                display_name: "Ana".into(),
                email: "ana@example.com".into(),
            },
            vec![],
            None,
        );
        let mut v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["completion"], json!(null));
        assert_eq!(v["concurrence"], json!(null));
        let obj = v.as_object_mut().unwrap();
        obj.remove("completion");
        obj.remove("concurrence");
        let back: DestructionManifest = serde_json::from_value(v).unwrap();
        assert!(back.completion.is_none() && back.concurrence.is_none());
    }
}
