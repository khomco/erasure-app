//! Content checks run by [`crate::verify`] after the signature holds.
//!
//! A valid signature proves the bytes are the ones the station signed; it
//! does not prove they agree with themselves. From v3 a destroyed drive's
//! certificate carries the destruction record inside the signed bytes, so
//! `verify` also refuses a v3 certificate whose record contradicts itself: a
//! statement that does not re-render from its fields, a reviewer stored as a
//! witness, a missing required concurrence, a label that does not match the
//! technique. Statements are re-rendered through the `wipe_common` functions
//! that rendered them, never a second copy of the wording.
//!
//! v2 certificates are checked by signature only, exactly as before v3.

use wipe_common::{
    attestation_statement, completion_statement, concurrence_statement, same_signer,
    AssetDisposition, Category, JobActivity, Method,
};

use crate::{CertError, CertResult, Certificate, DestructionRecord, SignedCertificate};

/// Dispatch on `cert_format_version`: 2 passes (signature-only), 3 runs the
/// v3 checks, anything else is refused by name.
pub fn check_contents(signed: &SignedCertificate) -> CertResult<()> {
    match signed.certificate.cert_format_version {
        2 => Ok(()),
        3 => check_v3(signed),
        v => Err(CertError::Verification(format!(
            "unsupported cert_format_version {v}"
        ))),
    }
}

fn fail(check: &str) -> CertError {
    CertError::Verification(format!("v3 content check failed: {check}"))
}

fn check_v3(signed: &SignedCertificate) -> CertResult<()> {
    if !signed.co_signatures.is_empty() {
        return Err(fail(
            "a v3 certificate carries any concurrence inside the signed bytes; a co-signature \
             can only have been appended afterwards",
        ));
    }
    let cert = &signed.certificate;
    match (&cert.destruction, cert.disposition) {
        (Some(record), AssetDisposition::Destroyed) => check_destruction(cert, record),
        (None, AssetDisposition::Destroyed) => {
            Err(fail("a destroyed certificate has no destruction record"))
        }
        (Some(_), other) => Err(fail(&format!(
            "a destruction record on a certificate whose disposition is {other:?}"
        ))),
        (None, _) => Ok(()),
    }
}

fn check_destruction(cert: &Certificate, rec: &DestructionRecord) -> CertResult<()> {
    let technique = rec.technique;

    // The record names the attested Destruction activity it summarises.
    let event = cert
        .activities
        .iter()
        .find(|a| a.id() == rec.destruction_event_id)
        .ok_or_else(|| fail("destruction_event_id names no activity"))?;
    let JobActivity::Destruction(event) = event else {
        return Err(fail(
            "destruction_event_id does not name a Destruction activity",
        ));
    };
    let attestation = event
        .attestation
        .as_ref()
        .ok_or_else(|| fail("the Destruction activity has no attestation"))?;
    if event.method != technique {
        return Err(fail(
            "the Destruction activity's technique differs from the record's",
        ));
    }
    if event.manifest_ref != Some(rec.manifest_id) {
        return Err(fail(
            "the Destruction activity is not linked to the record's destruction run",
        ));
    }

    // The label, flag and caution follow from the technique.
    if rec.standard_label != technique.standard_label() {
        return Err(fail("standard_label does not match the technique"));
    }
    if rec.nist_destroy != technique.is_nist_destroy() {
        return Err(fail("nist_destroy does not match the technique"));
    }
    if rec.caution.as_deref() != technique.caution(cert.spec.classification) {
        return Err(fail(
            "caution does not match the technique and classification",
        ));
    }
    if cert.sanitization.method != (Method::Destroy { method: technique }) {
        return Err(fail("sanitization.method is not the destruction technique"));
    }
    let category = technique.is_nist_destroy().then_some(Category::Destroy);
    if cert.sanitization.category != category {
        return Err(fail(
            "sanitization.category must be destroy for a NIST Destroy technique and absent \
             otherwise",
        ));
    }

    // Each statement re-renders from its own fields.
    let rendered = attestation_statement(
        technique,
        &attestation.signer.full_name,
        &attestation.signer.title,
        &attestation.equipment_id,
        &attestation.location,
    );
    if rendered.as_ref() != Ok(&attestation.statement) {
        return Err(fail(
            "the recorded-for-destruction statement does not match its fields",
        ));
    }
    let completion = &rec.completion;
    if !completion.verification.remnants_inspected
        || !completion.verification.equipment_cycle_completed
    {
        return Err(fail(
            "the completion does not record inspected remnants and a completed equipment cycle",
        ));
    }
    let rendered = completion_statement(
        completion.completed_at,
        &completion.verification.equipment_id,
        &completion.verification.criterion,
        completion.video_ref.as_deref(),
    );
    if rendered.as_ref() != Ok(&completion.statement) {
        return Err(fail("the completion statement does not match its fields"));
    }

    // Concurrence: present exactly when required, of that kind, for this
    // run, from someone other than the completion's signer.
    match (rec.required_concurrence, &rec.concurrence) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err(fail("a concurrence the run did not require")),
        (Some(kind), None) => Err(fail(&format!(
            "the required {kind:?} concurrence is missing"
        ))),
        (Some(kind), Some(c)) => {
            if c.kind != kind {
                return Err(fail(&format!(
                    "the concurrence is a {:?}, but the run required a {kind:?}",
                    c.kind
                )));
            }
            if c.manifest_id != rec.manifest_id {
                return Err(fail("the concurrence names another destruction run"));
            }
            let rendered =
                concurrence_statement(c.kind, c.manifest_id, &c.signer.full_name, &c.signer.title);
            if rendered.as_ref() != Ok(&c.statement) {
                return Err(fail("the concurrence statement does not match its fields"));
            }
            if same_signer(&completion.signer, &c.signer) {
                return Err(fail(
                    "the concurrence was signed by whoever confirmed completion",
                ));
            }
            Ok(())
        }
    }
}
