//! Stations and their licences (licence-delivery story 1, CAP-1, CAP-2,
//! ADR-0007 §5): an owner registers a station's instance key against one of
//! the organization's current entitlements, the hub signs a licence for it
//! with the configured vendor root, and anyone in the organization downloads
//! the exact file. The only code that writes the `stations` and `licences`
//! tables.
//!
//! - **Register** ([`register`]): with the entitlement row locked `FOR
//!   UPDATE`, checks the entitlement is current (not refunded, term not
//!   ended), the key is not already registered in the organization and a seat
//!   is free (seats used = stations on the entitlement not released), then
//!   signs through [`wipe_license::VendorRoot::issue`] and stores the
//!   station,
//!   the licence file and two audit rows (`station.registered`,
//!   `licence.issued`) in the caller's transaction. Nothing is written on
//!   any refusal.
//! - **The licence mirrors the entitlement exactly:** `customer_id` /
//!   `customer_name` are the organization's id and name, the quota is the
//!   order's `quota_erasures` (`None` is unlimited), the features its feature
//!   names, every method is allowed, `not_before` = `issued_at` = now and
//!   `not_after` = the entitlement's `term_end`. A machine entitlement's
//!   licence names `machine_fingerprint(station_id)` (the bench's
//!   `--station-id`); a site entitlement's names the entered site, or the
//!   organization id when none was entered.
//! - **Renewal reissue** ([`reissue`], licence-delivery story 2, CAP-3): when
//!   a paid renewal moves an entitlement's `term_end` past the end of a
//!   station's active licence, the station shows `renewal_available` and an
//!   owner asks for a renewed licence, or a connected station fetches it
//!   ([`crate::station_link`], station-enrollment story 5: the same path,
//!   audited with actor `station:<id>`). With the station and its entitlement
//!   locked `FOR UPDATE`, the hub supersedes the old licence (`licence.superseded
//!   {instance_key_id, reason: "renewed", superseded_by}`) and signs a new one
//!   for the same key, scope, quota and features to the new `term_end`
//!   (`licence.issued` with `reissue_of`). The station keeps its seat and
//!   registration. Nothing is reissued by the payment path itself: a missing
//!   vendor root can never make a renewal payment fail to apply.
//! - **Vendor issue** ([`vendor_issue`], licence-delivery story 3, CAP-4): a
//!   vendor admin signs a licence for a customer organization outside any
//!   entitlement and seat (a trial, an evaluation, a goodwill grant), with
//!   entitlements typed by hand and a mandatory reason. It belongs to no
//!   station and no entitlement and is stored with `source = 'vendor'`.
//! - Register, reissue and vendor issue sign and store through one core
//!   (`sign_and_store`), so the three cannot drift.
//! - **No lapse policy** (ADR-0013 §5): the organization lapse-policy
//!   setting is retired. Every licence the hub signs omits `lapse_policy`
//!   (so it parses as `evaluation`); the stored `organizations.lapse_policy`
//!   column is no longer read, and reissue is for renewals only.
//! - **Revoke** ([`revoke`], story 3): a vendor admin marks an `active` or
//!   `superseded` licence `revoked` with a reason; `revoked` is terminal. The
//!   station (if any) keeps its registration and seat and can be released,
//!   but is not reissued ([`MSG_LICENCE_REVOKED`]), and its owner cannot
//!   download the revoked file again. Revocation is recorded in the hub only:
//!   the file keeps verifying offline until its own `not_after`.
//! - **Release** ([`release`]): frees the station's seat and marks its active
//!   licence `superseded`, auditing `station.released` and
//!   `licence.superseded`. The released licence file keeps verifying offline
//!   until its own `not_after`: the hub does not distribute revocation or
//!   supersession to stations, and nothing here can stop a station erasing
//!   (ROADMAP §0.2).
//! - **Download** ([`licence_document`]): the stored text, byte for byte, as
//!   `wipe_license::save_license` would have written it
//!   (`serde_json::to_string_pretty`), audited as `licence.downloaded`. A
//!   revoked licence is refused (409 [`MSG_LICENCE_REVOKED`]).
//!
//! Every customer query is scoped to the caller's organization: another
//! organization's station, licence or entitlement is indistinguishable from
//! an unknown one, and customer routes see station licences only. The vendor
//! functions ([`vendor_issue`], [`revoke`]) take a [`VendorAdmin`]; their
//! reads live in [`crate::licence_admin`]. Migrations 0014 and 0015 repeat
//! the lifecycle rules as constraints and triggers.

