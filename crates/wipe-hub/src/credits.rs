//! The organization credit pool (controlled-licensing story 3, ADR-0013 §3):
//! an entitlement with an erasure quota is one pool for the whole
//! organization, and credits leave it as vendor-signed credit grants
//! (`wipe_license::CreditGrant`) bound to one station's instance key. The
//! only code that writes the `credit_grants` and `credit_returns` ledgers
//! (migration 0022, both append-only).
//!
//! - **Pool arithmetic**, per entitlement with a quota: `purchased` is the
//!   order snapshot's `quota_erasures`, `granted` the sum of its grants,
//!   `returned` the sum of its returns, and `available = purchased − granted +
//!   returned`. An unlimited entitlement has no pool.
//! - **Choosing credits** ([`decide`]), always under the entitlement row lock
//!   the caller already holds ([`crate::stations::register`] and
//!   [`crate::stations::reissue`]), so racing grants can never take
//!   `available` below 0: omitted means `min(target reserve, available)`; an
//!   explicit count above `available` is 409 [`MSG_NOT_ENOUGH_CREDITS`]; any
//!   explicit count on an unlimited entitlement is 400
//!   [`MSG_CREDITS_UNLIMITED`]. Enrollment approval and the station-link
//!   renewal fetch grant nothing ([`CreditChoice::Nothing`]): their responses
//!   carry the bare licence only, and a grant nobody receives would strand
//!   credits until release.
//! - **Grant** ([`grant`]): signs one grant for the station's key, stores the
//!   exact file linked to the licence it was issued with (none for a
//!   top-up grant: `credit_grants.licence_id` is NULL), and audits
//!   `credits.granted {grant_id, entitlement_id, station_id, licence_id,
//!   credits, available_after}` (subject `entitlement:<id>`), in the caller's
//!   transaction.
//! - **Return** ([`return_on_release`]): a released station on a quota
//!   entitlement returns `min(last reported balance, granted to it − already
//!   returned for it)`, 0 when it never reported, audited as
//!   `credits.returned {entitlement_id, station_id, credits, reported_at}`.
//!   The cap means a wrong report can never grow the pool beyond what was
//!   bought.
//!
//! A connected station's top-up ([`crate::station_link::top_up`], story 4)
//! grants here too, with no licence (`licence_id: None`), and records the
//! balance the station will hold once the answer lands in
//! `stations.credit_balance`, which a release returns.

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::postgres::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;
use wipe_license::VendorRoot;

use crate::audit::{self, AuditEntry};
use crate::entitlements;
use crate::stations::StationError;

pub const MSG_NOT_ENOUGH_CREDITS: &str =
    "not enough credits left in this entitlement's pool; choose fewer credits";
pub const MSG_CREDITS_UNLIMITED: &str =
    "credits apply only to an entitlement with an erasure quota; leave credits empty";
pub const MSG_CREDITS_NEGATIVE: &str = "credits must be a whole number, 0 or more";

/// The target reserve of an offering whose `credit_target_reserve` is NULL
/// (ADR-0013 §3).
pub const DEFAULT_TARGET_RESERVE: i64 = 50;

/// The top-up threshold of an offering whose `credit_topup_threshold` is
/// NULL, capped at its target reserve (ADR-0013 §3).
pub const DEFAULT_TOPUP_THRESHOLD: i64 = 10;

/// An entitlement's pool, as its organization sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditPool {
    pub purchased: i64,
    pub granted: i64,
    pub returned: i64,
    pub available: i64,
    /// How many credits a station is given when none are chosen.
    pub target_reserve: i64,
}

impl CreditPool {
    /// The pool of an entitlement with `quota` (`None`: unlimited, no pool).
    pub fn of(
        quota: Option<i64>,
        granted: i64,
        returned: i64,
        target_reserve: i64,
    ) -> Option<Self> {
        quota.map(|purchased| CreditPool {
            purchased,
            granted,
            returned,
            available: purchased - granted + returned,
            target_reserve,
        })
    }
}

/// A grant as a register or reissue response names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantSummary {
    pub id: Uuid,
    pub credits: i64,
}

/// How many credits a licence issue moves to the station.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditChoice {
    /// The owner chose none: `min(target reserve, available)` on a quota
    /// entitlement, nothing on an unlimited one.
    Default,
    /// The owner's count (validated ≥ 0); 0 issues the licence alone.
    Exactly(i64),
    /// An issue whose response carries only the licence (enrollment
    /// approval, the station-link fetch): never a grant.
    Nothing,
}

