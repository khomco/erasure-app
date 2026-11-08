//! Attempt throttles (ADR-0007 §3): shared, restart-proof counters in the
//! `auth_throttle` table, keyed by what is throttled (`email:<address>`,
//! `ip:<peer>`). Story 3 (reset and magic link) reuses this module with its
//! own keys and limits.
//!
//! The protocol, per key, around one expensive check (e.g. Argon2):
//!
//! 1. [`charge`] the attempt **before** the check. A key counts charges in a
//!    fixed window that starts at its first charge. The charge is refused
//!    while the key is locked, or when charges in flight plus confirmed
//!    failures have already reached the limit, so however many attempts
//!    arrive at once, at most `limit` checks run per window. A refusal bumps
//!    the row's `refused` counter and never extends a lock.
//! 2. On failure, [`confirm_failure`]: the failure that finds the count at
//!    the limit locks the key, and only the statement that wrote the lock
//!    reports it as engaged.
//! 3. On success, [`refund`] the charge, or [`clear`] the key outright.
//!
//! A charge on a key whose lock has elapsed, or whose unlocked window has
//! elapsed, restarts the window. [`purge_stale`] deletes long-dead rows.
//! Whenever a row with `refused > 0` is cleared, restarted or purged, the
//! call returns a [`LockSummary`] that the caller audits in the same
//! transaction, so a lock costs at most two audit rows however hard it is
//! hammered.
//!
//! Single-statement functions take any Postgres executor. [`charge`] may
//! need a second statement (to create the row), so it takes a connection:
//! `&mut *tx` inside a transaction, or `&mut *pool.acquire().await?`.

use std::time::Duration;

use serde_json::{json, Value};

/// Sign-in attempts per email address per window.
pub const SIGN_IN_EMAIL_LIMIT: i32 = 5;
/// Sign-in attempts per client IP (or IPv6 /64) per window.
pub const SIGN_IN_IP_LIMIT: i32 = 20;
/// The fixed window sign-in attempts are counted in.
pub const SIGN_IN_WINDOW: Duration = Duration::from_secs(15 * 60);
/// How long a sign-in lock lasts once engaged.
pub const SIGN_IN_LOCKOUT: Duration = Duration::from_secs(15 * 60);

/// Rows whose window started longer ago than this, and whose lock (if any)
/// has elapsed, are deleted by [`purge_stale`]. Every window passed to
/// [`charge`] must be no longer than this (checked with `assert!`).
pub const PURGE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// At most this many rows are purged per [`purge_stale`] call.
pub const PURGE_BATCH: i64 = 500;

/// `locked_until` rendered as RFC 3339 UTC with microseconds.
const RFC3339_UTC: &str = "'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'";

/// Whether an attempt may go ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charge {
    /// Charged: the attempt may run its check.
    Charged,
    /// Refused; retry after this many whole seconds (at least 1): the time
    /// left on the lock, or 1 when only charges in flight fill the window.
    Refused { retry_after_secs: u64 },
}

/// A cleared, restarted or purged row that had refused attempts: audit it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockSummary {
    pub key: String,
    /// The row's `locked_until` (RFC 3339, UTC), if it had one.
    pub until: Option<String>,
    pub refused: i32,
}

impl LockSummary {
    /// `{"until": …, "refused": n}`.
    pub fn detail(&self) -> Value {
        json!({ "until": self.until, "refused": self.refused })
    }

    fn from_row(key: String, refused: i32, until: Option<String>) -> Option<Self> {
        (refused > 0).then_some(Self {
            key,
            until,
            refused,
        })
    }
}

/// What [`charge`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Charged {
    pub outcome: Charge,
    /// Set when this charge restarted a window that had refusals.
    pub summary: Option<LockSummary>,
}

/// `assert!`, not `debug_assert!`: a window longer than [`PURGE_AFTER`]
/// would let [`purge_stale`] delete live counters in release builds too.
fn check_charge_args(limit: i32, window: Duration) {
    assert!(limit >= 1, "throttle limit must be at least 1");
    assert!(
        !window.is_zero() && window <= PURGE_AFTER,
        "throttle window must be non-zero and at most PURGE_AFTER"
    );
}

fn secs(d: Duration) -> f64 {
    d.as_secs_f64()
}

