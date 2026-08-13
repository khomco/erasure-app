//! Licensing by attestation (ADR-0005).
//!
//! The core claim this crate supports is deliberately narrow and provable:
//!
//! > An auditor holding only our published vendor **root public key** can
//! > establish, with no network access, that a certificate was signed by a key
//! > the vendor licensed, to a named customer, under stated entitlements, and
//! > that nothing has been altered since.
//!
//! What it deliberately does **not** claim is that consumption is enforceable
//! offline. It isn't — a station with no network can always under-report — and
//! [`lease`] is explicit about which of its levers are cryptographic and which
//! are speed bumps.
//!
//! * [`chain`] — vendor root -> license -> instance key -> erasure cert.
//! * [`entitlement`] — what the vendor signature makes uneditable.
//! * [`lease`] — offline expiry, anti-rollback watermark, counter seam.
//! * [`client`] — online reconciliation seam (no server built).
//! * [`anchor`] — optional public timestamping seam (not licensing).

pub mod anchor;
pub mod chain;
pub mod client;
pub mod entitlement;
pub mod install;
pub mod lease;

pub use chain::*;
pub use entitlement::*;
pub use install::*;
pub use lease::*;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LicenseError {
    #[error("licence verification failed: {0}")]
    Verification(String),
    /// Distinct from `Verification`: the payload itself was altered. Kept
    /// separate so a caller cannot accidentally report tampering as a
    /// generic failure.
    #[error("licence was tampered with: {0}")]
    Tampered(String),
    #[error("licence was signed by an untrusted root key: {0}")]
    UntrustedRoot(String),
    #[error("encoding failed: {0}")]
    Encoding(String),
    #[error("not supported on this build: {0}")]
    Unsupported(String),
    #[error("licensing server unreachable: {0}")]
    Unreachable(String),
}

pub type LicenseResult<T> = Result<T, LicenseError>;

/// Derive this station's machine fingerprint.
///
/// Today this is the station id hashed with a domain separator — stable, and
/// honestly weak: it is an identifier the customer controls, not an attested
/// hardware measurement. A TPM EK/AK-derived fingerprint replaces this when
/// the Linux backend lands, which is also what would make [`lease`]'s
/// anti-rollback claim real.
pub fn machine_fingerprint(station_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"wipestation/machine-fingerprint/v1");
    h.update(station_id.as_bytes());
    format!("mf1:{}", hex::encode(&h.finalize()[..12]))
}
