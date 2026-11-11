//! A connected station fetches its own renewed licence (station-enrollment
//! story 5, CAP-5, ADR-0008 §2, §3). The only code that writes the
//! `station_challenges` table (migration 0021); the routes are
//! [`crate::station_link_routes`].
//!
//! - **Challenges** ([`issue_challenge`], [`consume_challenge`]): 32 OS-RNG
//!   bytes, unpadded base64url, valid for [`CHALLENGE_TTL_SECS`] seconds and
//!   used at most once: the first validly signed request over one deletes it
//!   (`DELETE … WHERE expires_at > now() RETURNING`). Each issue first deletes
//!   up to [`throttle::PURGE_BATCH`] expired rows. Nothing is audited.
//! - **Fetch** ([`fetch_licence`]): the station is found by its instance key
//!   id among unreleased stations, every match locked `FOR UPDATE`. When the
//!   key is registered in several organizations, the one whose station owns
//!   the licence id the station holds wins; with none or several left the
//!   fetch is [`Fetch::Ambiguous`] (the hub never guesses an organization).
//!   No match is [`Fetch::NotFound`]. Then, in this order:
//!   - `renewal_available` (entitlement current, latest licence active and
//!     ending before the term): reissued through [`stations::reissue`], the
//!     owner's path, with actor `station:<station uuid>`, and the new licence
//!     returned. The station row lock serializes concurrent fetches, so one
//!     renewal is reissued once. Without a vendor root nothing is reissued
//!     and the fetch goes on as below.
//!   - the latest licence is not `active` (revoked, superseded by release, or
//!     none): [`Fetch::Current`], nothing new (revocation distribution is
//!     A2.3, still open).
//!   - the station holds the latest active licence: [`Fetch::Current`].
//!   - otherwise the latest active licence (an owner reissued it, or the
//!     station holds none): [`Fetch::Licence`].
//!
//!   Only a reissue writes or audits anything.
//!
//! Every time is the database's `now()`.

use rand::rngs::OsRng;
use rand::RngCore;
use serde_json::Value;
use sqlx::postgres::PgConnection;
use uuid::Uuid;
use wipe_license::VendorRoot;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

use crate::stations::{
    self, StationError, ENTITLEMENT_CURRENT, LATEST_LICENCE, LICENCE_BEHIND_TERM,
};
use crate::throttle;

/// Seconds from a challenge's issue to its expiry.
pub const CHALLENGE_TTL_SECS: i32 = 60;

/// Audit actor of a station acting for itself.
pub fn station_actor(id: Uuid) -> String {
    format!("station:{id}")
}

// ---------------------------------------------------------------------------
// Challenges

/// A fresh challenge: 32 OS-RNG bytes, unpadded base64url (43 chars).
pub fn generate_challenge() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Delete up to [`throttle::PURGE_BATCH`] expired challenges: how many.
pub async fn purge_expired<'e, E: sqlx::PgExecutor<'e>>(executor: E) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "DELETE FROM station_challenges WHERE challenge IN \
           (SELECT challenge FROM station_challenges \
            WHERE expires_at <= now() \
            ORDER BY expires_at LIMIT $1 \
            FOR UPDATE SKIP LOCKED)",
    )
    .bind(throttle::PURGE_BATCH)
    .execute(executor)
    .await?;
    Ok(done.rows_affected())
}

/// Purge expired challenges, then store and return a new one issued to
/// `peer` (its throttle key), in the caller's transaction.
pub async fn issue_challenge(
    conn: &mut PgConnection,
    peer: Option<&str>,
) -> Result<String, sqlx::Error> {
    purge_expired(&mut *conn).await?;
    let challenge = generate_challenge();
    sqlx::query(
        "INSERT INTO station_challenges (challenge, peer, expires_at) \
         VALUES ($1, $2, now() + make_interval(secs => $3))",
    )
    .bind(&challenge)
    .bind(peer)
    .bind(f64::from(CHALLENGE_TTL_SECS))
    .execute(&mut *conn)
    .await?;
    Ok(challenge)
}

/// Use `challenge`: true exactly once, for an issued, unexpired challenge
/// (the row is deleted); false for an unknown, used or expired one.
pub async fn consume_challenge<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    challenge: &str,
) -> Result<bool, sqlx::Error> {
    let used: Option<String> = sqlx::query_scalar(
        "DELETE FROM station_challenges WHERE challenge = $1 AND expires_at > now() \
         RETURNING challenge",
    )
    .bind(challenge)
    .fetch_optional(executor)
    .await?;
    Ok(used.is_some())
}

// ---------------------------------------------------------------------------
// Fetch

