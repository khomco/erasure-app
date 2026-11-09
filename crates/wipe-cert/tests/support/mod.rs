//! Builds destroyed Jobs the way the station does: through `JobRunner`
//! (`record_for_destruction`, `record_scanned_for_destruction`,
//! `link_to_manifest`, `mark_destroyed`) and `DestructionManifest::complete`
//! / `concur`.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use uuid::Uuid;
use wipe_cert::{
    sign, CertIssuer, Certificate, MediaStatus, SignedCertificate, SigningKey, ValidationBlock,
};
use wipe_common::{
    AttestationSigner, Classification, Concurrence, ConcurrenceKind, DestructMethod,
    DestructionAttestation, DestructionCompletion, DestructionEvent, DestructionManifest,
    DestructionVerification, DeviceId, ErasureEventState, IdentifierSource, Intent, Job, JobSpec,
    JobState, OperatorRef, RecordedIdentifiers,
};
use wipe_engine::JobRunner;
use wipe_engine_mock::{default_devices_public, MockBackend, MockTiming};

pub fn station_key() -> SigningKey {
    SigningKey::from_seed([42; 32])
}

pub fn operator(id: &str, name: &str) -> OperatorRef {
    OperatorRef {
        id: id.into(),
        display_name: name.into(),
        email: format!("{id}@example.com"),
    }
}

pub fn ana() -> AttestationSigner {
    AttestationSigner::self_asserted(operator("op-1", "Ana"), "Ana Ruiz", "Bench lead").unwrap()
}

pub fn bo() -> AttestationSigner {
    AttestationSigner::self_asserted(operator("op-2", "Bo"), "Bo Chen", "Site supervisor").unwrap()
}

pub fn runner() -> JobRunner {
    JobRunner::new(Arc::new(MockBackend::with_catalog(
        default_devices_public(),
        MockTiming::fast(),
    )))
}

fn spec(device_id: &str, classification: Classification, intent: Intent) -> JobSpec {
    JobSpec {
        device_id: DeviceId(device_id.into()),
        classification,
        intent,
        operator: operator("op-1", "Ana"),
        asset_tag: None,
        site_label: Some("Lab".into()),
        ticket_ref: None,
        work_order_ref: None,
        customer_ref: None,
        contract_ref: None,
        sanitization_profile_ref: None,
    }
}

pub fn attested_event(
    device_id: &str,
    technique: DestructMethod,
    identifiers: RecordedIdentifiers,
) -> DestructionEvent {
    let at = OffsetDateTime::now_utc();
    let attestation =
        DestructionAttestation::new(technique, at, "SHR-2", "Bay 3", identifiers, ana()).unwrap();
    DestructionEvent {
        id: Uuid::new_v4(),
        device_id: DeviceId(device_id.into()),
        at,
        method: technique,
        operator: operator("op-1", "Ana"),
        supervisor: None,
        manifest_ref: None,
        photo_refs: Vec::new(),
        notes: None,
        station_id: None,
        attestation: Some(attestation),
    }
}

/// `n` scanned dead drives recorded for destruction and assembled on one
/// pending run.
pub async fn scanned_run(
    runner: &JobRunner,
    technique: DestructMethod,
    classification: Classification,
    n: usize,
    required: Option<ConcurrenceKind>,
) -> DestructionManifest {
    let drives = (0..n)
        .map(|i| {
            let serial = format!("DEAD-{}-{i}", Uuid::new_v4().simple());
            let device = format!("scanned:{serial}");
            let ev = attested_event(
                &device,
                technique,
                RecordedIdentifiers {
                    asset_tag: Some(format!("AT-{serial}")),
                    serial: Some(serial.clone()),
                    source: IdentifierSource::Scanned,
                },
            );
            let mut s = spec(&device, classification, Intent::Destroy);
            s.asset_tag = Some(format!("AT-{serial}"));
            (s, ev)
        })
        .collect();
    let ids = runner.record_scanned_for_destruction(drives).await.unwrap();
    assemble(runner, ids, required)
}

