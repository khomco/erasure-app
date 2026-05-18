use std::sync::Arc;
use std::time::Duration;

use wipe_common::{Classification, DeviceId, Intent, JobSpec, JobState, OperatorRef};
use wipe_engine::JobRunner;
use wipe_engine_mock::{MockBackend, MockTiming};

fn op() -> OperatorRef {
    OperatorRef {
        id: "op-1".into(),
        display_name: "Alice Erasure".into(),
        email: "alice@example.com".into(),
    }
}

async fn wait_terminal(runner: &JobRunner, id: uuid::Uuid) -> wipe_common::Job {
    for _ in 0..200 {
        if let Some(j) = runner.get(id) {
            if j.state.is_terminal() {
                return j;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job {id} did not reach terminal state");
}

#[tokio::test]
async fn nvme_crypto_erase_happy_path() {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let runner = JobRunner::new(backend);

    let spec = JobSpec {
        device_id: DeviceId("dev-nvme-0".into()),
        classification: Classification::High,
        intent: Intent::Reuse,
        method: None,
        verify: true,
        verify_samples: 4,
        operator: op(),
        asset_tag: Some("ASSET-001".into()),
        site_label: Some("Lab A".into()),
        ticket_ref: Some("TKT-42".into()),
    };
    let id = runner.create_job(spec).await.unwrap();
    runner.start(id).unwrap();

    let job = wait_terminal(&runner, id).await;
    assert_eq!(job.state, JobState::Completed);
    assert!(job.verification.unwrap().all_passed);
    let method = job.resolved_method.unwrap();
    assert!(matches!(
        method,
        wipe_common::Method::NvmeSanitizeCryptoErase { .. }
    ));
}

#[tokio::test]
async fn sata_failure_propagates() {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let runner = JobRunner::new(backend);

    let spec = JobSpec {
        device_id: DeviceId("dev-sata-0".into()),
        classification: Classification::High,
        intent: Intent::Reuse,
        method: None,
        verify: true,
        verify_samples: 2,
        operator: op(),
        asset_tag: None,
        site_label: None,
        ticket_ref: None,
    };
    let id = runner.create_job(spec).await.unwrap();
    runner.start(id).unwrap();

    let job = wait_terminal(&runner, id).await;
    assert_eq!(job.state, JobState::Failed);
    // The failure should appear in the event log.
    let had_failure_event = job
        .events
        .iter()
        .any(|e| matches!(e.event, wipe_common::JobUpdateKind::Failed { .. }));
    assert!(had_failure_event, "expected a Failed event in the job log");
}

#[tokio::test]
async fn enumerate_returns_default_catalog() {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let runner = JobRunner::new(backend.clone());
    // The runner doesn't expose enumerate; tests go through the backend.
    let devices = wipe_engine::DeviceBackend::enumerate(&*backend).await.unwrap();
    assert_eq!(devices.len(), 4);
    assert!(devices.iter().any(|d| d.id == DeviceId("dev-nvme-0".into())));
    let _ = runner;
}

#[tokio::test]
async fn event_stream_observes_full_lifecycle() {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::fast(),
    ));
    let runner = JobRunner::new(backend);
    let mut rx = runner.subscribe();

    let spec = JobSpec {
        device_id: DeviceId("dev-nvme-1".into()),
        classification: Classification::High,
        intent: Intent::Reuse,
        method: None,
        verify: true,
        verify_samples: 2,
        operator: op(),
        asset_tag: None,
        site_label: None,
        ticket_ref: None,
    };
    let id = runner.create_job(spec).await.unwrap();
    runner.start(id).unwrap();

    let mut saw_running = false;
    let mut saw_completed = false;
    let mut saw_command_issued = false;
    for _ in 0..200 {
        tokio::select! {
            r = rx.recv() => {
                if let Ok(update) = r {
                    match update.event.event {
                        wipe_common::JobUpdateKind::StateChanged { to: JobState::Running, .. } => saw_running = true,
                        wipe_common::JobUpdateKind::StateChanged { to: JobState::Completed, .. } => saw_completed = true,
                        wipe_common::JobUpdateKind::CommandIssued(_) => saw_command_issued = true,
                        _ => {}
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        if saw_completed { break; }
    }
    assert!(saw_running, "should have observed Running state");
    assert!(saw_command_issued, "should have observed CommandIssued event");
    assert!(saw_completed, "should have observed Completed state");
}
