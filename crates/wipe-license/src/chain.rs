//! The attestation chain (ADR-0005 §1).
//!
//! ```text
//!   vendor root key  --signs-->  LicenseCertificate  --names-->  instance key
//!                                                                     |
//!                                                                   signs
//!                                                                     v
//!                                                          erasure certificate
//! ```
//!
//! The license travels inline with every erasure certificate, so an auditor
//! holding only our published root public key can establish — with no network
//! — that the cert was signed by a key the vendor licensed, to a named
//! customer, under stated entitlements, and has not been altered.
//!
//! The vendor key never signs erasure certificates and never sees them. That
//! is what keeps the model offline.

use base64::{engine::general_purpose::STANDARD_NO_PAD as B64, Engine as _};
use ed25519_dalek::{Signature, Signer, Verifier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use wipe_cert::{canonical::canonical_bytes, SigningKey, VerifyingKey};

use crate::entitlement::Entitlements;
use crate::{LicenseError, LicenseResult};

pub const LICENSE_FORMAT_VERSION: u32 = 1;

/// The body a vendor signs. Split from the signature so canonicalization has
/// exactly one subject, the same discipline `wipe-cert` already uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseBody {
    pub license_format_version: u32,
    /// Stable handle for support and for a future online CRL. Offline
    /// revocation is not solved — see ADR-0005 consequences.
    pub license_id: String,
    /// Which vendor root signed this, so a verifier can select among roots
    /// if we ever cross-sign a successor.
    pub root_key_id: String,
    /// The instance key this license entitles. Binding the *key id* rather
    /// than a machine name is what makes the chain checkable offline.
    pub instance_public_key_id: String,
    pub entitlements: Entitlements,
    #[serde(with = "time::serde::rfc3339")]
    pub issued_at: OffsetDateTime,
}

/// A vendor-signed license.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseCertificate {
    pub body: LicenseBody,
    pub signature: LicenseSignature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseSignature {
    pub algorithm: String,
    /// Vendor root key that produced this signature.
    pub public_key_id: String,
    pub canonical_sha256_hex: String,
    pub signature_b64: String,
}

/// The vendor root. In production the private half never leaves an offline
/// signer; nothing in the station binary needs it.
pub struct VendorRoot {
    key: SigningKey,
}

impl VendorRoot {
    pub fn from_signing_key(key: SigningKey) -> Self {
        Self { key }
    }

    pub fn generate() -> Self {
        Self {
            key: SigningKey::generate(),
        }
    }

    pub fn public_key_id(&self) -> String {
        self.key.public_key_id()
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.key.verifying_key()
    }

    /// Raw seed, for writing to an offline signer's key file. The only place
    /// this should ever be called is the issuance tool.
    pub fn seed_bytes(&self) -> [u8; 32] {
        self.key.0.to_bytes()
    }

    /// Issue a license binding `instance_public_key_id` to `entitlements`.
    pub fn issue(
        &self,
        license_id: impl Into<String>,
        instance_public_key_id: impl Into<String>,
        entitlements: Entitlements,
        issued_at: OffsetDateTime,
    ) -> LicenseResult<LicenseCertificate> {
        let body = LicenseBody {
            license_format_version: LICENSE_FORMAT_VERSION,
            license_id: license_id.into(),
            root_key_id: self.public_key_id(),
            instance_public_key_id: instance_public_key_id.into(),
            entitlements,
            issued_at,
        };
        let bytes = canonical_bytes(&body).map_err(|e| LicenseError::Encoding(e.to_string()))?;
        let sig: Signature = self.key.0.sign(&bytes);
        Ok(LicenseCertificate {
            signature: LicenseSignature {
                algorithm: "ed25519".into(),
                public_key_id: self.public_key_id(),
                canonical_sha256_hex: hex::encode(Sha256::digest(&bytes)),
                signature_b64: B64.encode(sig.to_bytes()),
            },
            body,
        })
    }
}

