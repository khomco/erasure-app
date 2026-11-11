//! Station enrollment by device authorization (station-enrollment story 1,
//! CAP-1, ADR-0008 §1; RFC 8628, the "enter this code on your phone" flow):
//! a station asks to be registered and polls for the outcome, and an
//! organization owner approves or denies it (story 2, CAP-2). The only code
//! that writes the `enrollments` table (migration 0019).
//!
//! - **Start** ([`validate`], [`start`]): the station sends its instance
//!   public key, station id and optional label, signed with that key
//!   ([`wipe_license::enrollment`]). Refused with 400 unless the public key
//!   decodes, the key id is exactly its `wipe_cert` id, the signature verifies
//!   over the fields as sent, the station id is not blank and no text is over
//!   [`stations::TEXT_MAX_CHARS`]; station id and label are then trimmed like
//!   `stations::optional_text`. A key has at most one `pending` enrollment:
//!   a new start deletes the old one in the same transaction (its device code
//!   then polls `invalid_grant`). Each start also deletes up to
//!   [`throttle::PURGE_BATCH`] enrollments that expired more than
//!   [`RETAIN_AFTER_EXPIRY_HOURS`] hours ago, whatever their status, so late
//!   polls still get the right error for a day. `enrollment.requested` is
//!   audited in the same transaction (actor `instance-key:<key id>`, subject
//!   `enrollment:<uuid>`, detail `{instance_key_id, station_id, label,
//!   replaced, peer}`, where `replaced` is the replaced enrollment's id or
//!   null). Nothing is audited for expiry, polls or collection.
//! - **Codes:** the user code is [`USER_CODE_LEN`] characters from
//!   [`USER_CODE_ALPHABET`] (no vowels or look-alikes, about 34 bits) drawn
//!   with the OS RNG, stored undashed, shown `XXXX-XXXX`, and unique among
//!   pending enrollments (a collision draws again). [`normalize_user_code`]
//!   accepts any case, with or without the dash. The device code is 32 OS-RNG
//!   bytes, base64url unpadded, stored only as [`accounts::hash_token`]
//!   (SHA-256), and never logged. An enrollment expires [`EXPIRES_IN_SECS`]
//!   after it starts.
//! - **Poll** ([`poll`]): signed by the enrollment's stored key. In order:
//!   an unknown or malformed device code, or a bad signature, is
//!   `invalid_grant` (RFC 6749 §5.2, which RFC 8628 inherits); a collected
//!   enrollment is `expired_token`; a denied one `access_denied`; an expired
//!   one (approved or not) `expired_token`; a poll sooner than the
//!   enrollment's interval after the previous poll's arrival is `slow_down`,
//!   which adds [`SLOW_DOWN_SECS`] to the interval and persists it; a pending
//!   one is `authorization_pending`; an approved one whose station has since
//!   been released, or whose licence is no longer `active` (revoked or
//!   superseded), is `expired_token` and stays `approved`; any other approved
//!   one returns its station
//!   (the [`stations::Station`] shape `GET /api/stations` gives) and its
//!   signed licence file parsed as JSON, and becomes `collected`, in one
//!   transaction. Every poll that reaches the interval check records its
//!   arrival.
//! - **Approval** (story 2, CAP-2; [`find_pending`], [`approve`], [`deny`]):
//!   an organization owner enters the user code in the portal. A code
//!   matches only an enrollment that is `pending` and unexpired, read
//!   `FOR UPDATE`; anything else (malformed, unknown, expired, approved,
//!   denied, collected) is one 404 [`MSG_NO_ENROLLMENT`], which reveals
//!   nothing about other organizations. A lookup shows the
//!   [`PendingEnrollment`] (never its row id or any device-code material).
//!   Approval registers the station in the approver's organization through
//!   [`stations::register`] (the enrollment's key, station id and label, no
//!   site), then marks the enrollment `approved` with its approver,
//!   organization, station and licence and audits `enrollment.approved`
//!   (actor `user:<id>`, subject `enrollment:<id>`, detail `{station,
//!   licence, entitlement_id, instance_key_id}`), in one transaction: any
//!   refusal of `register` comes back unchanged and leaves the enrollment
//!   `pending`. Denial marks it `denied` and audits `enrollment.denied`
//!   (detail `{instance_key_id, station_id}`); the station's next poll is
//!   `access_denied`. Who may approve and the per-user code throttle are the
//!   routes' ([`crate::enrollment_routes`]).
//!
//! Every time is the database's `now()`.

