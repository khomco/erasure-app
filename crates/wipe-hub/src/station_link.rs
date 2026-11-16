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
//!     ending before the end its entitlement now gives: the term, plus the
//!     renewal grace while it renews): reissued through [`stations::reissue`],
//!     the owner's path, with actor `station:<station uuid>`, and the new
//!     licence returned. The station row lock serializes concurrent fetches,
//!     so one renewal is reissued once. Without a vendor root nothing is
//!     reissued and the fetch goes on as below.
//!   - the latest licence carries a renewal grace that is no longer due (the
//!     subscription was cancelled or ended while this hub had no vendor root,
//!     or the station row was busy when it was applied): the grace is
//!     withdrawn through [`stations::withdraw_grace`] (actor `station:<station
//!     uuid>`, controlled-licensing story 5) and the trimmed licence, ending
//!     at the paid term, returned. Without a vendor root the fetch goes on as
//!     below.
//!   - the latest licence is not `active` (revoked, superseded by release, or
//!     none): [`Fetch::Current`], nothing new (revocation distribution is
//!     A2.3, still open).
//!   - the station holds the latest active licence: [`Fetch::Current`].
//!   - otherwise the latest active licence (an owner reissued it, or the
//!     station holds none): [`Fetch::Licence`].
//!
//!   Only a reissue or a withdrawal writes or audits anything.
//! - **Top-up** ([`top_up`], controlled-licensing story 4, ADR-0013 §3): the
//!   same station lookup and locks as the fetch, then its entitlement locked
//!   `FOR UPDATE` (station → entitlement, the order every path takes). A
//!   report naming no licence, or one that is not this station row's (a
//!   licence of an earlier registration, perhaps in another organization),
//!   is [`TopUp::ForeignLicence`], and an entitlement with no quota
//!   [`TopUp::NoPool`]; neither writes anything. Otherwise, in this order,
//!   every comparison against the spent count the station reported last
//!   (read before step 4 overwrites it). `balance` is what the station may
//!   still reserve (held, neither spent nor reserved by running Jobs):
//!   1. **Add-up check:** the report adds up when `balance + credits_spent`
//!      is at most every credit granted to this instance key in this
//!      organization (any station row, any source), and `credits_spent` is
//!      not below the last reported one. When it does not,
//!      `stations.credit_report_mismatch_at` is set to now (never cleared).
//!   2. **Redelivery:** when the station names, as its last top-up grant,
//!      the one issued just before its latest top-up grant `L` (or none, and
//!      `L` is its first), and its spent count has not gone backwards, the
//!      answer carrying `L` was lost: `L` is sent again and nothing new is
//!      granted. A wiped or restored reserve almost never gets credits back
//!      this way, with one narrow exception: a reserve wiped after taking
//!      its first-ever top-up grant `L`, while every report so far said 0
//!      credits spent, names no grant and reports 0 spent, which reads as a
//!      lost answer, so it is sent `L` again (only `L`'s credits, once per
//!      wipe).
//!   3. **Grant:** below the offering's threshold, `min(target reserve −
//!      balance, available)` credits, when the entitlement is current and
//!      the station's latest licence is active (not revoked); else
//!      `available` counts as 0 and nothing is resent either. Granted when
//!      that is above 0, through
//!      [`credits::grant`] with no licence and actor `station:<id>`.
//!   4. **Report:** `credit_balance` (the reported balance plus the credits
//!      of the grant in this answer, new or resent: what the station holds
//!      once the answer lands, which a release returns),
//!      `credit_balance_reported_at` and `credit_spent_reported` are
//!      stored, and `credits.reported
//!      {station_id, balance, credits_spent, spent_since_last, adds_up,
//!      grant_id, credits_delivered, resent}` audited (the grant in the
//!      answer or null, its credits or 0, and whether it was a resend, so
//!      the stored balance can be rebuilt from the log; subject
//!      `entitlement:<id>`, actor `station:<id>`).
//!
//!   Granting never stops for a flagged station: wiping lease state already
//!   loses a station its credits, and stopping it would stop the bench too.
//!
//! Every time is the database's `now()`.

