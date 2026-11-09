//! End-to-end tests for the runner driving the mock backend through
//! the new outer-Job model (ADR-0001).

use std::sync::Arc;
use std::time::Duration;

use wipe_common::{
    Classification, DeviceId, ErasureEventState, Intent, Job, JobActivity, JobSpec, JobState,
    JobUpdateKind, OperatorRef,
};
use wipe_engine::{JobBroadcast, JobRunner};
use wipe_engine_mock::{MockBackend, MockTiming};

fn op() -> OperatorRef {
    OperatorRef {
        id: "op-1".into(),
        display_name: "Alice Erasure".into(),
        email: "alice@example.com".into(),
    }
}

fn job_spec(device_id: &str) -> JobSpec {
    JobSpec {
        device_id: DeviceId(device_id.into()),
        classification: Classification::High,
        intent: Intent::Reuse,
        operator: op(),
        asset_tag: Some("ASSET-001".into()),
        site_label: Some("Lab A".into()),
        ticket_ref: Some("TKT-42".into()),
        work_order_ref: None,
        customer_ref: None,
        contract_ref: None,
        sanitization_profile_ref: None,
    }
}

async fn wait_until<F: Fn(&Job) -> bool>(runner: &JobRunner, id: uuid::Uuid, pred: F) -> Job {
    for _ in 0..400 {
        if let Some(j) = runner.get(id) {
            if pred(&j) {
                return j;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "job {id} never satisfied predicate; last state = {:?}",
        runner.get(id).map(|j| j.state)
    );
}

#[tokio::test]
async fn nvme_crypto_erase_happy_path_reaches_erased() {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let runner = JobRunner::new(backend);

    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();
    runner.start(id).unwrap();

    let job = wait_until(&runner, id, |j| j.state.is_terminal()).await;
    assert_eq!(job.state, JobState::Erased);

    // The activity chain must contain one Erasure (Completed) and one
    // Verification (all_passed).
    let erasure = job
        .latest_erasure()
        .expect("Job should have an Erasure activity");
    assert_eq!(erasure.state, ErasureEventState::Completed);
    assert!(matches!(
        erasure.resolved_method,
        Some(wipe_common::Method::NvmeSanitizeCryptoErase { .. })
    ));

    let verification = job.activities.iter().find_map(|a| match a {
        JobActivity::Verification(v) => Some(v),
        _ => None,
    });
    let v = verification.expect("Job should have a Verification activity");
    assert!(v.report.all_passed);
    assert_eq!(v.erasure_event_id, erasure.id);
}

#[tokio::test]
async fn sata_failure_keeps_outer_job_in_progress_for_operator_decision() {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let runner = JobRunner::new(backend);

    let id = runner.create_job(job_spec("dev-sata-0")).await.unwrap();
    runner.start(id).unwrap();

    // Wait for the inner ErasureEvent to reach Failed; the outer Job
    // must stay in InProgress (operator decides what to do next).
    let job = wait_until(&runner, id, |j| {
        j.latest_erasure()
            .map(|e| matches!(e.state, ErasureEventState::Failed))
            .unwrap_or(false)
    })
    .await;
    assert_eq!(job.state, JobState::InProgress);
    let erasure = job.latest_erasure().unwrap();
    assert_eq!(erasure.state, ErasureEventState::Failed);
}

#[tokio::test]
async fn enumerate_returns_default_catalog() {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let runner = JobRunner::new(backend.clone());
    let devices = wipe_engine::DeviceBackend::enumerate(&*backend)
        .await
        .unwrap();
    assert_eq!(devices.len(), 4);
    assert!(devices
        .iter()
        .any(|d| d.id == DeviceId("dev-nvme-0".into())));
    let _ = runner;
}

#[tokio::test]
async fn broadcast_stream_observes_outer_and_inner_transitions() {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let runner = JobRunner::new(backend);
    let mut rx = runner.subscribe();

    let id = runner.create_job(job_spec("dev-nvme-1")).await.unwrap();
    runner.start(id).unwrap();

    let mut saw_inprogress = false;
    let mut saw_erased = false;
    let mut saw_inner_running = false;
    let mut saw_command_issued = false;
    let mut saw_verification_activity = false;
    for _ in 0..400 {
        tokio::select! {
            r = rx.recv() => {
                if let Ok(b) = r {
                    match b {
                        JobBroadcast::JobStateChanged { to: JobState::InProgress, .. } => saw_inprogress = true,
                        JobBroadcast::JobStateChanged { to: JobState::Erased, .. } => saw_erased = true,
                        JobBroadcast::ErasureUpdate { update, .. } => {
                            if let JobUpdateKind::StateChanged { to: ErasureEventState::Running, .. } = update.event {
                                saw_inner_running = true;
                            }
                            if let JobUpdateKind::CommandIssued(_) = update.event {
                                saw_command_issued = true;
                            }
                        }
                        JobBroadcast::ActivityAdded { activity: JobActivity::Verification(_), .. } => {
                            saw_verification_activity = true;
                        }
                        _ => {}
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
        if saw_erased {
            break;
        }
    }
    assert!(
        saw_inprogress,
        "should have observed outer InProgress transition"
    );
    assert!(
        saw_inner_running,
        "should have observed inner Running transition"
    );
    assert!(
        saw_command_issued,
        "should have observed CommandIssued update"
    );
    assert!(
        saw_verification_activity,
        "should have observed a Verification activity"
    );
    assert!(saw_erased, "should have observed outer Erased disposition");
}

// --- hot-plug (identify mode drives this) ----------------------------------

#[tokio::test]
async fn detaching_a_device_removes_it_from_enumeration() {
    use wipe_engine::{DeviceBackend, DeviceSimulator};
    let backend = MockBackend::fast_catalog();
    let before = backend.enumerate().await.unwrap();
    assert!(before.len() >= 2);

    let target = before[1].id.clone();
    let pulled = backend.detach(&target).expect("device was attached");
    assert_eq!(pulled.id, target);

    let after = backend.enumerate().await.unwrap();
    assert_eq!(after.len(), before.len() - 1);
    assert!(!after.iter().any(|d| d.id == target));
    assert!(backend.detached().iter().any(|d| d.id == target));
}

#[tokio::test]
async fn attaching_puts_the_same_device_back() {
    use wipe_engine::{DeviceBackend, DeviceSimulator};
    let backend = MockBackend::fast_catalog();
    let before = backend.enumerate().await.unwrap();
    let target = before[0].id.clone();

    backend.detach(&target).unwrap();
    let plugged = backend.attach(Some(&target)).expect("re-attach");
    assert_eq!(plugged.id, target);

    let after = backend.enumerate().await.unwrap();
    assert_eq!(after.len(), before.len());
    assert!(after.iter().any(|d| d.id == target));
    assert!(backend.detached().is_empty());
    // Capabilities must come back with it, or the next Job would fail to probe.
    assert!(backend.capabilities(&target).await.is_ok());
}

#[tokio::test]
async fn detaching_an_unknown_device_is_a_no_op() {
    use wipe_engine::DeviceSimulator;
    let backend = MockBackend::fast_catalog();
    assert!(backend.detach(&DeviceId::from("dev-not-here")).is_none());
    assert!(backend
        .attach(Some(&DeviceId::from("dev-not-here")))
        .is_none());
}

#[tokio::test]
async fn attach_with_no_id_takes_the_most_recently_detached() {
    use wipe_engine::{DeviceBackend, DeviceSimulator};
    let backend = MockBackend::fast_catalog();
    let devs = backend.enumerate().await.unwrap();
    backend.detach(&devs[0].id).unwrap();
    backend.detach(&devs[1].id).unwrap();

    let back = backend.attach(None).expect("something to re-attach");
    assert_eq!(back.id, devs[1].id, "last one pulled is first one back");
}

// ---- Manifest linking (destruction-cert-integrity story 1) -------------

fn destruction(device_id: &str) -> wipe_common::DestructionEvent {
    wipe_common::DestructionEvent {
        id: uuid::Uuid::new_v4(),
        device_id: DeviceId(device_id.into()),
        at: time::OffsetDateTime::now_utc(),
        method: wipe_common::DestructMethod::Shred,
        operator: op(),
        supervisor: None,
        manifest_ref: None,
        photo_refs: Vec::new(),
        notes: None,
        station_id: None,
        attestation: None,
    }
}

fn destruction_events(job: &Job) -> Vec<&wipe_common::DestructionEvent> {
    job.activities
        .iter()
        .filter_map(|a| match a {
            JobActivity::Destruction(d) => Some(d),
            _ => None,
        })
        .collect()
}

fn mock_runner() -> JobRunner {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    JobRunner::new(backend)
}

#[tokio::test]
async fn link_to_manifest_stamps_job_and_latest_destruction_event() {
    let runner = mock_runner();
    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();
    runner
        .escalate_to_destroy(id, destruction("dev-nvme-0"))
        .unwrap();
    runner
        .escalate_to_destroy(id, destruction("dev-nvme-0"))
        .unwrap();
    let mut rx = runner.subscribe();

    let mid = uuid::Uuid::new_v4();
    runner.link_to_manifest(mid, &[id]).unwrap();

    let job = runner.get(id).unwrap();
    assert_eq!(job.state, JobState::PendingCoSign);
    assert_eq!(job.manifest_id, Some(mid));
    let events = destruction_events(&job);
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].manifest_ref, None,
        "superseded event stays as recorded"
    );
    assert_eq!(events[1].manifest_ref, Some(mid));
    assert!(
        rx.try_recv().is_err(),
        "linking must not broadcast (a JobStateChanged would re-sign the certificate)"
    );
}

#[tokio::test]
async fn link_to_manifest_with_a_queued_job_stamps_nothing() {
    let runner = mock_runner();
    let pending = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();
    runner
        .escalate_to_destroy(pending, destruction("dev-nvme-0"))
        .unwrap();
    let queued = runner.create_job(job_spec("dev-sata-0")).await.unwrap();

    let err = runner
        .link_to_manifest(uuid::Uuid::new_v4(), &[pending, queued])
        .unwrap_err();
    assert!(matches!(err, wipe_common::WipeError::InvalidState(_)));

    for id in [pending, queued] {
        let job = runner.get(id).unwrap();
        assert_eq!(job.manifest_id, None);
        assert!(destruction_events(&job)
            .iter()
            .all(|d| d.manifest_ref.is_none()));
    }
}

#[tokio::test]
async fn link_to_manifest_refuses_an_already_linked_job() {
    let runner = mock_runner();
    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();
    runner
        .escalate_to_destroy(id, destruction("dev-nvme-0"))
        .unwrap();
    let first = uuid::Uuid::new_v4();
    runner.link_to_manifest(first, &[id]).unwrap();

    let err = runner
        .link_to_manifest(uuid::Uuid::new_v4(), &[id])
        .unwrap_err();
    assert!(matches!(err, wipe_common::WipeError::InvalidState(_)));

    let job = runner.get(id).unwrap();
    assert_eq!(job.manifest_id, Some(first));
    assert_eq!(destruction_events(&job)[0].manifest_ref, Some(first));
}

#[tokio::test]
async fn escalating_a_linked_job_is_refused_and_appends_nothing() {
    let runner = mock_runner();
    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();
    runner
        .escalate_to_destroy(id, destruction("dev-nvme-0"))
        .unwrap();
    let mid = uuid::Uuid::new_v4();
    runner.link_to_manifest(mid, &[id]).unwrap();
    let before = runner.get(id).unwrap().activities.len();
    let mut rx = runner.subscribe();

    let err = runner
        .escalate_to_destroy(id, destruction("dev-nvme-0"))
        .unwrap_err();
    assert!(matches!(err, wipe_common::WipeError::InvalidState(_)));

    let job = runner.get(id).unwrap();
    assert_eq!(job.activities.len(), before);
    assert_eq!(job.state, JobState::PendingCoSign);
    assert_eq!(job.manifest_id, Some(mid));
    assert_eq!(
        destruction_events(&job).last().unwrap().manifest_ref,
        Some(mid)
    );
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn link_to_manifest_refuses_an_unknown_job_and_stamps_nothing() {
    let runner = mock_runner();
    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();
    runner
        .escalate_to_destroy(id, destruction("dev-nvme-0"))
        .unwrap();

    let err = runner
        .link_to_manifest(uuid::Uuid::new_v4(), &[id, uuid::Uuid::new_v4()])
        .unwrap_err();
    assert!(matches!(err, wipe_common::WipeError::JobNotFound(_)));

    let job = runner.get(id).unwrap();
    assert_eq!(job.manifest_id, None);
    assert!(destruction_events(&job)
        .iter()
        .all(|d| d.manifest_ref.is_none()));
}

#[tokio::test]
async fn link_to_manifest_refuses_a_job_listed_twice_and_stamps_nothing() {
    let runner = mock_runner();
    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();
    runner
        .escalate_to_destroy(id, destruction("dev-nvme-0"))
        .unwrap();

    let err = runner
        .link_to_manifest(uuid::Uuid::new_v4(), &[id, id])
        .unwrap_err();
    assert!(matches!(err, wipe_common::WipeError::InvalidState(_)));

    let job = runner.get(id).unwrap();
    assert_eq!(job.manifest_id, None);
    assert!(destruction_events(&job)
        .iter()
        .all(|d| d.manifest_ref.is_none()));
}

// A PendingCoSign Job with no DestructionEvent is not constructible through
// the runner (only `escalate_to_destroy` enters PendingCoSign, and it
// appends one), so that refusal has no test here.

// --- runner refusals (CAP-3: not-found vs. refused transition) -------------

fn erasure_count(job: &Job) -> usize {
    job.activities
        .iter()
        .filter(|a| matches!(a, JobActivity::Erasure(_)))
        .count()
}

#[tokio::test]
async fn starting_a_job_twice_is_refused_and_runs_one_erasure() {
    let runner = mock_runner();
    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();

    runner.start(id).unwrap();
    let err = runner.start(id).unwrap_err();
    assert!(
        matches!(err, wipe_common::WipeError::InvalidState(_)),
        "{err}"
    );
    assert!(err.to_string().contains("is in state"), "{err}");

    let job = wait_until(&runner, id, |j| j.state.is_terminal()).await;
    assert_eq!(job.state, JobState::Erased);
    assert_eq!(erasure_count(&job), 1);
}

#[tokio::test]
async fn start_moves_the_job_to_in_progress_before_returning() {
    let runner = mock_runner();
    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();
    let mut rx = runner.subscribe();

    runner.start(id).unwrap();

    let job = runner.get(id).unwrap();
    assert_eq!(job.state, JobState::InProgress);
    assert!(job.started_at.is_some());
    match rx.try_recv() {
        Ok(JobBroadcast::JobStateChanged {
            from: JobState::Queued,
            to: JobState::InProgress,
            ..
        }) => {}
        other => panic!("expected Queued->InProgress broadcast first, got {other:?}"),
    }
    wait_until(&runner, id, |j| j.state.is_terminal()).await;
}

#[tokio::test]
async fn starting_an_escalated_job_is_refused() {
    let runner = mock_runner();
    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();
    runner
        .escalate_to_destroy(id, destruction("dev-nvme-0"))
        .unwrap();
    let mut rx = runner.subscribe();

    let err = runner.start(id).unwrap_err();
    assert!(matches!(err, wipe_common::WipeError::InvalidState(_)));

    tokio::time::sleep(Duration::from_millis(100)).await;
    let job = runner.get(id).unwrap();
    assert_eq!(job.state, JobState::PendingCoSign);
    assert_eq!(erasure_count(&job), 0);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn aborting_an_erased_job_is_refused_and_changes_nothing() {
    let runner = mock_runner();
    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();
    runner.start(id).unwrap();
    let before = wait_until(&runner, id, |j| j.state.is_terminal()).await;
    assert_eq!(before.state, JobState::Erased);
    let mut rx = runner.subscribe();

    let err = runner.abort(id).await.unwrap_err();
    assert!(matches!(err, wipe_common::WipeError::InvalidState(_)));
    assert!(err.to_string().contains("already terminal"), "{err}");

    let job = runner.get(id).unwrap();
    assert_eq!(job.state, JobState::Erased);
    assert_eq!(job.ended_at, before.ended_at);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn aborting_an_aborted_job_is_an_ok_no_op() {
    let runner = mock_runner();
    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();
    runner.abort(id).await.unwrap();
    let before = runner.get(id).unwrap();
    assert_eq!(before.state, JobState::Aborted);
    let mut rx = runner.subscribe();

    runner.abort(id).await.unwrap();

    let job = runner.get(id).unwrap();
    assert_eq!(job.state, JobState::Aborted);
    assert_eq!(job.ended_at, before.ended_at);
    assert!(
        rx.try_recv().is_err(),
        "a repeated abort must not broadcast"
    );
}

#[tokio::test]
async fn unknown_job_ids_are_job_not_found() {
    let runner = mock_runner();
    let unknown = uuid::Uuid::new_v4();

    let err = runner.start(unknown).unwrap_err();
    assert!(matches!(err, wipe_common::WipeError::JobNotFound(id) if id == unknown));
    assert_eq!(err.to_string(), format!("job {unknown} not found"));

    let err = runner.abort(unknown).await.unwrap_err();
    assert!(matches!(err, wipe_common::WipeError::JobNotFound(id) if id == unknown));

    let err = runner
        .escalate_to_destroy(unknown, destruction("dev-nvme-0"))
        .unwrap_err();
    assert!(matches!(err, wipe_common::WipeError::JobNotFound(id) if id == unknown));

    let err = runner.mark_destroyed(unknown).unwrap_err();
    assert!(matches!(err, wipe_common::WipeError::JobNotFound(id) if id == unknown));
}

/// Give a spawned job task time to run (it would append an Erasure
/// activity almost at once if it ignored the state change).
async fn let_spawned_task_run() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn abort_right_after_start_runs_no_erasure() {
    let runner = mock_runner();
    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();
    let mut rx = runner.subscribe();

    runner.start(id).unwrap();
    runner.abort(id).await.unwrap();
    let_spawned_task_run().await;

    let job = runner.get(id).unwrap();
    assert_eq!(job.state, JobState::Aborted);
    assert_eq!(erasure_count(&job), 0);
    let mut into_in_progress = 0;
    while let Ok(b) = rx.try_recv() {
        if let JobBroadcast::JobStateChanged {
            to: JobState::InProgress,
            ..
        } = b
        {
            into_in_progress += 1;
        }
    }
    assert_eq!(into_in_progress, 1);
}

#[tokio::test]
async fn escalate_right_after_start_runs_no_erasure() {
    let runner = mock_runner();
    let id = runner.create_job(job_spec("dev-nvme-0")).await.unwrap();

    runner.start(id).unwrap();
    runner
        .escalate_to_destroy(id, destruction("dev-nvme-0"))
        .unwrap();
    let_spawned_task_run().await;

    let job = runner.get(id).unwrap();
    assert_eq!(job.state, JobState::PendingCoSign);
    assert_eq!(erasure_count(&job), 0);
}