use rand::rngs::OsRng;
use rand::Rng;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::postgres::PgConnection;
use uuid::Uuid;
use wipe_license::enrollment::{
    verify_poll, verify_start, EnrollmentSignatureError, PollRequest, StartRequest,
};
use wipe_license::VendorRoot;

use crate::accounts;
use crate::admin::RFC3339_UTC;
use crate::audit::{self, AuditEntry, AuditError};
use crate::auth::{actor, with_peer};
use crate::stations::{self, NewStation, Registered, Station, StationError};
use crate::throttle;

/// Seconds from start to expiry (`expires_in`).
pub const EXPIRES_IN_SECS: i32 = 600;
/// The poll interval a new enrollment starts with (`interval`).
pub const POLL_INTERVAL_SECS: i32 = 5;
/// Added to the interval by each `slow_down`.
pub const SLOW_DOWN_SECS: i32 = 5;
/// The user code's alphabet: no vowels, no look-alikes.
pub const USER_CODE_ALPHABET: &[u8; 20] = b"BCDFGHJKLMNPQRSTVWXZ";
/// Characters in a user code.
pub const USER_CODE_LEN: usize = 8;
/// Enrollments are purged this many hours after they expire.
pub const RETAIN_AFTER_EXPIRY_HOURS: i32 = 24;
/// The portal page an owner enters the user code on.
pub const ACTIVATE_PAGE: &str = "activate";

pub const MSG_BAD_PUBLIC_KEY: &str =
    "the public key must be the station's 32-byte Ed25519 instance key in unpadded base64";
pub const MSG_KEY_ID_MISMATCH: &str = "the instance key id does not match the public key";
pub const MSG_BAD_SIGNATURE: &str =
    "the request's signature does not verify with the station's instance key";
pub const MSG_STATION_ID_REQUIRED: &str =
    "the station id is required: the --station-id the station runs with";
pub const MSG_START_LOCKED: &str =
    "too many enrollment requests from this network; try again later";
/// Every code that matches no pending, unexpired enrollment, whatever the
/// reason.
pub const MSG_NO_ENROLLMENT: &str = "no station is waiting with this code; check the code the \
     station shows, or start again on the station if it has expired";
pub const MSG_LOOKUP_LOCKED: &str = "too many wrong enrollment codes; try again later";
pub const MSG_OWNER_ONLY_ENROLLMENTS: &str =
    "only an organization owner can approve or deny a station's enrollment";

/// Draws of a fresh user code before giving up (a collision among pending
/// codes is about one in 2^34 per pending enrollment).
const USER_CODE_ATTEMPTS: usize = 16;

/// Advisory-lock class serializing starts for one key, so two concurrent
/// starts replace each other instead of colliding on the pending index.
const START_LOCK_CLASS: i32 = 0x454e_524c; // "ENRL"

/// Audit subject of one enrollment.
pub fn enrollment_subject(id: Uuid) -> String {
    format!("enrollment:{id}")
}

/// Audit actor of a station's instance key.
pub fn instance_key_actor(key_id: &str) -> String {
    format!("instance-key:{key_id}")
}

// ---------------------------------------------------------------------------
// Codes

/// A fresh user code, undashed.
pub fn generate_user_code() -> String {
    let mut rng = OsRng;
    (0..USER_CODE_LEN)
        .map(|_| USER_CODE_ALPHABET[rng.gen_range(0..USER_CODE_ALPHABET.len())] as char)
        .collect()
}

/// `XXXX-XXXX` for an undashed user code.
pub fn format_user_code(code: &str) -> String {
    let (a, b) = code.split_at(code.len().min(USER_CODE_LEN / 2));
    format!("{a}-{b}")
}

/// The stored form of a user code as an owner typed it: any case, with or
/// without the dash after the fourth character, surrounding whitespace
/// ignored. `None` if it cannot be a user code.
pub fn normalize_user_code(input: &str) -> Option<String> {
    let upper = input.trim().to_ascii_uppercase();
    let half = USER_CODE_LEN / 2;
    let undashed = if upper.len() == USER_CODE_LEN + 1 && upper.as_bytes()[half] == b'-' {
        format!("{}{}", &upper[..half], &upper[half + 1..])
    } else {
        upper
    };
    (undashed.len() == USER_CODE_LEN && undashed.bytes().all(|b| USER_CODE_ALPHABET.contains(&b)))
        .then_some(undashed)
}