impl CreditChoice {
    /// The choice for a request's optional `credits`: negative is 400.
    pub fn from_request(credits: Option<i64>) -> Result<Self, StationError> {
        match credits {
            None => Ok(Self::Default),
            Some(n) if n < 0 => Err(StationError::BadRequest(MSG_CREDITS_NEGATIVE.into())),
            Some(n) => Ok(Self::Exactly(n)),
        }
    }
}

/// `available` of entitlement `entitlement_id` with quota `purchased`. The
/// caller holds the entitlement row lock.
pub(crate) async fn available(
    conn: &mut PgConnection,
    entitlement_id: Uuid,
    purchased: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT $2::bigint \
              - COALESCE((SELECT sum(credits) FROM credit_grants WHERE entitlement_id = $1), 0)::bigint \
              + COALESCE((SELECT sum(credits) FROM credit_returns WHERE entitlement_id = $1), 0)::bigint",
    )
    .bind(entitlement_id)
    .bind(purchased)
    .fetch_one(&mut *conn)
    .await
}

/// Refuse an explicit count on an unlimited entitlement: 400
/// [`MSG_CREDITS_UNLIMITED`]. Checked before any write.
pub(crate) fn check_applies(quota: Option<i64>, choice: CreditChoice) -> Result<(), StationError> {
    if quota.is_none() && matches!(choice, CreditChoice::Exactly(_)) {
        return Err(StationError::BadRequest(MSG_CREDITS_UNLIMITED.into()));
    }
    Ok(())
}

/// How many credits to grant from entitlement `entitlement_id` (quota
/// `quota`, target reserve `target_reserve`) for `choice`, with the
/// entitlement row locked by the caller; 0 means no grant. Before any write:
/// 400 [`MSG_CREDITS_UNLIMITED`] for a count on an unlimited entitlement,
/// 409 [`MSG_NOT_ENOUGH_CREDITS`] for a count above `available`.
pub(crate) async fn decide(
    conn: &mut PgConnection,
    entitlement_id: Uuid,
    quota: Option<i64>,
    target_reserve: i64,
    choice: CreditChoice,
) -> Result<i64, StationError> {
    check_applies(quota, choice)?;
    let Some(purchased) = quota else {
        return Ok(0);
    };
    match choice {
        CreditChoice::Nothing | CreditChoice::Exactly(0) => Ok(0),
        CreditChoice::Default => {
            let available = available(conn, entitlement_id, purchased).await?;
            Ok(target_reserve.min(available).max(0))
        }
        CreditChoice::Exactly(n) => {
            let available = available(conn, entitlement_id, purchased).await?;
            if n > available {
                return Err(StationError::Conflict(MSG_NOT_ENOUGH_CREDITS));
            }
            Ok(n)
        }
    }
}

/// One grant to sign and store.
pub(crate) struct NewGrant<'a> {
    pub organization_id: Uuid,
    pub entitlement_id: Uuid,
    /// The entitlement's quota (the pool's `purchased`).
    pub purchased: i64,
    /// The `stations` row.
    pub station: Uuid,
    /// The licence issued with it, in the same transaction; `None` for a
    /// top-up over the station link ([`crate::station_link::top_up`]).
    pub licence_id: Option<Uuid>,
    pub instance_key_id: &'a str,
    /// > 0, decided by [`decide`].
    pub credits: i64,
    pub issued_at: OffsetDateTime,
    /// The audit actor (`user:<id>`), also stored as `issued_by`.
    pub actor: &'a str,
}

/// Sign `new.credits` for the station's key with `root`, store the exact
/// file and audit `credits.granted`, in the caller's transaction (which holds
/// the entitlement row lock).
pub(crate) async fn grant(
    conn: &mut PgConnection,
    root: &VendorRoot,
    new: NewGrant<'_>,
) -> Result<GrantSummary, StationError> {
    let grant_id = Uuid::new_v4();
    let credits =
        u64::try_from(new.credits).map_err(|_| StationError::Issue("negative grant".into()))?;
    let document = root
        .issue_credit_grant(
            grant_id.to_string(),
            new.organization_id.to_string(),
            new.instance_key_id.to_string(),
            credits,
            new.issued_at,
        )
        .and_then(|g| {
            serde_json::to_string_pretty(&g)
                .map_err(|e| wipe_license::LicenseError::Encoding(e.to_string()))
        })
        .map_err(|e| StationError::Issue(e.to_string()))?;
    sqlx::query(
        "INSERT INTO credit_grants (id, organization_id, entitlement_id, station_id, licence_id, \
                                    instance_key_id, credits, issued_at, issued_by, document) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::text::timestamptz, $9, $10)",
    )
    .bind(grant_id)
    .bind(new.organization_id)
    .bind(new.entitlement_id)
    .bind(new.station)
    .bind(new.licence_id)
    .bind(new.instance_key_id)
    .bind(new.credits)
    .bind(
        new.issued_at
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| StationError::Issue(e.to_string()))?,
    )
    .bind(new.actor)
    .bind(&document)
    .execute(&mut *conn)
    .await?;
    let available_after = available(conn, new.entitlement_id, new.purchased).await?;
    if available_after < 0 {
        // Unreachable under the entitlement lock; never commit an overdraft.
        return Err(StationError::Issue(format!(
            "the pool of {} would go below 0",
            new.entitlement_id
        )));
    }
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: new.actor,
            action: "credits.granted",
            subject: &entitlements::subject(new.entitlement_id),
            detail: json!({
                "grant_id": grant_id,
                "entitlement_id": new.entitlement_id,
                "station_id": new.station,
                "licence_id": new.licence_id,
                "credits": new.credits,
                "available_after": available_after,
            }),
        },
    )
    .await?;
    Ok(GrantSummary {
        id: grant_id,
        credits: new.credits,
    })
}