use rand::rngs::OsRng;
use rand::RngCore;
use serde_json::Value;
use sqlx::postgres::PgConnection;
use uuid::Uuid;
use wipe_license::station_credits::TopUpAnswer;
use wipe_license::{CreditGrant, VendorRoot};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

use crate::audit::{self, AuditEntry};
use crate::credits;
use crate::stations::{
    self, StationError, ENTITLEMENT_CURRENT, LATEST_LICENCE, LICENCE_BEHIND_TERM, LICENCE_PAST_END,
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

/// Which unreleased station a station request comes from.
enum Lookup {
    Found {
        station: Uuid,
        organization_id: Uuid,
    },
    NotFound,
    Ambiguous,
}

/// Find and lock the unreleased station with instance key `key_id`,
/// choosing by `held` (the licence it holds) when several organizations
/// have it: one match wins, none or several left is ambiguous. Every
/// candidate is locked `FOR UPDATE`, in id order so two requests cannot
/// deadlock.
async fn lock_station(
    conn: &mut PgConnection,
    key_id: &str,
    held: Option<&str>,
) -> Result<Lookup, StationError> {
    let candidates: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, organization_id FROM stations \
         WHERE instance_key_id = $1 AND released_at IS NULL \
         ORDER BY id FOR UPDATE",
    )
    .bind(key_id)
    .fetch_all(&mut *conn)
    .await?;
    let (station, organization_id) = match candidates.as_slice() {
        [] => return Ok(Lookup::NotFound),
        [one] => *one,
        many => {
            let Some(held) = held else {
                return Ok(Lookup::Ambiguous);
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
                _ => return Ok(Lookup::Ambiguous),
            }
        }
    };
    Ok(Lookup::Found {
        station,
        organization_id,
    })
}

/// The station's latest licence and whether a renewal is waiting, read after
/// the lock.
#[derive(sqlx::FromRow)]
struct LatestState {
    licence_id: Option<Uuid>,
    status: Option<String>,
    document: Option<String>,
    renewal_available: bool,
    /// The latest licence carries a renewal grace that is no longer due.
    past_end: bool,
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
    let (station, organization_id) = match lock_station(conn, key_id, held).await? {
        Lookup::Found {
            station,
            organization_id,
        } => (station, organization_id),
        Lookup::NotFound => return Ok(Fetch::NotFound),
        Lookup::Ambiguous => return Ok(Fetch::Ambiguous),
    };

