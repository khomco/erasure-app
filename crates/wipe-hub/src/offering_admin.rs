//! The offerings admin data layer (offerings-and-payments story 1, CAP-1):
//! vendor admins create, edit, publish and retire offerings, the things the
//! vendor sells (ADR-0007 §4). The routes are in [`crate::admin_routes`].
//!
//! The rules of [`crate::admin`] apply: every function takes a
//! `&VendorAdmin` and a connection (a transaction in practice) and writes
//! its own audit row (actor `user:<admin id>`, subject `offering:<id>`, or
//! `search:offerings` for the list) in it, so no offering is returned, and
//! none changed, without its row; a lookup that finds no offering, and a
//! refused change, write nothing. Offerings hold no personal data, so audit
//! rows may carry offering values. Timestamps are RFC 3339 UTC strings
//! formatted in SQL.
//!
//! - **Lifecycle** `draft → published → retired`, only those two steps;
//!   `retired` is terminal and nothing is ever deleted.
//! - **Commercial terms freeze at publication**: a published or retired
//!   offering's price, currency, interval and grants never change. A
//!   published offering's name and description may; a retired offering
//!   cannot change at all. A new price means retiring and creating anew.
//! - **Mutations lock the row first** (`SELECT … FOR UPDATE`).
//! - **Names are unique among offerings not retired**, case-insensitively
//!   (migration 0010's partial unique index, mapped to a 409).
//! - Money is integer minor units plus an uppercase 3-letter code; no floats.
//!
//! Migration 0010 repeats every rule as a CHECK or trigger.

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};
use sqlx::postgres::PgConnection;
use uuid::Uuid;
use wipe_license::Feature;

use crate::admin::{audit_read, VendorAdmin, RFC3339_UTC};
use crate::audit::AuditError;

/// Longest name, in characters, after trimming.
pub const NAME_MAX_CHARS: usize = 100;
/// Longest description, in characters, after trimming.
pub const DESCRIPTION_MAX_CHARS: usize = 2000;
/// Highest price in minor units: Stripe Checkout's documented maximum.
pub const PRICE_MAX_MINOR: i64 = 99_999_999;
pub const SEATS_MAX: i64 = 10_000;
pub const QUOTA_MAX_ERASURES: i64 = 1_000_000_000;
pub const TERM_MAX_MONTHS: i64 = 120;
/// The only term an annual offering may have (story 1, decision 1).
pub const ANNUAL_TERM_MONTHS: i64 = 12;

pub const INTERVALS: [&str; 2] = ["one_off", "annual"];
pub const SCOPES: [&str; 2] = ["machine", "site"];
/// `wipe_license::Feature`'s serde names, in declaration order. A test
/// matches it exhaustively against the enum.
pub const FEATURES: [&str; 6] = [
    "enterprise_mode",
    "hub_sync",
    "erp_integration",
    "pdf_certificates",
    "cert_anchoring",
    "operator_sso",
];

pub const MSG_DUPLICATE_NAME: &str = "an offering with this name is already in draft or for sale";
pub const MSG_TERMS_FROZEN: &str = "a published offering's price, interval and grants cannot \
     change; retire it and create a new one";
pub const MSG_RETIRED: &str = "a retired offering cannot change";
pub const MSG_PUBLISH_NOT_DRAFT: &str = "only a draft offering can be published";
pub const MSG_RETIRE_NOT_PUBLISHED: &str = "only a published offering can be retired";

/// The partial unique index on `lower(name)` (migration 0010).
const NAME_INDEX: &str = "offerings_name_key";

/// Audit subject of one offering.
pub fn subject(id: Uuid) -> String {
    format!("offering:{id}")
}

// ---------------------------------------------------------------------------
// Requests and validation

/// A create or edit body: every field, always (an edit is a full body).
/// Unknown keys are refused, so a misspelt field never silently keeps its
/// old value.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfferingRequest {
    pub name: String,
    pub description: String,
    pub price_minor: i64,
    pub currency: String,
    pub interval: String,
    pub grants: GrantsRequest,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantsRequest {
    pub scope: String,
    pub seats: i64,
    /// Required, `null` for unlimited: the outer `None` is a missing key.
    #[serde(default, deserialize_with = "present")]
    pub quota_erasures: Option<Option<i64>>,
    pub features: Vec<String>,
    pub term_months: i64,
}

