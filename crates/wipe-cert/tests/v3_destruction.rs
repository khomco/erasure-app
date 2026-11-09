//! Certificate schema v3 (destruction-attestation story 4): a destroyed
//! drive is certified once, at `Destroyed`, with its run inside the signed
//! bytes, and `verify` refuses a v3 certificate that contradicts itself.

mod support;

use serde_json::Value;
use support::*;
use wipe_cert::{
    verify, CertError, Certificate, CoSignatureBlock, CoSignerRole, DestructionRecord,
    SignedCertificate, SigningKey,
};
use wipe_common::{
    concurrence_statement, AssetDisposition, Category, Classification, ConcurrenceKind,
    DestructMethod, JobActivity, Method, SHRED_PULVERIZE_CAUTION,
};

fn verify_err(signed: &SignedCertificate, key: &SigningKey) -> String {
    match verify(signed, &[key.verifying_key()]) {
        Err(CertError::Verification(msg)) => msg,
        other => panic!("expected a verification error, got {other:?}"),
    }
}

/// Edit a verified certificate, re-sign it with the station key, and return
/// the verify error.
fn tamper(
    signed: &SignedCertificate,
    key: &SigningKey,
    edit: impl FnOnce(&mut Certificate),
) -> String {
    let mut forged = signed.clone();
    edit(&mut forged.certificate);
    let forged = resign(&forged, key);
    verify_err(&forged, key)
}

fn record(c: &mut Certificate) -> &mut DestructionRecord {
    c.destruction.as_mut().expect("destroyed cert")
}

async fn shred_scanned(n: usize) -> (Vec<SignedCertificate>, SigningKey) {
    let runner = runner();
    let key = station_key();
    let mut run = scanned_run(
        &runner,
        DestructMethod::Shred,
        Classification::Moderate,
        n,
        None,
    )
    .await;
    complete(&mut run);
    let jobs = destroy(&runner, &run);
    let certs = jobs.iter().map(|j| destroyed_cert(j, &run, &key)).collect();
    (certs, key)
}

#[tokio::test]
async fn an_erased_certificate_is_v3_with_no_destruction_key() {
    let runner = runner();
    let key = station_key();
    let job = erased_job(&runner).await;
    let signed = erased_cert(&job, &key);
    verify(&signed, &[key.verifying_key()]).unwrap();
    let v = serde_json::to_value(&signed.certificate).unwrap();
    assert_eq!(v["cert_format_version"], 3);
    assert_eq!(
        v["@context"],
        "https://wipestation.dev/contexts/sanitization-cert-v3"
    );
    assert!(v.get("destruction").is_none());
    assert!(v.get("device").is_some());
    assert_eq!(v["sanitization"]["category"], "purge");
}

#[tokio::test]
async fn a_destroyed_drive_erased_first_is_certified_once_with_its_run() {
    let runner = runner();
    let key = station_key();
    let mut run =
        erased_first_run(&runner, DestructMethod::Shred, Classification::High, None).await;
    // Recorded but not destroyed: no certificate can be built yet.
    let pending = runner.get(run.job_ids[0]).unwrap();
    assert!(Certificate::from_job(
        &pending,
        issuer_stub(&key),
        validation(&pending),
        media(),
        true
    )
    .is_none());
    assert!(DestructionRecord::build(&pending, &run).is_err());

    complete(&mut run);
    let job = destroy(&runner, &run).remove(0);
    // `from_job` never certifies a destroyed Job: it needs the run.
    assert!(
        Certificate::from_job(&job, issuer_stub(&key), validation(&job), media(), true).is_none()
    );
    let signed = destroyed_cert(&job, &run, &key);
    verify(&signed, &[key.verifying_key()]).unwrap();

    let c = &signed.certificate;
    assert_eq!(c.disposition, AssetDisposition::Destroyed);
    assert!(c.device.is_some(), "an erased-first drive keeps its device");
    let rec = c.destruction.as_ref().unwrap();
    assert_eq!(rec.manifest_id, run.id);
    assert_eq!(Some(&rec.completion), run.completion.as_ref());
    assert_eq!(rec.required_concurrence, None);
    assert_eq!(rec.concurrence, None);
    assert_eq!(
        c.sanitization.method,
        Method::Destroy {
            method: DestructMethod::Shred
        }
    );
    let event = c
        .activities
        .iter()
        .find_map(|a| match a {
            JobActivity::Destruction(e) => Some(e),
            _ => None,
        })
        .unwrap();
    assert_eq!(event.id, rec.destruction_event_id);
    assert_eq!(event.manifest_ref, Some(run.id), "the run link is signed");
    assert!(c.media_status.damaged);

    let v = serde_json::to_value(c).unwrap();
    assert_eq!(v["disposition"], "destroyed");
    assert_eq!(v["destruction"]["concurrence"], Value::Null);
    assert_eq!(v["destruction"]["required_concurrence"], Value::Null);
    assert_eq!(v["sanitization"]["method"]["kind"], "destroy");
    assert_eq!(v["sanitization"]["method"]["method"], "shred");
}

