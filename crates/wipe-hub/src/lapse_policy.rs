//! The organization's lapse policy (licence-delivery story 4, CAP-6,
//! ADR-0012): what its stations do once a licence lapses. `evaluation` (the
//! default) keeps erasing and marks certificates `evaluation`; `stop` refuses
//! to start new jobs.
//!
//! Only an owner changes it, under the organization row lock
//! ([`crate::team::lock_organization`]), audited `org.lapse_policy_changed
//! {old, new}`; setting the current value writes and audits nothing. The hub
//! only signs the policy into licences issued afterwards
//! ([`crate::stations`]): no licence is reissued automatically, and nothing
//! here enforces the policy or can stop a station erasing (ROADMAP §0.2).

use serde::Deserialize;
use serde_json::json;
use sqlx::postgres::PgConnection;
use uuid::Uuid;
use wipe_license::LapsePolicy;

use crate::accounts::Role;
use crate::audit::{self, AuditEntry, AuditError};
use crate::auth::actor;
use crate::team;

/// What an owner posts.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LapsePolicyRequest {
    pub lapse_policy: LapsePolicy,
}

#[derive(Debug, thiserror::Error)]
pub enum LapsePolicyError {
    /// The acting user is no longer an owner of the organization.
    #[error("not an owner")]
    NotOwner,
    /// The organization row is gone, or holds a value this build cannot read.
    #[error("{0}")]
    Corrupt(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

fn decode(value: &str) -> Result<LapsePolicy, LapsePolicyError> {
    LapsePolicy::parse(value)
        .ok_or_else(|| LapsePolicyError::Corrupt(format!("unknown lapse policy {value:?}")))
}

/// The organization's current lapse policy.
pub async fn get<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
) -> Result<LapsePolicy, LapsePolicyError> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT lapse_policy FROM organizations WHERE id = $1")
            .bind(organization_id)
            .fetch_optional(executor)
            .await?;
    decode(
        &value
            .ok_or_else(|| LapsePolicyError::Corrupt("signed-in organization vanished".into()))?,
    )
}

/// Set the organization's lapse policy to `new` on behalf of `user_id`, in
/// the caller's transaction: the policy now in force. Takes the organization
/// row `FOR UPDATE`, then re-checks that `user_id` is still an owner
/// ([`LapsePolicyError::NotOwner`] otherwise, nothing written). A change
/// audits `org.lapse_policy_changed {old, new}` (actor `user:<id>`, subject
/// `organization:<id>`); the current value is a no-op with no audit row.
pub async fn set(
    conn: &mut PgConnection,
    organization_id: Uuid,
    user_id: Uuid,
    new: LapsePolicy,
) -> Result<LapsePolicy, LapsePolicyError> {
    team::lock_organization(&mut *conn, organization_id)
        .await?
        .ok_or_else(|| LapsePolicyError::Corrupt("signed-in organization vanished".into()))?;
    match team::member(&mut *conn, organization_id, user_id).await? {
        Some(u) if u.role == Role::Owner => {}
        _ => return Err(LapsePolicyError::NotOwner),
    }
    let old = get(&mut *conn, organization_id).await?;
    if old == new {
        return Ok(old);
    }
    sqlx::query("UPDATE organizations SET lapse_policy = $2 WHERE id = $1")
        .bind(organization_id)
        .bind(new.as_str())
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        &AuditEntry {
            actor: &actor(user_id),
            action: "org.lapse_policy_changed",
            subject: &format!("organization:{organization_id}"),
            detail: json!({ "old": old.as_str(), "new": new.as_str() }),
        },
    )
    .await?;
    Ok(new)
}
