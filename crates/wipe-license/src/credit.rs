//! Credit grants (ADR-0013 §3): erasure credits moved from an organization's
//! pool at the hub to one station, as a document the vendor root signs.
//!
//! A grant is bound to one station's instance key and carries a unique id,
//! so a station can verify it offline against the vendor root it already
//! trusts and ignore an id it has seen before (story 4). It is signed exactly
//! as a licence is (canonical bytes, Ed25519, the same [`LicenseSignature`])
//! and verified with the same checks and errors as
//! [`LicenseCertificate::verify`](crate::LicenseCertificate::verify).
//!
//! The grant and licence bodies are distinct structs with different required
//! keys (`grant_format_version` and `grant_id` against
//! `license_format_version` and `license_id`), so a signature over one never
//! parses, let alone verifies, as the other.
//!
//! A grant never rides inside the licence certificate: an air-gapped
//! station's licence file carries it beside the licence
//! ([`crate::install::LicenseFile`]), so erasure-certificate attestation
//! chains never carry grants.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use wipe_cert::VerifyingKey;

use crate::chain::{sign_body, verify_signed, LicenseSignature, VendorRoot};
use crate::install::InstallProblem;
use crate::LicenseResult;

pub const GRANT_FORMAT_VERSION: u32 = 1;

/// The body the vendor signs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditGrantBody {
    pub grant_format_version: u32,
    /// Unique per grant: a station spends each grant id once.
    pub grant_id: String,
    /// The vendor root that signed this.
    pub root_key_id: String,
    /// The organization whose pool the credits came from.
    pub organization_id: String,
    /// The one station key that may use these credits.
    pub instance_public_key_id: String,
    pub credits: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub issued_at: OffsetDateTime,
}

/// A vendor-signed credit grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditGrant {
    pub body: CreditGrantBody,
    pub signature: LicenseSignature,
}

impl VendorRoot {
    /// Sign a grant of `credits` from `organization_id`'s pool to the station
    /// holding `instance_public_key_id`.
    pub fn issue_credit_grant(
        &self,
        grant_id: impl Into<String>,
        organization_id: impl Into<String>,
        instance_public_key_id: impl Into<String>,
        credits: u64,
        issued_at: OffsetDateTime,
    ) -> LicenseResult<CreditGrant> {
        let body = CreditGrantBody {
            grant_format_version: GRANT_FORMAT_VERSION,
            grant_id: grant_id.into(),
            root_key_id: self.public_key_id(),
            organization_id: organization_id.into(),
            instance_public_key_id: instance_public_key_id.into(),
            credits,
            issued_at,
        };
        Ok(CreditGrant {
            signature: sign_body(self.signing_key(), &body)?,
            body,
        })
    }
}

impl CreditGrant {
    /// Verify this grant against a set of trusted vendor root keys: the id
    /// of the root that signed it. The same checks and errors as a licence:
    /// a changed body is `Tampered`, an unknown root `UntrustedRoot`.
    pub fn verify(&self, roots: &[VerifyingKey]) -> LicenseResult<String> {
        verify_signed(
            &self.body,
            &self.signature,
            roots,
            "credit grant",
            (
                "grant_format_version",
                self.body.grant_format_version,
                GRANT_FORMAT_VERSION,
            ),
        )
    }
}

/// Check that `grant` verifies against `roots` and is bound to
/// `station_key_id`, the key of the station that would use it.
pub fn check_grant_for(
    grant: &CreditGrant,
    station_key_id: &str,
    roots: &[VerifyingKey],
) -> Result<(), InstallProblem> {
    if let Err(e) = grant.verify(roots) {
        return Err(InstallProblem::NotTrusted(e.to_string()));
    }
    if grant.body.instance_public_key_id != station_key_id {
        return Err(InstallProblem::WrongInstanceKey {
            expected: station_key_id.to_string(),
            found: grant.body.instance_public_key_id.clone(),
        });
    }
    Ok(())
}
