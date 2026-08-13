//! Certificate schema. Fields are stable and versioned via `cert_format_version`.
//!
//! v2 (ADR-0001) — outer Job composition. The cert carries `activities`
//! (the full typed-event chain) and an explicit `disposition`. v1 — which
//! carried a single `evidence` block representing one attempt — is no
//! longer emitted; v1 information is representable in v2 as a Job with
//! one `Erasure` activity.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use wipe_common::{
    AssetDisposition, Capabilities, Category, Classification, Device, Intent, Job, JobActivity,
    JobUpdateKind, Method, OperatorRef,
};

pub const CONTEXT_URI: &str = "https://wipestation.dev/contexts/sanitization-cert-v2";
pub const CERT_FORMAT_VERSION: u32 = 2;

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

    /// The primary device snapshot for this Asset's Job. For Jobs that
    /// span multiple devices (a future extension), this is the device
    /// the Job was scoped to. The activity chain carries the per-event
    /// device snapshots which may differ as the Asset moves between
    /// stations.
    pub device: Device,
    pub capabilities_snapshot: Capabilities,

    /// Explicit terminal disposition. Auditors read this directly rather
    /// than deriving it from the activity chain.
    pub disposition: AssetDisposition,

    /// Summary of the resolved sanitization for the Asset. The activity
    /// chain carries the full per-attempt detail; this block is the
    /// auditor-readable headline.
    pub sanitization: SanitizationBlock,

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
    pub category: Category,
    pub method: Method,
    pub method_human: String,
    pub standard_refs: Vec<StandardRef>,
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

impl Certificate {
    /// Construct a cert from a Job that has reached a terminal disposition.
    ///
    /// Returns `None` if the Job is not in a state where a cert is
    /// meaningful (no disposition, no started/ended timestamps, no
    /// resolved method, or no Erasure activities to summarise from).
    /// Build a certificate for a Job.
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
        let disposition = job.state.disposition()?;
        let started = job.started_at?;
        let ended = job.ended_at?;
        let duration = (ended - started).whole_seconds().max(0) as u64;

        // Headline method + device snapshot come from the most relevant
        // ErasureEvent: the latest one for an Erased disposition (it's
        // the attempt that succeeded), or the latest one overall for a
        // Destroyed/Quarantined disposition (representing the last
        // attempt before destruction or quarantine).
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
            spec: CertSpecRef {
                classification: job.spec.classification,
                intent: job.spec.intent,
                asset_tag: job.spec.asset_tag.clone(),
                ticket_ref: job.spec.ticket_ref.clone(),
                site_label: job.spec.site_label.clone(),
                customer_ref: job.spec.customer_ref.clone(),
                work_order_ref: job.spec.work_order_ref.clone(),
                contract_ref: job.spec.contract_ref.clone(),
            },
            device,
            capabilities_snapshot: caps,
            disposition,
            sanitization: SanitizationBlock {
                category: method.category(),
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
