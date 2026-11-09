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

/// Physical destruction chain-of-custody record: the operator recorded the
/// Asset for destruction (ADR-0011 §1). The certificate signed when the Job
/// reaches `Destroyed` names this event from its `destruction` block.
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
    /// Pre-ADR-0011 in-line supervisor; nothing sets it now. Kept so v2
    /// certificates round-trip.
    pub supervisor: Option<OperatorRef>,
    /// The destruction run (DestructionManifest) this event was assembled
    /// onto; signed into the certificate at `Destroyed`.
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

/// A destruction run: N Jobs recorded for destruction, grouped so one
/// completion (and, when the site requires it, one concurrence) covers them
/// all (ADR-0011 §1, §3). Distinct from the operator-UX `Batch` concept in
/// CONTEXT §5; the manifest is auditor-facing and matches paper-shredder
/// convention.
///
/// Lifecycle: `Pending` (assembled, awaiting completion) →
/// `AwaitingConcurrence` (only when `required_concurrence` is set) →
/// `Completed`. Move it only through [`DestructionManifest::complete`] and
/// [`DestructionManifest::concur`]; the caller moves the member Jobs to
/// `Destroyed` when [`DestructionManifest::destroys_jobs`] says so.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestructionManifest {
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Operator who assembled the manifest (typically the floor lead).
    pub assembled_by: OperatorRef,
    /// Jobs included in this manifest. Each is in `PendingDestruction` until
    /// the run is completed (and concurred with, when that is required).
    pub job_ids: Vec<Uuid>,
    pub state: ManifestState,
    /// Free-form note attached at assembly (e.g. shredder run id, vendor pickup).
    pub note: Option<String>,
    /// The concurrence this run needs before its Jobs reach `Destroyed`,
    /// copied from the station's policy at assembly, so a later policy
    /// change never re-labels or skips it.
    #[serde(default)]
    pub required_concurrence: Option<crate::ConcurrenceKind>,
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
    /// Assembled; awaiting the operator's completion.
    Pending,
    /// Completion recorded; awaiting the required concurrence.
    AwaitingConcurrence,
    /// Done: every member Job is `Destroyed`. Before ADR-0011 this state was
    /// `signed` (supervisor co-sign); that name is still accepted on input.
    #[serde(alias = "signed")]
    Completed,
    /// Reserved; nothing sets it yet.
    Rejected,
}

/// Why a destruction run refused a completion or a concurrence. Nothing on
/// the run changed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestTransitionError {
    #[error("destruction run {id} is not pending completion (state {state:?})")]
    NotPending { id: Uuid, state: ManifestState },
    #[error("destruction run {id} needs no concurrence")]
    NoConcurrenceRequired { id: Uuid },
    #[error("destruction run {id} is not awaiting concurrence (state {state:?})")]
    NotAwaitingConcurrence { id: Uuid, state: ManifestState },
    #[error("destruction run {id} needs a {required:?} concurrence, not {given:?}")]
    WrongConcurrenceKind {
        id: Uuid,
        required: crate::ConcurrenceKind,
        given: crate::ConcurrenceKind,
    },
    #[error("the concurrence names destruction run {given}, not {id}")]
    WrongManifest { id: Uuid, given: Uuid },
    #[error(
        "the concurrence must come from someone other than the operator who confirmed \
         completion"
    )]
    SameSigner,
    #[error(
        "the completion time {at} is out of range: it must be no earlier than when the run \
         was assembled ({created_at}) and not in the future"
    )]
    CompletionTimeOutOfRange {
        at: OffsetDateTime,
        created_at: OffsetDateTime,
    },
}

impl ManifestTransitionError {
    /// A refusal caused by the run's state (a conflict), as opposed to the
    /// request's content.
    pub fn is_conflict(&self) -> bool {
        matches!(
            self,
            Self::NotPending { .. }
                | Self::NoConcurrenceRequired { .. }
                | Self::NotAwaitingConcurrence { .. }
                | Self::WrongConcurrenceKind { .. }
        )
    }
}

/// How far ahead of the station clock a completion time may be.
pub const COMPLETION_CLOCK_SKEW: time::Duration = time::Duration::seconds(60);