// ---------------------------------------------------------------------------
// Errors

#[derive(Debug, thiserror::Error)]
pub enum EnrollmentError {
    /// Stored data is unusable: a 500.
    #[error("enrollment failed: {0}")]
    Internal(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

impl From<StationError> for EnrollmentError {
    fn from(e: StationError) -> Self {
        match e {
            StationError::Db(e) => Self::Db(e),
            StationError::Audit(e) => Self::Audit(e),
            other => Self::Internal(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Start

/// A verified, trimmed start request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEnrollment {
    pub instance_key_id: String,
    pub public_key_b64: String,
    pub station_id: String,
    pub label: Option<String>,
}

/// Verify `req` (see the module docs): the 400 message on refusal.
pub fn validate(req: &StartRequest) -> Result<NewEnrollment, String> {
    verify_start(req).map_err(|e| {
        match e {
            EnrollmentSignatureError::BadPublicKey => MSG_BAD_PUBLIC_KEY,
            EnrollmentSignatureError::KeyIdMismatch => MSG_KEY_ID_MISMATCH,
            EnrollmentSignatureError::BadSignature => MSG_BAD_SIGNATURE,
        }
        .to_string()
    })?;
    let text =
        |field: &str, v: Option<&str>| stations::optional_text(field, v).map_err(|e| e.to_string());
    let station_id = text("station_id", Some(&req.station_id))?
        .ok_or_else(|| MSG_STATION_ID_REQUIRED.to_string())?;
    let label = text("label", req.label.as_deref())?;
    Ok(NewEnrollment {
        instance_key_id: req.instance_key_id.clone(),
        public_key_b64: req.public_key_b64.clone(),
        station_id,
        label,
    })
}

/// What [`start`] stored: the raw device code is returned once, here.
#[derive(Debug, Clone)]
pub struct Started {
    pub id: Uuid,
    pub device_code: String,
    /// Undashed.
    pub user_code: String,
    /// The pending enrollment this one replaced.
    pub replaced: Option<Uuid>,
}

/// Delete up to [`throttle::PURGE_BATCH`] enrollments that expired more than
/// [`RETAIN_AFTER_EXPIRY_HOURS`] hours ago: how many.
pub async fn purge_expired<'e, E: sqlx::PgExecutor<'e>>(executor: E) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "DELETE FROM enrollments WHERE id IN \
           (SELECT id FROM enrollments \
            WHERE expires_at < now() - make_interval(hours => $1) \
            ORDER BY expires_at LIMIT $2 \
            FOR UPDATE SKIP LOCKED)",
    )
    .bind(RETAIN_AFTER_EXPIRY_HOURS)
    .bind(throttle::PURGE_BATCH)
    .execute(executor)
    .await?;
    Ok(done.rows_affected())
}

/// Store `new` as a pending enrollment requested from `requested_from` (the
/// peer's address) and audit it with `peer` (its throttle key), in the
/// caller's transaction, after purging expired rows and deleting the key's
/// pending enrollment.
pub async fn start(
    conn: &mut PgConnection,
    new: &NewEnrollment,
    requested_from: Option<&str>,
    peer: Option<&str>,
) -> Result<Started, EnrollmentError> {
    purge_expired(&mut *conn).await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(START_LOCK_CLASS)
        .bind(&new.instance_key_id)
        .execute(&mut *conn)
        .await?;
    let replaced: Option<Uuid> = sqlx::query_scalar(
        "DELETE FROM enrollments WHERE instance_key_id = $1 AND status = 'pending' RETURNING id",
    )
    .bind(&new.instance_key_id)
    .fetch_optional(&mut *conn)
    .await?;

    let device = accounts::Token::generate();
    let mut stored = None;
    for _ in 0..USER_CODE_ATTEMPTS {
        let user_code = generate_user_code();
        let id: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO enrollments (instance_key_id, public_key_b64, station_id, label, \
                                      requested_from, user_code, device_code_hash, \
                                      poll_interval_secs, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + make_interval(secs => $9)) \
             ON CONFLICT (user_code) WHERE status = 'pending' DO NOTHING \
             RETURNING id",
        )
        .bind(&new.instance_key_id)
        .bind(&new.public_key_b64)
        .bind(&new.station_id)
        .bind(&new.label)
        .bind(requested_from)
        .bind(&user_code)
        .bind(&device.hash[..])
        .bind(POLL_INTERVAL_SECS)
        .bind(f64::from(EXPIRES_IN_SECS))
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(id) = id {
            stored = Some((id, user_code));
            break;
        }
    }
    let (id, user_code) = stored.ok_or_else(|| {
        EnrollmentError::Internal("no free user code after repeated draws".into())
    })?;

    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &instance_key_actor(&new.instance_key_id),
            action: "enrollment.requested",
            subject: &enrollment_subject(id),
            detail: with_peer(
                json!({
                    "instance_key_id": new.instance_key_id,
                    "station_id": new.station_id,
                    "label": new.label,
                    "replaced": replaced,
                }),
                peer,
            ),
        },
    )
    .await?;
    Ok(Started {
        id,
        device_code: device.raw,
        user_code,
        replaced,
    })
}

