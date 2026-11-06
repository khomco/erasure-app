//! Entitlements — what a customer is licensed to do (ADR-0005 §2).
//!
//! Everything here lives *inside* the vendor's signature on a
//! [`crate::LicenseCertificate`], so a customer cannot edit it: changing any
//! field invalidates the signature. That is the entire enforcement mechanism
//! for entitlement *content*, and unlike consumption it is airtight.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// How many erasures the license permits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Quota {
    /// Annual-unlimited per station — the Tier 1 model in CONTEXT §9.
    Unlimited,
    /// A hard count. Only meaningfully enforceable with online
    /// reconciliation (ADR-0005 §4); offline this is best-effort.
    Count { erasures: u64 },
}

impl Quota {
    pub fn permits(&self, used: u64) -> bool {
        match self {
            Self::Unlimited => true,
            Self::Count { erasures } => used < *erasures,
        }
    }

    pub fn remaining(&self, used: u64) -> Option<u64> {
        match self {
            Self::Unlimited => None,
            Self::Count { erasures } => Some(erasures.saturating_sub(used)),
        }
    }
}

/// What the license covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    /// One machine, identified by fingerprint. Moving the license to
    /// another station fails the binding check.
    Machine { fingerprint: String },
    /// Every station at a site. Convenient for a floor of 30 benches; the
    /// trade is that we cannot tell them apart offline.
    Site { site_id: String },
}

/// Feature flags a license may grant. Closed enum rather than free strings,
/// so a license cannot name a feature this build does not understand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// Customer/Contract/WorkOrder/Asset model (CONTEXT §11 v0.2 #4).
    EnterpriseMode,
    /// Push certs and config to a hub / control plane.
    HubSync,
    /// ITAD-ERP REST + webhook integration tier.
    ErpIntegration,
    /// PDF/A-3 certificate rendering.
    PdfCertificates,
    /// Optional public anchoring of cert hashes (ADR-0005 §6).
    CertAnchoring,
}

/// Which sanitization methods the license permits.
///
/// Discriminants, not free text: a license must not be able to permit a
/// method this build cannot perform, and the mapping to `wipe_common::Method`
/// has to survive the enum gaining variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodClass {
    NvmeSanitize,
    AtaSecureErase,
    BlockOverwrite,
    OpalRevert,
    Destroy,
}

impl MethodClass {
    /// Classify a concrete method for entitlement checks.
    pub fn of(method: &wipe_common::Method) -> Self {
        use wipe_common::Method::*;
        match method {
            NvmeSanitizeBlockErase { .. }
            | NvmeSanitizeCryptoErase { .. }
            | NvmeSanitizeOverwrite { .. } => Self::NvmeSanitize,
            AtaSecureErase { .. } => Self::AtaSecureErase,
            BlockOverwrite { .. } => Self::BlockOverwrite,
            OpalRevert => Self::OpalRevert,
            Destroy { .. } => Self::Destroy,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AllowedMethods {
    All,
    Only { classes: Vec<MethodClass> },
}

impl AllowedMethods {
    pub fn permits(&self, method: &wipe_common::Method) -> bool {
        match self {
            Self::All => true,
            Self::Only { classes } => classes.contains(&MethodClass::of(method)),
        }
    }
}

/// The vendor-signed grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entitlements {
    pub customer_id: String,
    pub customer_name: String,
    pub quota: Quota,
    pub scope: Scope,
    /// Lease window. `not_after` is the offline expiry lever (ADR-0005 §3a)
    /// and its clock-rollback weakness is documented there.
    #[serde(with = "time::serde::rfc3339")]
    pub not_before: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub not_after: OffsetDateTime,
    #[serde(default)]
    pub features: Vec<Feature>,
    pub allowed_methods: AllowedMethods,
    /// Optional machine fingerprint the station must match. Independent of
    /// `Scope::Machine` so a site license can still be pinned if a customer
    /// wants it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_binding: Option<String>,
}

impl Entitlements {
    pub fn grants(&self, feature: Feature) -> bool {
        self.features.contains(&feature)
    }

    /// Is `now` inside the lease window?
    pub fn within_window(&self, now: OffsetDateTime) -> bool {
        now >= self.not_before && now <= self.not_after
    }

    /// The fingerprint this license requires a station to present, if any.
    /// `Scope::Machine` implies its own fingerprint.
    pub fn required_fingerprint(&self) -> Option<&str> {
        self.machine_binding.as_deref().or(match &self.scope {
            Scope::Machine { fingerprint } => Some(fingerprint.as_str()),
            Scope::Site { .. } => None,
        })
    }
}
