//! Optional public anchoring of certificate hashes (ADR-0005 §6).
//!
//! **This is not licensing.** Anchoring answers one narrow question a
//! signature cannot: *did this certificate exist, unchanged, by date X?* Our
//! own `issued_at` is only as trustworthy as our clock, so a third-party
//! timestamp is genuinely additive for high-value disposals.
//!
//! It is explicitly **not** used for licensing, metering, entitlement or
//! revocation, and a certificate's validity never depends on it — an
//! air-gapped station stays first-class. Nothing is implemented.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{LicenseError, LicenseResult};

/// Proof that a hash was published, enough for an auditor to check it
/// themselves without trusting us.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorReceipt {
    /// The `canonical_sha256_hex` that was anchored.
    pub cert_sha256_hex: String,
    /// Where it went — e.g. "opentimestamps", "ethereum-sepolia".
    pub network: String,
    /// Transaction / attestation identifier on that network.
    pub reference: String,
    #[serde(with = "time::serde::rfc3339")]
    pub anchored_at: OffsetDateTime,
}

pub trait CertAnchor: Send + Sync {
    /// Publish a certificate hash. Only ever the hash — a certificate
    /// carries customer identity, asset tags and serials, none of which
    /// belongs on a public ledger.
    fn anchor(&self, cert_sha256_hex: &str) -> LicenseResult<AnchorReceipt>;
    /// Confirm a receipt independently.
    fn verify(&self, receipt: &AnchorReceipt) -> LicenseResult<bool>;
    fn network(&self) -> &str;
}

/// Placeholder. Fails rather than fabricating a receipt: a receipt that
/// cannot be independently checked is worse than none, because its whole
/// value is that a third party can confirm it.
pub struct UnconfiguredAnchor;

impl CertAnchor for UnconfiguredAnchor {
    fn anchor(&self, _cert_sha256_hex: &str) -> LicenseResult<AnchorReceipt> {
        Err(LicenseError::Unsupported(
            "no certificate anchor is configured (ADR-0005 §6 — optional, not built)".into(),
        ))
    }
    fn verify(&self, _receipt: &AnchorReceipt) -> LicenseResult<bool> {
        Err(LicenseError::Unsupported(
            "no certificate anchor is configured".into(),
        ))
    }
    fn network(&self) -> &str {
        "none"
    }
}