/// What [`fetch_licence`] found.
#[derive(Debug, Clone, PartialEq)]
pub enum Fetch {
    /// The station's latest licence (reissued now, or by an owner), the
    /// stored document parsed as JSON.
    Licence(Value),
    /// Nothing newer than what the station holds (or the latest licence is
    /// not active).
    Current,
    /// No unreleased station has this key.
    NotFound,
    /// The key is registered in several organizations and the held licence
    /// does not pick one.
    Ambiguous,
}

/// The station's latest licence and whether a renewal is waiting, read after
/// the lock.
#[derive(sqlx::FromRow)]
struct LatestState {
    licence_id: Option<Uuid>,
    status: Option<String>,
    document: Option<String>,
    renewal_available: bool,
}

/// Find the unreleased station with instance key `key_id` (choosing by
/// `held` when several organizations have it) and answer what it should
/// install, reissuing a waiting renewal with `root`, in the caller's
/// transaction. See the module docs.
pub async fn fetch_licence(
    conn: &mut PgConnection,
    root: Option<&VendorRoot>,
    key_id: &str,
    held: Option<&str>,
) -> Result<Fetch, StationError> {
    // Lock every candidate, in id order so two fetches cannot deadlock.
    let candidates: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, organization_id FROM stations \
         WHERE instance_key_id = $1 AND released_at IS NULL \
         ORDER BY id FOR UPDATE",
    )
    .bind(key_id)
    .fetch_all(&mut *conn)
    .await?;
    let (station, organization_id) = match candidates.as_slice() {
        [] => return Ok(Fetch::NotFound),
        [one] => *one,
        many => {
            let Some(held) = held else {
                return Ok(Fetch::Ambiguous);
            };
            let ids: Vec<Uuid> = many.iter().map(|(id, _)| *id).collect();
            let owners: Vec<Uuid> = sqlx::query_scalar(
                "SELECT DISTINCT station_id FROM licences \
                 WHERE id::text = $1 AND station_id = ANY($2)",
            )
            .bind(held)
            .bind(&ids)
            .fetch_all(&mut *conn)
            .await?;
            match owners.as_slice() {
                [owner] => *many
                    .iter()
                    .find(|(id, _)| id == owner)
                    .ok_or_else(|| StationError::Issue("a locked station vanished".into()))?,
                _ => return Ok(Fetch::Ambiguous),
            }
        }
    };

    // A statement of its own, after the lock: under READ COMMITTED it sees a
    // reissue that committed while this fetch waited, so that renewal is not
    // reissued twice.
    let latest: LatestState = sqlx::query_as(&format!(
        "SELECT l.id AS licence_id, l.status, l.document, \
                COALESCE({ENTITLEMENT_CURRENT} AND {LICENCE_BEHIND_TERM}, false) \
                    AS renewal_available \
         FROM stations s \
         JOIN entitlements e ON e.id = s.entitlement_id \
         {LATEST_LICENCE} \
         WHERE s.id = $1"
    ))
    .bind(station)
    .fetch_one(&mut *conn)
    .await?;

    if latest.renewal_available {
        if let Some(root) = root {
            let renewed = stations::reissue(
                conn,
                root,
                organization_id,
                &station_actor(station),
                station,
                // The fetch answers with the bare licence: no grant (story 4
                // tops connected stations up).
                crate::credits::CreditChoice::Nothing,
            )
            .await?;
            let document: String =
                sqlx::query_scalar("SELECT document FROM licences WHERE id = $1")
                    .bind(renewed.licence.id)
                    .fetch_one(&mut *conn)
                    .await?;
            return parse(renewed.licence.id, &document).map(Fetch::Licence);
        }
        tracing::warn!(
            station = %station,
            "a renewal is waiting for this station, but this hub has no vendor root to reissue \
             its licence"
        );
    }
    let (Some(id), Some("active"), Some(document)) = (
        latest.licence_id,
        latest.status.as_deref(),
        latest.document.as_deref(),
    ) else {
        return Ok(Fetch::Current);
    };
    if held == Some(id.to_string().as_str()) {
        return Ok(Fetch::Current);
    }
    parse(id, document).map(Fetch::Licence)
}

fn parse(id: Uuid, document: &str) -> Result<Value, StationError> {
    serde_json::from_str(document)
        .map_err(|e| StationError::Issue(format!("stored licence {id}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenges_are_43_url_safe_chars() {
        let a = generate_challenge();
        let b = generate_challenge();
        assert_ne!(a, b);
        for c in [a, b] {
            assert_eq!(c.len(), 43, "{c}");
            assert!(c
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
            assert_eq!(URL_SAFE_NO_PAD.decode(&c).unwrap().len(), 32);
        }
    }

    #[test]
    fn the_actor_names_the_station_row() {
        let id = Uuid::nil();
        assert_eq!(
            station_actor(id),
            "station:00000000-0000-0000-0000-000000000000"
        );
    }
}