impl DestructionManifest {
    pub fn new(
        assembled_by: OperatorRef,
        job_ids: Vec<Uuid>,
        note: Option<String>,
        required_concurrence: Option<crate::ConcurrenceKind>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            created_at: OffsetDateTime::now_utc(),
            assembled_by,
            job_ids,
            state: ManifestState::Pending,
            note,
            required_concurrence,
            completion: None,
            concurrence: None,
        }
    }

    /// Refuse a completion time before the run was assembled (to the
    /// second, as the statement prints it) or more than
    /// [`COMPLETION_CLOCK_SKEW`] after `now`.
    pub fn check_completion_time(
        &self,
        at: OffsetDateTime,
        now: OffsetDateTime,
    ) -> Result<(), ManifestTransitionError> {
        let assembled = self.assembled_to_the_second();
        if at < assembled || at > now + COMPLETION_CLOCK_SKEW {
            return Err(ManifestTransitionError::CompletionTimeOutOfRange {
                at,
                created_at: self.created_at,
            });
        }
        Ok(())
    }

    /// Record the run's completion. Only a `Pending` run accepts it; the run
    /// then awaits its required concurrence, or is `Completed` when none is
    /// required. Refuses a completion time before assembly.
    pub fn complete(
        &mut self,
        completion: crate::DestructionCompletion,
    ) -> Result<(), ManifestTransitionError> {
        if self.state != ManifestState::Pending {
            return Err(ManifestTransitionError::NotPending {
                id: self.id,
                state: self.state,
            });
        }
        let assembled = self.assembled_to_the_second();
        if completion.completed_at < assembled {
            return Err(ManifestTransitionError::CompletionTimeOutOfRange {
                at: completion.completed_at,
                created_at: self.created_at,
            });
        }
        self.state = if self.required_concurrence.is_some() {
            ManifestState::AwaitingConcurrence
        } else {
            ManifestState::Completed
        };
        self.completion = Some(completion);
        Ok(())
    }

    /// Record the required concurrence. Only a run `AwaitingConcurrence`
    /// accepts it, of exactly the required kind, for this run, from someone
    /// other than the completion's signer (a different operator id and a
    /// different full name, ignoring case and whitespace).
    pub fn concur(
        &mut self,
        concurrence: crate::Concurrence,
    ) -> Result<(), ManifestTransitionError> {
        let Some(required) = self.required_concurrence else {
            return Err(ManifestTransitionError::NoConcurrenceRequired { id: self.id });
        };
        if self.state != ManifestState::AwaitingConcurrence {
            return Err(ManifestTransitionError::NotAwaitingConcurrence {
                id: self.id,
                state: self.state,
            });
        }
        if concurrence.kind != required {
            return Err(ManifestTransitionError::WrongConcurrenceKind {
                id: self.id,
                required,
                given: concurrence.kind,
            });
        }
        if concurrence.manifest_id != self.id {
            return Err(ManifestTransitionError::WrongManifest {
                id: self.id,
                given: concurrence.manifest_id,
            });
        }
        let completer = &self
            .completion
            .as_ref()
            .expect("an AwaitingConcurrence run has a completion")
            .signer;
        if crate::same_signer(completer, &concurrence.signer) {
            return Err(ManifestTransitionError::SameSigner);
        }
        self.state = ManifestState::Completed;
        self.concurrence = Some(concurrence);
        Ok(())
    }

    /// `created_at` as the statement prints it, so a completion in the same
    /// second as assembly is not "before" it.
    fn assembled_to_the_second(&self) -> OffsetDateTime {
        self.created_at
            .replace_nanosecond(0)
            .expect("0 is a valid nanosecond")
    }

    /// Whether the run is done, so its member Jobs belong in `Destroyed`.
    pub fn destroys_jobs(&self) -> bool {
        self.state == ManifestState::Completed
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
        let mut m = DestructionManifest::new(op, vec![Uuid::new_v4()], None, None);
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
            None,
        );
        let mut v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["completion"], json!(null));
        assert_eq!(v["concurrence"], json!(null));
        let obj = v.as_object_mut().unwrap();
        obj.remove("completion");
        obj.remove("concurrence");
        obj.remove("required_concurrence");
        // A pre-ADR-0011 manifest also carried these; they are ignored.
        obj.insert("supervisor".into(), json!(null));
        obj.insert("signed_at".into(), json!(null));
        let back: DestructionManifest = serde_json::from_value(v).unwrap();
        assert!(back.completion.is_none() && back.concurrence.is_none());
        assert_eq!(back.required_concurrence, None);
    }

    #[test]
    fn the_old_signed_state_name_reads_as_completed() {
        let s: ManifestState = serde_json::from_value(json!("signed")).unwrap();
        assert_eq!(s, ManifestState::Completed);
        assert_eq!(serde_json::to_value(s).unwrap(), json!("completed"));
        assert_eq!(
            serde_json::to_value(ManifestState::AwaitingConcurrence).unwrap(),
            json!("awaiting_concurrence")
        );
    }

    // ---- run transitions (destruction-attestation story 3) ----

    use crate::{
        AttestationSigner, Concurrence, ConcurrenceKind, DestructionCompletion,
        DestructionVerification,
    };

    fn operator(id: &str, name: &str) -> OperatorRef {
        OperatorRef {
            id: id.into(),
            display_name: name.into(),
            email: format!("{id}@example.com"),
        }
    }

    fn signer(id: &str, full_name: &str) -> AttestationSigner {
        AttestationSigner::self_asserted(operator(id, full_name), full_name, "Bench lead").unwrap()
    }

    fn run(required: Option<ConcurrenceKind>) -> DestructionManifest {
        DestructionManifest::new(
            operator("op-1", "Ana"),
            vec![Uuid::new_v4(), Uuid::new_v4()],
            None,
            required,
        )
    }

    fn completion_at(at: OffsetDateTime) -> DestructionCompletion {
        DestructionCompletion::new(
            at,
            DestructionVerification {
                remnants_inspected: true,
                equipment_cycle_completed: true,
                equipment_id: "SHR-2".into(),
                criterion: "particles ≤ 2 mm".into(),
            },
            None,
            signer("op-1", "Ana Ruiz"),
        )
        .unwrap()
    }

    fn completion() -> DestructionCompletion {
        completion_at(OffsetDateTime::now_utc())
    }

    fn concurrence(
        m: &DestructionManifest,
        kind: ConcurrenceKind,
        s: AttestationSigner,
    ) -> Concurrence {
        Concurrence::new(kind, m.id, OffsetDateTime::now_utc(), s).unwrap()
    }

    #[test]
    fn completing_with_no_concurrence_required_completes_the_run() {
        let mut m = run(None);
        assert!(!m.destroys_jobs());
        m.complete(completion()).unwrap();
        assert_eq!(m.state, ManifestState::Completed);
        assert!(m.completion.is_some() && m.destroys_jobs());
    }

    #[test]
    fn completing_with_a_concurrence_required_awaits_it() {
        for kind in [ConcurrenceKind::Review, ConcurrenceKind::Witness] {
            let mut m = run(Some(kind));
            m.complete(completion()).unwrap();
            assert_eq!(m.state, ManifestState::AwaitingConcurrence);
            assert!(!m.destroys_jobs());
            let c = concurrence(&m, kind, signer("op-2", "Ben Ode"));
            m.concur(c.clone()).unwrap();
            assert_eq!(m.state, ManifestState::Completed);
            assert_eq!(m.concurrence, Some(c));
            assert!(m.destroys_jobs());
        }
    }

    #[test]
    fn a_run_completes_once() {
        let mut m = run(Some(ConcurrenceKind::Review));
        m.complete(completion()).unwrap();
        let before = m.clone();
        assert!(matches!(
            m.complete(completion()),
            Err(ManifestTransitionError::NotPending {
                state: ManifestState::AwaitingConcurrence,
                ..
            })
        ));
        assert_eq!(m.completion, before.completion);
        let mut done = run(None);
        done.complete(completion()).unwrap();
        let err = done.complete(completion()).unwrap_err();
        assert!(err.is_conflict());
        assert_eq!(done.state, ManifestState::Completed);
    }

    #[test]
    fn concurrence_is_refused_when_not_needed_too_early_or_twice() {
        let other = || signer("op-2", "Ben Ode");
        let mut none = run(None);
        let c = concurrence(&none, ConcurrenceKind::Review, other());
        assert!(matches!(
            none.concur(c.clone()),
            Err(ManifestTransitionError::NoConcurrenceRequired { .. })
        ));
        none.complete(completion()).unwrap();
        assert!(none.concur(c).unwrap_err().is_conflict());
        assert!(none.concurrence.is_none());

        let mut early = run(Some(ConcurrenceKind::Witness));
        let c = concurrence(&early, ConcurrenceKind::Witness, other());
        let err = early.concur(c.clone()).unwrap_err();
        assert!(matches!(
            err,
            ManifestTransitionError::NotAwaitingConcurrence {
                state: ManifestState::Pending,
                ..
            }
        ));
        assert!(err.is_conflict());
        assert_eq!(early.state, ManifestState::Pending);

        early.complete(completion()).unwrap();
        early.concur(c.clone()).unwrap();
        let first = early.concurrence.clone();
        let again = concurrence(&early, ConcurrenceKind::Witness, signer("op-3", "Cy Dee"));
        assert!(early.concur(again).unwrap_err().is_conflict());
        assert_eq!(early.concurrence, first);
    }

    #[test]
    fn a_concurrence_of_the_wrong_kind_or_run_is_refused() {
        let mut m = run(Some(ConcurrenceKind::Review));
        m.complete(completion()).unwrap();
        let witness = concurrence(&m, ConcurrenceKind::Witness, signer("op-2", "Ben Ode"));
        assert!(matches!(
            m.concur(witness),
            Err(ManifestTransitionError::WrongConcurrenceKind { .. })
        ));
        let elsewhere = Concurrence::new(
            ConcurrenceKind::Review,
            Uuid::new_v4(),
            OffsetDateTime::now_utc(),
            signer("op-2", "Ben Ode"),
        )
        .unwrap();
        assert!(matches!(
            m.concur(elsewhere),
            Err(ManifestTransitionError::WrongManifest { .. })
        ));
        assert_eq!(m.state, ManifestState::AwaitingConcurrence);
        assert!(m.concurrence.is_none());
    }

    #[test]
    fn the_completion_signer_cannot_concur() {
        let mut m = run(Some(ConcurrenceKind::Review));
        m.complete(completion()).unwrap();
        for s in [
            signer("op-1", "Somebody Else"),
            signer("op-9", "  ana   RUIZ "),
        ] {
            let c = concurrence(&m, ConcurrenceKind::Review, s);
            let err = m.concur(c).unwrap_err();
            assert_eq!(err, ManifestTransitionError::SameSigner);
            assert!(!err.is_conflict());
        }
        assert_eq!(m.state, ManifestState::AwaitingConcurrence);
        assert!(m.concurrence.is_none());
    }

    #[test]
    fn the_completion_time_must_fall_between_assembly_and_now() {
        let m = run(None);
        let now = m.created_at + time::Duration::minutes(5);
        assert!(m.check_completion_time(now, now).is_ok());
        assert!(m
            .check_completion_time(now + time::Duration::seconds(60), now)
            .is_ok());
        assert!(m
            .check_completion_time(now + time::Duration::seconds(61), now)
            .is_err());
        // The same second as assembly is not "before" it.
        let second = m.created_at.replace_nanosecond(0).unwrap();
        assert!(m.check_completion_time(second, now).is_ok());
        let before = second - time::Duration::seconds(1);
        let err = m.check_completion_time(before, now).unwrap_err();
        assert!(!err.is_conflict());

        let mut early = run(None);
        let err = early.complete(completion_at(before)).unwrap_err();
        assert!(matches!(
            err,
            ManifestTransitionError::CompletionTimeOutOfRange { .. }
        ));
        assert_eq!(early.state, ManifestState::Pending);
        assert!(early.completion.is_none());
    }
}