// ---------------------------------------------------------------------------
// Poll

/// How a poll stands (see the module docs for the order).
#[derive(Debug, Clone)]
pub enum PollOutcome {
    /// `authorization_pending`.
    Pending,
    /// `slow_down`, with the new interval in seconds.
    SlowDown { interval: i32 },
    /// `expired_token`: expired, already collected, or its station released
    /// or licence no longer active since approval.
    Expired,
    /// `access_denied`.
    Denied,
    /// `invalid_grant`: unknown or malformed device code, or bad signature.
    InvalidGrant,
    /// Approved: the station and its signed licence file, now collected.
    Collected {
        station: Box<Station>,
        licence: Value,
    },
}

impl PollOutcome {
    /// The RFC 6749 §5.2 / RFC 8628 §3.5 error code; `None` when collected.
    pub fn error_code(&self) -> Option<&'static str> {
        match self {
            PollOutcome::Pending => Some("authorization_pending"),
            PollOutcome::SlowDown { .. } => Some("slow_down"),
            PollOutcome::Expired => Some("expired_token"),
            PollOutcome::Denied => Some("access_denied"),
            PollOutcome::InvalidGrant => Some("invalid_grant"),
            PollOutcome::Collected { .. } => None,
        }
    }
}

#[derive(sqlx::FromRow)]
struct PolledRow {
    id: Uuid,
    public_key_b64: String,
    status: String,
    organization_id: Option<Uuid>,
    registered_station: Option<Uuid>,
    licence_id: Option<Uuid>,
    expired: bool,
    too_soon: bool,
}

/// Answer one poll, in the caller's transaction (which the caller commits
/// whatever the outcome, so the interval and arrival time persist).
pub async fn poll(
    conn: &mut PgConnection,
    req: &PollRequest,
) -> Result<PollOutcome, EnrollmentError> {
    let Some(hash) = accounts::hash_token(&req.device_code) else {
        return Ok(PollOutcome::InvalidGrant);
    };
    let row: Option<PolledRow> = sqlx::query_as(
        "SELECT id, public_key_b64, status, organization_id, \
                registered_station, licence_id, \
                expires_at <= now() AS expired, \
                COALESCE(now() < last_polled_at + make_interval(secs => poll_interval_secs), \
                         false) AS too_soon \
         FROM enrollments WHERE device_code_hash = $1 FOR UPDATE",
    )
    .bind(&hash[..])
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(PollOutcome::InvalidGrant);
    };
    if verify_poll(&row.public_key_b64, req).is_err() {
        return Ok(PollOutcome::InvalidGrant);
    }
    match row.status.as_str() {
        "collected" => return Ok(PollOutcome::Expired),
        "denied" => return Ok(PollOutcome::Denied),
        "pending" | "approved" => {}
        other => {
            return Err(EnrollmentError::Internal(format!(
                "unknown status {other:?}"
            )))
        }
    }
    if row.expired {
        return Ok(PollOutcome::Expired);
    }
    if row.too_soon {
        let interval: i32 = sqlx::query_scalar(
            "UPDATE enrollments \
             SET poll_interval_secs = poll_interval_secs + $2, last_polled_at = now() \
             WHERE id = $1 RETURNING poll_interval_secs",
        )
        .bind(row.id)
        .bind(SLOW_DOWN_SECS)
        .fetch_one(&mut *conn)
        .await?;
        return Ok(PollOutcome::SlowDown { interval });
    }
    if row.status == "pending" {
        sqlx::query("UPDATE enrollments SET last_polled_at = now() WHERE id = $1")
            .bind(row.id)
            .execute(&mut *conn)
            .await?;
        return Ok(PollOutcome::Pending);
    }

    let missing = |what: &str| EnrollmentError::Internal(format!("approved without {what}"));
    let org = row.organization_id.ok_or_else(|| missing("organization"))?;
    let station_row = row.registered_station.ok_or_else(|| missing("station"))?;
    let licence_id = row.licence_id.ok_or_else(|| missing("licence"))?;
    let station = stations::get(&mut *conn, org, station_row)
        .await?
        .ok_or_else(|| missing("its station row"))?;
    let (document, licence_status): (String, String) = sqlx::query_as(
        "SELECT document, status FROM licences \
         WHERE id = $1 AND station_id = $2 AND organization_id = $3",
    )
    .bind(licence_id)
    .bind(station_row)
    .bind(org)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| missing("its licence row"))?;
    if station.released_at.is_some() || licence_status != "active" {
        // Released, or its licence revoked or superseded, since approval:
        // nothing to hand over. The row stays `approved` until purged.
        return Ok(PollOutcome::Expired);
    }
    let licence: Value = serde_json::from_str(&document)
        .map_err(|e| EnrollmentError::Internal(format!("stored licence {licence_id}: {e}")))?;
    sqlx::query(
        "UPDATE enrollments SET status = 'collected', last_polled_at = now() WHERE id = $1",
    )
    .bind(row.id)
    .execute(&mut *conn)
    .await?;
    Ok(PollOutcome::Collected {
        station: Box::new(station),
        licence,
    })
}