use base64::{engine::general_purpose::STANDARD_NO_PAD as B64, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::postgres::PgConnection;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;
use wipe_license::{
    machine_fingerprint, AllowedMethods, Entitlements, Feature, Quota, Scope, VendorRoot,
};

use crate::admin::{VendorAdmin, RFC3339_UTC};
use crate::audit::{self, AuditEntry, AuditError};
use crate::auth::actor;
use crate::licence_admin::NewVendorLicence;

pub const MSG_NO_FREE_SEAT: &str = "every seat of this entitlement is in use; release a station \
     or buy more seats";
pub const MSG_STATION_ID_REQUIRED: &str = "a machine licence needs the station's id: the \
     --station-id it was started with, or standalone if it has none";
pub const MSG_BAD_INSTANCE_KEY: &str = "the instance key id must be ed25519: followed by 22 \
     characters; copy it from the station";
pub const MSG_ALREADY_REGISTERED: &str =
    "a station with this instance key is already registered in your organization";
pub const MSG_ENTITLEMENT_NOT_CURRENT: &str =
    "this entitlement has ended or was refunded, so it has no seats to register";
pub const MSG_ISSUANCE_UNAVAILABLE: &str =
    "licence issuance is unavailable: this hub has no vendor root configured";
pub const MSG_OWNER_ONLY_STATIONS: &str =
    "only an organization owner can register, renew or release stations";
pub const MSG_LICENCE_UP_TO_DATE: &str =
    "this station's licence already runs to the end of its entitlement's term";
pub const MSG_ALREADY_RELEASED: &str = "this station has already been released";
pub const MSG_NO_STATION: &str = "no such station";
pub const MSG_NO_LICENCE: &str = "no such licence";
pub const MSG_LICENCE_REVOKED: &str = "this licence was revoked by the vendor; contact support";
pub const MSG_ALREADY_REVOKED: &str = "this licence is already revoked";

/// The longest station id, label or site accepted.
pub const TEXT_MAX_CHARS: usize = 200;

/// The prefix of a `wipe_cert` public key id.
pub const KEY_ID_PREFIX: &str = "ed25519:";

/// Audit subject of one station.
pub fn station_subject(id: Uuid) -> String {
    format!("station:{id}")
}

/// Audit subject of one licence.
pub fn licence_subject(id: Uuid) -> String {
    format!("licence:{id}")
}

// ---------------------------------------------------------------------------
// Shapes

/// A station licence as its organization sees it (never the file itself,
/// never a revocation reason). Timestamps are RFC 3339 UTC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Licence {
    /// The signed body's `license_id`.
    pub id: Uuid,
    /// The station row it was issued for.
    pub station_id: Uuid,
    pub entitlement_id: Uuid,
    pub instance_key_id: String,
    pub root_key_id: String,
    pub not_after: String,
    pub issued_at: String,
    /// `active`, `superseded` or `revoked`.
    pub status: String,
    pub superseded_at: Option<String>,
    /// Set exactly when `revoked`.
    pub revoked_at: Option<String>,
}

/// The [`Licence`] columns of `licences` (unqualified).
fn licence_columns() -> String {
    format!(
        "id, station_id, entitlement_id, instance_key_id, root_key_id, \
         to_char(not_after AT TIME ZONE 'UTC', {RFC3339_UTC}) AS not_after, \
         to_char(issued_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS issued_at, \
         status, \
         to_char(superseded_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS superseded_at, \
         to_char(revoked_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS revoked_at"
    )
}

/// The station licence `id`, as its organization sees it.
async fn station_licence(conn: &mut PgConnection, id: Uuid) -> Result<Licence, StationError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {} FROM licences WHERE id = $1 AND source = 'station'",
        licence_columns()
    ))
    .bind(id)
    .fetch_one(&mut *conn)
    .await?)
}

/// A registered station and its latest licence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Station {
    pub id: Uuid,
    pub entitlement_id: Uuid,
    /// The entitlement's offering name when it was ordered.
    pub offering_name: String,
    /// The entitlement's scope: `machine` or `site`.
    pub scope: String,
    pub instance_key_id: String,
    /// The bench's `--station-id`, as entered.
    pub station_id: Option<String>,
    pub label: Option<String>,
    pub site: Option<String>,
    pub registered_at: String,
    pub released_at: Option<String>,
    /// The station's most recently issued licence.
    pub licence: Option<Licence>,
    /// A renewed licence can be issued: the station is not released, its
    /// entitlement is current and its latest licence is active but ends
    /// before the entitlement's `term_end` (it was renewed). [`reissue`] is
    /// allowed only when this is true.
    pub renewal_available: bool,
}

#[derive(sqlx::FromRow)]
struct StationRow {
    id: Uuid,
    entitlement_id: Uuid,
    offering_name: String,
    scope: String,
    instance_key_id: String,
    station_id: Option<String>,
    label: Option<String>,
    site: Option<String>,
    registered_at: String,
    released_at: Option<String>,
    licence_id: Option<Uuid>,
    root_key_id: Option<String>,
    not_after: Option<String>,
    issued_at: Option<String>,
    licence_status: Option<String>,
    superseded_at: Option<String>,
    revoked_at: Option<String>,
    renewal_available: bool,
}

impl From<StationRow> for Station {
    fn from(r: StationRow) -> Self {
        let licence = match (r.licence_id, r.root_key_id, r.not_after, r.issued_at) {
            (Some(id), Some(root_key_id), Some(not_after), Some(issued_at)) => Some(Licence {
                id,
                station_id: r.id,
                entitlement_id: r.entitlement_id,
                instance_key_id: r.instance_key_id.clone(),
                root_key_id,
                not_after,
                issued_at,
                status: r.licence_status.unwrap_or_default(),
                superseded_at: r.superseded_at,
                revoked_at: r.revoked_at,
            }),
            _ => None,
        };
        Station {
            id: r.id,
            entitlement_id: r.entitlement_id,
            offering_name: r.offering_name,
            scope: r.scope,
            instance_key_id: r.instance_key_id,
            station_id: r.station_id,
            label: r.label,
            site: r.site,
            registered_at: r.registered_at,
            released_at: r.released_at,
            licence,
            renewal_available: r.renewal_available,
        }
    }
}

/// An entitlement `e` that still grants seats: not refunded, term not ended
/// ([`crate::entitlements::CURRENT_SQL`]).
pub(crate) const ENTITLEMENT_CURRENT: &str = crate::entitlements::CURRENT_SQL;

/// The station `s`'s latest licence as `l` (absent: all NULL): its active
/// one if it has one, else the most recently issued. Active first, so a hub
/// clock that stepped back between two issuances cannot rank a superseded
/// licence above the one that replaced it.
pub(crate) const LATEST_LICENCE: &str = "LEFT JOIN LATERAL (SELECT * FROM licences \
                                                 WHERE station_id = s.id \
                                                 ORDER BY (status = 'active') DESC, \
                                                          issued_at DESC, id DESC \
                                                 LIMIT 1) l ON true";

