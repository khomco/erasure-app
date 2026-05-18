//! Certificate schema. Fields are stable and versioned via `cert_format_version`.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use wipe_common::{
    Capabilities, Category, Classification, CommandEvidence, Device, Intent, JobUpdate,
    JobUpdateKind, Method, OperatorRef, VerificationReport,
};

pub const CONTEXT_URI: &str = "https://wipestation.dev/contexts/sanitization-cert-v1";
pub const CERT_FORMAT_VERSION: u32 = 1;

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
    pub device: Device,
    pub capabilities_snapshot: Capabilities,
    pub sanitization: SanitizationBlock,
    pub evidence: EvidenceBlock,
    pub validation: ValidationBlock,
    pub media_status: MediaStatus,
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
pub struct EvidenceBlock {
    pub command_evidence: Vec<CommandEvidence>,
    pub verification: Option<VerificationReport>,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ended_at: OffsetDateTime,
    pub duration_seconds: u64,
    pub events: Vec<JobUpdate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationBlock {
    /// True if this method + media class has a current Validation record
    /// (NIST 800-88 Rev. 2 introduces this programmatic Validate step,
    /// distinct from per-event verification).
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

impl Certificate {
    /// Construct a cert from a completed Job and operator/issuer context.
    /// Returns `None` if the job is not in a state where a cert is meaningful.
    pub fn from_job(
        job: &wipe_common::Job,
        issuer: CertIssuer,
        validation: ValidationBlock,
        media_status: MediaStatus,
    ) -> Option<Self> {
        let method = job.resolved_method.clone()?;
        let started = job.started_at?;
        let ended = job.ended_at?;
        let duration = (ended - started).whole_seconds().max(0) as u64;

        let command_evidence: Vec<CommandEvidence> = job
            .events
            .iter()
            .filter_map(|e| match &e.event {
                JobUpdateKind::CommandIssued(c) | JobUpdateKind::CommandResult(c) => {
                    Some(c.clone())
                }
                _ => None,
            })
            .collect();

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
            },
            device: job.device_snapshot.clone(),
            capabilities_snapshot: job.capabilities_snapshot.clone(),
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
            evidence: EvidenceBlock {
                command_evidence,
                verification: job.verification.clone(),
                started_at: started,
                ended_at: ended,
                duration_seconds: duration,
                events: job.events.clone(),
            },
            validation,
            media_status,
        })
    }
}
