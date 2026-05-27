//! Job orchestration — outer `Job` lifecycle composed of typed activities.
//!
//! Per ADR-0001, a Job composes one or more typed events (Diagnostic,
//! ErasureEvent, Verification, Destruction) and reaches a terminal
//! disposition (Erased / Destroyed / Quarantined / Aborted). One signed
//! Certificate covers the full chain.
//!
//! The runner has two levels:
//!  * `JobRunner` — outer orchestrator. Owns the outer-Job state machine
//!    and decides when to emit each typed activity. Cert generation +
//!    signing happens when the Job reaches Erased; for Destroyed, the
//!    cert is generated at PendingCoSign and the supervisor co-signature
//!    is attached when the manifest is signed.
//!  * inner `run_erasure_event` — drives one ErasureEvent through its
//!    inner state machine (Queued → Probing → … → Completed/Failed).
//!    Today's v0.1 `run_job` logic, minus the cert/sign tail.

use std::{collections::HashMap, sync::Arc, time::Duration};

use parking_lot::RwLock;
use time::OffsetDateTime;
use tokio::sync::broadcast;
use tracing::{debug, info};
use uuid::Uuid;

use wipe_common::{
    select_method, AssetDisposition, DestructionEvent, ErasureEvent, ErasureEventSpec,
    ErasureEventState, Job, JobActivity, JobSpec, JobUpdate, JobUpdateKind, Progress,
    VerificationEvent, VerificationMethod, VerificationReport, WipeError, WipeResult,
};

use crate::{BackendProgress, DynBackend};

/// How frequently to poll a backend handle for progress.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Broadcast events from a running Job. The wire format is the same
/// channel the WebSocket fans out; the variant distinguishes outer-Job
/// state changes from inner ErasureEvent updates and activity additions.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobBroadcast {
    /// Outer Job state changed.
    JobStateChanged {
        job_id: Uuid,
        from: wipe_common::JobState,
        to: wipe_common::JobState,
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
    },
    /// A new typed activity was appended to the Job's chain.
    ActivityAdded {
        job_id: Uuid,
        activity: JobActivity,
    },
    /// Low-level update from inside a running ErasureEvent.
    ErasureUpdate {
        job_id: Uuid,
        erasure_event_id: Uuid,
        update: JobUpdate,
    },
}

#[derive(Clone)]
pub struct JobRunner {
    backend: DynBackend,
    inner: Arc<RwLock<RunnerInner>>,
    events: broadcast::Sender<JobBroadcast>,
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

