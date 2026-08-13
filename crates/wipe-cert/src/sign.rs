//! Detached Ed25519 signatures over canonical cert JSON.
//!
//! The primary signature is the station/operator signature attached at
//! cert generation. `co_signatures` carry additional attestations —
//! today, supervisor co-sign on Destroy certs (per ADR-0001 batched
//! manifest co-sign). Each co-signer signs the same canonical Certificate
//! bytes with their own key; the `manifest_ref` field records which
//! `DestructionManifest` the co-sign was rolled into.

use base64::{engine::general_purpose::STANDARD_NO_PAD as B64, Engine as _};
use ed25519_dalek::{
    Signature as Ed25519Signature, Signer, SigningKey as Ed25519SigningKey, Verifier,
    VerifyingKey as Ed25519VerifyingKey, SECRET_KEY_LENGTH,
};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use wipe_common::OperatorRef;

use crate::{canonical::canonical_bytes, schema::Certificate, CertError, CertResult};

pub struct SigningKey(pub Ed25519SigningKey);
pub struct VerifyingKey(pub Ed25519VerifyingKey);

impl SigningKey {
    pub fn generate() -> Self {
        Self(Ed25519SigningKey::generate(&mut OsRng))
    }

    pub fn from_seed(seed: [u8; SECRET_KEY_LENGTH]) -> Self {
        Self(Ed25519SigningKey::from_bytes(&seed))
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        VerifyingKey(self.0.verifying_key())
    }

    pub fn public_key_id(&self) -> String {
        self.verifying_key().public_key_id()
    }
}

impl VerifyingKey {
    pub fn from_bytes(bytes: &[u8; 32]) -> CertResult<Self> {
        Ed25519VerifyingKey::from_bytes(bytes)
            .map(Self)
            .map_err(|e| CertError::Invalid(e.to_string()))
    }

    /// Public-key identifier — first 16 bytes of SHA-256 of the raw key bytes,
    /// base64 encoded. Short, stable, suitable for embedding in certs.
    pub fn public_key_id(&self) -> String {
        let bytes = self.0.to_bytes();
        let hash = Sha256::digest(bytes);
        format!("ed25519:{}", B64.encode(&hash[..16]))
    }