/// Charge one attempt against `key` (see the module docs). One atomic
/// statement decides and records the charge: it restarts an elapsed window,
/// refuses (counting the refusal) when the key is locked or `failures` has
/// reached `limit`, and otherwise increments `failures`. A key with no row
/// gets one, with this charge, from an insert instead.
///
/// # Panics
///
/// If `limit < 1`, or `window` is zero or longer than [`PURGE_AFTER`].
pub async fn charge(
    conn: &mut sqlx::PgConnection,
    key: &str,
    limit: i32,
    window: Duration,
) -> Result<Charged, sqlx::Error> {
    check_charge_args(limit, window);
    // `o` locks the latest row version before the update, so its columns
    // are exactly the pre-image this statement replaces, even under
    // concurrent charges.
    let update = format!(
        "UPDATE auth_throttle AS t SET \
             failures = CASE WHEN o.restart THEN 1 \
                             WHEN o.refuse THEN o.failures \
                             ELSE o.failures + 1 END, \
             refused = CASE WHEN o.restart THEN 0 \
                            WHEN o.refuse THEN o.refused + 1 \
                            ELSE o.refused END, \
             window_started_at = CASE WHEN o.restart THEN now() \
                                      ELSE o.window_started_at END, \
             locked_until = CASE WHEN o.restart THEN NULL ELSE o.locked_until END \
         FROM ( \
             SELECT s.*, (NOT s.restart AND (s.locked OR s.failures >= $2)) AS refuse \
             FROM ( \
                 SELECT key, failures, refused, window_started_at, locked_until, \
                        coalesce(locked_until > now(), false) AS locked, \
                        CASE WHEN locked_until IS NOT NULL THEN locked_until <= now() \
                             ELSE window_started_at + make_interval(secs => $3) <= now() \
                        END AS restart \
                 FROM auth_throttle WHERE key = $1 FOR UPDATE \
             ) s \
         ) o \
         WHERE t.key = o.key \
         RETURNING o.refuse, o.restart, o.refused, \
                   to_char(o.locked_until AT TIME ZONE 'UTC', {RFC3339_UTC}), \
                   CASE WHEN o.locked \
                        THEN greatest(ceil(extract(epoch FROM o.locked_until - now())), 1) \
                        ELSE 1 END::bigint"
    );
    // A concurrent `clear` or purge can delete the row between the update
    // finding nothing and the insert, and a concurrent insert can win the
    // race the other way; each retry sees a fresh snapshot.
    for _ in 0..5 {
        let row: Option<(bool, bool, i32, Option<String>, i64)> = sqlx::query_as(&update)
            .bind(key)
            .bind(limit)
            .bind(secs(window))
            .fetch_optional(&mut *conn)
            .await?;
        if let Some((refuse, restart, prior_refused, prior_until, retry)) = row {
            let summary = if restart {
                LockSummary::from_row(key.to_string(), prior_refused, prior_until)
            } else {
                None
            };
            let outcome = if refuse {
                Charge::Refused {
                    retry_after_secs: retry.max(1) as u64,
                }
            } else {
                Charge::Charged
            };
            return Ok(Charged { outcome, summary });
        }
        let inserted: Option<i32> = sqlx::query_scalar(
            "INSERT INTO auth_throttle (key, failures, refused, window_started_at) \
             VALUES ($1, 1, 0, now()) ON CONFLICT (key) DO NOTHING RETURNING failures",
        )
        .bind(key)
        .fetch_optional(&mut *conn)
        .await?;
        if inserted.is_some() {
            return Ok(Charged {
                outcome: Charge::Charged,
                summary: None,
            });
        }
    }
    Err(sqlx::Error::Protocol(
        "auth_throttle charge for a key kept racing; giving up after 5 tries".into(),
    ))
}

