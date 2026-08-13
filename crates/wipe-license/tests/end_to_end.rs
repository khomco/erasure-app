//! End-to-end: a real erasure certificate carrying a real attestation chain.
//!
//! The unit tests exercise the chain in isolation. This one puts it where it
//! actually lives — stapled to a signed `SignedCertificate` — and pins the
//! interaction between the two signatures, which is where the interesting
//! failure modes are.

use time::{Duration, OffsetDateTime};

use wipe_cert::{
    attach_attestation, sign, CertIssuer, Certificate, MediaStatus, SigningKey, ValidationBlock,
};
use wipe_common::{
    AssetDisposition, Capabilities, Category, Classification, Device, Intent, Job, JobSpec, Method,
    OperatorRef,
};
use wipe_license::{
    verify_chain, AllowedMethods, AttestationChain, ChainVerdict, Entitlements, Quota, Scope,
    VendorRoot,
};

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_780_000_000).unwrap()
}

fn entitlements() -> Entitlements {
    Entitlements {
        customer_id: "cust-7".into(),
        customer_name: "Acme Recycling".into(),
        quota: Quota::Unlimited,
        scope: Scope::Site {
            site_id: "site-a".into(),
        },
        not_before: now() - Duration::days(1),
        not_after: now() + Duration::days(364),
        features: vec![],
        allowed_methods: AllowedMethods::All,
        machine_binding: None,
    }
}

/// Minimal finished Job, built directly rather than by running the engine —
/// this test is about signatures, not orchestration.
fn finished_job() -> Job {
    let device = Device {
        id: wipe_common::DeviceId::from("dev-1"),
        vendor: "TestCo".into(),
        model: "TM-1".into(),
        serial: "SN-1".into(),
        wwn: None,
        capacity_bytes: 1_000_000,
        media_type: wipe_common::MediaType::SsdNvme,
        bus: wipe_common::BusType::Nvme,
        firmware: None,
        removable: false,
        block_size: 512,
        path: "/dev/nvme0n1".into(),
    };
    let spec = JobSpec {
        device_id: device.id.clone(),
        classification: Classification::High,
        intent: Intent::Reuse,
        operator: OperatorRef {
            id: "op-1".into(),
            display_name: "Bench Op".into(),
            email: "op@example.com".into(),
        },
        asset_tag: None,
        site_label: None,
        ticket_ref: None,
        work_order_ref: None,
        customer_ref: None,
        contract_ref: None,
        sanitization_profile_ref: None,
    };
    let mut job = Job::new(spec.clone());
    let mut erasure = wipe_common::ErasureEvent::new(
        device,
        Capabilities::default(),
        wipe_common::ErasureEventSpec {
            device_id: spec.device_id.clone(),
            classification: spec.classification,
            intent: spec.intent,
            method: None,
            verify: false,
            verify_samples: 0,
            operator: spec.operator.clone(),
            asset_tag: None,
            site_label: None,
            ticket_ref: None,
        },
    );
    erasure.resolved_method = Some(Method::NvmeSanitizeCryptoErase {
        ause: false,
        no_deallocate: false,
    });
    erasure.state = wipe_common::ErasureEventState::Completed;
    job.activities
        .push(wipe_common::JobActivity::Erasure(erasure));
    job.state = wipe_common::JobState::Erased;
    job.started_at = Some(now() - Duration::seconds(30));
    job.ended_at = Some(now());
    job
}

fn build_cert(job: &Job, key: &SigningKey, evaluation: bool) -> Certificate {
    Certificate::from_job(
        job,
        CertIssuer {
            tool_name: "wipestation".into(),
            tool_version: "0.1.0".into(),
            public_key_id: key.public_key_id(),
        },
        ValidationBlock {
            validated: false,
            media_class: "ssd-nvme".into(),
            validation_ref: None,
            validation_expires: None,
        },
        MediaStatus {
            operational: true,
            damaged: false,
            notes: None,
        },
        evaluation,
    )
    .expect("cert builds")
}

#[test]
fn a_licensed_certificate_verifies_on_both_axes() {
    let root = VendorRoot::generate();
    let instance = SigningKey::generate();
    let license = root
        .issue("lic-9", instance.public_key_id(), entitlements(), now())
        .unwrap();

    let job = finished_job();
    let cert = build_cert(&job, &instance, false);
    let mut signed = sign(cert, &instance).unwrap();
    attach_attestation(
        &mut signed,
        serde_json::to_value(AttestationChain::new(license)).unwrap(),
    );

    // Axis 1: the payload is intact and was signed by the instance key.
    let key_id = wipe_cert::verify(&signed, &[instance.verifying_key()]).unwrap();
    assert_eq!(key_id, instance.public_key_id());

    // Axis 2: that key was licensed. These are separate questions and the
    // API keeps them separate.
    let chain: AttestationChain =
        serde_json::from_value(signed.attestation.clone().unwrap()).unwrap();
    let verdict = verify_chain(
        Some(&chain),
        &signed.signature.public_key_id,
        signed.certificate.issued_at,
        &[root.verifying_key()],
    );
    assert!(verdict.is_licensed(), "{verdict:?}");
    assert!(!signed.certificate.evaluation);
}