    pub fn to_base64(&self) -> String {
        B64.encode(self.0.to_bytes())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedCertificate {
    pub certificate: Certificate,
    pub signature: SignatureBlock,
    /// Additional attestations on the same cert payload. Empty for Erased
    /// certs; for Destroyed certs, contains at least one Supervisor co-sig
    /// attached when the linked `DestructionManifest` is signed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub co_signatures: Vec<CoSignatureBlock>,
    /// Vendor attestation chain (ADR-0005). `None` is a valid, fully
    /// verifiable *evaluation* certificate — see `Certificate.evaluation`.
    ///
    /// Carried as opaque JSON so `wipe-cert` does not depend on
    /// `wipe-license`: the licence model must be free to evolve without
    /// touching the certificate crate, and a verifier that only cares about
    /// the erasure signature never needs to parse this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureBlock {
    pub algorithm: String,
    pub public_key_id: String,
    pub canonical_sha256_hex: String,
    pub signature_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoSignatureBlock {
    pub signature: SignatureBlock,
    pub role: CoSignerRole,
    /// The DestructionManifest this co-signature was attached as part of,
    /// when applicable. Always set for `Supervisor` co-sigs on Destroyed
    /// certs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_ref: Option<Uuid>,
    pub signer: OperatorRef,
    #[serde(with = "time::serde::rfc3339")]
    pub signed_at: time::OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoSignerRole {
    /// Supervisor co-sign required for Destroyed certs (ADR-0001).
    Supervisor,
    /// Auditor attestation. Future use.
    Auditor,
}

pub fn sign(cert: Certificate, key: &SigningKey) -> CertResult<SignedCertificate> {
    let bytes = canonical_bytes(&cert)?;
    let sig: Ed25519Signature = key.0.sign(&bytes);
    let digest = Sha256::digest(&bytes);
    Ok(SignedCertificate {
        certificate: cert,
        signature: SignatureBlock {
            algorithm: "ed25519".into(),
            public_key_id: key.public_key_id(),
            canonical_sha256_hex: hex::encode(digest),
            signature_b64: B64.encode(sig.to_bytes()),
        },
        co_signatures: Vec::new(),
        attestation: None,
    })
}

/// Attach a vendor attestation chain to an already-signed certificate.
///
/// Deliberately *outside* the signed payload: the chain proves who was
/// entitled to hold the signing key, not what the erasure did. Keeping it out
/// means a licence can be re-stapled (say, after a renewal reissue) without
/// invalidating the erasure signature an auditor already verified.
///
/// The link that makes it a chain rather than two unrelated signatures is
/// checked at verification time — the licence must name the key that signed
/// this certificate.
pub fn attach_attestation(signed: &mut SignedCertificate, attestation: serde_json::Value) {
    signed.attestation = Some(attestation);
}

/// Attach a co-signature to an already-signed certificate. Used for
/// supervisor co-sign on Destroyed certs.
///
/// The co-signer signs the same canonical Certificate bytes the primary
/// signer used, so a verifier with the co-signer's public key can
/// independently confirm "this party attested to this exact cert."
pub fn co_sign(
    signed: &mut SignedCertificate,
    key: &SigningKey,
    role: CoSignerRole,
    signer: OperatorRef,
    manifest_ref: Option<Uuid>,
) -> CertResult<()> {
    let bytes = canonical_bytes(&signed.certificate)?;
    let sig: Ed25519Signature = key.0.sign(&bytes);
    let digest = Sha256::digest(&bytes);
    signed.co_signatures.push(CoSignatureBlock {
        signature: SignatureBlock {
            algorithm: "ed25519".into(),
            public_key_id: key.public_key_id(),
            canonical_sha256_hex: hex::encode(digest),
            signature_b64: B64.encode(sig.to_bytes()),
        },
        role,
        manifest_ref,
        signer,
        signed_at: time::OffsetDateTime::now_utc(),
    });
    Ok(())
}

/// Verify a signed certificate's primary signature against a list of
/// trusted public keys. Returns `Ok(matched-key-id)` on success.
///
/// Co-signatures are verified separately via `verify_co_signatures`.
pub fn verify(signed: &SignedCertificate, trusted: &[VerifyingKey]) -> CertResult<String> {
    verify_one(&signed.certificate, &signed.signature, trusted)
}

/// Verify every co-signature against a list of trusted public keys.
/// Returns the matched key ids in order. A co-signer's key not being in
/// `trusted` produces an error; callers who want a partial-trust model
/// can iterate co-signatures manually.
pub fn verify_co_signatures(
    signed: &SignedCertificate,
    trusted: &[VerifyingKey],
) -> CertResult<Vec<String>> {
    signed
        .co_signatures
        .iter()
        .map(|cs| verify_one(&signed.certificate, &cs.signature, trusted))
        .collect()
}

fn verify_one(
    cert: &Certificate,
    sig: &SignatureBlock,
    trusted: &[VerifyingKey],
) -> CertResult<String> {
    if sig.algorithm != "ed25519" {
        return Err(CertError::Verification(format!(
            "unsupported algorithm: {}",
            sig.algorithm
        )));
    }
    let bytes = canonical_bytes(cert)?;
    let actual_digest = hex::encode(Sha256::digest(&bytes));
    if actual_digest != sig.canonical_sha256_hex {
        return Err(CertError::Verification(
            "canonical SHA-256 mismatch — cert payload was modified after signing".into(),
        ));
    }

    let sig_bytes = B64
        .decode(&sig.signature_b64)
        .map_err(|e| CertError::Verification(format!("base64: {e}")))?;
    let ed_sig = Ed25519Signature::from_slice(&sig_bytes)
        .map_err(|e| CertError::Verification(e.to_string()))?;

    for vk in trusted {
        if vk.public_key_id() != sig.public_key_id {
            continue;
        }
        return match vk.0.verify(&bytes, &ed_sig) {
            Ok(()) => Ok(vk.public_key_id()),
            Err(e) => Err(CertError::Verification(format!("signature: {e}"))),
        };
    }
    Err(CertError::Verification(format!(
        "no trusted key matches public_key_id {}",
        sig.public_key_id
    )))
}