/// The latest licence `l` is active and ends before its entitlement `e`'s
/// term (compared as timestamps): a renewal moved the term on.
pub(crate) const LICENCE_BEHIND_TERM: &str = "(l.status = 'active' AND l.not_after < e.term_end)";

/// `SELECT … FROM stations s …`, every column of [`StationRow`] with the
/// station's latest licence; callers add the `WHERE` on `s`.
fn select_stations() -> String {
    format!(
        "SELECT s.id, s.entitlement_id, o.offering_name, o.scope, s.instance_key_id, \
                s.station_id, s.label, s.site, \
                to_char(s.registered_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS registered_at, \
                to_char(s.released_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS released_at, \
                l.id AS licence_id, l.root_key_id, \
                to_char(l.not_after AT TIME ZONE 'UTC', {RFC3339_UTC}) AS not_after, \
                to_char(l.issued_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS issued_at, \
                l.status AS licence_status, \
                to_char(l.superseded_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS superseded_at, \
                to_char(l.revoked_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS revoked_at, \
                COALESCE(s.released_at IS NULL AND {ENTITLEMENT_CURRENT} \
                         AND {LICENCE_BEHIND_TERM}, false) AS renewal_available \
         FROM stations s \
         JOIN entitlements e ON e.id = s.entitlement_id \
         JOIN orders o ON o.id = e.order_id \
         {LATEST_LICENCE}"
    )
}

/// The organization's stations, released ones last, newest first.
pub async fn list<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
) -> Result<Vec<Station>, sqlx::Error> {
    let rows: Vec<StationRow> = sqlx::query_as(&format!(
        "{} WHERE s.organization_id = $1 \
         ORDER BY s.released_at IS NOT NULL, s.registered_at DESC, s.id DESC",
        select_stations()
    ))
    .bind(organization_id)
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(Station::from).collect())
}

/// One of the organization's stations; `None` for another organization's.
pub async fn get<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    id: Uuid,
) -> Result<Option<Station>, sqlx::Error> {
    let row: Option<StationRow> = sqlx::query_as(&format!(
        "{} WHERE s.organization_id = $1 AND s.id = $2",
        select_stations()
    ))
    .bind(organization_id)
    .bind(id)
    .fetch_optional(executor)
    .await?;
    Ok(row.map(Station::from))
}

// ---------------------------------------------------------------------------
// Validation

/// What an owner asks to register.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterRequest {
    pub entitlement_id: Uuid,
    pub instance_key_id: String,
    #[serde(default)]
    pub station_id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub site: Option<String>,
}

/// A validated [`RegisterRequest`]: trimmed, blanks as `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewStation {
    pub entitlement_id: Uuid,
    pub instance_key_id: String,
    pub station_id: Option<String>,
    pub label: Option<String>,
    pub site: Option<String>,
}

/// Whether `id` (already trimmed) has the shape of a `wipe_cert` public key
/// id: `ed25519:` and 22 chars of unpadded standard base64 encoding 16 bytes.
pub fn is_instance_key_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix(KEY_ID_PREFIX) else {
        return false;
    };
    rest.len() == 22
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
        && B64.decode(rest).is_ok_and(|bytes| bytes.len() == 16)
}

pub(crate) fn optional_text(
    field: &str,
    value: Option<&str>,
) -> Result<Option<String>, StationError> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > TEXT_MAX_CHARS {
        return Err(StationError::BadRequest(format!(
            "{field} must be at most {TEXT_MAX_CHARS} characters"
        )));
    }
    Ok(Some(v.to_string()))
}

impl RegisterRequest {
    /// Trim every field; check the key id's shape and the text lengths. The
    /// machine-scope station id requirement needs the entitlement, so
    /// [`register`] checks it.
    pub fn validate(&self) -> Result<NewStation, StationError> {
        let instance_key_id = self.instance_key_id.trim();
        if !is_instance_key_id(instance_key_id) {
            return Err(StationError::BadRequest(MSG_BAD_INSTANCE_KEY.into()));
        }
        Ok(NewStation {
            entitlement_id: self.entitlement_id,
            instance_key_id: instance_key_id.to_string(),
            station_id: optional_text("station_id", self.station_id.as_deref())?,
            label: optional_text("label", self.label.as_deref())?,
            site: optional_text("site", self.site.as_deref())?,
        })
    }
}

// ---------------------------------------------------------------------------
// Errors