/// A present key, `null` or not, as `Some(value)`.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<i64>>, D::Error> {
    Option::<i64>::deserialize(d).map(Some)
}

/// What an offering grants its buyer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants {
    /// One of [`SCOPES`].
    pub scope: String,
    pub seats: i32,
    /// `None` is unlimited.
    pub quota_erasures: Option<i64>,
    /// [`FEATURES`] names, sorted and de-duplicated.
    pub features: Vec<String>,
    pub term_months: i32,
}

/// A validated [`OfferingRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOffering {
    pub name: String,
    pub description: String,
    pub price_minor: i64,
    pub currency: String,
    pub interval: String,
    pub grants: Grants,
}

fn in_range(field: &str, v: i64, lo: i64, hi: i64) -> Result<i64, String> {
    if (lo..=hi).contains(&v) {
        Ok(v)
    } else {
        Err(format!("{field} must be between {lo} and {hi}"))
    }
}

fn one_of(field: &str, v: &str, allowed: &[&str]) -> Result<String, String> {
    if allowed.contains(&v) {
        Ok(v.to_string())
    } else {
        Err(format!("{field} must be one of {}", allowed.join(", ")))
    }
}

/// Validate a body; the error is the 400 message, naming the field.
pub fn validate(req: &OfferingRequest) -> Result<NewOffering, String> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err("name must not be blank".into());
    }
    if name.chars().count() > NAME_MAX_CHARS {
        return Err(format!("name must be at most {NAME_MAX_CHARS} characters"));
    }
    if name.chars().any(char::is_control) {
        return Err("name must not contain control characters".into());
    }
    let description = req.description.trim();
    if description.chars().count() > DESCRIPTION_MAX_CHARS {
        return Err(format!(
            "description must be at most {DESCRIPTION_MAX_CHARS} characters"
        ));
    }
    if description
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err("description must not contain control characters".into());
    }
    let price_minor = in_range("price_minor", req.price_minor, 1, PRICE_MAX_MINOR)?;
    let currency = req.currency.trim();
    if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err("currency must be a 3-letter ISO 4217 code, such as USD".into());
    }
    let interval = one_of("interval", &req.interval, &INTERVALS)?;
    let g = &req.grants;
    let scope = one_of("grants.scope", &g.scope, &SCOPES)?;
    let seats = in_range("grants.seats", g.seats, 1, SEATS_MAX)?;
    let quota_erasures = match g.quota_erasures {
        None => {
            return Err("grants.quota_erasures is required: null for unlimited, or a count".into())
        }
        Some(None) => None,
        Some(Some(n)) => Some(in_range("grants.quota_erasures", n, 1, QUOTA_MAX_ERASURES)?),
    };
    let mut features = BTreeSet::new();
    for f in &g.features {
        // `wipe_license::Feature` decides; FEATURES only words the error.
        if serde_json::from_value::<Feature>(Value::String(f.clone())).is_err() {
            return Err(format!(
                "grants.features must contain only {}",
                FEATURES.join(", ")
            ));
        }
        features.insert(f.clone());
    }
    let term_months = in_range("grants.term_months", g.term_months, 1, TERM_MAX_MONTHS)?;
    if interval == "annual" && term_months != ANNUAL_TERM_MONTHS {
        return Err(format!(
            "grants.term_months must be {ANNUAL_TERM_MONTHS} for an annual offering"
        ));
    }
    Ok(NewOffering {
        name: name.to_string(),
        description: description.to_string(),
        price_minor,
        currency: currency.to_ascii_uppercase(),
        interval,
        grants: Grants {
            scope,
            seats: seats as i32,
            quota_erasures,
            features: features.into_iter().collect(),
            term_months: term_months as i32,
        },
    })
}

// ---------------------------------------------------------------------------
// Shapes

/// An offering as the admin area shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offering {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub price_minor: i64,
    pub currency: String,
    /// One of [`INTERVALS`].
    pub interval: String,
    /// `draft`, `published` or `retired`.
    pub status: String,
    pub grants: Grants,
    /// RFC 3339, UTC.
    pub created_at: String,
    pub updated_at: String,
    pub published_at: Option<String>,
    pub retired_at: Option<String>,
}