// ---------------------------------------------------------------------------
// Approval

/// A pending enrollment as the approving owner sees it: what is asking, from
/// where and when. Never its row id or any device-code material. Times are
/// RFC 3339 UTC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct PendingEnrollment {
    /// `XXXX-XXXX`.
    pub user_code: String,
    /// The bench's `--station-id`.
    pub station_id: String,
    pub label: Option<String>,
    pub instance_key_id: String,
    /// The address the request came from; `None` when the hub saw no peer.
    pub requested_from: Option<String>,
    pub requested_at: String,
    pub expires_at: String,
}

/// A pending enrollment a code matched, locked `FOR UPDATE` in the caller's
/// transaction.
#[derive(Debug, Clone)]
pub struct FoundEnrollment {
    pub id: Uuid,
    pub enrollment: PendingEnrollment,
}

#[derive(sqlx::FromRow)]
struct FoundRow {
    id: Uuid,
    #[sqlx(flatten)]
    enrollment: PendingEnrollment,
}

/// The pending, unexpired enrollment `code` names (as an owner typed it; see
/// [`normalize_user_code`]), locked `FOR UPDATE`; `None` for anything else.
pub async fn find_pending(
    conn: &mut PgConnection,
    code: &str,
) -> Result<Option<FoundEnrollment>, sqlx::Error> {
    let Some(code) = normalize_user_code(code) else {
        return Ok(None);
    };
    let row: Option<FoundRow> = sqlx::query_as(&format!(
        "SELECT id, user_code, station_id, label, instance_key_id, requested_from, \
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS requested_at, \
                to_char(expires_at AT TIME ZONE 'UTC', {RFC3339_UTC}) AS expires_at \
         FROM enrollments \
         WHERE user_code = $1 AND status = 'pending' AND expires_at > now() \
         FOR UPDATE"
    ))
    .bind(&code)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(|r| {
        let mut enrollment = r.enrollment;
        enrollment.user_code = format_user_code(&enrollment.user_code);
        FoundEnrollment {
            id: r.id,
            enrollment,
        }
    }))
}