#[derive(Debug, thiserror::Error)]
pub enum StationError {
    /// 400 with this message.
    #[error("{0}")]
    BadRequest(String),
    /// 404 with this message.
    #[error("{0}")]
    NotFound(&'static str),
    /// 409 with this message.
    #[error("{0}")]
    Conflict(&'static str),
    /// Signing or encoding failed, or stored data is unusable: a 500.
    #[error("licence issuance failed: {0}")]
    Issue(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

fn is_unique_violation(e: &sqlx::Error, constraint: &str) -> bool {
    matches!(e, sqlx::Error::Database(d)
        if d.code().as_deref() == Some("23505") && d.constraint() == Some(constraint))
}

// ---------------------------------------------------------------------------
// Register

/// The entitlement a registration draws from, or a reissued station's,
/// locked `FOR UPDATE`.
#[derive(sqlx::FromRow)]
struct LockedEntitlement {
    id: Uuid,
    scope: String,
    seats: i32,
    quota_erasures: Option<i64>,
    features: Vec<String>,
    /// RFC 3339 UTC with microseconds: exactly the stored `term_end`.
    term_end: String,
    current: bool,
}

/// RFC 3339 UTC with microseconds, so a timestamp round-trips through
/// Postgres unchanged.
const RFC3339_UTC_MICROS: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS.US"Z"'"#;

/// The [`LockedEntitlement`] columns, from `entitlements e JOIN orders o`.
fn locked_entitlement_columns() -> String {
    format!(
        "e.id, o.scope, o.seats, o.quota_erasures, o.features, \
         to_char(e.term_end AT TIME ZONE 'UTC', {RFC3339_UTC_MICROS}) AS term_end, \
         {ENTITLEMENT_CURRENT} AS current"
    )
}

/// What [`register`] or [`reissue`] stored.
#[derive(Debug, Clone)]
pub struct Registered {
    pub station: Station,
    pub licence: Licence,
}

/// The licence's entitlements for `ent`, held by organization `org`, for a
/// station registered with `station_id` (the bench's `--station-id`) and
/// `site`.
fn licence_entitlements(
    ent: &LockedEntitlement,
    org_id: Uuid,
    org_name: &str,
    station_id: Option<&str>,
    site: Option<&str>,
    now: OffsetDateTime,
    not_after: OffsetDateTime,
) -> Result<Entitlements, StationError> {
    let features = ent
        .features
        .iter()
        .map(|f| {
            serde_json::from_value::<Feature>(Value::String(f.clone()))
                .map_err(|_| StationError::Issue(format!("unknown feature {f:?} on {}", ent.id)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let quota = match ent.quota_erasures {
        None => Quota::Unlimited,
        Some(n) => Quota::Count {
            erasures: u64::try_from(n)
                .map_err(|_| StationError::Issue(format!("negative quota on {}", ent.id)))?,
        },
    };
    let scope = match ent.scope.as_str() {
        "machine" => Scope::Machine {
            fingerprint: machine_fingerprint(
                station_id
                    .ok_or_else(|| StationError::BadRequest(MSG_STATION_ID_REQUIRED.into()))?,
            ),
        },
        "site" => Scope::Site {
            site_id: site.map_or_else(|| org_id.to_string(), str::to_string),
        },
        other => {
            return Err(StationError::Issue(format!(
                "unknown scope {other:?} on {}",
                ent.id
            )))
        }
    };
    Ok(Entitlements {
        customer_id: org_id.to_string(),
        customer_name: org_name.to_string(),
        quota,
        scope,
        not_before: now,
        not_after,
        features,
        allowed_methods: AllowedMethods::All,
        machine_binding: None,
    })
}

/// Now, truncated to microseconds, so a timestamp stored in Postgres equals
/// the signed one.
pub(crate) fn now_micros() -> Result<OffsetDateTime, StationError> {
    let now = OffsetDateTime::now_utc();
    now.replace_nanosecond(now.nanosecond() / 1_000 * 1_000)
        .map_err(|e| StationError::Issue(e.to_string()))
}

/// One licence to sign and store: what [`sign_and_store`] needs.
struct SignAndStore<'a> {
    organization_id: Uuid,
    /// The audit actor (`user:<id>`).
    actor: &'a str,
    /// The `stations` row and entitlement of a station licence; `None` for
    /// a vendor-issued one.
    station: Option<(Uuid, Uuid)>,
    instance_key_id: &'a str,
    /// Ready to sign: `not_before` is the issue time.
    entitlements: Entitlements,
    /// The vendor admin's reason for a vendor-issued licence; `None` for a
    /// station licence.
    issue_reason: Option<&'a str>,
    /// The `licence.issued` audit detail, without `root_key_id` and
    /// `not_after`, which are added here.
    detail: Value,
}

/// Sign `req.entitlements` with `root` (`issued_at` = their `not_before`),
/// store the exact file and audit `licence.issued`, in the caller's
/// transaction: the licence's id. The only code that signs or inserts a
/// licence, so register, reissue and vendor issue cannot drift. A station
/// must have no active licence left (`licences_active_station_idx`). The
/// file carries no `lapse_policy` (ADR-0013 §5): [`VendorRoot::issue`] signs
/// the default, which is omitted from the body.
async fn sign_and_store(
    conn: &mut PgConnection,
    root: &VendorRoot,
    req: SignAndStore<'_>,
) -> Result<Uuid, StationError> {
    let issued = req.entitlements.not_before;
    let not_after = req.entitlements.not_after;
    let licence_id = Uuid::new_v4();
    let certificate = root
        .issue(
            licence_id.to_string(),
            req.instance_key_id.to_string(),
            req.entitlements,
            issued,
        )
        .map_err(|e| StationError::Issue(e.to_string()))?;
    // Exactly what `wipe_license::save_license` writes.
    let document = serde_json::to_string_pretty(&certificate)
        .map_err(|e| StationError::Issue(e.to_string()))?;
    let rfc3339 = |t: OffsetDateTime| {
        t.format(&Rfc3339)
            .map_err(|e| StationError::Issue(e.to_string()))
    };
    let root_key_id = certificate.body.root_key_id.clone();
    let (source, station_id, entitlement_id) = match req.station {
        Some((station, entitlement)) => ("station", Some(station), Some(entitlement)),
        None => ("vendor", None, None),
    };
    let stored_not_after: String = sqlx::query_scalar(&format!(
        "INSERT INTO licences (id, organization_id, station_id, entitlement_id, \
                               instance_key_id, root_key_id, not_after, issued_at, document, \
                               source, issue_reason) \
         VALUES ($1, $2, $3, $4, $5, $6, $7::text::timestamptz, $8::text::timestamptz, $9, \
                 $10, $11) \
         RETURNING to_char(not_after AT TIME ZONE 'UTC', {RFC3339_UTC})"
    ))
    .bind(licence_id)
    .bind(req.organization_id)
    .bind(station_id)
    .bind(entitlement_id)
    .bind(req.instance_key_id)
    .bind(&root_key_id)
    .bind(rfc3339(not_after)?)
    .bind(rfc3339(issued)?)
    .bind(&document)
    .bind(source)
    .bind(req.issue_reason)
    .fetch_one(&mut *conn)
    .await?;

    let mut detail = req.detail;
    detail["root_key_id"] = json!(root_key_id);
    detail["not_after"] = json!(stored_not_after);
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: req.actor,
            action: "licence.issued",
            subject: &licence_subject(licence_id),
            detail,
        },
    )
    .await?;
    Ok(licence_id)
}

/// One licence to sign and store for a station row that already exists.
struct IssueLicence<'a> {
    ent: &'a LockedEntitlement,
    organization_id: Uuid,
    /// The audit actor: `user:<id>`, or `station:<id>` for a station's own
    /// fetch (station-enrollment story 5).
    actor: &'a str,
    /// The `stations` row.
    station: Uuid,
    instance_key_id: &'a str,
    /// The bench's `--station-id`, as stored.
    station_id: Option<&'a str>,
    /// The site, as stored.
    site: Option<&'a str>,
    /// The licence this one replaces, recorded in the audit detail.
    reissue_of: Option<Uuid>,
}

/// Sign a licence mirroring `req.ent` with `root` (`not_before` = `issued_at`
/// = now, `not_after` = the entitlement's `term_end`), store it as the
/// station's active licence and audit `licence.issued`, in the caller's
/// transaction, through [`sign_and_store`]. A term that ended between the
/// database's clock and ours is 409 [`MSG_ENTITLEMENT_NOT_CURRENT`]; the
/// caller's transaction then rolls back whatever it wrote.
async fn issue_licence(
    conn: &mut PgConnection,
    root: &VendorRoot,
    req: IssueLicence<'_>,
) -> Result<Licence, StationError> {
    let ent = req.ent;
    let org_name: String = sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
        .bind(req.organization_id)
        .fetch_one(&mut *conn)
        .await?;
    let now = now_micros()?;
    let not_after = OffsetDateTime::parse(&ent.term_end, &Rfc3339)
        .map_err(|e| StationError::Issue(format!("term_end {:?}: {e}", ent.term_end)))?;
    if not_after <= now {
        // The term ended between the database's clock and ours.
        return Err(StationError::Conflict(MSG_ENTITLEMENT_NOT_CURRENT));
    }
    let entitlements = licence_entitlements(
        ent,
        req.organization_id,
        &org_name,
        req.station_id,
        req.site,
        now,
        not_after,
    )?;
    let mut detail = json!({
        "station_id": req.station,
        "entitlement_id": ent.id,
        "instance_key_id": req.instance_key_id,
    });
    if let Some(old) = req.reissue_of {
        detail["reissue_of"] = json!(old);
    }
    let id = sign_and_store(
        conn,
        root,
        SignAndStore {
            organization_id: req.organization_id,
            actor: req.actor,
            station: Some((req.station, ent.id)),
            instance_key_id: req.instance_key_id,
            entitlements,
            issue_reason: None,
            detail,
        },
    )
    .await?;
    station_licence(conn, id).await
}

/// Register `new` for organization `organization_id` on behalf of
/// `user_id`, signing its licence with `root`, in the caller's transaction
/// (which the caller commits). See the module docs for the checks. Every
/// refusal but one returns before the first write; that one (the term ends
/// in the instant between the database's clock and ours, found while
/// signing) leaves its writes to roll back with the caller's transaction.
pub async fn register(
    conn: &mut PgConnection,
    root: &VendorRoot,
    organization_id: Uuid,
    user_id: Uuid,
    new: &NewStation,
) -> Result<Registered, StationError> {
    let ent: LockedEntitlement = sqlx::query_as(&format!(
        "SELECT {} \
         FROM entitlements e JOIN orders o ON o.id = e.order_id \
         WHERE e.organization_id = $1 AND e.id = $2 \
         FOR UPDATE OF e",
        locked_entitlement_columns()
    ))
    .bind(organization_id)
    .bind(new.entitlement_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(StationError::NotFound(
        crate::order_routes::MSG_NO_ENTITLEMENT,
    ))?;
    if !ent.current {
        return Err(StationError::Conflict(MSG_ENTITLEMENT_NOT_CURRENT));
    }
    if ent.scope == "machine" && new.station_id.is_none() {
        return Err(StationError::BadRequest(MSG_STATION_ID_REQUIRED.into()));
    }
    let registered: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM stations \
                        WHERE organization_id = $1 AND instance_key_id = $2 \
                          AND released_at IS NULL)",
    )
    .bind(organization_id)
    .bind(&new.instance_key_id)
    .fetch_one(&mut *conn)
    .await?;
    if registered {
        return Err(StationError::Conflict(MSG_ALREADY_REGISTERED));
    }
    // The entitlement row lock serializes registrations against it, so this
    // count cannot go stale before the insert below.
    let used: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM stations WHERE entitlement_id = $1 AND released_at IS NULL",
    )
    .bind(ent.id)
    .fetch_one(&mut *conn)
    .await?;
    if used >= i64::from(ent.seats) {
        return Err(StationError::Conflict(MSG_NO_FREE_SEAT));
    }
    let station_id: Uuid = sqlx::query_scalar(
        "INSERT INTO stations (organization_id, entitlement_id, instance_key_id, station_id, \
                               label, site, registered_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(organization_id)
    .bind(ent.id)
    .bind(&new.instance_key_id)
    .bind(&new.station_id)
    .bind(&new.label)
    .bind(&new.site)
    .bind(user_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| {
        // A registration of the same key against another entitlement won
        // the race: the partial unique index has the last word.
        if is_unique_violation(&e, "stations_active_key_idx") {
            StationError::Conflict(MSG_ALREADY_REGISTERED)
        } else {
            e.into()
        }
    })?;

    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &actor(user_id),
            action: "station.registered",
            subject: &station_subject(station_id),
            detail: json!({
                "entitlement_id": ent.id,
                "instance_key_id": new.instance_key_id,
                "station_id": new.station_id,
                "label": new.label,
                "site": new.site,
            }),
        },
    )
    .await?;
    let licence = issue_licence(
        conn,
        root,
        IssueLicence {
            ent: &ent,
            organization_id,
            actor: &actor(user_id),
            station: station_id,
            instance_key_id: &new.instance_key_id,
            station_id: new.station_id.as_deref(),
            site: new.site.as_deref(),
            reissue_of: None,
        },
    )
    .await?;
    let station = get(&mut *conn, organization_id, station_id)
        .await?
        .ok_or_else(|| StationError::Issue("a registered station vanished".into()))?;
    Ok(Registered { station, licence })
}

// ---------------------------------------------------------------------------
// Renewal reissue

/// A station to reissue, locked `FOR UPDATE` with its entitlement.
#[derive(sqlx::FromRow)]
struct LockedStation {
    released: bool,
    instance_key_id: String,
    station_id: Option<String>,
    site: Option<String>,
    #[sqlx(flatten)]
    ent: LockedEntitlement,
}

/// Issue the organization's station `id` a renewed licence on behalf of
/// `who` (the audit actor: `user:<id>` for an owner's click, `station:<id>`
/// for the station's own fetch, [`crate::station_link`]), signing it with
/// `root`, in the caller's transaction (which the
/// caller commits). Refusals, in this order and before any write (but see
/// below): 404
/// [`MSG_NO_STATION`] for an unknown or foreign station, 409
/// [`MSG_ALREADY_RELEASED`], 409 [`MSG_ENTITLEMENT_NOT_CURRENT`], 409
/// [`MSG_LICENCE_REVOKED`] when its latest licence is revoked, and 409
/// [`MSG_LICENCE_UP_TO_DATE`] unless its latest licence is active and ends
/// before the entitlement's `term_end` (the station's `renewal_available`).
/// Otherwise the old licence is `superseded` and a new one is signed exactly
/// as [`register`] signs, for the stored key, station id and site, to the
/// current `term_end`; audits `licence.issued {…, reissue_of}` and
/// `licence.superseded {station_id, instance_key_id, reason: "renewed",
/// superseded_by}`.
/// One refusal comes after the supersede: a term that ends in the instant
/// between the database's clock and ours, found while signing, is 409
/// [`MSG_ENTITLEMENT_NOT_CURRENT`] and leaves its writes to roll back with
/// the caller's transaction.
pub async fn reissue(
    conn: &mut PgConnection,
    root: &VendorRoot,
    organization_id: Uuid,
    who: &str,
    id: Uuid,
) -> Result<Registered, StationError> {
    // Station and entitlement are locked in one statement: register locks
    // only the entitlement and release only the station, so no lock cycle.
    let locked: LockedStation = sqlx::query_as(&format!(
        "SELECT s.released_at IS NOT NULL AS released, s.instance_key_id, s.station_id, \
                s.site, {} \
         FROM stations s \
         JOIN entitlements e ON e.id = s.entitlement_id \
         JOIN orders o ON o.id = e.order_id \
         WHERE s.organization_id = $1 AND s.id = $2 \
         FOR UPDATE OF s, e",
        locked_entitlement_columns()
    ))
    .bind(organization_id)
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(StationError::NotFound(MSG_NO_STATION))?;
    if locked.released {
        return Err(StationError::Conflict(MSG_ALREADY_RELEASED));
    }
    if !locked.ent.current {
        return Err(StationError::Conflict(MSG_ENTITLEMENT_NOT_CURRENT));
    }
    // A statement of its own, after the lock: under READ COMMITTED it sees a
    // concurrent reissue's licence, or a revocation, that committed while
    // this one waited (revoke takes the same station lock), so that loser
    // is refused here.
    let (old, latest_status, behind): (Option<Uuid>, Option<String>, bool) =
        sqlx::query_as(&format!(
            "SELECT l.id, l.status, COALESCE({LICENCE_BEHIND_TERM}, false) \
             FROM stations s \
             JOIN entitlements e ON e.id = s.entitlement_id \
             {LATEST_LICENCE} \
             WHERE s.id = $1"
        ))
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    if latest_status.as_deref() == Some("revoked") {
        return Err(StationError::Conflict(MSG_LICENCE_REVOKED));
    }
    let (Some(old), true) = (old, behind) else {
        return Err(StationError::Conflict(MSG_LICENCE_UP_TO_DATE));
    };

    // One active licence per station: supersede before inserting.
    sqlx::query_scalar::<_, Uuid>(
        "UPDATE licences SET status = 'superseded', superseded_at = now() \
         WHERE id = $1 AND status = 'active' RETURNING id",
    )
    .bind(old)
    .fetch_one(&mut *conn)
    .await?;
    let licence = issue_licence(
        conn,
        root,
        IssueLicence {
            ent: &locked.ent,
            organization_id,
            actor: who,
            station: id,
            instance_key_id: &locked.instance_key_id,
            station_id: locked.station_id.as_deref(),
            site: locked.site.as_deref(),
            reissue_of: Some(old),
        },
    )
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: who,
            action: "licence.superseded",
            subject: &licence_subject(old),
            detail: json!({
                "station_id": id,
                "instance_key_id": locked.instance_key_id,
                "reason": "renewed",
                "superseded_by": licence.id,
            }),
        },
    )
    .await?;
    let station = get(&mut *conn, organization_id, id)
        .await?
        .ok_or_else(|| StationError::Issue("a reissued station vanished".into()))?;
    Ok(Registered { station, licence })
}

