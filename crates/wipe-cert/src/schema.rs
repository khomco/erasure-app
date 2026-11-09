//! Certificate schema. Fields are stable and versioned via `cert_format_version`.
//!
//! v3 (ADR-0011) — every new certificate. A destroyed drive's certificate is
//! signed once, when its Job reaches `Destroyed`, and carries a `destruction`
//! block (the run, the honest technique label, the caution, the completion
//! and any concurrence) inside the signed bytes. A drive that was never
//! erased has no `device` / `capabilities_snapshot`; its identifiers are the
//! ones recorded on the attested Destruction activity. On a destroyed
//! certificate `sanitization` describes the destruction, with category
//! `destroy` only for the NIST SP 800-88 Destroy techniques.
//!
//! v2 (ADR-0001) — outer Job composition. The cert carries `activities`
//! (the full typed-event chain) and an explicit `disposition`. No longer
//! emitted, but still verified: every field v3 added or relaxed is omitted
//! when absent, so a v2 certificate re-canonicalizes to its signed bytes
//! (pinned by `tests/fixtures/v2-*.json`). v1 — a single `evidence` block
//! for one attempt — is long gone.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use wipe_common::{
    AssetDisposition, Capabilities, Category, Classification, Concurrence, ConcurrenceKind,
    DestructMethod, DestructionCompletion, DestructionManifest, Device, Intent, Job, JobActivity,
    JobState, JobUpdateKind, Method, OperatorRef,
};

use crate::{CertError, CertResult};

pub const CONTEXT_URI: &str = "https://wipestation.dev/contexts/sanitization-cert-v3";
pub const CERT_FORMAT_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Certificate {
    #[serde(rename = "@context")]
    pub context: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub id: String,
    pub cert_format_version: u32,
    pub issuer: CertIssuer,
    #[serde(with = "time::serde::rfc3339")]
    pub issued_at: OffsetDateTime,

    pub job_id: Uuid,
    pub operator: OperatorRef,
    pub spec: CertSpecRef,

    /// The device snapshot read from the drive by its latest erasure.
    /// Omitted (v3) for a drive that was never erased: nothing was read
    /// from it, and the identifiers recorded on its attested Destruction
    /// activity, with their `source`, are the honest record (ADR-0011 §2).
    /// Always present on v2 certificates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<Device>,
    /// Capabilities read alongside `device`; omitted exactly when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities_snapshot: Option<Capabilities>,

    /// Explicit terminal disposition. Auditors read this directly rather
    /// than deriving it from the activity chain.
    pub disposition: AssetDisposition,

    /// Summary of the resolved sanitization for the Asset. The activity
    /// chain carries the full per-attempt detail; this block is the
    /// auditor-readable headline. On a v3 destroyed certificate it describes
    /// the destruction technique, never the last failed erasure.
    pub sanitization: SanitizationBlock,

    /// The destruction run that destroyed this drive (v3). Set on a
    /// `destroyed` certificate and only there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destruction: Option<DestructionRecord>,

    /// The Job's full typed-event composition: Diagnostic, ErasureEvent(s),
    /// Verification(s), Destruction. Carried in event order.
    pub activities: Vec<JobActivity>,

    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ended_at: OffsetDateTime,
    pub duration_seconds: u64,

    pub validation: ValidationBlock,
    pub media_status: MediaStatus,

    /// True when this certificate was produced by a station with no valid
    /// vendor licence (ADR-0005 §5).
    ///
    /// Such a certificate is still fully valid and offline-verifiable — the
    /// erasure really happened and the signature really holds. The marker
    /// exists so unlicensed output can never masquerade as licensed, and it
    /// lives *inside* the signed payload so it cannot be stripped.
    ///
    /// Defaults to false so pre-ADR-0005 certificates deserialize unchanged;
    /// this marker is a compatibility commitment and must stay stable, or old
    /// evaluation certs become indistinguishable from licensed ones.
    #[serde(default)]
    pub evaluation: bool,

    /// Loose pointer for the future R2v3 audit-sample verification entity.
    /// Out of ADR-0001's scope to populate (the audit-sample verification
    /// happens *after* the cert ships); the schema slot exists so the
    /// future entity links cleanly without a cert_format_version bump.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_verification_ref: Option<AuditVerificationRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertIssuer {
    pub tool_name: String,
    pub tool_version: String,
    pub public_key_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertSpecRef {
    pub classification: Classification,
    pub intent: Intent,
    pub asset_tag: Option<String>,
    pub ticket_ref: Option<String>,
    pub site_label: Option<String>,

    // Enterprise-mode references. Populated when an upstream ERP record
    // drove the Job; otherwise None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_order_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SanitizationBlock {
    /// NIST SP 800-88 category. Absent only on a v3 destroyed certificate
    /// whose technique is physical damage, not NIST Destroy (ADR-0011 §5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
    pub method: Method,
    pub method_human: String,
    pub standard_refs: Vec<StandardRef>,
}

/// How a drive was destroyed, signed inside a v3 `destroyed` certificate
/// (ADR-0011 §2). The operator's "recorded for destruction" attestation is
/// not copied here: it is signed once, on the Destruction activity
/// `destruction_event_id` names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestructionRecord {
    /// The destruction run (`DestructionManifest`) the drive was destroyed in.
    pub manifest_id: Uuid,
    /// The attested Destruction activity in `activities`.
    pub destruction_event_id: Uuid,
    pub technique: DestructMethod,
    /// "NIST SP 800-88 Destroy" or "physical damage — not NIST Destroy".
    pub standard_label: String,
    pub nist_destroy: bool,
    /// NIST's shred/pulverize caution, when the classification is above low.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caution: Option<String>,
    /// "Destruction completed" for the run.
    pub completion: DestructionCompletion,
    /// The concurrence the run required when it was assembled.
    pub required_concurrence: Option<ConcurrenceKind>,
    /// The run's concurrence; present exactly when one was required.
    pub concurrence: Option<Concurrence>,
}