impl Offering {
    /// Whether `new` has this offering's price, currency, interval and
    /// grants.
    fn same_terms(&self, new: &NewOffering) -> bool {
        self.price_minor == new.price_minor
            && self.currency == new.currency
            && self.interval == new.interval
            && self.grants == new.grants
    }

    /// `{field: {from, to}}` for every field `new` changes; `grants` is one
    /// field.
    fn changes(&self, new: &NewOffering) -> Map<String, Value> {
        let mut out = Map::new();
        let mut diff = |field: &str, from: Value, to: Value| {
            if from != to {
                out.insert(field.into(), json!({ "from": from, "to": to }));
            }
        };
        diff("name", json!(self.name), json!(new.name));
        diff(
            "description",
            json!(self.description),
            json!(new.description),
        );
        diff(
            "price_minor",
            json!(self.price_minor),
            json!(new.price_minor),
        );
        diff("currency", json!(self.currency), json!(new.currency));
        diff("interval", json!(self.interval), json!(new.interval));
        diff("grants", json!(self.grants), json!(new.grants));
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferingList {
    /// Drafts, then published, then retired; each by lower-cased name, id.
    pub offerings: Vec<Offering>,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    name: String,
    description: String,
    price_minor: i64,
    currency: String,
    billing_interval: String,
    status: String,
    scope: String,
    seats: i32,
    quota_erasures: Option<i64>,
    features: Vec<String>,
    term_months: i32,
    created_at: String,
    updated_at: String,
    published_at: Option<String>,
    retired_at: Option<String>,
}

impl From<Row> for Offering {
    fn from(r: Row) -> Self {
        Offering {
            id: r.id,
            name: r.name,
            description: r.description,
            price_minor: r.price_minor,
            currency: r.currency,
            interval: r.billing_interval,
            status: r.status,
            grants: Grants {
                scope: r.scope,
                seats: r.seats,
                quota_erasures: r.quota_erasures,
                features: r.features,
                term_months: r.term_months,
            },
            created_at: r.created_at,
            updated_at: r.updated_at,
            published_at: r.published_at,
            retired_at: r.retired_at,
        }
    }
}

/// A refused change (409), or a failure (500).
#[derive(Debug, thiserror::Error)]
pub enum OfferingError {
    /// The 409 message.
    #[error("{0}")]
    Conflict(&'static str),
    #[error(transparent)]
    Db(sqlx::Error),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

impl From<sqlx::Error> for OfferingError {
    fn from(e: sqlx::Error) -> Self {
        match &e {
            sqlx::Error::Database(d) if d.constraint() == Some(NAME_INDEX) => {
                Self::Conflict(MSG_DUPLICATE_NAME)
            }
            _ => Self::Db(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Queries

fn select() -> String {
    format!(
        "SELECT id, name, description, price_minor, currency, billing_interval, status, scope, \
                seats, quota_erasures, features, term_months, \
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS created_at, \
                to_char(updated_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS updated_at, \
                to_char(published_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS published_at, \
                to_char(retired_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS retired_at \
         FROM offerings"
    )
}

async fn by_id(conn: &mut PgConnection, id: Uuid) -> Result<Option<Offering>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(&format!("{} WHERE id = $1", select()))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(row.map(Offering::from))
}

/// Lock the offering row and read it; `None` when there is none.
async fn lock(conn: &mut PgConnection, id: Uuid) -> Result<Option<Offering>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(&format!("{} WHERE id = $1 FOR UPDATE", select()))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(row.map(Offering::from))
}

async fn fetch(conn: &mut PgConnection, id: Uuid) -> Result<Offering, sqlx::Error> {
    by_id(conn, id).await?.ok_or(sqlx::Error::RowNotFound)
}

// ---------------------------------------------------------------------------
// Reads

/// Every offering, drafts first, then published, then retired; each by
/// lower-cased name, then id. Audited as `admin.offerings_listed
/// {offering_ids}`.
pub async fn list(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
) -> Result<OfferingList, OfferingError> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "{} ORDER BY CASE status WHEN 'draft' THEN 0 WHEN 'published' THEN 1 ELSE 2 END, \
                  lower(name), id",
        select()
    ))
    .fetch_all(&mut *conn)
    .await?;
    let offerings: Vec<Offering> = rows.into_iter().map(Offering::from).collect();
    let ids: Vec<Uuid> = offerings.iter().map(|o| o.id).collect();
    audit_read(
        conn,
        admin,
        "admin.offerings_listed",
        "search:offerings",
        json!({ "offering_ids": ids }),
    )
    .await?;
    Ok(OfferingList { offerings })
}

/// One offering; `None` (no audit row) when there is none. Audited as
/// `admin.offering_viewed`.
pub async fn detail(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
) -> Result<Option<Offering>, OfferingError> {
    let Some(offering) = by_id(conn, id).await? else {
        return Ok(None);
    };
    audit_read(
        conn,
        admin,
        "admin.offering_viewed",
        &subject(id),
        json!({}),
    )
    .await?;
    Ok(Some(offering))
}

// ---------------------------------------------------------------------------
// Changes

/// Create a draft offering by `admin`. Audited as `admin.offering_created
/// {offering}`. A name in use by another offering not retired is
/// [`MSG_DUPLICATE_NAME`].
pub async fn create(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    new: &NewOffering,
) -> Result<Offering, OfferingError> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO offerings (name, description, price_minor, currency, billing_interval, \
                                scope, seats, quota_erasures, features, term_months, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id",
    )
    .bind(&new.name)
    .bind(&new.description)
    .bind(new.price_minor)
    .bind(&new.currency)
    .bind(&new.interval)
    .bind(&new.grants.scope)
    .bind(new.grants.seats)
    .bind(new.grants.quota_erasures)
    .bind(&new.grants.features)
    .bind(new.grants.term_months)
    .bind(admin.user_id())
    .fetch_one(&mut *conn)
    .await?;
    let offering = fetch(conn, id).await?;
    audit_read(
        conn,
        admin,
        "admin.offering_created",
        &subject(id),
        json!({ "offering": offering }),
    )
    .await?;
    Ok(offering)
}

/// Replace the offering's fields with `new`. A retired offering is
/// [`MSG_RETIRED`]; a published one whose terms `new` changes is
/// [`MSG_TERMS_FROZEN`]. A change bumps `updated_at` and is audited as
/// `admin.offering_updated {changes: {field: {from, to}}}`; a body that
/// changes nothing is audited as `admin.offering_viewed`. `None` (no audit
/// row) when there is no such offering.
pub async fn update(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
    new: &NewOffering,
) -> Result<Option<Offering>, OfferingError> {
    let Some(current) = lock(conn, id).await? else {
        return Ok(None);
    };
    match current.status.as_str() {
        "retired" => return Err(OfferingError::Conflict(MSG_RETIRED)),
        "published" if !current.same_terms(new) => {
            return Err(OfferingError::Conflict(MSG_TERMS_FROZEN))
        }
        _ => {}
    }
    let changes = current.changes(new);
    if changes.is_empty() {
        audit_read(
            conn,
            admin,
            "admin.offering_viewed",
            &subject(id),
            json!({}),
        )
        .await?;
        return Ok(Some(current));
    }
    sqlx::query(
        "UPDATE offerings SET name = $2, description = $3, price_minor = $4, currency = $5, \
                billing_interval = $6, scope = $7, seats = $8, quota_erasures = $9, \
                features = $10, term_months = $11, updated_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(&new.name)
    .bind(&new.description)
    .bind(new.price_minor)
    .bind(&new.currency)
    .bind(&new.interval)
    .bind(&new.grants.scope)
    .bind(new.grants.seats)
    .bind(new.grants.quota_erasures)
    .bind(&new.grants.features)
    .bind(new.grants.term_months)
    .execute(&mut *conn)
    .await?;
    audit_read(
        conn,
        admin,
        "admin.offering_updated",
        &subject(id),
        json!({ "changes": changes }),
    )
    .await?;
    Ok(Some(fetch(conn, id).await?))
}

/// Move the offering from `from` to `to`, stamping `stamp` (`published_at`
/// or `retired_at`); any other status is `refusal`.
async fn transition(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
    (from, to, stamp): (&str, &str, &str),
    refusal: &'static str,
    action: &str,
) -> Result<Option<Offering>, OfferingError> {
    let Some(current) = lock(conn, id).await? else {
        return Ok(None);
    };
    if current.status != from {
        return Err(OfferingError::Conflict(refusal));
    }
    sqlx::query(&format!(
        "UPDATE offerings SET status = $2, {stamp} = now(), updated_at = now() WHERE id = $1"
    ))
    .bind(id)
    .bind(to)
    .execute(&mut *conn)
    .await?;
    audit_read(conn, admin, action, &subject(id), json!({})).await?;
    Ok(Some(fetch(conn, id).await?))
}

/// Publish a draft (else [`MSG_PUBLISH_NOT_DRAFT`]): its terms freeze.
/// Audited as `admin.offering_published`. `None` (no audit row) when there
/// is no such offering.
pub async fn publish(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
) -> Result<Option<Offering>, OfferingError> {
    transition(
        conn,
        admin,
        id,
        ("draft", "published", "published_at"),
        MSG_PUBLISH_NOT_DRAFT,
        "admin.offering_published",
    )
    .await
}

/// Retire a published offering (else [`MSG_RETIRE_NOT_PUBLISHED`]); it can
/// never change again and its name is free for a new offering. Audited as
/// `admin.offering_retired`. `None` (no audit row) when there is no such
/// offering.
pub async fn retire(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
) -> Result<Option<Offering>, OfferingError> {
    transition(
        conn,
        admin,
        id,
        ("published", "retired", "retired_at"),
        MSG_RETIRE_NOT_PUBLISHED,
        "admin.offering_retired",
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> OfferingRequest {
        serde_json::from_value(json!({
            "name": "  Station — annual, 1 seat ",
            "description": "One station.\nUnlimited erasures.",
            "price_minor": 120000,
            "currency": "usd",
            "interval": "annual",
            "grants": {
                "scope": "machine",
                "seats": 1,
                "quota_erasures": null,
                "features": ["pdf_certificates", "hub_sync", "pdf_certificates"],
                "term_months": 12
            }
        }))
        .unwrap()
    }

    fn err(f: impl FnOnce(&mut OfferingRequest)) -> String {
        let mut r = req();
        f(&mut r);
        validate(&r).unwrap_err()
    }

    /// Exhaustive: a new `Feature` variant fails to compile here until it is
    /// added to [`FEATURES`] (and the portal's `adminOfferings.ts`).
    fn name_of(f: Feature) -> &'static str {
        match f {
            Feature::EnterpriseMode => "enterprise_mode",
            Feature::HubSync => "hub_sync",
            Feature::ErpIntegration => "erp_integration",
            Feature::PdfCertificates => "pdf_certificates",
            Feature::CertAnchoring => "cert_anchoring",
            Feature::OperatorSso => "operator_sso",
        }
    }

    #[test]
    fn features_mirror_wipe_license_feature() {
        let all = [
            Feature::EnterpriseMode,
            Feature::HubSync,
            Feature::ErpIntegration,
            Feature::PdfCertificates,
            Feature::CertAnchoring,
            Feature::OperatorSso,
        ];
        assert_eq!(all.len(), FEATURES.len());
        for (f, name) in all.into_iter().zip(FEATURES) {
            assert_eq!(name_of(f), name);
            assert_eq!(serde_json::to_value(f).unwrap(), json!(name));
        }
    }

    #[test]
    fn a_good_body_is_normalised() {
        let o = validate(&req()).unwrap();
        assert_eq!(o.name, "Station — annual, 1 seat");
        assert_eq!(o.currency, "USD");
        assert_eq!(o.interval, "annual");
        assert_eq!(o.grants.features, ["hub_sync", "pdf_certificates"]);
        assert_eq!(o.grants.quota_erasures, None);
        let mut r = req();
        r.interval = "one_off".into();
        r.grants.term_months = 120;
        r.grants.quota_erasures = Some(Some(QUOTA_MAX_ERASURES));
        r.grants.seats = SEATS_MAX;
        r.price_minor = PRICE_MAX_MINOR;
        r.description = String::new();
        r.name = "x".repeat(100);
        let o = validate(&r).unwrap();
        assert_eq!(o.grants.term_months, 120);
        assert_eq!(o.description, "");
    }

    #[test]
    fn bad_bodies_name_the_field() {
        assert_eq!(err(|r| r.name = " ".into()), "name must not be blank");
        assert!(err(|r| r.name = "x".repeat(101)).starts_with("name must be at most 100"));
        assert!(err(|r| r.name = "a\nb".into()).starts_with("name must not contain"));
        assert!(err(|r| r.description = "x".repeat(2001))
            .starts_with("description must be at most 2000"));
        assert!(err(|r| r.description = "a\u{7}".into()).starts_with("description"));
        for p in [0, -1, PRICE_MAX_MINOR + 1] {
            assert!(err(|r| r.price_minor = p).starts_with("price_minor"), "{p}");
        }
        for c in ["US", "USDT", "U5D", "", "ÜSD"] {
            assert!(
                err(|r| r.currency = c.into()).starts_with("currency"),
                "{c}"
            );
        }
        assert!(err(|r| r.interval = "monthly".into()).starts_with("interval"));
        assert!(err(|r| r.interval = "Annual".into()).starts_with("interval"));
        assert!(err(|r| r.grants.scope = "fleet".into()).starts_with("grants.scope"));
        for s in [0, SEATS_MAX + 1] {
            assert!(
                err(|r| r.grants.seats = s).starts_with("grants.seats"),
                "{s}"
            );
        }
        for q in [0, -5, QUOTA_MAX_ERASURES + 1] {
            assert!(
                err(|r| r.grants.quota_erasures = Some(Some(q)))
                    .starts_with("grants.quota_erasures"),
                "{q}"
            );
        }
        assert!(err(|r| r.grants.quota_erasures = None).starts_with("grants.quota_erasures"));
        assert!(err(|r| r.grants.features.push("Hub_Sync".into())).starts_with("grants.features"));
        assert!(err(|r| {
            r.interval = "one_off".into();
            r.grants.term_months = 0;
        })
        .starts_with("grants.term_months"));
        assert!(err(|r| {
            r.interval = "one_off".into();
            r.grants.term_months = 121;
        })
        .starts_with("grants.term_months"));
        assert_eq!(
            err(|r| r.grants.term_months = 24),
            "grants.term_months must be 12 for an annual offering"
        );
    }

    #[test]
    fn unknown_keys_and_a_missing_quota_are_refused() {
        let mut v = json!({
            "name": "n", "description": "", "price_minor": 1, "currency": "USD",
            "interval": "one_off",
            "grants": {"scope": "site", "seats": 1, "quota_erasures": 5, "features": [],
                       "term_months": 1}
        });
        assert!(serde_json::from_value::<OfferingRequest>(v.clone()).is_ok());
        v["status"] = json!("published");
        assert!(serde_json::from_value::<OfferingRequest>(v.clone()).is_err());
        v.as_object_mut().unwrap().remove("status");
        v["grants"]["fingerprint"] = json!("x");
        assert!(serde_json::from_value::<OfferingRequest>(v.clone()).is_err());
        v["grants"].as_object_mut().unwrap().remove("fingerprint");
        v["grants"]
            .as_object_mut()
            .unwrap()
            .remove("quota_erasures");
        let r: OfferingRequest = serde_json::from_value(v).unwrap();
        assert_eq!(r.grants.quota_erasures, None);
        assert!(validate(&r).is_err());
    }

    #[test]
    fn changes_list_only_changed_fields() {
        let new = validate(&req()).unwrap();
        let current = Offering {
            id: Uuid::nil(),
            name: new.name.clone(),
            description: new.description.clone(),
            price_minor: 99000,
            currency: new.currency.clone(),
            interval: new.interval.clone(),
            status: "draft".into(),
            grants: new.grants.clone(),
            created_at: String::new(),
            updated_at: String::new(),
            published_at: None,
            retired_at: None,
        };
        assert!(!current.same_terms(&new));
        assert_eq!(
            Value::Object(current.changes(&new)),
            json!({"price_minor": {"from": 99000, "to": 120000}})
        );
        let mut copy = current.clone();
        copy.price_minor = new.price_minor;
        assert!(copy.same_terms(&new));
        assert!(copy.changes(&new).is_empty());
        copy.name = "Old".into();
        assert!(copy.same_terms(&new));
        assert_eq!(copy.changes(&new).len(), 1);
    }
}