// ---------------------------------------------------------------------------
// Release

/// Release the organization's station `id` on behalf of `user_id`, in the
/// caller's transaction: 404 [`MSG_NO_STATION`] for an unknown or foreign
/// one, 409 [`MSG_ALREADY_RELEASED`] if it is released. Its seat is free
/// again and its active licence is `superseded`; audits `station.released`
/// and `licence.superseded {reason: "station released"}`.
pub async fn release(
    conn: &mut PgConnection,
    organization_id: Uuid,
    user_id: Uuid,
    id: Uuid,
) -> Result<Station, StationError> {
    let released: Option<bool> = sqlx::query_scalar(
        "SELECT released_at IS NOT NULL FROM stations \
         WHERE organization_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(organization_id)
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    match released {
        None => return Err(StationError::NotFound(MSG_NO_STATION)),
        Some(true) => return Err(StationError::Conflict(MSG_ALREADY_RELEASED)),
        Some(false) => {}
    }
    let (entitlement_id, instance_key_id): (Uuid, String) = sqlx::query_as(
        "UPDATE stations SET released_by = $2, released_at = now() WHERE id = $1 \
         RETURNING entitlement_id, instance_key_id",
    )
    .bind(id)
    .bind(user_id)
    .fetch_one(&mut *conn)
    .await?;
    let superseded: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE licences SET status = 'superseded', superseded_at = now() \
         WHERE station_id = $1 AND status = 'active' RETURNING id",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let who = actor(user_id);
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &who,
            action: "station.released",
            subject: &station_subject(id),
            detail: json!({
                "entitlement_id": entitlement_id,
                "instance_key_id": instance_key_id,
            }),
        },
    )
    .await?;
    for licence_id in superseded {
        audit::record(
            &mut *conn,
            &AuditEntry {
                actor: &who,
                action: "licence.superseded",
                subject: &licence_subject(licence_id),
                detail: json!({ "station_id": id, "reason": "station released" }),
            },
        )
        .await?;
    }
    get(&mut *conn, organization_id, id)
        .await?
        .ok_or_else(|| StationError::Issue("a released station vanished".into()))
}

