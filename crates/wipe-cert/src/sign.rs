//! Detached Ed25519 signatures over canonical cert JSON.

use base64::{engine::general_purpose::STANDARD_NO_PAD as B64, Engine as _};
use ed25519_dalek::{
    Signature as Ed25519Signature, Signer, SigningKey as Ed25519SigningKey, Verifier,
    VerifyingKey as Ed25519VerifyingKey, SECRET_KEY_LENGTH,
};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureBlock {
    pub algorithm: String,
    pub public_key_id: String,
    pub canonical_sha256_hex: String,
    pub signature_b64: String,
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
    })
}

/// Verify a signed certificate against a list of trusted public keys.
/// Returns Ok(matched-key-id) on success, Err on any failure.
pub fn verify(signed: &SignedCertificate, trusted: &[VerifyingKey]) -> CertResult<String> {
    if signed.signature.algorithm != "ed25519" {
        return Err(CertError::Verification(format!(
            "unsupported algorithm: {}",
            signed.signature.algorithm
        )));
    }
    let bytes = canonical_bytes(&signed.certificate)?;
    let actual_digest = hex::encode(Sha256::digest(&bytes));
    if actual_digest != signed.signature.canonical_sha256_hex {
        return Err(CertError::Verification(
            "canonical SHA-256 mismatch — cert payload was modified after signing".into(),
        ));
    }

    let sig_bytes = B64
        .decode(&signed.signature.signature_b64)
        .map_err(|e| CertError::Verification(format!("base64: {e}")))?;
    let sig = Ed25519Signature::from_slice(&sig_bytes)
        .map_err(|e| CertError::Verification(e.to_string()))?;

    for vk in trusted {
        if vk.public_key_id() != signed.signature.public_key_id {
            continue;
        }
        return match vk.0.verify(&bytes, &sig) {
            Ok(()) => Ok(vk.public_key_id()),
            Err(e) => Err(CertError::Verification(format!("signature: {e}"))),
        };
    }
    Err(CertError::Verification(format!(
        "no trusted key matches public_key_id {}",
        signed.signature.public_key_id
    )))
}
