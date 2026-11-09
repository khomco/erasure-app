//! Job orchestration — outer `Job` lifecycle composed of typed activities.
//!
//! Per ADR-0001, a Job composes one or more typed events (Diagnostic,
//! ErasureEvent, Verification, Destruction) and reaches a terminal
//! disposition (Erased / Destroyed / Quarantined / Aborted). One signed
//! Certificate covers the full chain.
//!
//! The runner has two levels:
//!  * `JobRunner` — outer orchestrator. Owns the outer-Job state machine
//!    and decides when to emit each typed activity. The server signs the
//!    one certificate on the `JobStateChanged` into Erased, or into
//!    Destroyed on the Destroy path (ADR-0011 §2); entering
//!    PendingDestruction signs nothing. A Job enters PendingDestruction
//!    only through `record_for_destruction` /
//!    `record_scanned_for_destruction`, each carrying the operator's
//!    `DestructionAttestation` (ADR-0011 §1), and leaves it for Destroyed
//!    only through `mark_destroyed`, once its destruction run is completed
//!    (and concurred with, when required).
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
    ErasureEventState, Job, JobActivity, JobSpec, JobState, JobUpdate, JobUpdateKind, Progress,
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

    /// Begin processing a queued Job. Moves it `Queued` → `InProgress`
    /// (check and transition under one write guard, so a second `start`
    /// is refused), broadcasts that change, then spawns an async task that
    /// drives the Job through one ErasureEvent (with verification if
    /// requested) to a terminal disposition. If the Job has left
    /// `InProgress` (aborted or recorded for destruction) before the task begins, the task
    /// does nothing.
    ///
    /// Refused with `JobNotFound` for an unknown id and `InvalidState` if
    /// the Job is not `Queued` (already started, recorded for destruction, aborted or
    /// finished).
    ///
    /// Retries and recording for destruction are explicit operator actions
    /// in v0.2 (`retry_erasure`, `record_for_destruction`) — automatic
    /// policy is deferred to SanitizationProfile in v0.2 #3.
    pub fn start(&self, id: Uuid) -> WipeResult<()> {
        {
            let mut inner = self.inner.write();
            let job = inner.jobs.get_mut(&id).ok_or(WipeError::JobNotFound(id))?;
            if job.state != wipe_common::JobState::Queued {
                return Err(WipeError::InvalidState(format!(
                    "job {id} is in state {:?}",
                    job.state
                )));
            }
            job.state = wipe_common::JobState::InProgress;
            job.started_at = Some(OffsetDateTime::now_utc());
        }
        let _ = self.events.send(JobBroadcast::JobStateChanged {
            job_id: id,
            from: wipe_common::JobState::Queued,
            to: wipe_common::JobState::InProgress,
            at: OffsetDateTime::now_utc(),
        });
        let runner = self.clone();
        tokio::spawn(async move {
            if let Err(e) = runner.run_job(id).await {
                // Inner ErasureEvent failed. The outer Job stays
                // InProgress so the operator can decide next step
                // (retry, record for destruction, or abort). The failure
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
    ///
    /// Refused with `JobNotFound` for an unknown id and `InvalidState` for
    /// a Job in any terminal state other than `Aborted` (`Erased`,
    /// `Destroyed`, `Quarantined`). Aborting an already `Aborted` Job is an `Ok` no-op
    /// with no broadcast, so a retried abort succeeds.
    pub async fn abort(&self, id: Uuid) -> WipeResult<()> {
        let from = {
            let mut inner = self.inner.write();
            let job = inner.jobs.get_mut(&id).ok_or(WipeError::JobNotFound(id))?;
            if job.state == wipe_common::JobState::Aborted {
                return Ok(());
            }
            if job.state.is_terminal() {
                return Err(WipeError::InvalidState(format!(
                    "job {id} already terminal (state {:?})",
                    job.state
                )));
            }
            let from = job.state;
            job.state = wipe_common::JobState::Aborted;
            job.ended_at = Some(OffsetDateTime::now_utc());
            from
        };
        let _ = self.events.send(JobBroadcast::JobStateChanged {
            job_id: id,
            from,
            to: wipe_common::JobState::Aborted,
            at: OffsetDateTime::now_utc(),
        });
        Ok(())
    }

    /// Record a Job for destruction (ADR-0011 §1, first step): append the
    /// operator's `DestructionEvent`, which must carry a
    /// `DestructionAttestation`, and move the Job to `PendingDestruction`.
    /// Refused with `JobNotFound` for an unknown id, and `InvalidState` if
    /// the event has no attestation or the Job is terminal or already on a
    /// DestructionManifest. A pending Job not yet on a manifest may be
    /// recorded again; the latest DestructionEvent is the one that counts.
    pub fn record_for_destruction(
        &self,
        id: Uuid,
        destruction: DestructionEvent,
    ) -> WipeResult<()> {
        // Check and mutate under one write guard so a concurrent
        // `link_to_manifest` cannot slip between them.
        let from = {
            let mut inner = self.inner.write();
            let job = inner.jobs.get_mut(&id).ok_or(WipeError::JobNotFound(id))?;
            if destruction.attestation.is_none() {
                return Err(WipeError::InvalidState(format!(
                    "job {id}: a destruction record needs the operator's attestation"
                )));
            }
            if job.state.is_terminal() {
                return Err(WipeError::InvalidState(format!(
                    "job {id} already terminal"
                )));
            }
            // Once a Job is on a DestructionManifest, its record must not
            // change: a new DestructionEvent would not carry the manifest
            // reference the certificate signs at Destroyed.
            if let Some(mid) = job.manifest_id {
                return Err(WipeError::InvalidState(format!(
                    "job {id} is on destruction manifest {mid}; it cannot be recorded again"
                )));
            }
            let from = job.state;
            job.activities
                .push(JobActivity::Destruction(destruction.clone()));
            job.state = JobState::PendingDestruction;
            from
        };
        self.broadcast_recorded(id, from, destruction);
        Ok(())
    }

    /// Record a scanned group of drives for destruction, one new Job per
    /// drive, each created directly in `PendingDestruction` with its
    /// attested `DestructionEvent`. Used for drives that never enumerate
    /// (dead drives) or that are meant for destruction from the start; the
    /// `JobSpec::device_id` is the caller's synthetic `scanned:…` id and is
    /// not checked against the backend.
    ///
    /// All-or-nothing, refused with `InvalidState` (creating nothing) when
    /// the list is empty, any event lacks an attestation, two drives share
    /// an asset tag or serial, or a drive matches the asset tag or serial of
    /// a non-terminal Job. That last message names the Job so the operator
    /// can record from it instead. An open Job's serial is its latest
    /// erasure snapshot's, else the enumerated device's (read best-effort),
    /// plus any identifiers already recorded on its destruction events.
    pub async fn record_scanned_for_destruction(
        &self,
        drives: Vec<(JobSpec, DestructionEvent)>,
    ) -> WipeResult<Vec<Uuid>> {
        if drives.is_empty() {
            return Err(WipeError::InvalidState(
                "no drives to record for destruction".into(),
            ));
        }
        let mut wanted: Vec<(Option<String>, Option<String>)> = Vec::new();
        for (spec, ev) in &drives {
            let Some(att) = &ev.attestation else {
                return Err(WipeError::InvalidState(
                    "a destruction record needs the operator's attestation".into(),
                ));
            };
            let tag = nonblank(att.identifiers.asset_tag.as_deref())
                .or_else(|| nonblank(spec.asset_tag.as_deref()));
            let serial = nonblank(att.identifiers.serial.as_deref());
            for (other_tag, other_serial) in &wanted {
                if same(&tag, other_tag) || same(&serial, other_serial) {
                    return Err(WipeError::InvalidState(format!(
                        "drive {} is listed twice",
                        describe(&tag, &serial)
                    )));
                }
            }
            wanted.push((tag, serial));
        }

        // Best-effort: a backend that cannot enumerate must not block
        // recording dead drives; erasure snapshots and recorded identifiers
        // still guard against duplicates.
        let attached: HashMap<wipe_common::DeviceId, String> = match self.backend.enumerate().await
        {
            Ok(devices) => devices.into_iter().map(|d| (d.id, d.serial)).collect(),
            Err(e) => {
                debug!(
                    ?e,
                    "enumeration failed; open-Job serials come from snapshots only"
                );
                HashMap::new()
            }
        };

        let now = OffsetDateTime::now_utc();
        let ids = {
            let mut inner = self.inner.write();
            for job in inner.jobs.values().filter(|j| !j.state.is_terminal()) {
                let (tags, serials) = open_job_identifiers(job, &attached);
                for (tag, serial) in &wanted {
                    let hit = tag
                        .as_ref()
                        .is_some_and(|t| tags.iter().any(|o| o.eq_ignore_ascii_case(t)))
                        || serial
                            .as_ref()
                            .is_some_and(|s| serials.iter().any(|o| o.eq_ignore_ascii_case(s)));
                    if hit {
                        return Err(WipeError::InvalidState(format!(
                            "drive {} matches open job {}; record it for destruction from \
                             that job instead",
                            describe(tag, serial),
                            job.id
                        )));
                    }
                }
            }
            let mut ids = Vec::with_capacity(drives.len());
            for (spec, ev) in &drives {
                let mut job = Job::new(spec.clone());
                job.created_at = now;
                job.state = JobState::PendingDestruction;
                job.activities.push(JobActivity::Destruction(ev.clone()));
                ids.push(job.id);
                inner.jobs.insert(job.id, job);
            }
            ids
        };
        for (id, (_, ev)) in ids.iter().zip(drives) {
            self.broadcast_recorded(*id, JobState::Queued, ev);
        }
        Ok(ids)
    }

    /// Every entry into `PendingDestruction` announces the attested
    /// DestructionEvent and the state change.
    fn broadcast_recorded(&self, id: Uuid, from: JobState, destruction: DestructionEvent) {
        let _ = self.events.send(JobBroadcast::ActivityAdded {
            job_id: id,
            activity: JobActivity::Destruction(destruction),
        });
        let _ = self.events.send(JobBroadcast::JobStateChanged {
            job_id: id,
            from,
            to: JobState::PendingDestruction,
            at: OffsetDateTime::now_utc(),
        });
    }

    /// Move every Job of a completed destruction run from
    /// `PendingDestruction` to `Destroyed`. Called once the run's completion
    /// (and any required concurrence) is recorded (ADR-0011).
    ///
    /// All-or-nothing: every Job is validated under one write lock before
    /// any is changed, and the call refuses (changing nothing, broadcasting
    /// nothing) if any Job is missing, listed more than once or not in
    /// `PendingDestruction`. Each moved Job then gets its own
    /// `JobStateChanged`.
    pub fn mark_destroyed(&self, ids: &[Uuid]) -> WipeResult<()> {
        let now = OffsetDateTime::now_utc();
        {
            let mut inner = self.inner.write();
            for (i, id) in ids.iter().enumerate() {
                if ids[..i].contains(id) {
                    return Err(WipeError::InvalidState(format!("job {id} is listed twice")));
                }
                let job = inner.jobs.get(id).ok_or(WipeError::JobNotFound(*id))?;
                if job.state != JobState::PendingDestruction {
                    return Err(WipeError::InvalidState(format!(
                        "job {id} not in PendingDestruction (got {:?})",
                        job.state
                    )));
                }
            }
            for id in ids {
                let job = inner.jobs.get_mut(id).expect("validated above");
                job.state = JobState::Destroyed;
                job.ended_at = Some(now);
            }
        }
        for id in ids {
            let _ = self.events.send(JobBroadcast::JobStateChanged {
                job_id: *id,
                from: JobState::PendingDestruction,
                to: JobState::Destroyed,
                at: now,
            });
        }
        Ok(())
    }

    /// Record that `job_ids` were assembled onto DestructionManifest
    /// `manifest_id`: stamps `Job::manifest_id` and the `manifest_ref` of
    /// each Job's latest DestructionEvent.
    ///
    /// All-or-nothing: every Job is validated under one write lock before
    /// any is changed, and the call refuses (changing nothing) if any Job is
    /// missing, listed more than once, not in `PendingDestruction`, already on a
    /// manifest, or has no DestructionEvent. The link changes no Job state
    /// and emits no broadcast; the certificate signed at `Destroyed`
    /// carries the link.
    pub fn link_to_manifest(&self, manifest_id: Uuid, job_ids: &[Uuid]) -> WipeResult<()> {
        let mut inner = self.inner.write();
        for (i, id) in job_ids.iter().enumerate() {
            if job_ids[..i].contains(id) {
                return Err(WipeError::InvalidState(format!("job {id} is listed twice")));
            }
            let job = inner.jobs.get(id).ok_or(WipeError::JobNotFound(*id))?;
            if job.state != JobState::PendingDestruction {
                return Err(WipeError::InvalidState(format!(
                    "job {id} not in PendingDestruction (got {:?})",
                    job.state
                )));
            }
            if let Some(other) = job.manifest_id {
                return Err(WipeError::InvalidState(format!(
                    "job {id} is already on destruction manifest {other}"
                )));
            }
            if !job
                .activities
                .iter()
                .any(|a| matches!(a, JobActivity::Destruction(_)))
            {
                return Err(WipeError::InvalidState(format!(
                    "job {id} has no destruction event"
                )));
            }
        }
        for id in job_ids {
            let job = inner.jobs.get_mut(id).expect("validated above");
            job.manifest_id = Some(manifest_id);
            if let Some(JobActivity::Destruction(ev)) = job
                .activities
                .iter_mut()
                .rev()
                .find(|a| matches!(a, JobActivity::Destruction(_)))
            {
                ev.manifest_ref = Some(manifest_id);
            }
        }
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

    /// Drive a Job that `start` has already moved to `InProgress` through
    /// one ErasureEvent + Verification to `Erased` (happy path). On
    /// verification failure or wipe failure, the inner ErasureEvent
    /// terminates as `Failed` and the outer Job stays `InProgress` — the
    /// operator can call `retry_erasure` or `record_for_destruction` to drive
    /// it further.
    async fn run_job(&self, id: Uuid) -> WipeResult<()> {
        let job = self.get(id).ok_or(WipeError::JobNotFound(id))?;
        // An `abort` or `record_for_destruction` may land between `start`
        // returning and this task running; those take precedence, so no
        // erasure is attempted.
        if job.state != wipe_common::JobState::InProgress {
            debug!(%id, state = ?job.state, "job left InProgress before its task ran; not erasing");
            return Ok(());
        }

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
                // `abort` or `record_for_destruction` while the inner
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
                // InProgress so the operator can decide: retry, record
                // for destruction, or abort.
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

fn nonblank(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

fn same(a: &Option<String>, b: &Option<String>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b))
}

fn describe(tag: &Option<String>, serial: &Option<String>) -> String {
    match (tag, serial) {
        (Some(t), Some(s)) => format!("with asset tag {t} and serial {s}"),
        (Some(t), None) => format!("with asset tag {t}"),
        (None, Some(s)) => format!("with serial {s}"),
        (None, None) => "with no identifiers".into(),
    }
}

/// The asset tags and serials an open Job already stands for: its spec's
/// asset tag, its device serial (latest erasure snapshot, else the attached
/// device), and every identifier recorded on its destruction events.
fn open_job_identifiers(
    job: &Job,
    attached: &HashMap<wipe_common::DeviceId, String>,
) -> (Vec<String>, Vec<String>) {
    let mut tags: Vec<String> = nonblank(job.spec.asset_tag.as_deref())
        .into_iter()
        .collect();
    let device_serial = job
        .latest_erasure()
        .and_then(|e| nonblank(Some(&e.device_snapshot.serial)))
        .or_else(|| nonblank(attached.get(&job.spec.device_id).map(String::as_str)));
    let mut serials: Vec<String> = device_serial.into_iter().collect();
    for a in &job.activities {
        if let JobActivity::Destruction(DestructionEvent {
            attestation: Some(att),
            ..
        }) = a
        {
            tags.extend(nonblank(att.identifiers.asset_tag.as_deref()));
            serials.extend(nonblank(att.identifiers.serial.as_deref()));
        }
    }
    (tags, serials)
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