    /// Subscribe to the runner's broadcast stream.
    pub fn subscribe(&self) -> broadcast::Receiver<JobBroadcast> {
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

    /// Create a Job from a spec. The Job starts in `Queued`; no work has
    /// been done yet. Returns the Job's id.
    pub async fn create_job(&self, spec: JobSpec) -> WipeResult<Uuid> {
        // Validate the device exists at create time so we fail fast.
        self.backend
            .enumerate()
            .await?
            .into_iter()
            .find(|d| d.id == spec.device_id)
            .ok_or_else(|| WipeError::DeviceNotFound(spec.device_id.to_string()))?;
        let job = Job::new(spec);
        let id = job.id;
        self.inner.write().jobs.insert(id, job);
        Ok(id)
    }

    /// Begin processing a queued Job. Spawns an async task that drives
    /// the Job through one ErasureEvent (with verification if requested)
    /// to a terminal disposition.
    ///
    /// Retries and escalation-to-destroy are explicit operator actions
    /// in v0.2 (`retry_erasure`, `escalate_to_destroy`) — automatic
    /// policy is deferred to SanitizationProfile in v0.2 #3.
    pub fn start(&self, id: Uuid) -> WipeResult<()> {
        let job = self
            .get(id)
            .ok_or_else(|| WipeError::InvalidState(format!("job {id} not found")))?;
        if job.state != wipe_common::JobState::Queued {
            return Err(WipeError::InvalidState(format!(
                "job {id} is in state {:?}",
                job.state
            )));
        }
        let runner = self.clone();
        tokio::spawn(async move {
            if let Err(e) = runner.run_job(id).await {
                // Inner ErasureEvent failed. The outer Job stays
                // InProgress so the operator can decide next step
                // (retry, escalate to destroy, or abort). The failure
                // is visible on the latest ErasureEvent's state and
                // event log.
                debug!(?e, %id, "job run returned error; outer Job awaits operator action");
            }
        });
        Ok(())
    }

    /// Mark an in-flight Job as Aborted. Active ErasureEvent (if any) is
    /// not torn down — the running task will observe the state change
    /// and exit.
    pub async fn abort(&self, id: Uuid) -> WipeResult<()> {
        let job = self
            .get(id)
            .ok_or_else(|| WipeError::InvalidState(format!("job {id} not found")))?;
        if job.state.is_terminal() {
            return Ok(());
        }
        let from = job.state;
        self.mutate_job(id, |j| {
            j.state = wipe_common::JobState::Aborted;
            j.ended_at = Some(OffsetDateTime::now_utc());
        });
        let _ = self.events.send(JobBroadcast::JobStateChanged {
            job_id: id,
            from,
            to: wipe_common::JobState::Aborted,
            at: OffsetDateTime::now_utc(),
        });
        Ok(())
    }

    /// Append an operator-supplied `DestructionEvent` to a Job and move
    /// it to `PendingCoSign`. Used when erasure is exhausted and the
    /// Asset must be physically destroyed; the Job awaits supervisor
    /// co-sign on the linked `DestructionManifest`.
    pub fn escalate_to_destroy(
        &self,
        id: Uuid,
        destruction: DestructionEvent,
    ) -> WipeResult<()> {
        let job = self
            .get(id)
            .ok_or_else(|| WipeError::InvalidState(format!("job {id} not found")))?;
        if job.state.is_terminal() {
            return Err(WipeError::InvalidState(format!(
                "job {id} already terminal"
            )));
        }
        let from = job.state;
        self.mutate_job(id, |j| {
            j.activities
                .push(JobActivity::Destruction(destruction.clone()));
            j.state = wipe_common::JobState::PendingCoSign;
        });
        let _ = self.events.send(JobBroadcast::ActivityAdded {
            job_id: id,
            activity: JobActivity::Destruction(destruction),
        });
        let _ = self.events.send(JobBroadcast::JobStateChanged {
            job_id: id,
            from,
            to: wipe_common::JobState::PendingCoSign,
            at: OffsetDateTime::now_utc(),
        });
        Ok(())
    }

    /// Move a `PendingCoSign` Job to `Destroyed`. Called by the manifest
    /// signing flow once the supervisor has co-signed the manifest
    /// (and, transitively, every cert it groups).
    pub fn mark_destroyed(&self, id: Uuid) -> WipeResult<()> {
        let job = self
            .get(id)
            .ok_or_else(|| WipeError::InvalidState(format!("job {id} not found")))?;
        if job.state != wipe_common::JobState::PendingCoSign {
            return Err(WipeError::InvalidState(format!(
                "job {id} not in PendingCoSign (got {:?})",
                job.state
            )));
        }
        self.mutate_job(id, |j| {
            j.state = wipe_common::JobState::Destroyed;
            j.ended_at = Some(OffsetDateTime::now_utc());
        });
        let _ = self.events.send(JobBroadcast::JobStateChanged {
            job_id: id,
            from: wipe_common::JobState::PendingCoSign,
            to: wipe_common::JobState::Destroyed,
            at: OffsetDateTime::now_utc(),
        });
        Ok(())
    }

    fn mutate_job<F: FnOnce(&mut Job)>(&self, id: Uuid, f: F) {
        if let Some(j) = self.inner.write().jobs.get_mut(&id) {
            f(j);
        }
    }

    fn append_activity(&self, job_id: Uuid, activity: JobActivity) {
        self.mutate_job(job_id, |j| j.activities.push(activity.clone()));
        let _ = self.events.send(JobBroadcast::ActivityAdded { job_id, activity });
    }

    fn emit_erasure_update(
        &self,
        job_id: Uuid,
        erasure_event_id: Uuid,
        kind: JobUpdateKind,
    ) -> JobUpdate {
        let update = JobUpdate {
            at: OffsetDateTime::now_utc(),
            event: kind,
        };
        // Mutate the ErasureEvent inside the Job's activities.
        self.mutate_job(job_id, |j| {
            if let Some(JobActivity::Erasure(ev)) =
                j.activities.iter_mut().rev().find(|a| matches!(a, JobActivity::Erasure(e) if e.id == erasure_event_id))
            {
                ev.events.push(update.clone());
            }
        });
        let _ = self.events.send(JobBroadcast::ErasureUpdate {
            job_id,
            erasure_event_id,
            update: update.clone(),
        });
        update
    }

    fn mutate_erasure<F: FnOnce(&mut ErasureEvent)>(
        &self,
        job_id: Uuid,
        erasure_event_id: Uuid,
        f: F,
    ) {
        self.mutate_job(job_id, |j| {
            for a in j.activities.iter_mut().rev() {
                if let JobActivity::Erasure(ev) = a {
                    if ev.id == erasure_event_id {
                        f(ev);
                        return;
                    }
                }
            }
        });
    }

    /// Drive a Job from `Queued` through one ErasureEvent + Verification
    /// to `Erased` (happy path). On verification failure or wipe failure,
    /// the inner ErasureEvent terminates as `Failed` and the outer Job
    /// stays `InProgress` — the operator can call `retry_erasure` or
    /// `escalate_to_destroy` to drive it further.
    async fn run_job(&self, id: Uuid) -> WipeResult<()> {
        let job = self
            .get(id)
            .ok_or_else(|| WipeError::InvalidState(format!("job {id} not found")))?;

        // Outer state: Queued → InProgress.
        self.mutate_job(id, |j| {
            j.state = wipe_common::JobState::InProgress;
            j.started_at = Some(OffsetDateTime::now_utc());
        });
        let _ = self.events.send(JobBroadcast::JobStateChanged {
            job_id: id,
            from: wipe_common::JobState::Queued,
            to: wipe_common::JobState::InProgress,
            at: OffsetDateTime::now_utc(),
        });

        // Build the spec for the first ErasureEvent from the outer JobSpec.
        let erasure_spec = ErasureEventSpec {
            device_id: job.spec.device_id.clone(),
            classification: job.spec.classification,
            intent: job.spec.intent,
            method: None,
            verify: true,
            verify_samples: 8,
            operator: job.spec.operator.clone(),
            asset_tag: job.spec.asset_tag.clone(),
            site_label: job.spec.site_label.clone(),
            ticket_ref: job.spec.ticket_ref.clone(),
        };

        match self.run_erasure_event(id, erasure_spec).await {
            Ok(()) => {
                // Erased — but only if the outer Job hasn't been moved
                // out from under us. The operator may have called
                // `abort` or `escalate_to_destroy` while the inner
                // ErasureEvent was running; in either case those
                // explicit transitions take precedence over the
                // implicit success path.
                let current = self.get(id).map(|j| j.state);
                if current == Some(wipe_common::JobState::InProgress) {
                    self.mutate_job(id, |j| {
                        j.state = wipe_common::JobState::Erased;
                        j.ended_at = Some(OffsetDateTime::now_utc());
                    });
                    let _ = self.events.send(JobBroadcast::JobStateChanged {
                        job_id: id,
                        from: wipe_common::JobState::InProgress,
                        to: wipe_common::JobState::Erased,
                        at: OffsetDateTime::now_utc(),
                    });
                    info!(%id, "job erased");
                } else {
                    debug!(%id, ?current, "erasure succeeded but outer Job already transitioned");
                }
                Ok(())
            }
            Err(e) => {
                // Inner ErasureEvent failed. The outer Job stays
                // InProgress so the operator can decide: retry, escalate
                // to destroy, or abort.
                debug!(?e, %id, "erasure failed; awaiting operator action");
                Err(e)
            }
        }
    }

    /// Inner ErasureEvent driver. Creates and runs one wipe attempt
    /// through its inner state machine, captures command evidence, runs
    /// verification on success, and appends a `VerificationEvent`
    /// activity to the Job.
    async fn run_erasure_event(
        &self,
        job_id: Uuid,
        spec: ErasureEventSpec,
    ) -> WipeResult<()> {
        // Snapshot the device + capabilities.
        let device = self
            .backend
            .enumerate()
            .await?
            .into_iter()
            .find(|d| d.id == spec.device_id)
            .ok_or_else(|| WipeError::DeviceNotFound(spec.device_id.to_string()))?;
        let caps = self.backend.capabilities(&spec.device_id).await?;

        let mut erasure = ErasureEvent::new(device.clone(), caps.clone(), spec.clone());
        erasure.resolved_method = if let Some(m) = &spec.method {
            Some(m.clone())
        } else {
            select_method(&caps, device.media_type, spec.classification, spec.intent)
        };
        let method = erasure
            .resolved_method
            .clone()
            .ok_or_else(|| WipeError::MethodUnsupported("no method resolved".into()))?;
        let erasure_id = erasure.id;
        erasure.started_at = Some(OffsetDateTime::now_utc());

        // Append the ErasureEvent activity in Probing state.
        erasure.state = ErasureEventState::Probing;
        self.append_activity(job_id, JobActivity::Erasure(erasure));

        let transition_inner = |to: ErasureEventState| {
            let prev = self
                .get(job_id)
                .and_then(|j| {
                    j.activities.iter().rev().find_map(|a| match a {
                        JobActivity::Erasure(e) if e.id == erasure_id => Some(e.state),
                        _ => None,
                    })
                })
                .unwrap_or(ErasureEventState::Queued);
            self.mutate_erasure(job_id, erasure_id, |e| e.state = to);
            let _ = self.emit_erasure_update(
                job_id,
                erasure_id,
                JobUpdateKind::StateChanged { from: prev, to },
            );
        };

        // Re-probe capabilities right before issue.
        let caps_live = self.backend.capabilities(&spec.device_id).await?;
        self.mutate_erasure(job_id, erasure_id, |e| {
            e.capabilities_snapshot = caps_live.clone()
        });

        // Unfreeze if needed.
        let needs_unfreeze = caps_live
            .ata_security
            .as_ref()
            .map(|s| s.frozen)
            .unwrap_or(false);
        if needs_unfreeze {
            transition_inner(ErasureEventState::Unfreezing);
            self.backend.unfreeze(&spec.device_id).await?;
        }

        transition_inner(ErasureEventState::Confirming);
        transition_inner(ErasureEventState::Running);

        let handle = self.backend.issue(&spec.device_id, &method).await?;
        self.emit_erasure_update(
            job_id,
            erasure_id,
            JobUpdateKind::CommandIssued(handle.issued_evidence.clone()),
        );

        // Poll loop.
        loop {
            // Honour an outer-Job abort.
            if matches!(
                self.get(job_id).map(|j| j.state),
                Some(wipe_common::JobState::Aborted)
            ) {
                let _ = self.backend.cancel(&handle).await;
                self.mutate_erasure(job_id, erasure_id, |e| {
                    e.state = ErasureEventState::Aborted;
                    e.ended_at = Some(OffsetDateTime::now_utc());
                });
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
                        bytes_total: Some(device.capacity_bytes),
                    };
                    self.mutate_erasure(job_id, erasure_id, |e| e.progress = Some(p.clone()));
                    self.emit_erasure_update(job_id, erasure_id, JobUpdateKind::Progress(p));
                    if let Some(ev) = latest_evidence {
                        self.emit_erasure_update(
                            job_id,
                            erasure_id,
                            JobUpdateKind::CommandResult(ev),
                        );
                    }
                }
                BackendProgress::Completed { final_evidence } => {
                    self.emit_erasure_update(
                        job_id,
                        erasure_id,
                        JobUpdateKind::CommandResult(final_evidence),
                    );
                    break;
                }
                BackendProgress::Failed { evidence, reason } => {
                    self.emit_erasure_update(
                        job_id,
                        erasure_id,
                        JobUpdateKind::CommandResult(evidence),
                    );
                    self.mutate_erasure(job_id, erasure_id, |e| {
                        e.state = ErasureEventState::Failed;
                        e.ended_at = Some(OffsetDateTime::now_utc());
                    });
                    return Err(WipeError::Backend(reason));
                }
            }
        }

