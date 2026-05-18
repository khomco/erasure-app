use std::{collections::HashMap, sync::Arc, time::Duration};

use futures::Future;
use parking_lot::RwLock;
use time::OffsetDateTime;
use tokio::sync::broadcast;
use tracing::{debug, error, info};
use uuid::Uuid;

use wipe_common::{
    select_method, Job, JobSpec, JobState, JobUpdate, JobUpdateKind, Progress, VerificationMethod,
    WipeError, WipeResult,
};

use crate::{BackendProgress, DynBackend};

/// How frequently to poll a backend handle for progress.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Broadcast envelope: a `JobUpdate` plus the `job_id` it belongs to.
/// Subscribers (WebSocket, persistence, etc.) need the id; the stored
/// `Job::events` list does not because it's implicit. Wire format
/// preserved across the rename of the inner type.
#[derive(Debug, Clone, serde::Serialize)]
pub struct JobUpdateMessage {
    pub job_id: Uuid,
    pub event: JobUpdate,
}

#[derive(Clone)]
pub struct JobRunner {
    backend: DynBackend,
    inner: Arc<RwLock<RunnerInner>>,
    events: broadcast::Sender<JobUpdateMessage>,
}

struct RunnerInner {
    jobs: HashMap<Uuid, Job>,
}

impl JobRunner {
    pub fn new(backend: DynBackend) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            backend,
            inner: Arc::new(RwLock::new(RunnerInner {
                jobs: HashMap::new(),
            })),
            events,
        }
    }

    /// Subscribe to the runner's event stream. Use this to drive a
    /// WebSocket or Tauri event channel.
    pub fn subscribe(&self) -> broadcast::Receiver<JobUpdateMessage> {
        self.events.subscribe()
    }

    pub fn backend(&self) -> DynBackend {
        self.backend.clone()
    }

    pub fn list(&self) -> Vec<Job> {
        self.inner.read().jobs.values().cloned().collect()
    }

    pub fn get(&self, id: Uuid) -> Option<Job> {
        self.inner.read().jobs.get(&id).cloned()
    }

    /// Probe a device and prepare a job (no command issued yet).
    pub async fn create_job(&self, spec: JobSpec) -> WipeResult<Uuid> {
        let device = self
            .backend
            .enumerate()
            .await?
            .into_iter()
            .find(|d| d.id == spec.device_id)
            .ok_or_else(|| WipeError::DeviceNotFound(spec.device_id.to_string()))?;
        let caps = self.backend.capabilities(&spec.device_id).await?;
        let mut job = Job::new(device, caps, spec);
        // Pre-resolve the method using common's selector if the operator
        // didn't pin one. The runner re-validates this against caps later.
        if job.spec.method.is_none() {
            job.resolved_method = select_method(
                &job.capabilities_snapshot,
                job.device_snapshot.media_type,
                job.spec.classification,
                job.spec.intent,
            );
        } else {
            job.resolved_method = job.spec.method.clone();
        }

        let id = job.id;
        self.inner.write().jobs.insert(id, job);
        Ok(id)
    }

    /// Confirm + run a previously-created job.
    pub fn start(&self, id: Uuid) -> WipeResult<()> {
        let job = self
            .get(id)
            .ok_or_else(|| WipeError::InvalidState(format!("job {id} not found")))?;
        if job.state != JobState::Queued {
            return Err(WipeError::InvalidState(format!(
                "job {id} is in state {:?}",
                job.state
            )));
        }
        let runner = self.clone();
        tokio::spawn(async move {
            if let Err(e) = runner.run_job(id).await {
                error!(?e, %id, "job run failed");
                let _ = runner.emit_event(
                    id,
                    JobUpdateKind::Failed {
                        reason: e.to_string(),
                    },
                );
                runner.mutate_job(id, |j| {
                    j.state = JobState::Failed;
                    j.ended_at = Some(OffsetDateTime::now_utc());
                });
            }
        });
        Ok(())
    }

    pub async fn abort(&self, id: Uuid) -> WipeResult<()> {
        let job = self
            .get(id)
            .ok_or_else(|| WipeError::InvalidState(format!("job {id} not found")))?;
        if job.state.is_terminal() {
            return Ok(());
        }
        self.mutate_job(id, |j| {
            j.state = JobState::Aborted;
            j.ended_at = Some(OffsetDateTime::now_utc());
        });
        let _ = self.emit_state_change(id, job.state, JobState::Aborted);
        Ok(())
    }

    fn mutate_job<F: FnOnce(&mut Job)>(&self, id: Uuid, f: F) {
        if let Some(j) = self.inner.write().jobs.get_mut(&id) {
            f(j);
        }
    }

    fn emit_event(&self, job_id: Uuid, kind: JobUpdateKind) -> JobUpdate {
        let ev = JobUpdate {
            at: OffsetDateTime::now_utc(),
            event: kind,
        };
        self.mutate_job(job_id, |j| j.events.push(ev.clone()));
        let _ = self.events.send(JobUpdateMessage {
            job_id,
            event: ev.clone(),
        });
        ev
    }

    fn emit_state_change(&self, job_id: Uuid, from: JobState, to: JobState) -> JobUpdate {
        self.emit_event(job_id, JobUpdateKind::StateChanged { from, to })
    }

    fn transition(&self, job_id: Uuid, to: JobState) {
        let from = self.get(job_id).map(|j| j.state).unwrap_or(JobState::Queued);
        self.mutate_job(job_id, |j| j.state = to);
        let _ = self.emit_state_change(job_id, from, to);
    }

    /// The full job lifecycle. Each transition emits an event.
    async fn run_job(&self, id: Uuid) -> WipeResult<()> {
        let job_snapshot = self.get(id).expect("job exists");
        let method = job_snapshot
            .resolved_method
            .clone()
            .ok_or_else(|| WipeError::MethodUnsupported("no method resolved".into()))?;

        // -- Probing -------------------------------------------------------
        self.transition(id, JobState::Probing);
        self.mutate_job(id, |j| j.started_at = Some(OffsetDateTime::now_utc()));
        debug!(%id, "probing");

        // Re-probe capabilities right before issue, in case device state changed
        // (e.g. another process secured/froze it).
        let caps = self
            .backend
            .capabilities(&job_snapshot.spec.device_id)
            .await?;
        self.mutate_job(id, |j| j.capabilities_snapshot = caps.clone());

        // -- Unfreeze if needed -------------------------------------------
        let needs_unfreeze = caps
            .ata_security
            .as_ref()
            .map(|s| s.frozen)
            .unwrap_or(false);
        if needs_unfreeze {
            self.transition(id, JobState::Unfreezing);
            self.backend.unfreeze(&job_snapshot.spec.device_id).await?;
        }

        // -- Confirming ----------------------------------------------------
        self.transition(id, JobState::Confirming);

        // -- Running -------------------------------------------------------
        self.transition(id, JobState::Running);
        let handle = self
            .backend
            .issue(&job_snapshot.spec.device_id, &method)
            .await?;
        self.emit_event(
            id,
            JobUpdateKind::CommandIssued(handle.issued_evidence.clone()),
        );

        // Poll loop
        loop {
            if matches!(self.get(id).map(|j| j.state), Some(JobState::Aborted)) {
                let _ = self.backend.cancel(&handle).await;
                return Err(WipeError::Aborted);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
            let progress = self.backend.poll(&handle).await?;
            match progress {
                BackendProgress::InProgress {
                    fraction,
                    eta_seconds,
                    bytes_processed,
                    latest_evidence,
                } => {
                    let p = Progress {
                        fraction,
                        eta_seconds,
                        stage: format!("running:{}", method.human_name()),
                        bytes_processed,
                        bytes_total: Some(job_snapshot.device_snapshot.capacity_bytes),
                    };
                    self.mutate_job(id, |j| j.progress = Some(p.clone()));
                    self.emit_event(id, JobUpdateKind::Progress(p));
                    if let Some(ev) = latest_evidence {
                        self.emit_event(id, JobUpdateKind::CommandResult(ev));
                    }
                }
                BackendProgress::Completed { final_evidence } => {
                    self.emit_event(id, JobUpdateKind::CommandResult(final_evidence));
                    break;
                }
                BackendProgress::Failed { evidence, reason } => {
                    self.emit_event(id, JobUpdateKind::CommandResult(evidence));
                    return Err(WipeError::Backend(reason));
                }
            }
        }

        // -- Verifying -----------------------------------------------------
        if job_snapshot.spec.verify {
            self.transition(id, JobState::Verifying);
            let verification_method = pick_verification_method(&method);
            let report = self
                .backend
                .verify(
                    &job_snapshot.spec.device_id,
                    verification_method,
                    job_snapshot.spec.verify_samples.max(1),
                )
                .await?;
            self.emit_event(id, JobUpdateKind::Verification(report.clone()));
            self.mutate_job(id, |j| j.verification = Some(report.clone()));
            if !report.all_passed {
                return Err(WipeError::VerificationFailed(format!(
                    "{} of {} samples failed",
                    report.samples.iter().filter(|s| !s.passed).count(),
                    report.samples.len()
                )));
            }
        }

        // -- Cert generation handled by caller; runner just marks state ----
        self.transition(id, JobState::GeneratingCert);
        self.transition(id, JobState::Signing);
        self.transition(id, JobState::Completed);
        self.mutate_job(id, |j| j.ended_at = Some(OffsetDateTime::now_utc()));
        info!(%id, "job complete");
        Ok(())
    }
}

