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

async fn wait_until<F: Fn(&Job) -> bool>(
    runner: &JobRunner,
    id: uuid::Uuid,
    pred: F,
) -> Job {
    for _ in 0..400 {
        if let Some(j) = runner.get(id) {
            if pred(&j) {
                return j;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("job {id} never satisfied predicate; last state = {:?}", runner.get(id).map(|j| j.state));
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
    let devices = wipe_engine::DeviceBackend::enumerate(&*backend).await.unwrap();
    assert_eq!(devices.len(), 4);
    assert!(devices.iter().any(|d| d.id == DeviceId("dev-nvme-0".into())));
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
        if saw_erased { break; }
    }
    assert!(saw_inprogress, "should have observed outer InProgress transition");
    assert!(saw_inner_running, "should have observed inner Running transition");
    assert!(saw_command_issued, "should have observed CommandIssued update");
    assert!(saw_verification_activity, "should have observed a Verification activity");
    assert!(saw_erased, "should have observed outer Erased disposition");
}