// ---------------------------------------------------------------------------
// Download

/// The stored file of the organization's station licence `id` and an audit
/// row `licence.downloaded {station_id}` by `user_id`, in the caller's
/// transaction; `None` (nothing audited) for an unknown or foreign licence
/// or a vendor-issued one, and 409 [`MSG_LICENCE_REVOKED`] (nothing
/// audited) for a revoked one: the hub never hands out a file it revoked.
pub async fn licence_document(
    conn: &mut PgConnection,
    organization_id: Uuid,
    user_id: Uuid,
    id: Uuid,
) -> Result<Option<String>, StationError> {
    let row: Option<(Uuid, String, String)> = sqlx::query_as(
        "SELECT station_id, status, document FROM licences \
         WHERE organization_id = $1 AND id = $2 AND source = 'station'",
    )
    .bind(organization_id)
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((station_id, status, document)) = row else {
        return Ok(None);
    };
    if status == "revoked" {
        return Err(StationError::Conflict(MSG_LICENCE_REVOKED));
    }
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &actor(user_id),
            action: "licence.downloaded",
            subject: &licence_subject(id),
            detail: json!({ "station_id": station_id }),
        },
    )
    .await?;
    Ok(Some(document))
}

// ---------------------------------------------------------------------------
// Vendor issue and revocation (story 3)