impl LicenseCertificate {
    /// Verify this license against a set of trusted vendor root keys.
    ///
    /// Checks the payload digest before the signature so a mutated license
    /// reports as tampered rather than as an opaque signature failure.
    pub fn verify(&self, roots: &[VerifyingKey]) -> LicenseResult<String> {
        if self.signature.algorithm != "ed25519" {
            return Err(LicenseError::Verification(format!(
                "unsupported algorithm: {}",
                self.signature.algorithm
            )));
        }
        if self.body.license_format_version != LICENSE_FORMAT_VERSION {
            return Err(LicenseError::Verification(format!(
                "license_format_version {} but this build understands {}",
                self.body.license_format_version, LICENSE_FORMAT_VERSION
            )));
        }

        let bytes =
            canonical_bytes(&self.body).map_err(|e| LicenseError::Encoding(e.to_string()))?;
        let digest = hex::encode(Sha256::digest(&bytes));
        if digest != self.signature.canonical_sha256_hex {
            return Err(LicenseError::Tampered(
                "license payload was modified after signing".into(),
            ));
        }

        let sig_bytes = B64
            .decode(&self.signature.signature_b64)
            .map_err(|e| LicenseError::Verification(format!("base64: {e}")))?;
        let sig = Signature::from_slice(&sig_bytes)
            .map_err(|e| LicenseError::Verification(e.to_string()))?;

        for root in roots {
            if root.public_key_id() != self.signature.public_key_id {
                continue;
            }
            return match root.0.verify(&bytes, &sig) {
                Ok(()) => Ok(root.public_key_id()),
                Err(e) => Err(LicenseError::Tampered(format!("license signature: {e}"))),
            };
        }
        Err(LicenseError::UntrustedRoot(
            self.signature.public_key_id.clone(),
        ))
    }
}

/// What rides inside an erasure certificate so the chain travels with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestationChain {
    pub license: LicenseCertificate,
    /// Convenience mirror of `license.body.root_key_id` so a reader can pick
    /// the right published root without parsing the license first.
    pub root_key_id: String,
}

impl AttestationChain {
    pub fn new(license: LicenseCertificate) -> Self {
        let root_key_id = license.body.root_key_id.clone();
        Self {
            license,
            root_key_id,
        }
    }
}

/// Outcome of verifying a certificate's attestation chain.
///
/// Deliberately a typed verdict rather than a bool: "unlicensed" and
/// "tampered" are wildly different findings and a caller must not be able to
/// conflate them (ADR-0005 §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum ChainVerdict {
    /// Full chain verified to a trusted root.
    Licensed {
        customer_id: String,
        customer_name: String,
        license_id: String,
        root_key_id: String,
        /// True when the lease had already expired at the time the cert was
        /// signed. The chain is still authentic — the licence had lapsed.
        expired_at_signing: bool,
    },
    /// No chain present. A valid, self-signed evaluation certificate
    /// (ADR-0005 §5) — not an error.
    Unlicensed,
    /// A chain is present but does not hold. Always a hard finding.
    Invalid { reason: String },
}

impl ChainVerdict {
    pub fn is_licensed(&self) -> bool {
        matches!(self, Self::Licensed { .. })
    }
    pub fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid { .. })
    }
}

/// Verify the chain carried by a signed erasure certificate.
///
/// `signed_by` is the instance key id that signed the erasure certificate —
/// normally `signed.signature.public_key_id`. The link that makes this a
/// *chain* rather than two unrelated signatures is that the license must name
/// exactly that key.
pub fn verify_chain(
    chain: Option<&AttestationChain>,
    signed_by: &str,
    signed_at: OffsetDateTime,
    roots: &[VerifyingKey],
) -> ChainVerdict {
    let Some(chain) = chain else {
        return ChainVerdict::Unlicensed;
    };

    let root_id = match chain.license.verify(roots) {
        Ok(id) => id,
        Err(e) => {
            return ChainVerdict::Invalid {
                reason: e.to_string(),
            }
        }
    };

    // The chain link itself: the license must entitle the key that actually
    // signed this cert. Without this a valid license could be stapled to a
    // certificate signed by any key at all.
    if chain.license.body.instance_public_key_id != signed_by {
        return ChainVerdict::Invalid {
            reason: format!(
                "license entitles instance key {} but this certificate was signed by {}",
                chain.license.body.instance_public_key_id, signed_by
            ),
        };
    }

    if chain.root_key_id != chain.license.body.root_key_id {
        return ChainVerdict::Invalid {
            reason: "attestation root_key_id does not match the license body".into(),
        };
    }

    let ent = &chain.license.body.entitlements;
    ChainVerdict::Licensed {
        customer_id: ent.customer_id.clone(),
        customer_name: ent.customer_name.clone(),
        license_id: chain.license.body.license_id.clone(),
        root_key_id: root_id,
        // Reported, not fatal: an expired licence does not make the erasure
        // any less real, and an auditor still wants the evidence.
        expired_at_signing: !ent.within_window(signed_at),
    }
}
