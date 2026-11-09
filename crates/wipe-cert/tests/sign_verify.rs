use std::sync::Arc;
use std::time::Duration;

use wipe_cert::{
    sign, verify, CertIssuer, Certificate, MediaStatus, SignedCertificate, SigningKey,
    ValidationBlock, VerifyingKey,
};
use wipe_common::{Classification, DeviceId, Intent, Job, JobSpec, JobState, OperatorRef};
use wipe_engine::JobRunner;
use wipe_engine_mock::{default_devices_public, MockBackend, MockTiming};

fn op() -> OperatorRef {
    OperatorRef {
        id: "op-1".into(),
        display_name: "Alice".into(),
        email: "alice@example.com".into(),
    }
}

async fn finished_job() -> Job {
    let backend = Arc::new(MockBackend::with_catalog(
        default_devices_public(),
        MockTiming::fast(),
    ));
    let runner = JobRunner::new(backend);
    let spec = JobSpec {
        device_id: DeviceId("dev-nvme-0".into()),
        classification: Classification::High,
        intent: Intent::Reuse,
        operator: op(),
        asset_tag: Some("A-1".into()),
        site_label: Some("Lab".into()),
        ticket_ref: Some("T-1".into()),
        work_order_ref: None,
        customer_ref: None,
        contract_ref: None,
        sanitization_profile_ref: None,
    };
    let id = runner.create_job(spec).await.unwrap();
    runner.start(id).unwrap();
    for _ in 0..400 {
        if let Some(j) = runner.get(id) {
            if j.state == JobState::Erased {
                return j;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("job did not reach Erased");
}

fn build_cert(job: &Job, key: &SigningKey) -> Certificate {
    let device = job
        .latest_erasure()
        .map(|e| e.device_snapshot.clone())
        .expect("erasure activity present");
    Certificate::from_job(
        job,
        CertIssuer {
            tool_name: "wipestation".into(),
            tool_version: "0.1.0".into(),
            public_key_id: key.public_key_id(),
        },
        ValidationBlock {
            validated: false,
            media_class: device.media_type.class_label().into(),
            validation_ref: None,
            validation_expires: None,
        },
        MediaStatus {
            operational: true,
            damaged: false,
            notes: None,
        },
        // These fixtures exercise the signature machinery, not licensing;
        // an unlicensed station is the free-tier default (ADR-0005 §5).
        true,
    )
    .unwrap()
}

#[tokio::test]
async fn sign_then_verify_with_correct_key() {
    let key = SigningKey::generate();
    let job = finished_job().await;
    let cert = build_cert(&job, &key);
    let signed = sign(cert, &key).unwrap();
    let trusted = [key.verifying_key()];
    let matched = verify(&signed, &trusted).unwrap();
    assert_eq!(matched, key.verifying_key().public_key_id());
}

#[tokio::test]
async fn verify_fails_with_unknown_key() {
    let key = SigningKey::generate();
    let other = SigningKey::generate();
    let job = finished_job().await;
    let cert = build_cert(&job, &key);
    let signed = sign(cert, &key).unwrap();
    let trusted = [other.verifying_key()];
    let err = verify(&signed, &trusted).unwrap_err();
    assert!(err.to_string().contains("no trusted key"));
}

#[tokio::test]
async fn tampered_cert_fails_verification() {
    let key = SigningKey::generate();
    let job = finished_job().await;
    let cert = build_cert(&job, &key);
    let mut signed = sign(cert, &key).unwrap();
    // Tamper: change the asset tag after signing.
    signed.certificate.spec.asset_tag = Some("DIFFERENT".into());
    let trusted = [key.verifying_key()];
    let err = verify(&signed, &trusted).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("SHA-256 mismatch") || msg.contains("signature"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn signed_cert_round_trips_through_json() {
    let key = SigningKey::generate();
    let job = finished_job().await;
    let cert = build_cert(&job, &key);
    let signed = sign(cert, &key).unwrap();
    let s = serde_json::to_string(&signed).unwrap();
    let back: SignedCertificate = serde_json::from_str(&s).unwrap();
    let trusted = [key.verifying_key()];
    verify(&back, &trusted).unwrap();
}

#[tokio::test]
async fn cert_carries_activity_chain_with_erasure_and_verification() {
    let key = SigningKey::generate();
    let job = finished_job().await;
    let cert = build_cert(&job, &key);
    assert_eq!(cert.cert_format_version, 3);
    assert!(cert.destruction.is_none());
    assert_eq!(cert.disposition, wipe_common::AssetDisposition::Erased);
    let has_erasure = cert
        .activities
        .iter()
        .any(|a| matches!(a, wipe_common::JobActivity::Erasure(_)));
    let has_verification = cert
        .activities
        .iter()
        .any(|a| matches!(a, wipe_common::JobActivity::Verification(_)));
    assert!(has_erasure, "cert.activities should include an Erasure");
    assert!(has_verification, "cert.activities should include a Verification");
    // The convenience flat-command-list still works for renderers.
    assert!(!cert.command_evidence().is_empty());
}

#[tokio::test]
async fn public_key_id_stable_across_runs() {
    let seed = [42u8; 32];
    let a = SigningKey::from_seed(seed);
    let b = SigningKey::from_seed(seed);
    assert_eq!(a.public_key_id(), b.public_key_id());
}

#[test]
fn verifying_key_round_trip() {
    let key = SigningKey::generate();
    let vk = key.verifying_key();
    let bytes = vk.0.to_bytes();
    let restored = VerifyingKey::from_bytes(&bytes).unwrap();
    assert_eq!(vk.public_key_id(), restored.public_key_id());
}