impl DestructionRecord {
    /// Build the record for `job` destroyed in `run`. Fallible, and the only
    /// fallible part of a destroyed certificate: the server calls it for
    /// every member before moving any Job to `Destroyed`, so no Job reaches
    /// `Destroyed` uncertifiable. Does not look at the Job's state.
    ///
    /// Refuses when `run` is not completed or lacks its completion or a
    /// required concurrence, when `job` is not a member linked to `run`, or
    /// when the Job's latest Destruction activity has no attestation or
    /// names another run.
    pub fn build(job: &Job, run: &DestructionManifest) -> CertResult<Self> {
        let refuse = |why: String| Err(CertError::Invalid(format!("job {} {why}", job.id)));
        if !run.job_ids.contains(&job.id) || job.manifest_id != Some(run.id) {
            return refuse(format!("is not on destruction run {}", run.id));
        }
        if !run.destroys_jobs() {
            return refuse(format!(
                "cannot be certified yet: destruction run {} is not completed (state {:?})",
                run.id, run.state
            ));
        }
        let Some(completion) = run.completion.clone() else {
            return refuse(format!(
                "cannot be certified: destruction run {} has no completion",
                run.id
            ));
        };
        let concurrence = match run.required_concurrence {
            None => None,
            Some(kind) => match &run.concurrence {
                Some(c) if c.kind == kind && c.manifest_id == run.id => Some(c.clone()),
                _ => {
                    return refuse(format!(
                        "cannot be certified: destruction run {} has no {kind:?} concurrence",
                        run.id
                    ))
                }
            },
        };
        let Some(event) = job.activities.iter().rev().find_map(|a| match a {
            JobActivity::Destruction(e) => Some(e),
            _ => None,
        }) else {
            return refuse("has no destruction record".into());
        };
        if event.attestation.is_none() {
            return refuse("has a destruction record with no attestation".into());
        }
        if event.manifest_ref != Some(run.id) {
            return refuse(format!(
                "has a destruction record not linked to destruction run {}",
                run.id
            ));
        }
        let technique = event.method;
        Ok(Self {
            manifest_id: run.id,
            destruction_event_id: event.id,
            technique,
            standard_label: technique.standard_label().to_owned(),
            nist_destroy: technique.is_nist_destroy(),
            caution: technique
                .caution(job.spec.classification)
                .map(str::to_owned),
            completion,
            required_concurrence: run.required_concurrence,
            concurrence,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandardRef {
    pub standard: String,
    pub section: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationBlock {
    /// True if this method + media class has a current Validation record
    /// (NIST 800-88 Rev. 2's programmatic Validate step, distinct from
    /// per-event verification).
    pub validated: bool,
    pub media_class: String,
    pub validation_ref: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub validation_expires: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaStatus {
    pub operational: bool,
    pub damaged: bool,
    pub notes: Option<String>,
}

/// Loose reference to a future R2v3 audit-sample verification record.
/// The audit-sample entity itself lives in a separate (future) ADR; this
/// is just the link the cert leaves behind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditVerificationRef {
    /// Opaque id of the future AuditVerification record.
    pub id: String,
    /// When the audit-sample verification was performed, if known at
    /// cert-sign time. Typically `None` at sign time and populated via
    /// addendum once the audit-sample is processed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub at: Option<OffsetDateTime>,
}

fn spec_ref(job: &Job) -> CertSpecRef {
    CertSpecRef {
        classification: job.spec.classification,
        intent: job.spec.intent,
        asset_tag: job.spec.asset_tag.clone(),
        ticket_ref: job.spec.ticket_ref.clone(),
        site_label: job.spec.site_label.clone(),
        customer_ref: job.spec.customer_ref.clone(),
        work_order_ref: job.spec.work_order_ref.clone(),
        contract_ref: job.spec.contract_ref.clone(),
    }
}

impl Certificate {
    /// Build the certificate of an erased (or quarantined) Job.
    ///
    /// Returns `None` if the Job is not in a state where this cert is
    /// meaningful: no disposition, no started/ended timestamps, no resolved
    /// method, no Erasure activity to summarise from, or `Destroyed` (use
    /// [`Certificate::from_destroyed_job`], which needs the run).
    ///
    /// `evaluation` marks the cert as produced without a valid vendor licence.
    /// There is deliberately no default: a caller must state which it is, so a
    /// host that has not been taught about licensing cannot silently emit
    /// unmarked certificates (ADR-0005 §5).
    pub fn from_job(
        job: &Job,
        issuer: CertIssuer,
        validation: ValidationBlock,
        media_status: MediaStatus,
        evaluation: bool,
    ) -> Option<Self> {
        if job.state == JobState::Destroyed {
            return None;
        }
        let disposition = job.state.disposition()?;
        let started = job.started_at?;
        let ended = job.ended_at?;
        let duration = (ended - started).whole_seconds().max(0) as u64;

        // Headline method + device snapshot come from the latest
        // ErasureEvent: the attempt that succeeded for Erased, the last
        // attempt before quarantine for Quarantined.
        let summary_erasure = job.latest_erasure()?;
        let method = summary_erasure.resolved_method.clone()?;
        let device = summary_erasure.device_snapshot.clone();
        let caps = summary_erasure.capabilities_snapshot.clone();

        Some(Certificate {
            context: CONTEXT_URI.into(),
            type_: "SanitizationCertificate".into(),
            id: format!("urn:uuid:{}", Uuid::new_v4()),
            cert_format_version: CERT_FORMAT_VERSION,
            issuer,
            issued_at: OffsetDateTime::now_utc(),
            job_id: job.id,
            operator: job.spec.operator.clone(),
            spec: spec_ref(job),
            device: Some(device),
            capabilities_snapshot: Some(caps),
            disposition,
            sanitization: SanitizationBlock {
                category: Some(method.category()),
                method_human: method.human_name().to_string(),
                method,
                standard_refs: vec![
                    StandardRef {
                        standard: "NIST SP 800-88 Rev. 2".into(),
                        section: "decision flow + IEEE 2883 deferral".into(),
                    },
                    StandardRef {
                        standard: "IEEE 2883-2022".into(),
                        section: "Clear / Purge / Destruct mappings".into(),
                    },
                ],
            },
            destruction: None,
            activities: job.activities.clone(),
            started_at: started,
            ended_at: ended,
            duration_seconds: duration,
            validation,
            media_status,
            evaluation,
            audit_verification_ref: None,
        })
    }

    /// Build the certificate of a Job destroyed in `run` (ADR-0011 §2):
    /// signed once, at `Destroyed`, with the run's completion and any
    /// concurrence inside. A Job that was erased before carries its latest
    /// erasure's device snapshot; a never-erased Job carries none, and is
    /// identified by its attested Destruction activity. A never-started
    /// Job's `started_at` is its `created_at`.
    ///
    /// Refuses a Job that is not `Destroyed` or has no `ended_at`, and
    /// anything [`DestructionRecord::build`] refuses.
    pub fn from_destroyed_job(
        job: &Job,
        run: &DestructionManifest,
        issuer: CertIssuer,
        validation: ValidationBlock,
        media_status: MediaStatus,
        evaluation: bool,
    ) -> CertResult<Self> {
        if job.state != JobState::Destroyed {
            return Err(CertError::Invalid(format!(
                "job {} is not destroyed (state {:?})",
                job.id, job.state
            )));
        }
        let record = DestructionRecord::build(job, run)?;
        let ended = job
            .ended_at
            .ok_or_else(|| CertError::Invalid(format!("job {} has no end time", job.id)))?;
        let started = job.started_at.unwrap_or(job.created_at);
        let duration = (ended - started).whole_seconds().max(0) as u64;
        let erasure = job.latest_erasure();
        let technique = record.technique;
        let mut standard_refs = Vec::new();
        if record.nist_destroy {
            standard_refs.push(StandardRef {
                standard: "NIST SP 800-88 Rev. 2".into(),
                section: "§3.1.3 Destroy techniques".into(),
            });
        }
        standard_refs.push(StandardRef {
            standard: "NIST SP 800-88 Rev. 2".into(),
            section: "§4.6 certificate of sanitization".into(),
        });
        Ok(Certificate {
            context: CONTEXT_URI.into(),
            type_: "SanitizationCertificate".into(),
            id: format!("urn:uuid:{}", Uuid::new_v4()),
            cert_format_version: CERT_FORMAT_VERSION,
            issuer,
            issued_at: OffsetDateTime::now_utc(),
            job_id: job.id,
            operator: job.spec.operator.clone(),
            spec: spec_ref(job),
            device: erasure.map(|e| e.device_snapshot.clone()),
            capabilities_snapshot: erasure.map(|e| e.capabilities_snapshot.clone()),
            disposition: AssetDisposition::Destroyed,
            sanitization: SanitizationBlock {
                category: record.nist_destroy.then_some(Category::Destroy),
                method: Method::Destroy { method: technique },
                method_human: format!("Physical destruction — {}", technique.human_name()),
                standard_refs,
            },
            destruction: Some(record),
            activities: job.activities.clone(),
            started_at: started,
            ended_at: ended,
            duration_seconds: duration,
            validation,
            media_status,
            evaluation,
            audit_verification_ref: None,
        })
    }

    /// All `CommandEvidence` records across every ErasureEvent in this
    /// cert, in event order. Provided as a convenience for cert renderers
    /// that want a flat command list; the source of truth is the activity
    /// chain.
    pub fn command_evidence(&self) -> Vec<wipe_common::CommandEvidence> {
        self.activities
            .iter()
            .flat_map(|a| {
                let erasure = match a {
                    JobActivity::Erasure(e) => Some(e),
                    _ => None,
                };
                erasure
                    .into_iter()
                    .flat_map(|e| e.events.iter())
                    .filter_map(|u| match &u.event {
                        JobUpdateKind::CommandIssued(c) | JobUpdateKind::CommandResult(c) => {
                            Some(c.clone())
                        }
                        _ => None,
                    })
            })
            .collect()
    }
}