/// The file to hand out for licence `licence_id` whose stored document is
/// `document`: that document byte for byte when it was issued with no
/// credit grant, else the pretty JSON `wipe_license::LicenseFile`
/// `{license, credit_grant}` of the stored licence and grant. Both the
/// customer and the vendor-admin download serve this, so they never differ.
pub(crate) async fn licence_file(
    conn: &mut PgConnection,
    licence_id: Uuid,
    document: String,
) -> Result<String, StationError> {
    let Some(grant) = grant_document(conn, licence_id).await? else {
        return Ok(document);
    };
    let parse_err =
        |e: serde_json::Error| StationError::Issue(format!("stored file {licence_id}: {e}"));
    let file = wipe_license::LicenseFile {
        license: serde_json::from_str(&document).map_err(parse_err)?,
        credit_grant: Some(serde_json::from_str(&grant).map_err(parse_err)?),
    };
    serde_json::to_string_pretty(&file).map_err(|e| StationError::Issue(e.to_string()))
}

/// The stored grant file issued with licence `licence_id`, if any.
pub(crate) async fn grant_document(
    conn: &mut PgConnection,
    licence_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT document FROM credit_grants WHERE licence_id = $1")
        .bind(licence_id)
        .fetch_optional(&mut *conn)
        .await
}

/// Record the return of released station `station` (locked, as is its
/// entitlement `entitlement_id`, by the caller) on behalf of `actor`, if the
/// entitlement has a quota: `min(last reported balance, granted − returned
/// for this station)`, 0 when nothing was reported. Audits
/// `credits.returned`. Nothing for an unlimited entitlement.
pub(crate) async fn return_on_release(
    conn: &mut PgConnection,
    organization_id: Uuid,
    entitlement_id: Uuid,
    station: Uuid,
    actor: &str,
) -> Result<(), StationError> {
    let quota: Option<i64> = sqlx::query_scalar(
        "SELECT o.quota_erasures FROM entitlements e JOIN orders o ON o.id = e.order_id \
         WHERE e.id = $1",
    )
    .bind(entitlement_id)
    .fetch_one(&mut *conn)
    .await?;
    if quota.is_none() {
        return Ok(());
    }
    let (balance, reported_at, outstanding): (Option<i64>, Option<String>, i64) =
        sqlx::query_as(&format!(
            "SELECT s.credit_balance, \
                to_char(s.credit_balance_reported_at AT TIME ZONE 'UTC', {}), \
                (COALESCE((SELECT sum(credits) FROM credit_grants WHERE station_id = s.id), 0) \
                 - COALESCE((SELECT sum(credits) FROM credit_returns WHERE station_id = s.id), 0)\
                )::bigint \
         FROM stations s WHERE s.id = $1",
            crate::admin::RFC3339_UTC
        ))
        .bind(station)
        .fetch_one(&mut *conn)
        .await?;
    let credits = balance.unwrap_or(0).min(outstanding).max(0);
    sqlx::query(
        "INSERT INTO credit_returns (organization_id, entitlement_id, station_id, credits, \
                                     reported_at, returned_by) \
         SELECT $1, $2, s.id, $4, s.credit_balance_reported_at, $5 \
         FROM stations s WHERE s.id = $3",
    )
    .bind(organization_id)
    .bind(entitlement_id)
    .bind(station)
    .bind(credits)
    .bind(actor)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor,
            action: "credits.returned",
            subject: &entitlements::subject(entitlement_id),
            detail: json!({
                "entitlement_id": entitlement_id,
                "station_id": station,
                "credits": credits,
                "reported_at": reported_at,
            }),
        },
    )
    .await?;
    Ok(())
}