/// Sign `new` for customer organization `organization_id` on behalf of
/// `admin`, with `root`, in the caller's transaction: the new licence's id.
/// It belongs to no station and no entitlement and uses no seat:
/// `customer_id` / `customer_name` are the organization's id and current
/// name, `not_before` = `issued_at` = now, `not_after` = now + `new.days`,
/// every method allowed, no machine binding; machine scope names
/// `machine_fingerprint(station_id)`, site scope the entered site or the
/// organization id. Audits `licence.issued {organization_id,
/// instance_key_id, root_key_id, not_after, source: "vendor", scope,
/// quota_erasures, features, reason}`. An unknown organization or the
/// vendor's own is 404 [`crate::admin_routes::MSG_NO_ORGANIZATION`], before
/// any write.
pub async fn vendor_issue(
    conn: &mut PgConnection,
    root: &VendorRoot,
    admin: &VendorAdmin,
    organization_id: Uuid,
    new: &NewVendorLicence,
) -> Result<Uuid, StationError> {
    let org_name: String =
        sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1 AND NOT is_vendor")
            .bind(organization_id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(StationError::NotFound(
                crate::admin_routes::MSG_NO_ORGANIZATION,
            ))?;
    let now = now_micros()?;
    let not_after = now + time::Duration::days(i64::from(new.days));
    let scope = match (new.scope.as_str(), new.station_id.as_deref()) {
        ("machine", Some(station_id)) => Scope::Machine {
            fingerprint: machine_fingerprint(station_id),
        },
        ("machine", None) => return Err(StationError::BadRequest(MSG_STATION_ID_REQUIRED.into())),
        ("site", _) => Scope::Site {
            site_id: new
                .site
                .clone()
                .unwrap_or_else(|| organization_id.to_string()),
        },
        (other, _) => return Err(StationError::Issue(format!("unknown scope {other:?}"))),
    };
    let quota = match new.quota_erasures {
        None => Quota::Unlimited,
        Some(n) => Quota::Count {
            erasures: u64::try_from(n)
                .map_err(|_| StationError::Issue(format!("negative quota {n}")))?,
        },
    };
    let features = new
        .features
        .iter()
        .map(|f| {
            serde_json::from_value::<Feature>(Value::String(f.clone()))
                .map_err(|_| StationError::Issue(format!("unknown feature {f:?}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let detail = json!({
        "organization_id": organization_id,
        "instance_key_id": new.instance_key_id,
        "source": "vendor",
        "scope": scope,
        "quota_erasures": new.quota_erasures,
        "features": new.features,
        "reason": new.reason,
    });
    let entitlements = Entitlements {
        customer_id: organization_id.to_string(),
        customer_name: org_name,
        quota,
        scope,
        not_before: now,
        not_after,
        features,
        allowed_methods: AllowedMethods::All,
        machine_binding: None,
    };
    sign_and_store(
        conn,
        root,
        SignAndStore {
            organization_id,
            actor: &actor(admin.user_id()),
            station: None,
            instance_key_id: &new.instance_key_id,
            entitlements,
            issue_reason: Some(&new.reason),
            detail,
        },
    )
    .await
}

/// Revoke licence `id` (any organization's) on behalf of `admin`, with
/// `reason` (already validated), in the caller's transaction. Locks the
/// licence's station, if any, `FOR UPDATE` before the licence row (the
/// order reissue and release use), so it serializes with them. 404
/// [`MSG_NO_LICENCE`] for an unknown licence and 409 [`MSG_ALREADY_REVOKED`]
/// for a revoked one, both before any write. The station keeps its
/// registration and seat. Audits `licence.revoked {organization_id,
/// station_id, previous_status, reason}`.
pub async fn revoke(
    conn: &mut PgConnection,
    admin: &VendorAdmin,
    id: Uuid,
    reason: &str,
) -> Result<(), StationError> {
    let station: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT station_id FROM licences WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(station) = station else {
        return Err(StationError::NotFound(MSG_NO_LICENCE));
    };
    if let Some(station) = station {
        sqlx::query("SELECT 1 FROM stations WHERE id = $1 FOR UPDATE")
            .bind(station)
            .execute(&mut *conn)
            .await?;
    }
    let (organization_id, station_id, previous_status): (Uuid, Option<Uuid>, String) =
        sqlx::query_as(
            "SELECT organization_id, station_id, status FROM licences WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    if previous_status == "revoked" {
        return Err(StationError::Conflict(MSG_ALREADY_REVOKED));
    }
    sqlx::query(
        "UPDATE licences SET status = 'revoked', revoked_at = now(), revocation_reason = $2 \
         WHERE id = $1",
    )
    .bind(id)
    .bind(reason)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &actor(admin.user_id()),
            action: "licence.revoked",
            subject: &licence_subject(id),
            detail: json!({
                "organization_id": organization_id,
                "station_id": station_id,
                "previous_status": previous_status,
                "reason": reason,
            }),
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(key: &str) -> RegisterRequest {
        RegisterRequest {
            entitlement_id: Uuid::nil(),
            instance_key_id: key.into(),
            station_id: None,
            label: None,
            site: None,
        }
    }

    #[test]
    fn real_key_ids_are_accepted() {
        let id = wipe_cert::SigningKey::generate().public_key_id();
        assert!(is_instance_key_id(&id), "{id}");
        let v = RegisterRequest {
            station_id: Some("  bench-1 ".into()),
            label: Some("   ".into()),
            site: Some(" Floor 2 ".into()),
            ..req(&format!("  {id}\n"))
        }
        .validate()
        .unwrap();
        assert_eq!(v.instance_key_id, id);
        assert_eq!(v.station_id.as_deref(), Some("bench-1"));
        assert_eq!(v.label, None);
        assert_eq!(v.site.as_deref(), Some("Floor 2"));
    }

    #[test]
    fn malformed_key_ids_are_refused() {
        for bad in [
            "",
            "ed25519:",
            "ed25519:AAAAAAAAAAAAAAAAAAAAA",
            "ed25519:AAAAAAAAAAAAAAAAAAAAAAA",
            "ed25519:AAAAAAAAAAAAAAAAAAAA==",
            "ed25519:AAAAAAAAAAAAAAAAAAAA-_",
            "rsa:AAAAAAAAAAAAAAAAAAAAAA",
            "ED25519:AAAAAAAAAAAAAAAAAAAAAA",
            // Non-canonical trailing bits.
            "ed25519:AAAAAAAAAAAAAAAAAAAAAB",
        ] {
            assert!(!is_instance_key_id(bad), "{bad}");
            assert!(matches!(
                req(bad).validate(),
                Err(StationError::BadRequest(m)) if m == MSG_BAD_INSTANCE_KEY
            ));
        }
        assert!(is_instance_key_id("ed25519:AAAAAAAAAAAAAAAAAAAAAA"));
    }

    #[test]
    fn long_text_is_refused() {
        let id = wipe_cert::SigningKey::generate().public_key_id();
        let long = "x".repeat(TEXT_MAX_CHARS + 1);
        let r = RegisterRequest {
            label: Some(long),
            ..req(&id)
        };
        assert!(matches!(r.validate(), Err(StationError::BadRequest(m)) if m.contains("label")));
    }
}
