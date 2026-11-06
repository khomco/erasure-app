//! Reading and writing licences on disk (ADR-0005).
//!
//! Two artefacts, deliberately separate files:
//!
//! * the **vendor root seed** — 32 raw bytes, and the highest-value secret in
//!   the product. Nothing in a station binary should ever read one; only the
//!   issuance tool does.
//! * the **licence** — a `LicenseCertificate` as JSON, safe to hand a
//!   customer. It contains no secrets: it is a vendor signature over public
//!   entitlements naming a public key id.

use std::path::Path;

use crate::chain::LicenseCertificate;
use crate::{LicenseError, LicenseResult};

/// Load a licence from disk.
///
/// Deliberately does *not* verify: verification needs the root public key and
/// the instance key id, which are the caller's to supply. Loading and
/// trusting must stay separate steps.
pub fn load_license(path: &Path) -> LicenseResult<LicenseCertificate> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| LicenseError::Encoding(format!("reading {}: {e}", path.display())))?;
    serde_json::from_str(&raw)
        .map_err(|e| LicenseError::Encoding(format!("parsing {}: {e}", path.display())))
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
