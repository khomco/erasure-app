//! What a station signs to enroll with the hub (station-enrollment story 1,
//! ADR-0008 §1): the request bodies, the messages signed, the signer and the
//! verifier, here so the hub (which verifies) and the station (which signs,
//! story 3) share one implementation.
//!
//! Start and poll are the first calls a station makes, before any hub
//! challenge exists (ADR-0008 §2 covers the signed requests *after*
//! enrollment), so each signs a domain-separated message without one:
//!
//! - start: [`START_DOMAIN`] followed by the JSON of `{instance_key_id,
//!   public_key_b64, station_id, label}` in that field order, with the values
//!   exactly as sent (`label` is `null` when absent);
//! - poll: [`POLL_DOMAIN`] followed by the device code, as sent.
//!
//! A replayed start only replaces the station's pending enrollment, and a
//! replayed poll yields a licence usable only with the instance key, which
//! ADR-0008 already accepts.
//!
//! Keys are the `wipe_cert` instance key: the public key is its 32 raw bytes
//! and signatures their 64, both in unpadded standard base64
//! (`wipe_cert::VerifyingKey::to_base64`). Verification is strict
//! (`verify_strict`), so weak or non-canonical keys and signatures fail.

use base64::{engine::general_purpose::STANDARD_NO_PAD as B64, Engine as _};
use ed25519_dalek::{Signature, Signer, VerifyingKey as Ed25519VerifyingKey};
use serde::{Deserialize, Serialize};
use wipe_cert::{SigningKey, VerifyingKey};

/// Prefix of every enrollment start message.
pub const START_DOMAIN: &str = "wipestation-enrollment-start-v1\n";
/// Prefix of every enrollment poll message.
pub const POLL_DOMAIN: &str = "wipestation-enrollment-poll-v1\n";

/// `POST /api/enrollments`: a station asks to enroll.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartRequest {
    pub instance_key_id: String,
    pub public_key_b64: String,
    pub station_id: String,
    #[serde(default)]
    pub label: Option<String>,
    pub signature_b64: String,
}

/// `POST /api/enrollments/poll`: a station asks how its enrollment stands.
/// Missing fields are empty strings, which never verify.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollRequest {
    #[serde(default)]
    pub device_code: String,
    #[serde(default)]
    pub signature_b64: String,
}

/// Why a start or poll signature was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EnrollmentSignatureError {
    /// The public key is not 32 bytes of unpadded standard base64, or not a
    /// valid Ed25519 point.
    #[error("the public key is not a valid Ed25519 key in unpadded base64")]
    BadPublicKey,
    /// The instance key id is not the id of the public key sent with it.
    #[error("the instance key id does not belong to the public key")]
    KeyIdMismatch,
    /// The signature does not decode, or does not verify over the message.
    #[error("the signature does not verify")]
    BadSignature,
}

#[derive(Serialize)]
struct StartFields<'a> {
    instance_key_id: &'a str,
    public_key_b64: &'a str,
    station_id: &'a str,
    label: Option<&'a str>,
}

/// The bytes a station signs to start enrolling.
pub fn start_message(
    instance_key_id: &str,
    public_key_b64: &str,
    station_id: &str,
    label: Option<&str>,
) -> Vec<u8> {
    let fields = StartFields {
        instance_key_id,
        public_key_b64,
        station_id,
        label,
    };
    let mut out = START_DOMAIN.as_bytes().to_vec();
    // Serializing borrowed strings and an option cannot fail.
    out.extend(serde_json::to_vec(&fields).expect("start fields serialize"));
    out
}

/// The bytes a station signs to poll with `device_code`.
pub fn poll_message(device_code: &str) -> Vec<u8> {
    let mut out = POLL_DOMAIN.as_bytes().to_vec();
    out.extend_from_slice(device_code.as_bytes());
    out
}

fn sign(key: &SigningKey, message: &[u8]) -> String {
    B64.encode(key.0.sign(message).to_bytes())
}

/// A signed [`StartRequest`] for `key`'s station `station_id`.
pub fn sign_start(key: &SigningKey, station_id: &str, label: Option<&str>) -> StartRequest {
    let instance_key_id = key.public_key_id();
    let public_key_b64 = key.verifying_key().to_base64();
    let signature_b64 = sign(
        key,
        &start_message(&instance_key_id, &public_key_b64, station_id, label),
    );
    StartRequest {
        instance_key_id,
        public_key_b64,
        station_id: station_id.to_string(),
        label: label.map(str::to_string),
        signature_b64,
    }
}

/// A signed [`PollRequest`] for `device_code`.
pub fn sign_poll(key: &SigningKey, device_code: &str) -> PollRequest {
    PollRequest {
        device_code: device_code.to_string(),
        signature_b64: sign(key, &poll_message(device_code)),
    }
}

/// The public key in `public_key_b64`.
pub fn decode_public_key(public_key_b64: &str) -> Result<VerifyingKey, EnrollmentSignatureError> {
    let bytes: [u8; 32] = B64
        .decode(public_key_b64)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(EnrollmentSignatureError::BadPublicKey)?;
    Ed25519VerifyingKey::from_bytes(&bytes)
        .map(VerifyingKey)
        .map_err(|_| EnrollmentSignatureError::BadPublicKey)
}

