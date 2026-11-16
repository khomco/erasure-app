//! The credit top-up's wire contract (ADR-0013 §3, controlled-licensing
//! story 4): what a connected station sends to `POST /api/station/credits`
//! and what the hub answers. One definition, used by the hub's route and the
//! station's client, so the two sides cannot drift apart.
//!
//! The request is signed like every station request
//! ([`crate::station_request`]); these types are only its body and the 200
//! answer. A grant in the answer is not trusted until the station checks it
//! ([`crate::check_grant_for`]) against its own vendor roots.

use serde::{Deserialize, Serialize};

use crate::CreditGrant;

/// Where a station tops up its credit reserve.
pub const STATION_CREDITS_PATH: &str = "/api/station/credits";

/// A top-up request's body: the station's report on its credit reserve for
/// the installed licence's organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopUpRequest {
    /// The station's instance public key, base64 (as enrollment sends it).
    pub instance_public_key: String,
    /// The licence the station holds; `None` when it holds none.
    pub licence_id: Option<String>,
    /// Credits a start may still reserve: held, and neither spent nor
    /// reserved by running Jobs.
    pub balance: u64,
    /// Credits spent by signed certificates, cumulative over the life of the
    /// reserve (an idempotent counter: a resent report counts nothing twice).
    pub credits_spent: u64,
    /// The last grant a top-up delivered, so the hub can resend an answer
    /// that was lost; `None` before any.
    pub last_grant_id: Option<String>,
}

/// The hub's 200 answer to a top-up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopUpAnswer {
    /// A credit grant for this station, new or resent; `None` when none was
    /// issued.
    pub credit_grant: Option<CreditGrant>,
    /// The offering's top-up threshold: top up below this many credits.
    pub threshold: u64,
    /// The offering's target reserve: a top-up brings the station up to this
    /// many.
    pub target_reserve: u64,
    /// Credits left in the organization's pool after any grant (0 when the
    /// entitlement is not current).
    pub available: u64,
}
