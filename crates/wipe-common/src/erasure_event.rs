//! ErasureEvent — one attempted wipe within a Job.
//!
//! Per ADR-0001 and CONTEXT.md §5, a `Job` is the outcome-bearing unit
//! that processes an Asset to a terminal disposition. An ErasureEvent
//! is one *attempt* inside that Job; a single Job may contain several
//! (retries, method fallback). Cryptographic instant-purge counts as
//! one ErasureEvent that completes in milliseconds.
//!
//! Before this rename (v0.1), this type was `wipe_common::Job` with
//! state machine `JobState`. The shape is preserved across the rename;
//! only the name changed. The composed-events wrapper lives in
//! `crate::job`.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Capabilities, CommandEvidence, Device, DeviceId, Method, OperatorRef, StationId};

/// What was asked for at the point of starting one wipe attempt.
///
/// In Simple mode, this is what the operator picked in the wizard.
/// In Enterprise mode, this is what the inherited policy resolved to
/// (classification + intent come from the WorkOrder/Contract chain).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErasureEventSpec {
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

/// The inner state machine of one wipe attempt.
///
/// Note the shape change vs v0.1 `JobState`: `Verifying`, `GeneratingCert`,
/// and `Signing` have moved *out* of this state machine. Verification is
/// now a sibling activity on the outer Job (`JobActivity::Verification`),
/// and cert generation/signing happens when the outer Job reaches a
/// terminal disposition. This state machine is purely about one attempt
/// of one wipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ErasureEventState {
    Queued,
    Probing,
    Unfreezing,
    Confirming,
    Running,
    Completed,
    Failed,
    Aborted,
}

impl ErasureEventState {
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
    StateChanged {
        from: ErasureEventState,
        to: ErasureEventState,
    },
    Progress(Progress),
    CommandIssued(CommandEvidence),
    CommandResult(CommandEvidence),
    Warning {
        code: String,
        message: String,
    },
    Failed {
        reason: String,
    },
}

/// One timestamped record from inside a running ErasureEvent.
///
/// `JobUpdate` is the *low-level streamed record* of a running event —
/// state change, progress tick, command issued or returned, warning,
/// failure. Wrapped in `wipe_engine::JobUpdateMessage` for broadcast
/// (which adds the `job_id`/`event_id`). The name is preserved across
/// the rename; the wire format is identical to v0.1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobUpdate {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub event: JobUpdateKind,
}

/// One attempted wipe of one device. Several may exist inside one Job
/// (retries, method fallback). Cross-station processing records the
/// `station_id` here — useful for analysis but not a Job boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErasureEvent {
    pub id: Uuid,
    pub device_snapshot: Device,
    pub capabilities_snapshot: Capabilities,
    pub spec: ErasureEventSpec,
    pub resolved_method: Option<Method>,
    pub state: ErasureEventState,
    pub progress: Option<Progress>,
    /// Low-level update stream from inside this attempt.
    pub events: Vec<JobUpdate>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
    /// Set by the runner when this attempt completes and the station
    /// recording it is known. Useful when one outer Job spans stations.
    pub station_id: Option<StationId>,
}

impl ErasureEvent {
    pub fn new(device: Device, caps: Capabilities, spec: ErasureEventSpec) -> Self {
        Self {
            id: Uuid::new_v4(),
            device_snapshot: device,
            capabilities_snapshot: caps,
            spec,
            resolved_method: None,
            state: ErasureEventState::Queued,
            progress: None,
            events: Vec::new(),
            created_at: OffsetDateTime::now_utc(),
            started_at: None,
            ended_at: None,
            station_id: None,
        }
    }
}
