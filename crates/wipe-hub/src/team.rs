//! Organization and team data layer (ADR-0007 §3, portal-accounts story 4):
//! members, roles, the organization's name and invitations.
//!
//! Every function that reads or writes one organization's records takes its
//! `organization_id` first, and handlers pass the signed-in user's, from the
//! session, never one from the request. A user or invitation id from another
//! organization therefore matches nothing, exactly like an unknown id.
//!
//! - Only `owner` and `member` are assignable or targetable here; a
//!   `vendor_admin` is never found by [`member`] and never created.
//! - Every owner write (rename, invite, revoke, role change, removal) runs
//!   after [`lock_organization`] (`SELECT … FOR UPDATE` on the organization
//!   row), so the at-least-one-owner check cannot race (two owners demoting
//!   each other are serialised) and an address has at most one pending
//!   invitation.
//! - Invitation tokens are 32 OS-RNG bytes; only their SHA-256 is stored
//!   ([`accounts::Token`]). The raw token leaves only in the invitation email.
//! - Timestamps are RFC 3339 UTC strings formatted in SQL.
//!
//! Functions take any Postgres executor (or a connection, when they run more
//! than one statement) so callers can put a write and its audit row in one
//! transaction.

use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::accounts::{self, CreateAccountError, Role, Token, User};

/// How long an invitation link is valid.
pub const INVITATION_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// `to_char` format for RFC 3339 UTC, whole seconds.
const RFC3339_UTC: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// A pending invitation as the API shows it. Never carries the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invitation {
    pub id: Uuid,
    pub email: String,
    pub role: Role,
    /// RFC 3339, UTC.
    pub created_at: String,
    /// RFC 3339, UTC.
    pub expires_at: String,
}

/// A newly created invitation, the raw token for its email, and the
/// pending invitations for the same address it replaced (id, email).
pub struct IssuedInvitation {
    pub token: String,
    pub invitation: Invitation,
    pub replaced: Vec<(Uuid, String)>,
}

/// What an invitee sees before accepting: the organization, the invited
/// address and the role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvitationPreview {
    pub organization_name: String,
    pub email: String,
    pub role: Role,
}

/// An invitation that [`consume_invitation`] just marked accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedInvitation {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub email: String,
    pub role: Role,
}

/// What [`remove_member`] deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedMember {
    pub email: String,
    pub role: Role,
    pub sessions_ended: i64,
}

pub const MSG_BAD_ROLE: &str = "role must be owner or member";

/// A role an owner may assign: `owner` or `member`, nothing else.
pub fn assignable_role(s: &str) -> Result<Role, String> {
    match s {
        "owner" => Ok(Role::Owner),
        "member" => Ok(Role::Member),
        _ => Err(MSG_BAD_ROLE.into()),
    }
}

fn decode_role(s: &str) -> Result<Role, sqlx::Error> {
    Role::from_str(s).map_err(|e| sqlx::Error::Decode(e.into()))
}

type UserRow = (Uuid, String, String, String, bool);

fn user_of((id, email, name, role, email_verified): UserRow) -> Result<User, sqlx::Error> {
    Ok(User {
        id,
        email,
        name,
        role: decode_role(&role)?,
        email_verified,
    })
}

// ---------------------------------------------------------------------------
// Members and the organization

/// Every user of the organization, ordered by name then email.
pub async fn members<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
) -> Result<Vec<User>, sqlx::Error> {
    let rows: Vec<UserRow> = sqlx::query_as(
        "SELECT id, email, name, role, email_verified_at IS NOT NULL FROM users \
         WHERE organization_id = $1 ORDER BY name, lower(email), id",
    )
    .bind(organization_id)
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(user_of).collect()
}

/// One `owner` or `member` of the organization; `None` for an unknown id,
/// another organization's user or a `vendor_admin`.
pub async fn member<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    user_id: Uuid,
) -> Result<Option<User>, sqlx::Error> {
    let row: Option<UserRow> = sqlx::query_as(
        "SELECT id, email, name, role, email_verified_at IS NOT NULL FROM users \
         WHERE organization_id = $1 AND id = $2 AND role IN ('owner', 'member')",
    )
    .bind(organization_id)
    .bind(user_id)
    .fetch_optional(executor)
    .await?;
    row.map(user_of).transpose()
}

/// Set a user's own name; `false` if they are not in the organization.
pub async fn set_user_name<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    user_id: Uuid,
    name: &str,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("UPDATE users SET name = $3 WHERE organization_id = $1 AND id = $2")
        .bind(organization_id)
        .bind(user_id)
        .bind(name)
        .execute(executor)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// Lock the organization row (`FOR UPDATE`) until the transaction ends and
/// return its name. Every role change, removal and rename takes this lock
/// first, so owner counts read after it are stable.
pub async fn lock_organization<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1 FOR UPDATE")
        .bind(organization_id)
        .fetch_optional(executor)
        .await
}