/// A Job whose erasure failed, recorded for destruction from the device,
/// assembled on its own pending run.
pub async fn erased_first_run(
    runner: &JobRunner,
    technique: DestructMethod,
    classification: Classification,
    required: Option<ConcurrenceKind>,
) -> DestructionManifest {
    let id = runner
        .create_job(spec("dev-sata-0", classification, Intent::Reuse))
        .await
        .unwrap();
    runner.start(id).unwrap();
    let job = wait(runner, id, |j| {
        j.latest_erasure()
            .is_some_and(|e| e.state == ErasureEventState::Failed)
    })
    .await;
    let serial = job.latest_erasure().unwrap().device_snapshot.serial.clone();
    let ev = attested_event(
        "dev-sata-0",
        technique,
        RecordedIdentifiers {
            asset_tag: None,
            serial: Some(serial),
            source: IdentifierSource::Device,
        },
    );
    runner.record_for_destruction(id, ev).unwrap();
    assemble(runner, vec![id], required)
}

/// An erased Job.
pub async fn erased_job(runner: &JobRunner) -> Job {
    let id = runner
        .create_job(spec("dev-nvme-0", Classification::High, Intent::Reuse))
        .await
        .unwrap();
    runner.start(id).unwrap();
    wait(runner, id, |j| j.state == JobState::Erased).await
}

fn assemble(
    runner: &JobRunner,
    ids: Vec<Uuid>,
    required: Option<ConcurrenceKind>,
) -> DestructionManifest {
    let run = DestructionManifest::new(operator("op-1", "Ana"), ids.clone(), None, required);
    runner.link_to_manifest(run.id, &ids).unwrap();
    run
}

pub fn complete(run: &mut DestructionManifest) {
    let at = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
    let completion = DestructionCompletion::new(
        at,
        DestructionVerification {
            remnants_inspected: true,
            equipment_cycle_completed: true,
            equipment_id: "SHR-2".into(),
            criterion: "particles no larger than 2 mm".into(),
        },
        Some("CAM-4"),
        ana(),
    )
    .unwrap();
    run.complete(completion).unwrap();
}

pub fn concur(run: &mut DestructionManifest, kind: ConcurrenceKind) {
    let c = Concurrence::new(kind, run.id, OffsetDateTime::now_utc(), bo()).unwrap();
    run.concur(c).unwrap();
}

/// Move the run's Jobs to `Destroyed`, as the server does once it is done.
pub fn destroy(runner: &JobRunner, run: &DestructionManifest) -> Vec<Job> {
    assert!(run.destroys_jobs());
    runner.mark_destroyed(&run.job_ids).unwrap();
    run.job_ids
        .iter()
        .map(|id| runner.get(*id).unwrap())
        .collect()
}

fn issuer(key: &SigningKey) -> CertIssuer {
    CertIssuer {
        tool_name: "wipestation".into(),
        tool_version: "0.1.0".into(),
        public_key_id: key.public_key_id(),
    }
}

pub fn validation(job: &Job) -> ValidationBlock {
    ValidationBlock {
        validated: false,
        media_class: job
            .latest_erasure()
            .map(|e| e.device_snapshot.media_type.class_label())
            .unwrap_or("unknown")
            .into(),
        validation_ref: None,
        validation_expires: None,
    }
}

pub fn destroyed_cert(job: &Job, run: &DestructionManifest, key: &SigningKey) -> SignedCertificate {
    let cert = Certificate::from_destroyed_job(
        job,
        run,
        issuer(key),
        validation(job),
        MediaStatus {
            operational: false,
            damaged: true,
            notes: None,
        },
        true,
    )
    .unwrap();
    sign(cert, key).unwrap()
}

pub fn erased_cert(job: &Job, key: &SigningKey) -> SignedCertificate {
    let cert = Certificate::from_job(
        job,
        issuer(key),
        validation(job),
        MediaStatus {
            operational: true,
            damaged: false,
            notes: None,
        },
        true,
    )
    .unwrap();
    sign(cert, key).unwrap()
}

/// Re-sign a certificate after editing it, as a forger holding the station
/// key would.
pub fn resign(signed: &SignedCertificate, key: &SigningKey) -> SignedCertificate {
    let mut again = sign(signed.certificate.clone(), key).unwrap();
    again.co_signatures = signed.co_signatures.clone();
    again
}

async fn wait<F: Fn(&Job) -> bool>(runner: &JobRunner, id: Uuid, pred: F) -> Job {
    for _ in 0..400 {
        if let Some(j) = runner.get(id) {
            if pred(&j) {
                return j;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("job {id} never satisfied the predicate");
}