#[tokio::test]
async fn never_erased_drives_are_certified_from_their_recorded_identifiers() {
    let (certs, key) = shred_scanned(3).await;
    assert_eq!(certs.len(), 3);
    for signed in &certs {
        verify(signed, &[key.verifying_key()]).unwrap();
        let v = serde_json::to_value(&signed.certificate).unwrap();
        assert!(v.get("device").is_none());
        assert!(v.get("capabilities_snapshot").is_none());
        assert_eq!(v["validation"]["media_class"], "unknown");
        let ev = v["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["type"] == "destruction")
            .unwrap();
        assert_eq!(ev["attestation"]["identifiers"]["source"], "scanned");
        assert!(ev["attestation"]["identifiers"]["serial"]
            .as_str()
            .unwrap()
            .starts_with("DEAD-"));
        // Never started: started_at is the Job's created_at.
        assert!(signed.certificate.started_at <= signed.certificate.ended_at);
    }
}

#[tokio::test]
async fn witness_policy_certifies_only_after_concurrence() {
    let runner = runner();
    let key = station_key();
    let mut run = scanned_run(
        &runner,
        DestructMethod::Shred,
        Classification::Low,
        1,
        Some(ConcurrenceKind::Witness),
    )
    .await;
    complete(&mut run);
    let job = runner.get(run.job_ids[0]).unwrap();
    let err = DestructionRecord::build(&job, &run).unwrap_err();
    assert!(err.to_string().contains("not completed"), "{err}");

    concur(&mut run, ConcurrenceKind::Witness);
    let job = destroy(&runner, &run).remove(0);
    let signed = destroyed_cert(&job, &run, &key);
    verify(&signed, &[key.verifying_key()]).unwrap();
    let rec = signed.certificate.destruction.as_ref().unwrap();
    assert_eq!(rec.required_concurrence, Some(ConcurrenceKind::Witness));
    assert_eq!(
        rec.concurrence.as_ref().map(|c| c.kind),
        Some(ConcurrenceKind::Witness)
    );
    assert_eq!(rec.caution, None, "no caution at Low");
}

#[tokio::test]
async fn techniques_are_labelled_honestly() {
    let runner = runner();
    let key = station_key();
    let mut certs = Vec::new();
    for technique in [DestructMethod::Crush, DestructMethod::Shred] {
        let mut run = scanned_run(&runner, technique, Classification::Moderate, 1, None).await;
        complete(&mut run);
        let job = destroy(&runner, &run).remove(0);
        let signed = destroyed_cert(&job, &run, &key);
        verify(&signed, &[key.verifying_key()]).unwrap();
        certs.push(signed.certificate);
    }
    let (crush, shred) = (&certs[0], &certs[1]);
    let rec = crush.destruction.as_ref().unwrap();
    assert_eq!(rec.standard_label, "physical damage — not NIST Destroy");
    assert!(!rec.nist_destroy);
    assert_eq!(rec.caution, None);
    assert_eq!(crush.sanitization.category, None);
    let v = serde_json::to_value(crush).unwrap();
    assert!(v["sanitization"].get("category").is_none());
    assert!(!v.to_string().contains("NIST SP 800-88 Destroy\""));

    let rec = shred.destruction.as_ref().unwrap();
    assert_eq!(rec.standard_label, "NIST SP 800-88 Destroy");
    assert!(rec.nist_destroy);
    assert_eq!(rec.caution.as_deref(), Some(SHRED_PULVERIZE_CAUTION));
    assert_eq!(shred.sanitization.category, Some(Category::Destroy));
}

#[tokio::test]
async fn edited_statements_fail_verification_even_when_re_signed() {
    let (certs, key) = shred_scanned(1).await;
    let signed = &certs[0];

    let msg = tamper(signed, &key, |c| {
        record(c).completion.statement.push_str(" Also: nothing.");
    });
    assert!(msg.contains("completion statement"), "{msg}");

    let msg = tamper(signed, &key, |c| {
        record(c).completion.verification.criterion = "looked fine".into();
    });
    assert!(msg.contains("completion statement"), "{msg}");

    let msg = tamper(signed, &key, |c| {
        for a in &mut c.activities {
            if let JobActivity::Destruction(e) = a {
                e.attestation.as_mut().unwrap().location = "Elsewhere".into();
            }
        }
    });
    assert!(msg.contains("recorded-for-destruction statement"), "{msg}");

    let msg = tamper(signed, &key, |c| {
        record(c).completion.verification.remnants_inspected = false;
    });
    assert!(msg.contains("inspected remnants"), "{msg}");
}

#[tokio::test]
async fn concurrence_must_match_the_runs_requirement() {
    let runner = runner();
    let key = station_key();
    let mut run = scanned_run(
        &runner,
        DestructMethod::Melt,
        Classification::High,
        1,
        Some(ConcurrenceKind::Review),
    )
    .await;
    complete(&mut run);
    concur(&mut run, ConcurrenceKind::Review);
    let job = destroy(&runner, &run).remove(0);
    let signed = destroyed_cert(&job, &run, &key);
    verify(&signed, &[key.verifying_key()]).unwrap();

    // Missing while required.
    let msg = tamper(&signed, &key, |c| record(c).concurrence = None);
    assert!(
        msg.contains("required Review concurrence is missing"),
        "{msg}"
    );

    // A reviewer recorded as a witness: the kind...
    let msg = tamper(&signed, &key, |c| {
        let rec = record(c);
        let k = rec.concurrence.as_mut().unwrap();
        k.kind = ConcurrenceKind::Witness;
        k.statement =
            concurrence_statement(k.kind, k.manifest_id, &k.signer.full_name, &k.signer.title)
                .unwrap();
    });
    assert!(
        msg.contains("is a Witness, but the run required a Review"),
        "{msg}"
    );
    // ...or only the wording.
    let msg = tamper(&signed, &key, |c| {
        let k = record(c).concurrence.as_mut().unwrap();
        k.statement = concurrence_statement(
            ConcurrenceKind::Witness,
            k.manifest_id,
            &k.signer.full_name,
            &k.signer.title,
        )
        .unwrap();
    });
    assert!(msg.contains("concurrence statement"), "{msg}");

    // The completion's signer concurring with themself.
    let msg = tamper(&signed, &key, |c| {
        let rec = record(c);
        let signer = rec.completion.signer.clone();
        let k = rec.concurrence.as_mut().unwrap();
        k.statement =
            concurrence_statement(k.kind, k.manifest_id, &signer.full_name, &signer.title).unwrap();
        k.signer = signer;
    });
    assert!(msg.contains("whoever confirmed completion"), "{msg}");

    // A concurrence for another run, its statement re-rendered to match.
    let msg = tamper(&signed, &key, |c| {
        let k = record(c).concurrence.as_mut().unwrap();
        k.manifest_id = uuid::Uuid::new_v4();
        k.statement =
            concurrence_statement(k.kind, k.manifest_id, &k.signer.full_name, &k.signer.title)
                .unwrap();
    });
    assert!(
        msg.contains("the concurrence names another destruction run"),
        "{msg}"
    );

    // A concurrence the run did not require.
    let msg = tamper(&signed, &key, |c| record(c).required_concurrence = None);
    assert!(msg.contains("did not require"), "{msg}");
}

#[tokio::test]
async fn the_label_must_match_the_technique() {
    let runner = runner();
    let key = station_key();
    let mut run = scanned_run(&runner, DestructMethod::Crush, Classification::Low, 1, None).await;
    complete(&mut run);
    let job = destroy(&runner, &run).remove(0);
    let signed = destroyed_cert(&job, &run, &key);

    let msg = tamper(&signed, &key, |c| {
        record(c).standard_label = "NIST SP 800-88 Destroy".into();
    });
    assert!(msg.contains("standard_label"), "{msg}");
    let msg = tamper(&signed, &key, |c| record(c).nist_destroy = true);
    assert!(msg.contains("nist_destroy"), "{msg}");
    let msg = tamper(&signed, &key, |c| {
        c.sanitization.category = Some(Category::Destroy);
    });
    assert!(msg.contains("sanitization.category"), "{msg}");
    let msg = tamper(&signed, &key, |c| {
        record(c).caution = Some(SHRED_PULVERIZE_CAUTION.into());
    });
    assert!(msg.contains("caution"), "{msg}");
    // Re-labelling the technique itself breaks the link to the attested event.
    let msg = tamper(&signed, &key, |c| {
        let rec = record(c);
        rec.technique = DestructMethod::Shred;
        rec.standard_label = DestructMethod::Shred.standard_label().into();
        rec.nist_destroy = true;
    });
    assert!(msg.contains("technique differs"), "{msg}");
    // Detaching the run link.
    let msg = tamper(&signed, &key, |c| {
        record(c).manifest_id = uuid::Uuid::new_v4()
    });
    assert!(msg.contains("not linked"), "{msg}");
}

#[tokio::test]
async fn destruction_belongs_only_on_a_destroyed_certificate() {
    let runner = runner();
    let key = station_key();
    let (certs, _) = shred_scanned(1).await;
    let record = certs[0].certificate.destruction.clone();
    let job = erased_job(&runner).await;
    let erased = erased_cert(&job, &key);

    let msg = tamper(&erased, &key, |c| c.destruction = record);
    assert!(
        msg.contains("destruction record on a certificate whose disposition is Erased"),
        "{msg}"
    );

    let msg = tamper(&certs[0], &key, |c| c.destruction = None);
    assert!(
        msg.contains("destroyed certificate has no destruction record"),
        "{msg}"
    );
}

#[tokio::test]
async fn the_record_must_name_its_attested_destruction_activity() {
    let runner = runner();
    let key = station_key();
    let mut run = erased_first_run(&runner, DestructMethod::Shred, Classification::Low, None).await;
    complete(&mut run);
    let job = destroy(&runner, &run).remove(0);
    let signed = destroyed_cert(&job, &run, &key);
    verify(&signed, &[key.verifying_key()]).unwrap();

    let msg = tamper(&signed, &key, |c| {
        record(c).destruction_event_id = uuid::Uuid::new_v4();
    });
    assert!(
        msg.contains("destruction_event_id names no activity"),
        "{msg}"
    );

    let msg = tamper(&signed, &key, |c| {
        let erasure = c
            .activities
            .iter()
            .find(|a| matches!(a, JobActivity::Erasure(_)))
            .unwrap()
            .id();
        record(c).destruction_event_id = erasure;
    });
    assert!(
        msg.contains("destruction_event_id does not name a Destruction activity"),
        "{msg}"
    );

    let msg = tamper(&signed, &key, |c| {
        for a in &mut c.activities {
            if let JobActivity::Destruction(e) = a {
                e.attestation = None;
            }
        }
    });
    assert!(
        msg.contains("the Destruction activity has no attestation"),
        "{msg}"
    );
}

#[tokio::test]
async fn sanitization_and_completion_flags_are_checked() {
    let (certs, key) = shred_scanned(1).await;
    let signed = &certs[0];

    let msg = tamper(signed, &key, |c| {
        c.sanitization.method = Method::Destroy {
            method: DestructMethod::Crush,
        };
    });
    assert!(
        msg.contains("sanitization.method is not the destruction technique"),
        "{msg}"
    );

    let msg = tamper(signed, &key, |c| {
        record(c).completion.verification.equipment_cycle_completed = false;
    });
    assert!(msg.contains("a completed equipment cycle"), "{msg}");
}

#[tokio::test]
async fn a_v3_certificate_with_a_co_signature_is_refused() {
    let (certs, key) = shred_scanned(1).await;
    let mut signed = certs[0].clone();
    signed.co_signatures.push(CoSignatureBlock {
        signature: signed.signature.clone(),
        role: CoSignerRole::Supervisor,
        manifest_ref: None,
        signer: operator("sup-1", "Sandra"),
        signed_at: time::OffsetDateTime::now_utc(),
    });
    let msg = verify_err(&signed, &key);
    assert!(msg.contains("co-signature"), "{msg}");
}

#[tokio::test]
async fn an_unknown_format_version_is_refused_by_name() {
    let (certs, key) = shred_scanned(1).await;
    let msg = tamper(&certs[0], &key, |c| c.cert_format_version = 4);
    assert_eq!(msg, "unsupported cert_format_version 4");
}

#[tokio::test]
async fn a_signature_failure_is_reported_before_content() {
    let (certs, key) = shred_scanned(1).await;
    let mut signed = certs[0].clone();
    record(&mut signed.certificate).completion.statement = "edited".into();
    let msg = verify_err(&signed, &key);
    assert!(msg.contains("SHA-256 mismatch"), "{msg}");
}

fn issuer_stub(key: &SigningKey) -> wipe_cert::CertIssuer {
    wipe_cert::CertIssuer {
        tool_name: "wipestation".into(),
        tool_version: "0.1.0".into(),
        public_key_id: key.public_key_id(),
    }
}

fn media() -> wipe_cert::MediaStatus {
    wipe_cert::MediaStatus {
        operational: false,
        damaged: true,
        notes: None,
    }
}