/// Rename the organization.
pub async fn rename_organization<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE organizations SET name = $2 WHERE id = $1")
        .bind(organization_id)
        .bind(name)
        .execute(executor)
        .await?;
    Ok(())
}

/// How many `owner`s the organization has.
pub async fn owner_count<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM users WHERE organization_id = $1 AND role = 'owner'")
        .bind(organization_id)
        .fetch_one(executor)
        .await
}

/// Set an `owner`'s or `member`'s role to `role` (`owner` or `member`);
/// `false` if no such member.
pub async fn set_role<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    user_id: Uuid,
    role: Role,
) -> Result<bool, sqlx::Error> {
    debug_assert!(matches!(role, Role::Owner | Role::Member));
    let done = sqlx::query(
        "UPDATE users SET role = $3 \
         WHERE organization_id = $1 AND id = $2 AND role IN ('owner', 'member') \
           AND $3 IN ('owner', 'member')",
    )
    .bind(organization_id)
    .bind(user_id)
    .bind(role.as_str())
    .execute(executor)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Delete an `owner` or `member` (their sessions and emailed tokens cascade);
/// `None` if no such member. `sessions_ended` counts only unexpired sessions.
pub async fn remove_member<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    user_id: Uuid,
) -> Result<Option<RemovedMember>, sqlx::Error> {
    let row: Option<(String, String, i64)> = sqlx::query_as(
        "WITH s AS (SELECT count(*) AS n FROM sessions WHERE user_id = $2 AND expires_at > now()) \
         DELETE FROM users \
         WHERE organization_id = $1 AND id = $2 AND role IN ('owner', 'member') \
         RETURNING email, role, (SELECT n FROM s)",
    )
    .bind(organization_id)
    .bind(user_id)
    .fetch_optional(executor)
    .await?;
    row.map(|(email, role, sessions_ended)| {
        Ok(RemovedMember {
            email,
            role: decode_role(&role)?,
            sessions_ended,
        })
    })
    .transpose()
}

/// Whether `email` (any case) belongs to a user of this organization.
pub async fn is_member_email<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    email: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users \
         WHERE organization_id = $1 AND lower(email) = lower($2))",
    )
    .bind(organization_id)
    .bind(email.trim())
    .fetch_one(executor)
    .await
}

// ---------------------------------------------------------------------------
// Invitations

/// Invite `email` as `role`: revoke this organization's other pending
/// invitations for the same address (any case, returned in
/// [`IssuedInvitation::replaced`] for auditing), delete its expired
/// unaccepted ones, and insert a new one valid for [`INVITATION_TTL`]. Call
/// inside a transaction, after [`lock_organization`].
pub async fn create_invitation(
    conn: &mut sqlx::PgConnection,
    organization_id: Uuid,
    email: &str,
    role: Role,
    invited_by: Uuid,
) -> Result<IssuedInvitation, sqlx::Error> {
    debug_assert!(matches!(role, Role::Owner | Role::Member));
    let replaced: Vec<(Uuid, String)> = sqlx::query_as(
        "UPDATE invitations SET revoked_at = now() \
         WHERE organization_id = $1 AND lower(email) = lower($2) \
           AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now() \
         RETURNING id, email",
    )
    .bind(organization_id)
    .bind(email)
    .fetch_all(&mut *conn)
    .await?;
    sqlx::query(
        "DELETE FROM invitations \
         WHERE organization_id = $1 AND accepted_at IS NULL AND expires_at <= now()",
    )
    .bind(organization_id)
    .execute(&mut *conn)
    .await?;
    let token = Token::generate();
    let (id, created_at, expires_at): (Uuid, String, String) = sqlx::query_as(&format!(
        "INSERT INTO invitations (organization_id, email, role, token_hash, invited_by, expires_at) \
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(secs => $6)) \
         RETURNING id, to_char(created_at AT TIME ZONE 'UTC', {RFC3339_UTC}), \
                   to_char(expires_at AT TIME ZONE 'UTC', {RFC3339_UTC})"
    ))
    .bind(organization_id)
    .bind(email)
    .bind(role.as_str())
    .bind(&token.hash[..])
    .bind(invited_by)
    .bind(INVITATION_TTL.as_secs_f64())
    .fetch_one(&mut *conn)
    .await?;
    Ok(IssuedInvitation {
        token: token.raw,
        invitation: Invitation {
            id,
            email: email.to_string(),
            role,
            created_at,
            expires_at,
        },
        replaced,
    })
}

/// The organization's pending invitations (not accepted, revoked or
/// expired), oldest first.
pub async fn pending_invitations<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
) -> Result<Vec<Invitation>, sqlx::Error> {
    let rows: Vec<(Uuid, String, String, String, String)> = sqlx::query_as(&format!(
        "SELECT id, email, role, to_char(created_at AT TIME ZONE 'UTC', {RFC3339_UTC}), \
                to_char(expires_at AT TIME ZONE 'UTC', {RFC3339_UTC}) \
         FROM invitations \
         WHERE organization_id = $1 AND accepted_at IS NULL AND revoked_at IS NULL \
           AND expires_at > now() \
         ORDER BY created_at, id"
    ))
    .bind(organization_id)
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|(id, email, role, created_at, expires_at)| {
            Ok(Invitation {
                id,
                email,
                role: decode_role(&role)?,
                created_at,
                expires_at,
            })
        })
        .collect()
}