fn pick_verification_method(method: &wipe_common::Method) -> VerificationMethod {
    use wipe_common::Method::*;
    match method {
        NvmeSanitizeCryptoErase { .. } | AtaSecureErase { enhanced: true } => {
            VerificationMethod::SampledEntropy
        }
        BlockOverwrite { .. }
        | NvmeSanitizeOverwrite { .. }
        | AtaSecureErase { enhanced: false } => VerificationMethod::SampledPattern,
        NvmeSanitizeBlockErase { .. } => VerificationMethod::SampledPattern,
        OpalRevert => VerificationMethod::SampledEntropy,
        Destroy { .. } => VerificationMethod::SampledPattern,
    }
}

/// Helper for callers that want to await a job's completion without
/// subscribing to the event channel directly.
pub async fn wait_for_terminal(
    runner: JobRunner,
    id: Uuid,
    timeout: Duration,
) -> impl Future<Output = WipeResult<Job>> {
    let deadline = tokio::time::Instant::now() + timeout;
    async move {
        loop {
            match runner.get(id) {
                Some(j) if j.state.is_terminal() => return Ok(j),
                Some(_) => {}
                None => {
                    return Err(WipeError::InvalidState(format!(
                        "job {id} disappeared while waiting"
                    )))
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(WipeError::InvalidState("timeout waiting for job".into()));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
