//! What a station signs on every request to the hub after enrollment
//! (ADR-0008 §2, station-enrollment story 3): the message, the signer and
//! the verifier, here so the station (which signs) and the hub (which
//! verifies, story 5) share one implementation.
//!
//! The station holds no token, password or session: its instance key is its
//! only credential. Each request is signed over a hub-issued, single-use
//! challenge (not a timestamp, so a station whose clock is wrong still
//! works), the hub's origin, the method, the path and a hash of the body.
//! The message is, with `\n` between fields:
//!
//! ```text
//! wipestation-station-request-v1
//! <challenge>
//! <hub origin, scheme://host[:port]>
//! <METHOD, upper case>
//! <path, with any query, exactly as sent>
//! <lowercase hex SHA-256 of the body bytes>
//! ```
//!
//! The request carries the key id, the challenge and the signature in the
//! [`HEADER_KEY_ID`], [`HEADER_CHALLENGE`] and [`HEADER_SIGNATURE`] headers.
//! Signatures are 64 bytes in unpadded standard base64 and verify strictly
//! (`verify_strict`), as in [`crate::enrollment`].

use base64::{engine::general_purpose::STANDARD_NO_PAD as B64, Engine as _};
use ed25519_dalek::Signer;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wipe_cert::{SigningKey, VerifyingKey};

/// Prefix of every station request message.
pub const DOMAIN: &str = "wipestation-station-request-v1\n";
/// Header carrying the signing instance key id.
pub const HEADER_KEY_ID: &str = "x-wipestation-key-id";
/// Header carrying the hub challenge the request is signed over.
pub const HEADER_CHALLENGE: &str = "x-wipestation-challenge";
/// Header carrying the signature.
pub const HEADER_SIGNATURE: &str = "x-wipestation-signature";

/// The three values a signed station request carries in its headers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedStationRequest {
    pub instance_key_id: String,
    pub challenge: String,
    pub signature_b64: String,
}

/// Why a station request's signature was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StationRequestError {
    /// The key id sent is not the id of the key the hub holds for it.
    #[error("the instance key id does not belong to the station's key")]
    KeyIdMismatch,
    /// The signature does not decode, or does not verify over the message.
    #[error("the signature does not verify")]
    BadSignature,
}

/// The bytes a station signs for one request.
pub fn message(challenge: &str, origin: &str, method: &str, path: &str, body: &[u8]) -> Vec<u8> {
    format!(
        "{DOMAIN}{challenge}\n{origin}\n{}\n{path}\n{}",
        method.to_ascii_uppercase(),
        hex::encode(Sha256::digest(body))
    )
    .into_bytes()
}

/// Sign one request to `origin` with the station's instance key.
pub fn sign(
    key: &SigningKey,
    challenge: &str,
    origin: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> SignedStationRequest {
    let msg = message(challenge, origin, method, path, body);
    SignedStationRequest {
        instance_key_id: key.public_key_id(),
        challenge: challenge.to_string(),
        signature_b64: B64.encode(key.0.sign(&msg).to_bytes()),
    }
}

/// Check a request signed by `key` (the key the hub holds for the station).
/// Refuses a key id that is not `key`'s, and a signature that does not
/// strictly verify over [`message`] of what the hub received. Whether the
/// challenge was issued, unexpired and unused is the caller's check.
pub fn verify(
    key: &VerifyingKey,
    req: &SignedStationRequest,
    origin: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<(), StationRequestError> {
    if key.public_key_id() != req.instance_key_id {
        return Err(StationRequestError::KeyIdMismatch);
    }
    crate::enrollment::verify(
        key,
        &message(&req.challenge, origin, method, path, body),
        &req.signature_b64,
    )
    .map_err(|_| StationRequestError::BadSignature)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "https://hub.example";
    const PATH: &str = "/api/station/licence";
    const BODY: &[u8] = b"{\"licence_id\":\"lic-1\"}";

    fn signed(key: &SigningKey) -> SignedStationRequest {
        sign(key, "chal-1", ORIGIN, "post", PATH, BODY)
    }

    #[test]
    fn a_signed_request_round_trips() {
        let key = SigningKey::generate();
        let req = signed(&key);
        assert_eq!(req.instance_key_id, key.public_key_id());
        assert_eq!(req.challenge, "chal-1");
        verify(&key.verifying_key(), &req, ORIGIN, "POST", PATH, BODY).unwrap();
        // The method is upper-cased on both sides.
        verify(&key.verifying_key(), &req, ORIGIN, "post", PATH, BODY).unwrap();
    }

    #[test]
    fn the_message_is_pinned() {
        let msg = message("c", "https://h", "get", "/p?q=1", b"");
        assert_eq!(
            String::from_utf8(msg).unwrap(),
            "wipestation-station-request-v1\nc\nhttps://h\nGET\n/p?q=1\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn each_changed_field_fails() {
        let key = SigningKey::generate();
        let vk = key.verifying_key();
        let req = signed(&key);
        let bad = |r: &SignedStationRequest, origin, method, path, body| {
            assert_eq!(
                verify(&vk, r, origin, method, path, body),
                Err(StationRequestError::BadSignature)
            );
        };
        let mut other_challenge = req.clone();
        other_challenge.challenge = "chal-2".into();
        bad(&other_challenge, ORIGIN, "POST", PATH, BODY);
        bad(&req, "https://other.example", "POST", PATH, BODY);
        bad(&req, ORIGIN, "PUT", PATH, BODY);
        bad(&req, ORIGIN, "POST", "/api/station/licence?x=1", BODY);
        bad(&req, ORIGIN, "POST", PATH, b"{\"licence_id\":\"lic-2\"}");
        bad(&req, ORIGIN, "POST", PATH, b"");
        let mut garbled = req.clone();
        garbled.signature_b64 = "not base64!".into();
        bad(&garbled, ORIGIN, "POST", PATH, BODY);
    }

    #[test]
    fn another_key_fails() {
        let key = SigningKey::generate();
        let other = SigningKey::generate();
        let req = signed(&key);
        // The other key, named honestly: the signature does not verify.
        let mut claimed = req.clone();
        claimed.instance_key_id = other.public_key_id();
        assert_eq!(
            verify(&other.verifying_key(), &claimed, ORIGIN, "POST", PATH, BODY),
            Err(StationRequestError::BadSignature)
        );
        // A key id that is not the verifying key's.
        assert_eq!(
            verify(&other.verifying_key(), &req, ORIGIN, "POST", PATH, BODY),
            Err(StationRequestError::KeyIdMismatch)
        );
        // Signed by the other key over the same request, sent as ours.
        let mut forged = signed(&other);
        forged.instance_key_id = key.public_key_id();
        assert_eq!(
            verify(&key.verifying_key(), &forged, ORIGIN, "POST", PATH, BODY),
            Err(StationRequestError::BadSignature)
        );
    }
}