    // A statement of its own, after the lock: under READ COMMITTED it sees a
    // reissue that committed while this fetch waited, so that renewal is not
    // reissued twice.
    let latest: LatestState = sqlx::query_as(&format!(
        "SELECT l.id AS licence_id, l.status, l.document, \
                COALESCE({ENTITLEMENT_CURRENT} AND {LICENCE_BEHIND_TERM}, false) \
                    AS renewal_available, \
                COALESCE({LICENCE_PAST_END}, false) AS past_end \
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
    if latest.past_end {
        if let Some(root) = root {
            if let Some(trimmed) =
                stations::withdraw_grace(conn, root, station, &station_actor(station)).await?
            {
                let document: String =
                    sqlx::query_scalar("SELECT document FROM licences WHERE id = $1")
                        .bind(trimmed.id)
                        .fetch_one(&mut *conn)
                        .await?;
                return parse(trimmed.id, &document).map(Fetch::Licence);
            }
        } else {
            tracing::warn!(
                station = %station,
                "this station's licence carries a renewal grace its subscription no longer \
                 has, but this hub has no vendor root to reissue it"
            );
        }
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

// ---------------------------------------------------------------------------
// Top-up

/// 409 when a station on an unlimited entitlement asks for credits.
pub const MSG_NO_CREDIT_POOL: &str =
    "this station's entitlement has no credit pool; its licence does not count credits";

/// 409 when a station's report names no licence, or one that is not of its
/// current registration: a report about one organization is never judged,
/// or granted, against another's pool.
pub const MSG_FOREIGN_LICENCE: &str = "this station holds a licence from another registration; \
     install the licence issued for its current registration, then try again";

/// What a station reports with a top-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreditReport {
    /// Credits a start may still reserve: held, and neither spent nor
    /// reserved by running Jobs.
    pub balance: i64,
    /// Credits spent, cumulative.
    pub credits_spent: i64,
    /// The last grant a top-up delivered to it, if any.
    pub last_grant_id: Option<String>,
}

/// What [`top_up`] found.
#[derive(Debug, Clone, PartialEq)]
pub enum TopUp {
    /// The 200 answer, in the wire type the station reads.
    Answer(TopUpAnswer),
    /// The report names no licence of this station row:
    /// [`MSG_FOREIGN_LICENCE`].
    ForeignLicence,
    /// The station's entitlement has no quota: [`MSG_NO_CREDIT_POOL`].
    NoPool,
    /// No unreleased station has this key.
    NotFound,
    /// As [`Fetch::Ambiguous`].
    Ambiguous,
}

/// The station's entitlement, locked, and what the top-up reads with it.
#[derive(sqlx::FromRow)]
struct TopUpState {
    entitlement_id: Uuid,
    quota_erasures: Option<i64>,
    current: bool,
    target_reserve: i64,
    threshold: i64,
    instance_key_id: String,
    credit_spent_reported: Option<i64>,
    /// The report's licence is one of this station row's.
    own_licence: bool,
    /// The station's latest licence is active (not revoked).
    licence_active: bool,
}

/// Top up the station with instance key `key_id` from its entitlement's
/// pool, record its `report`, and answer, in the caller's transaction. See
/// the module docs. Without `root` nothing is granted or resent. Nothing is
/// written unless the answer is [`TopUp::Answer`].
pub async fn top_up(
    conn: &mut PgConnection,
    root: Option<&VendorRoot>,
    key_id: &str,
    held: Option<&str>,
    report: &CreditReport,
) -> Result<TopUp, StationError> {
    let (station, organization_id) = match lock_station(conn, key_id, held).await? {
        Lookup::Found {
            station,
            organization_id,
        } => (station, organization_id),
        Lookup::NotFound => return Ok(TopUp::NotFound),
        Lookup::Ambiguous => return Ok(TopUp::Ambiguous),
    };
    let st: TopUpState = sqlx::query_as(&format!(
        "SELECT e.id AS entitlement_id, o.quota_erasures, {ENTITLEMENT_CURRENT} AS current, \
                COALESCE(f.credit_target_reserve, {RESERVE}) AS target_reserve, \
                COALESCE(f.credit_topup_threshold, \
                         LEAST({THRESHOLD}, COALESCE(f.credit_target_reserve, {RESERVE}))) \
                    AS threshold, \
                s.instance_key_id, s.credit_spent_reported, \
                EXISTS (SELECT 1 FROM licences h \
                        WHERE h.station_id = s.id AND h.id::text = $2) AS own_licence, \
                COALESCE(l.status = 'active', false) AS licence_active \
         FROM stations s \
         JOIN entitlements e ON e.id = s.entitlement_id \
         JOIN orders o ON o.id = e.order_id \
         LEFT JOIN offerings f ON f.id = o.offering_id \
         {LATEST_LICENCE} \
         WHERE s.id = $1 \
         FOR UPDATE OF e",
        RESERVE = credits::DEFAULT_TARGET_RESERVE,
        THRESHOLD = credits::DEFAULT_TOPUP_THRESHOLD,
    ))
    .bind(station)
    .bind(held)
    .fetch_one(&mut *conn)
    .await?;
    // 0. A report about another registration's licence (or none).
    if !st.own_licence {
        return Ok(TopUp::ForeignLicence);
    }
    let Some(purchased) = st.quota_erasures else {
        return Ok(TopUp::NoPool);
    };
    // Credits leave the pool only for a current entitlement and a station
    // whose latest licence is active: a revoked station gets nothing.
    let grantable = st.current && st.licence_active;
    let actor = station_actor(station);
    let last_spent = st.credit_spent_reported.unwrap_or(0);
    let not_backwards = report.credits_spent >= last_spent;

    // 1. Does the report add up?
    let granted_to_key: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(credits), 0)::bigint FROM credit_grants \
         WHERE organization_id = $1 AND instance_key_id = $2",
    )
    .bind(organization_id)
    .bind(&st.instance_key_id)
    .fetch_one(&mut *conn)
    .await?;
    let adds_up = report
        .balance
        .checked_add(report.credits_spent)
        .is_some_and(|claimed| claimed <= granted_to_key)
        && not_backwards;
    if !adds_up {
        sqlx::query("UPDATE stations SET credit_report_mismatch_at = now() WHERE id = $1")
            .bind(station)
            .execute(&mut *conn)
            .await?;
    }

    // 2. A lost answer: resend the latest top-up grant.
    let recent: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, document FROM credit_grants \
         WHERE station_id = $1 AND licence_id IS NULL \
         ORDER BY issued_at DESC, id DESC LIMIT 2",
    )
    .bind(station)
    .fetch_all(&mut *conn)
    .await?;
    let named = report.last_grant_id.as_deref();
    let resend = match recent.as_slice() {
        [(latest, document), rest @ ..] if grantable && root.is_some() && not_backwards => {
            let before = rest.first().map(|(id, _)| id.to_string());
            (named != Some(latest.to_string().as_str()) && named == before.as_deref())
                .then_some((latest, document))
        }
        _ => None,
    };