        // Inner ErasureEvent reaches Completed.
        self.mutate_erasure(job_id, erasure_id, |e| {
            e.state = ErasureEventState::Completed;
            e.ended_at = Some(OffsetDateTime::now_utc());
        });
        let _ = self.emit_erasure_update(
            job_id,
            erasure_id,
            JobUpdateKind::StateChanged {
                from: ErasureEventState::Running,
                to: ErasureEventState::Completed,
            },
        );

        // Verification — sibling activity on the outer Job, not an inner
        // state. Only emitted when the spec asked for verification.
        if spec.verify {
            let verification_method = pick_verification_method(&method);
            let report: VerificationReport = self
                .backend
                .verify(
                    &spec.device_id,
                    verification_method,
                    spec.verify_samples.max(1),
                )
                .await?;
            let ver = VerificationEvent {
                id: Uuid::new_v4(),
                erasure_event_id: erasure_id,
                device_id: spec.device_id.clone(),
                at: OffsetDateTime::now_utc(),
                report: report.clone(),
                station_id: None,
            };
            self.append_activity(job_id, JobActivity::Verification(ver));
            if !report.all_passed {
                return Err(WipeError::VerificationFailed(format!(
                    "{} of {} samples failed",
                    report.samples.iter().filter(|s| !s.passed).count(),
                    report.samples.len()
                )));
            }
        }

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

/// Convenience: poll the runner until the given Job reaches a terminal
/// disposition, or `timeout` elapses. Used by tests and CLI verifiers.
pub async fn wait_for_terminal(
    runner: JobRunner,
    id: Uuid,
    timeout: Duration,
) -> WipeResult<Job> {
    let deadline = tokio::time::Instant::now() + timeout;
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

/// Convenience: wait for a Job to reach `Erased`. Returns the disposition
/// for the caller's convenience.
pub async fn wait_for_erased(
    runner: JobRunner,
    id: Uuid,
    timeout: Duration,
) -> WipeResult<AssetDisposition> {
    let job = wait_for_terminal(runner, id, timeout).await?;
    job.state
        .disposition()
        .ok_or_else(|| WipeError::InvalidState(format!("job {id} terminal but no disposition")))
}
