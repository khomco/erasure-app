//! Reading and writing licences on disk (ADR-0005).
//!
//! Two artefacts, deliberately separate files:
//!
//! * the **vendor root seed** — 32 raw bytes, and the highest-value secret in
//!   the product. Nothing in a station binary should ever read one; only the
//!   issuance tool does.
//! * the **licence** — a `LicenseCertificate` as JSON, safe to hand a
//!   customer. It contains no secrets: it is a vendor signature over public
//!   entitlements naming a public key id. A licence file is either that bare
//!   licence or a [`LicenseFile`] `{license, credit_grant}` carrying one
//!   credit grant beside it (ADR-0013 §3); every parser here accepts both.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::chain::LicenseCertificate;
use crate::credit::CreditGrant;
use crate::{LicenseError, LicenseResult};

/// A licence file with a credit grant beside the licence: what the hub
/// hands an air-gapped station whose owner moved credits from the pool
/// (ADR-0013 §3). The licence itself is unchanged, so erasure-certificate
/// attestation chains never carry a grant.
///
/// Unknown keys are ignored, so a station accepts a file a later hub adds
/// keys to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseFile {
    pub license: LicenseCertificate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_grant: Option<CreditGrant>,
}

/// Load a licence from disk.
///
/// Deliberately does *not* verify: verification needs the root public key and
/// the instance key id, which are the caller's to supply. Loading and
/// trusting must stay separate steps.
pub fn load_license(path: &Path) -> LicenseResult<LicenseCertificate> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| LicenseError::Encoding(format!("reading {}: {e}", path.display())))?;
    parse_license(&raw).map_err(|e| match e {
        LicenseError::Encoding(why) => LicenseError::Encoding(format!("{}: {why}", path.display())),
        other => other,
    })
}

/// Parse a licence file's JSON, as read from disk or received over the API.
///
/// Like [`load_license`], parsing is not trusting: the caller still has to
/// run [`check_installable`]. The error is an `Encoding` error whose text
/// starts `not a licence file: `.
pub fn parse_license(raw: &str) -> LicenseResult<LicenseCertificate> {
    parse_license_file(raw).map(|file| file.license)
}

/// Parse a licence file in either form: the bare licence (no grant) or a
/// [`LicenseFile`] `{license, credit_grant}`. An object with a `license` key
/// is the second form. Parsing is not trusting: the grant, like the licence,
/// is still to be checked ([`crate::check_grant_for`]). Errors as
/// [`parse_license`].
pub fn parse_license_file(raw: &str) -> LicenseResult<LicenseFile> {
    let not_a_file =
        |e: serde_json::Error| LicenseError::Encoding(format!("not a licence file: {e}"));
    let value: serde_json::Value = serde_json::from_str(raw).map_err(not_a_file)?;
    if value.get("license").is_some() {
        serde_json::from_value(value).map_err(not_a_file)
    } else {
        serde_json::from_value(value)
            .map(|license| LicenseFile {
                license,
                credit_grant: None,
            })
            .map_err(not_a_file)
    }
}

pub fn save_license(path: &Path, license: &LicenseCertificate) -> LicenseResult<()> {
    let body =
        serde_json::to_string_pretty(license).map_err(|e| LicenseError::Encoding(e.to_string()))?;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .map_err(|e| LicenseError::Encoding(format!("creating {}: {e}", dir.display())))?;
        }
    }
    std::fs::write(path, body)
        .map_err(|e| LicenseError::Encoding(format!("writing {}: {e}", path.display())))
}

/// Why a licence could not be installed on this station.
///
/// Separate from verification failure: a perfectly valid licence issued to a
/// *different* station is not forged, it is simply not ours, and the operator
/// message has to say which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallProblem {
    /// The licence names a different instance key.
    WrongInstanceKey { expected: String, found: String },
    /// The licence does not verify against any supplied root.
    NotTrusted(String),
}

impl std::fmt::Display for InstallProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongInstanceKey { expected, found } => write!(
                f,
                "licence was issued to instance key {found}, but this station's signing key is \
                 {expected}. A licence is bound to one key; reissue it for this station."
            ),
            Self::NotTrusted(reason) => write!(f, "licence did not verify: {reason}"),
        }
    }
}

/// Check that a licence can be installed on a station holding
/// `station_key_id`, verifying it against the trusted roots first.
///
/// Both halves matter: an unverified licence is worthless, and a verified
/// licence for someone else's key would let a station claim entitlements it
/// was never granted.
pub fn check_installable(
    license: &LicenseCertificate,
    station_key_id: &str,
    roots: &[wipe_cert::VerifyingKey],
) -> Result<(), InstallProblem> {
    if let Err(e) = license.verify(roots) {
        return Err(InstallProblem::NotTrusted(e.to_string()));
    }
    if license.body.instance_public_key_id != station_key_id {
        return Err(InstallProblem::WrongInstanceKey {
            expected: station_key_id.to_string(),
            found: license.body.instance_public_key_id.clone(),
        });
    }
    Ok(())
}
