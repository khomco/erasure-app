//! Job orchestration types.
//!
//! **Glossary-vs-code drift in flight.** In `CONTEXT.md` §5 / ADR-0001,
//! `Job` is the outcome-bearing unit that composes one or more typed
//! events (DiagnosticEvent, ErasureEvent, VerificationEvent,
//! DestructionEvent) and reaches a terminal disposition
//! (Erased / Destroyed / Quarantined / Aborted). The `Job` struct in
//! *this* file is what the glossary calls `ErasureEvent` — a single
//! erasure attempt — and the `JobState` enum is what the glossary
//! calls `ErasureEventState`. The rename is sequenced as v0.2 item #2
//! in `CONTEXT.md` §11; reading source under the old names is
//! correct for v0.1 code.
//!
//! `JobEvent`/`EventKind`/`JobUpdate` were renamed in this pass to
//! `JobUpdate`/`JobUpdateKind`/`JobUpdateMessage` (see runner.rs for
//! the message envelope). Wire format is preserved.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    Capabilities, CommandEvidence, Device, DeviceId, Method, OperatorRef, VerificationReport,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    pub device_id: DeviceId,
    pub classification: crate::Classification,
    pub intent: crate::Intent,
    /// Operator may pin a specific method. None means auto-select.
    pub method: Option<Method>,
    pub verify: bool,
    pub verify_samples: u32,
    pub operator: OperatorRef,
    pub asset_tag: Option<String>,
    pub site_label: Option<String>,
    pub ticket_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Probing,
    Confirming,
    Unfreezing,
    Running,
    Verifying,
    GeneratingCert,
    Signing,
    Completed,
    Failed,
    Aborted,
}

impl JobState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Aborted)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub fraction: f32,
    pub eta_seconds: Option<u64>,
    pub stage: String,
    pub bytes_processed: Option<u64>,
    pub bytes_total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobUpdateKind {
    StateChanged { from: JobState, to: JobState },
    Progress(Progress),
    CommandIssued(CommandEvidence),
    CommandResult(CommandEvidence),
    Verification(VerificationReport),
    Warning { code: String, message: String },
    Failed { reason: String },
}

/// One timestamped record of something that happened during a Job:
/// a state change, a progress tick, a command issued or returned, a
/// verification, a warning, or a failure. Stored as part of a Job's
/// history; wrapped in `wipe_engine::JobUpdateMessage` for broadcast
/// (which adds the `job_id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobUpdate {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub event: JobUpdateKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    pub device_snapshot: Device,
    pub capabilities_snapshot: Capabilities,
    pub spec: JobSpec,
    pub resolved_method: Option<Method>,
    pub state: JobState,
    pub progress: Option<Progress>,
    // Historically named `events`; preserved on the wire. Each element is
    // a `JobUpdate` (formerly `JobEvent`).
    pub events: Vec<JobUpdate>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
    pub verification: Option<VerificationReport>,
    /// Set once a cert has been generated and signed; an external identifier.
    pub certificate_id: Option<String>,
}

impl Job {
    pub fn new(device: Device, caps: Capabilities, spec: JobSpec) -> Self {
        Self {
            id: Uuid::new_v4(),
            device_snapshot: device,
            capabilities_snapshot: caps,
            spec,
            resolved_method: None,
            state: JobState::Queued,
            progress: None,
            events: Vec::new(),
            created_at: OffsetDateTime::now_utc(),
            started_at: None,
            ended_at: None,
            verification: None,
            certificate_id: None,
        }
    }
}
