//! Certificate of Sanitization — JSON-LD payload + detached Ed25519 signature.
//!
//! Design goals:
//!  * **Offline verifiable** — anyone with our public key can verify a cert
//!    without contacting the vendor (differentiator vs Blancco's vendor-DB
//!    lookup).
//!  * **Command-level evidence** — captured `CommandEvidence` rides inside
//!    the cert so auditors see actual issued opcodes, return codes, log
//!    pages — not just a marketing summary.
//!  * **NIST SP 800-88 Rev. 2 fields** — operator email, media operational
//!    status, validation reference, all surfaced as first-class fields.

pub mod canonical;
pub mod schema;
pub mod sign;

pub use schema::*;
pub use sign::*;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CertError {
    #[error("signing failed: {0}")]
    Signing(String),
    #[error("verification failed: {0}")]
    Verification(String),
    #[error("serialization failed: {0}")]
    Serialization(String),
    #[error("invalid input: {0}")]
    Invalid(String),
}

pub type CertResult<T> = Result<T, CertError>;