/// Revoke one of the organization's pending invitations; returns its email,
/// or `None` if there is no such pending invitation in this organization.
pub async fn revoke_invitation<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    invitation_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "UPDATE invitations SET revoked_at = now() \
         WHERE organization_id = $1 AND id = $2 \
           AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now() \
         RETURNING email",
    )
    .bind(organization_id)
    .bind(invitation_id)
    .fetch_optional(executor)
    .await
}

/// Revoke every pending invitation of the organization that `user_id` sent
/// (they are being removed or demoted); returns each one's id and email.
pub async fn revoke_invitations_sent_by<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE invitations SET revoked_at = now() \
         WHERE organization_id = $1 AND invited_by = $2 \
           AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now() \
         RETURNING id, email",
    )
    .bind(organization_id)
    .bind(user_id)
    .fetch_all(executor)
    .await
}

/// The pending invitation a raw token belongs to (any organization: the
/// token is the credential), without using it up. `None` for an unknown,
/// malformed, accepted, revoked or expired token.
pub async fn peek_invitation<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    raw: &str,
) -> Result<Option<InvitationPreview>, sqlx::Error> {
    let Some(hash) = accounts::hash_token(raw) else {
        return Ok(None);
    };
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT o.name, i.email, i.role FROM invitations i \
         JOIN organizations o ON o.id = i.organization_id \
         WHERE i.token_hash = $1 AND i.accepted_at IS NULL AND i.revoked_at IS NULL \
           AND i.expires_at > now()",
    )
    .bind(&hash[..])
    .fetch_optional(executor)
    .await?;
    row.map(|(organization_name, email, role)| {
        Ok(InvitationPreview {
            organization_name,
            email,
            role: decode_role(&role)?,
        })
    })
    .transpose()
}

/// Mark a pending invitation accepted: one atomic statement, so of any
/// number of concurrent uses exactly one gets it. `None` for an unknown,
/// malformed, accepted, revoked or expired token.
pub async fn consume_invitation<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    raw: &str,
) -> Result<Option<AcceptedInvitation>, sqlx::Error> {
    let Some(hash) = accounts::hash_token(raw) else {
        return Ok(None);
    };
    let row: Option<(Uuid, Uuid, String, String)> = sqlx::query_as(
        "UPDATE invitations SET accepted_at = now() \
         WHERE token_hash = $1 AND accepted_at IS NULL AND revoked_at IS NULL \
           AND expires_at > now() \
         RETURNING id, organization_id, email, role",
    )
    .bind(&hash[..])
    .fetch_optional(executor)
    .await?;
    row.map(|(id, organization_id, email, role)| {
        Ok(AcceptedInvitation {
            id,
            organization_id,
            email,
            role: decode_role(&role)?,
        })
    })
    .transpose()
}

/// Create the invited user in the invitation's organization with its role,
/// email already verified (following the emailed link proves the mailbox).
/// A taken email (an account anywhere) is
/// [`CreateAccountError::DuplicateEmail`], which aborts the transaction.
pub async fn create_invited_user<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    invitation: &AcceptedInvitation,
    name: &str,
    password_hash: &str,
) -> Result<Uuid, CreateAccountError> {
    sqlx::query_scalar(
        "INSERT INTO users (organization_id, email, name, password_hash, role, email_verified_at) \
         VALUES ($1, $2, $3, $4, $5, now()) RETURNING id",
    )
    .bind(invitation.organization_id)
    .bind(&invitation.email)
    .bind(name)
    .bind(password_hash)
    .bind(invitation.role.as_str())
    .fetch_one(executor)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("users_email_lower_key") => {
            CreateAccountError::DuplicateEmail
        }
        _ => CreateAccountError::Db(e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_owner_and_member_are_assignable() {
        assert_eq!(assignable_role("owner"), Ok(Role::Owner));
        assert_eq!(assignable_role("member"), Ok(Role::Member));
        for bad in ["vendor_admin", "admin", "Owner", " member", ""] {
            assert_eq!(assignable_role(bad), Err(MSG_BAD_ROLE.to_string()), "{bad}");
        }
    }

    #[test]
    fn invitations_last_seven_days() {
        assert_eq!(INVITATION_TTL.as_secs(), 7 * 24 * 60 * 60);
    }

    #[test]
    fn invitation_json_has_no_token() {
        let inv = Invitation {
            id: Uuid::nil(),
            email: "a@example.com".into(),
            role: Role::Member,
            created_at: "2026-11-08T00:00:00Z".into(),
            expires_at: "2026-11-15T00:00:00Z".into(),
        };
        let v = serde_json::to_value(&inv).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["created_at", "email", "expires_at", "id", "role"]);
        assert_eq!(v["role"], "member");
    }
}