    let resent = resend.is_some();
    let credit_grant = if let Some((id, document)) = resend {
        Some(parse_grant(*id, document)?)
    } else {
        // 3. Grant below the threshold.
        let available = if grantable {
            credits::available(conn, st.entitlement_id, purchased).await?
        } else {
            0
        };
        let credits = if report.balance < st.threshold {
            (st.target_reserve - report.balance).min(available)
        } else {
            0
        };
        match root {
            Some(root) if credits > 0 => {
                let granted = credits::grant(
                    conn,
                    root,
                    credits::NewGrant {
                        organization_id,
                        entitlement_id: st.entitlement_id,
                        purchased,
                        station,
                        licence_id: None,
                        instance_key_id: &st.instance_key_id,
                        credits,
                        issued_at: stations::now_micros()?,
                        actor: &actor,
                    },
                )
                .await?;
                let document: String =
                    sqlx::query_scalar("SELECT document FROM credit_grants WHERE id = $1")
                        .bind(granted.id)
                        .fetch_one(&mut *conn)
                        .await?;
                Some(parse_grant(granted.id, &document)?)
            }
            None if credits > 0 => {
                tracing::warn!(
                    station = %station,
                    "a station asked for credits, but this hub has no vendor root to sign a grant"
                );
                None
            }
            _ => None,
        }
    };

    // 4. Record the report. The stored balance is what the station holds
    // once this answer lands, so the portal and a release see the credits
    // just delivered.
    let delivered = credit_grant
        .as_ref()
        .map_or(0, |g| i64::try_from(g.body.credits).unwrap_or(i64::MAX));
    sqlx::query(
        "UPDATE stations SET credit_balance = $2, credit_balance_reported_at = now(), \
                credit_spent_reported = $3 \
         WHERE id = $1",
    )
    .bind(station)
    .bind(report.balance.saturating_add(delivered))
    .bind(report.credits_spent)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &actor,
            action: "credits.reported",
            subject: &crate::entitlements::subject(st.entitlement_id),
            detail: serde_json::json!({
                "station_id": station,
                "balance": report.balance,
                "credits_spent": report.credits_spent,
                "spent_since_last": report.credits_spent - last_spent,
                "adds_up": adds_up,
                "grant_id": credit_grant.as_ref().map(|g| g.body.grant_id.clone()),
                "credits_delivered": delivered,
                "resent": resent,
            }),
        },
    )
    .await?;

    let available = if grantable {
        credits::available(conn, st.entitlement_id, purchased).await?
    } else {
        0
    };
    let unsigned = |n: i64| {
        u64::try_from(n).map_err(|_| StationError::Issue(format!("a negative credit count: {n}")))
    };
    Ok(TopUp::Answer(TopUpAnswer {
        credit_grant,
        threshold: unsigned(st.threshold)?,
        target_reserve: unsigned(st.target_reserve)?,
        available: unsigned(available)?,
    }))
}

fn parse_grant(id: Uuid, document: &str) -> Result<CreditGrant, StationError> {
    serde_json::from_str(document)
        .map_err(|e| StationError::Issue(format!("stored credit grant {id}: {e}")))
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