/// Confirm that a charged attempt against `key` failed. If the key's count
/// has reached `limit` and it is not already locked, lock it for `lockout`.
/// Returns `Some(locked_until)` (RFC 3339, UTC) only when the row this
/// statement wrote engaged the lock, so concurrent failures report it once.
///
/// # Panics
///
/// If `limit < 1` or `lockout` is zero.
pub async fn confirm_failure<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    key: &str,
    limit: i32,
    lockout: Duration,
) -> Result<Option<String>, sqlx::Error> {
    assert!(limit >= 1, "throttle limit must be at least 1");
    assert!(!lockout.is_zero(), "throttle lockout must be non-zero");
    sqlx::query_scalar(&format!(
        "UPDATE auth_throttle SET locked_until = now() + make_interval(secs => $3) \
         WHERE key = $1 AND failures >= $2 \
           AND (locked_until IS NULL OR locked_until <= now()) \
         RETURNING to_char(locked_until AT TIME ZONE 'UTC', {RFC3339_UTC})"
    ))
    .bind(key)
    .bind(limit)
    .bind(secs(lockout))
    .fetch_optional(executor)
    .await
}

/// Give back one charge on `key` (its attempt succeeded).
pub async fn refund<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE auth_throttle SET failures = failures - 1 WHERE key = $1 AND failures > 0")
        .bind(key)
        .execute(executor)
        .await?;
    Ok(())
}

/// Forget `key`'s charges and any lock.
pub async fn clear<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    key: &str,
) -> Result<Option<LockSummary>, sqlx::Error> {
    let row: Option<(i32, Option<String>)> = sqlx::query_as(&format!(
        "DELETE FROM auth_throttle WHERE key = $1 \
         RETURNING refused, to_char(locked_until AT TIME ZONE 'UTC', {RFC3339_UTC})"
    ))
    .bind(key)
    .fetch_optional(executor)
    .await?;
    Ok(row.and_then(|(refused, until)| LockSummary::from_row(key.to_string(), refused, until)))
}

/// Delete up to [`PURGE_BATCH`] rows whose window started more than
/// [`PURGE_AFTER`] ago and that are not locked. Rows another transaction
/// holds are skipped, so the purge never waits on or deadlocks with a
/// concurrent charge.
pub async fn purge_stale<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
) -> Result<Vec<LockSummary>, sqlx::Error> {
    let rows: Vec<(String, i32, Option<String>)> = sqlx::query_as(&format!(
        "DELETE FROM auth_throttle WHERE key IN ( \
             SELECT key FROM auth_throttle \
             WHERE window_started_at < now() - make_interval(secs => $1) \
               AND (locked_until IS NULL OR locked_until <= now()) \
             ORDER BY window_started_at LIMIT $2 \
             FOR UPDATE SKIP LOCKED \
         ) \
         RETURNING key, refused, to_char(locked_until AT TIME ZONE 'UTC', {RFC3339_UTC})"
    ))
    .bind(secs(PURGE_AFTER))
    .bind(PURGE_BATCH)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(key, refused, until)| LockSummary::from_row(key, refused, until))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_constants() {
        assert_eq!(SIGN_IN_EMAIL_LIMIT, 5);
        assert_eq!(SIGN_IN_IP_LIMIT, 20);
        assert_eq!(SIGN_IN_WINDOW.as_secs(), 900);
        assert_eq!(SIGN_IN_LOCKOUT.as_secs(), 900);
        assert!(SIGN_IN_WINDOW <= PURGE_AFTER);
    }

    #[test]
    fn summaries_only_for_refusals() {
        assert_eq!(LockSummary::from_row("k".into(), 0, None), None);
        let s = LockSummary::from_row("k".into(), 3, Some("t".into())).unwrap();
        assert_eq!(s.detail(), json!({"until": "t", "refused": 3}));
        let s = LockSummary::from_row("k".into(), 1, None).unwrap();
        assert_eq!(s.detail(), json!({"until": null, "refused": 1}));
    }

    #[test]
    #[should_panic(expected = "at most PURGE_AFTER")]
    fn windows_longer_than_the_purge_age_are_refused() {
        check_charge_args(1, PURGE_AFTER + Duration::from_secs(1));
    }

    #[test]
    #[should_panic(expected = "at least 1")]
    fn a_zero_limit_is_refused() {
        check_charge_args(0, SIGN_IN_WINDOW);
    }

    #[test]
    fn sign_in_arguments_pass_the_checks() {
        check_charge_args(SIGN_IN_EMAIL_LIMIT, SIGN_IN_WINDOW);
        check_charge_args(SIGN_IN_IP_LIMIT, PURGE_AFTER);
    }
}