#[test]
fn attaching_a_chain_does_not_disturb_the_erasure_signature() {
    // The chain rides *outside* the signed payload precisely so a licence can
    // be re-stapled after a renewal without invalidating a signature an
    // auditor already checked.
    let root = VendorRoot::generate();
    let instance = SigningKey::generate();
    let job = finished_job();
    let mut signed = sign(build_cert(&job, &instance, false), &instance).unwrap();

    let before = signed.signature.clone();
    let license = root
        .issue("lic-9", instance.public_key_id(), entitlements(), now())
        .unwrap();
    attach_attestation(
        &mut signed,
        serde_json::to_value(AttestationChain::new(license)).unwrap(),
    );

    assert_eq!(
        before.canonical_sha256_hex,
        signed.signature.canonical_sha256_hex
    );
    assert!(wipe_cert::verify(&signed, &[instance.verifying_key()]).is_ok());
}

#[test]
fn an_evaluation_certificate_is_fully_valid_and_clearly_unlicensed() {
    // Free tier (ADR-0005 §5): the erasure was real, the signature holds, and
    // nothing about it can be mistaken for licensed output.
    let root = VendorRoot::generate();
    let instance = SigningKey::generate();
    let job = finished_job();
    let signed = sign(build_cert(&job, &instance, true), &instance).unwrap();

    assert!(wipe_cert::verify(&signed, &[instance.verifying_key()]).is_ok());
    assert!(signed.certificate.evaluation);
    assert!(signed.attestation.is_none());
    assert_eq!(
        verify_chain(
            None,
            &signed.signature.public_key_id,
            signed.certificate.issued_at,
            &[root.verifying_key()]
        ),
        ChainVerdict::Unlicensed
    );
}

#[test]
fn the_evaluation_marker_cannot_be_stripped_without_breaking_the_signature() {
    // The marker lives inside the signed payload for exactly this reason: an
    // unlicensed cert must not be convertible into an apparently-licensed one.
    let instance = SigningKey::generate();
    let job = finished_job();
    let mut signed = sign(build_cert(&job, &instance, true), &instance).unwrap();

    signed.certificate.evaluation = false;

    let err = wipe_cert::verify(&signed, &[instance.verifying_key()]).unwrap_err();
    assert!(
        err.to_string().contains("canonical SHA-256 mismatch"),
        "expected a tamper detection, got: {err}"
    );
}

#[test]
fn a_stolen_licence_cannot_launder_another_stations_certificate() {
    // The attack this design exists to stop: take a genuine licence from a
    // paying customer and staple it to certificates signed elsewhere.
    let root = VendorRoot::generate();
    let paying = SigningKey::generate();
    let freeloader = SigningKey::generate();

    let license = root
        .issue("lic-9", paying.public_key_id(), entitlements(), now())
        .unwrap();

    let job = finished_job();
    let mut signed = sign(build_cert(&job, &freeloader, false), &freeloader).unwrap();
    attach_attestation(
        &mut signed,
        serde_json::to_value(AttestationChain::new(license)).unwrap(),
    );

    // The erasure signature is perfectly valid...
    assert!(wipe_cert::verify(&signed, &[freeloader.verifying_key()]).is_ok());

    // ...and the chain still refuses, because the licence names a different key.
    let chain: AttestationChain =
        serde_json::from_value(signed.attestation.clone().unwrap()).unwrap();
    let verdict = verify_chain(
        Some(&chain),
        &signed.signature.public_key_id,
        signed.certificate.issued_at,
        &[root.verifying_key()],
    );
    assert!(verdict.is_invalid(), "{verdict:?}");
}

#[test]
fn a_certificate_carries_its_disposition_and_category_alongside_the_chain() {
    // Sanity that licensing did not disturb the compliance payload.
    let instance = SigningKey::generate();
    let job = finished_job();
    let signed = sign(build_cert(&job, &instance, true), &instance).unwrap();
    assert_eq!(signed.certificate.disposition, AssetDisposition::Erased);
    assert_eq!(signed.certificate.sanitization.category, Category::Purge);
}
