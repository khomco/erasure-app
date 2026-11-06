//! Online reconciliation seam (ADR-0005 §4).
//!
//! Hard counts need a server, because offline consumption is unprovable. This
//! mirrors `wipe_server::store::ControlPlaneStore` exactly: config-first
//! endpoint, boring wire contract, and **fails visibly** when unreachable.
//! The licensing server itself is not built.
//!
//! Paid tiers remain fully functional offline by default (ADR-0005 §4). A
//! station that refuses to wipe drives in an air-gapped facility is worthless
//! to precisely our best-fit customer, so under-reporting is the accepted
//! risk.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{LicenseError, LicenseResult};

/// What a station reports at check-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReport {
    pub license_id: String,
    pub station_id: String,
    pub machine_fingerprint: String,
    /// Successful erasures since the last acknowledged check-in.
    pub erasures_since_last_checkin: u64,
    /// Locally-observed lifetime total, so the server can spot a station
    /// whose local state was reset.
    pub erasures_lifetime: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub reported_at: OffsetDateTime,
}

/// What the server returns, letting it extend a lease or adjust quota
/// without reissuing the licence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckinResponse {
    pub accepted_through: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub lease_extended_to: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub trait LicenseClient: Send + Sync {
    /// Bind a licence to this machine on first use.
    fn activate(&self, license_id: &str, fingerprint: &str) -> LicenseResult<()>;
    /// Report usage and pick up any lease extension.
    fn checkin(&self, report: &UsageReport) -> LicenseResult<CheckinResponse>;
    fn endpoint(&self) -> String;
}

/// HTTP client against a vendor licensing server.
///
/// Wire contract, deliberately boring:
///   `POST {base}/api/licenses/{license_id}/activate`
///   `POST {base}/api/licenses/{license_id}/checkin`
///
/// No transport is wired in yet, so both calls report unreachable rather than
/// pretending to succeed — a stub that silently "accepted" a check-in would
/// make a station believe its quota was reconciled when it was not.
pub struct HttpLicenseClient {
    base_url: String,
}

impl HttpLicenseClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    fn url(&self, license_id: &str, action: &str) -> String {
        format!(
            "{}/api/licenses/{}/{}",
            self.base_url.trim_end_matches('/'),
            license_id,
            action
        )
    }

    fn unbuilt(&self) -> LicenseError {
        LicenseError::Unreachable(
            "licensing-server transport is not implemented yet (ADR-0005 §4)".into(),
        )
    }
}

impl LicenseClient for HttpLicenseClient {
    fn activate(&self, _license_id: &str, _fingerprint: &str) -> LicenseResult<()> {
        Err(self.unbuilt())
    }

    fn checkin(&self, _report: &UsageReport) -> LicenseResult<CheckinResponse> {
        Err(self.unbuilt())
    }

    fn endpoint(&self) -> String {
        self.base_url.clone()
    }
}

impl HttpLicenseClient {
    /// Exposed for tests and for showing the operator where check-in would go.
    pub fn activate_url(&self, license_id: &str) -> String {
        self.url(license_id, "activate")
    }
    pub fn checkin_url(&self, license_id: &str) -> String {
        self.url(license_id, "checkin")
    }
}