/// Approve `found` (from [`find_pending`] in this transaction) on behalf of
/// `user_id`, registering its station in `organization_id` against
/// `entitlement_id` with [`stations::register`] and signing the licence with
/// `root`; the caller commits. A refusal of `register` is returned as is,
/// and the caller's rollback leaves the enrollment `pending`.
pub async fn approve(
    conn: &mut PgConnection,
    root: &VendorRoot,
    organization_id: Uuid,
    user_id: Uuid,
    found: &FoundEnrollment,
    entitlement_id: Uuid,
) -> Result<Registered, StationError> {
    let e = &found.enrollment;
    let registered = stations::register(
        &mut *conn,
        root,
        organization_id,
        user_id,
        &NewStation {
            entitlement_id,
            instance_key_id: e.instance_key_id.clone(),
            station_id: Some(e.station_id.clone()),
            label: e.label.clone(),
            site: None,
            // The approval answers with the bare licence: no grant.
            credits: crate::credits::CreditChoice::Nothing,
        },
    )
    .await?;
    let updated = sqlx::query(
        "UPDATE enrollments \
         SET status = 'approved', approved_by = $2, approved_at = now(), \
             organization_id = $3, registered_station = $4, licence_id = $5 \
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(found.id)
    .bind(user_id)
    .bind(organization_id)
    .bind(registered.station.id)
    .bind(registered.licence.id)
    .execute(&mut *conn)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StationError::Issue(format!(
            "enrollment {} was no longer pending",
            found.id
        )));
    }
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &actor(user_id),
            action: "enrollment.approved",
            subject: &enrollment_subject(found.id),
            detail: json!({
                "station": registered.station.id,
                "licence": registered.licence.id,
                "entitlement_id": entitlement_id,
                "instance_key_id": e.instance_key_id,
            }),
        },
    )
    .await?;
    Ok(registered)
}

/// Deny `found` (from [`find_pending`] in this transaction) on behalf of
/// `user_id`, audited; the caller commits.
pub async fn deny(
    conn: &mut PgConnection,
    user_id: Uuid,
    found: &FoundEnrollment,
) -> Result<(), EnrollmentError> {
    let updated = sqlx::query(
        "UPDATE enrollments SET status = 'denied' WHERE id = $1 AND status = 'pending'",
    )
    .bind(found.id)
    .execute(&mut *conn)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(EnrollmentError::Internal(format!(
            "enrollment {} was no longer pending",
            found.id
        )));
    }
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &actor(user_id),
            action: "enrollment.denied",
            subject: &enrollment_subject(found.id),
            detail: json!({
                "instance_key_id": found.enrollment.instance_key_id,
                "station_id": found.enrollment.station_id,
            }),
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_codes_have_the_documented_shape() {
        assert_eq!(USER_CODE_ALPHABET.len(), 20);
        for c in USER_CODE_ALPHABET {
            assert!(!b"AEIOUY01".contains(c), "{}", *c as char);
        }
        for _ in 0..200 {
            let code = generate_user_code();
            assert_eq!(code.len(), USER_CODE_LEN);
            assert!(code.bytes().all(|b| USER_CODE_ALPHABET.contains(&b)));
            assert_eq!(normalize_user_code(&code).as_deref(), Some(code.as_str()));
            let shown = format_user_code(&code);
            assert_eq!(shown.len(), 9);
            assert_eq!(&shown[4..5], "-");
            assert_eq!(normalize_user_code(&shown).as_deref(), Some(code.as_str()));
        }
    }

    #[test]
    fn user_codes_normalize_any_case_with_or_without_the_dash() {
        for input in [
            "BCDF-GHJK",
            "bcdf-ghjk",
            "BcDfGhJk",
            "  bcdfghjk  ",
            "bcdf-GHJK",
        ] {
            assert_eq!(
                normalize_user_code(input).as_deref(),
                Some("BCDFGHJK"),
                "{input}"
            );
        }
        for bad in [
            "",
            "BCDF",
            "BCDFGHJ",
            "BCDFGHJKL",
            "BCD-FGHJK",
            "BCDF--GHJK",
            "BCDF GHJK",
            "ABCDEFGH",
            "BCDF-GHJ0",
            "BCDFGHJ-",
            "ＢCDFGHJK",
        ] {
            assert_eq!(normalize_user_code(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_rfc_codes_are_pinned() {
        assert_eq!(
            PollOutcome::Pending.error_code(),
            Some("authorization_pending")
        );
        assert_eq!(
            PollOutcome::SlowDown { interval: 10 }.error_code(),
            Some("slow_down")
        );
        assert_eq!(PollOutcome::Expired.error_code(), Some("expired_token"));
        assert_eq!(PollOutcome::Denied.error_code(), Some("access_denied"));
        assert_eq!(
            PollOutcome::InvalidGrant.error_code(),
            Some("invalid_grant")
        );
    }

    #[test]
    fn subjects_and_limits() {
        let id = Uuid::nil();
        assert_eq!(
            enrollment_subject(id),
            "enrollment:00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(instance_key_actor("ed25519:x"), "instance-key:ed25519:x");
        assert_eq!(EXPIRES_IN_SECS, 600);
        assert_eq!(POLL_INTERVAL_SECS, 5);
        assert_eq!(SLOW_DOWN_SECS, 5);
    }
}