fn verify(
    key: &VerifyingKey,
    message: &[u8],
    signature_b64: &str,
) -> Result<(), EnrollmentSignatureError> {
    let bytes: [u8; 64] = B64
        .decode(signature_b64)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(EnrollmentSignatureError::BadSignature)?;
    key.0
        .verify_strict(message, &Signature::from_bytes(&bytes))
        .map_err(|_| EnrollmentSignatureError::BadSignature)
}

/// Check a start request: the public key decodes, the key id is exactly its
/// id, and the signature verifies over [`start_message`] of the fields as
/// sent. The verified key.
pub fn verify_start(req: &StartRequest) -> Result<VerifyingKey, EnrollmentSignatureError> {
    let key = decode_public_key(&req.public_key_b64)?;
    if key.public_key_id() != req.instance_key_id {
        return Err(EnrollmentSignatureError::KeyIdMismatch);
    }
    verify(
        &key,
        &start_message(
            &req.instance_key_id,
            &req.public_key_b64,
            &req.station_id,
            req.label.as_deref(),
        ),
        &req.signature_b64,
    )?;
    Ok(key)
}

/// Check a poll's signature by the enrollment's stored `public_key_b64`.
pub fn verify_poll(
    public_key_b64: &str,
    req: &PollRequest,
) -> Result<(), EnrollmentSignatureError> {
    let key = decode_public_key(public_key_b64)?;
    verify(&key, &poll_message(&req.device_code), &req.signature_b64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start_err(r: &StartRequest) -> EnrollmentSignatureError {
        verify_start(r).err().expect("refused")
    }

    #[test]
    fn a_signed_start_verifies_and_names_its_key() {
        let key = SigningKey::generate();
        let req = sign_start(&key, "bench-1", Some("Bench 1"));
        assert_eq!(req.instance_key_id, key.public_key_id());
        let verified = verify_start(&req).unwrap();
        assert_eq!(verified.public_key_id(), key.public_key_id());
        // No label is signed as null.
        verify_start(&sign_start(&key, "bench-1", None)).unwrap();
    }

    #[test]
    fn the_start_message_is_pinned() {
        let msg = start_message("ed25519:k", "pk", "s", None);
        assert_eq!(
            String::from_utf8(msg).unwrap(),
            "wipestation-enrollment-start-v1\n\
             {\"instance_key_id\":\"ed25519:k\",\"public_key_b64\":\"pk\",\
             \"station_id\":\"s\",\"label\":null}"
        );
        let msg = poll_message("abc");
        assert_eq!(
            String::from_utf8(msg).unwrap(),
            "wipestation-enrollment-poll-v1\nabc"
        );
    }

    #[test]
    fn a_tampered_start_field_fails() {
        let key = SigningKey::generate();
        let good = sign_start(&key, "bench-1", Some("Bench 1"));
        let mut r = good.clone();
        r.station_id = "bench-2".into();
        assert_eq!(start_err(&r), EnrollmentSignatureError::BadSignature);
        let mut r = good.clone();
        r.label = None;
        assert_eq!(start_err(&r), EnrollmentSignatureError::BadSignature);
        let mut r = good.clone();
        r.label = Some(" Bench 1".into());
        assert_eq!(start_err(&r), EnrollmentSignatureError::BadSignature);
        let mut r = good.clone();
        r.signature_b64 = "not base64!".into();
        assert_eq!(start_err(&r), EnrollmentSignatureError::BadSignature);
    }

    #[test]
    fn a_start_signed_by_another_key_fails() {
        let key = SigningKey::generate();
        let other = SigningKey::generate();
        let mut r = sign_start(&key, "bench-1", None);
        // The other key's signature over the same fields.
        r.signature_b64 = sign(
            &other,
            &start_message(&r.instance_key_id, &r.public_key_b64, &r.station_id, None),
        );
        assert_eq!(start_err(&r), EnrollmentSignatureError::BadSignature);
        // Another key's id with this key.
        let mut r = sign_start(&key, "bench-1", None);
        r.instance_key_id = other.public_key_id();
        assert_eq!(start_err(&r), EnrollmentSignatureError::KeyIdMismatch);
    }

    #[test]
    fn a_malformed_public_key_fails() {
        let key = SigningKey::generate();
        for bad in [
            "",
            "AAAA",
            "!!!",
            &format!("{}=", key.verifying_key().to_base64()),
        ] {
            let mut r = sign_start(&key, "bench-1", None);
            r.public_key_b64 = bad.to_string();
            assert_eq!(
                start_err(&r),
                EnrollmentSignatureError::BadPublicKey,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn polls_verify_only_with_the_signing_key_and_code() {
        let key = SigningKey::generate();
        let other = SigningKey::generate();
        let pk = key.verifying_key().to_base64();
        let poll = sign_poll(&key, "code-1");
        verify_poll(&pk, &poll).unwrap();
        assert_eq!(
            verify_poll(&other.verifying_key().to_base64(), &poll).unwrap_err(),
            EnrollmentSignatureError::BadSignature
        );
        let mut tampered = poll.clone();
        tampered.device_code = "code-2".into();
        assert_eq!(
            verify_poll(&pk, &tampered).unwrap_err(),
            EnrollmentSignatureError::BadSignature
        );
        // A start signature is never a poll signature (domain separation).
        let start = sign_start(&key, "code-1", None);
        let crossed = PollRequest {
            device_code: "code-1".into(),
            signature_b64: start.signature_b64,
        };
        assert!(verify_poll(&pk, &crossed).is_err());
        let empty: PollRequest = serde_json::from_str("{}").unwrap();
        assert!(verify_poll(&pk, &empty).is_err());
    }
}
