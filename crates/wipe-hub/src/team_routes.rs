//! Account, organization and team routes (portal-accounts story 4):
//!
//! - `POST /api/account/profile` — the signed-in user's own name.
//! - `GET /api/organization/members` — any role: the organization's users.
//! - Owner only: `POST /api/organization` (rename),
//!   `POST|GET /api/organization/invitations`,
//!   `POST /api/organization/invitations/:id/revoke`,
//!   `POST /api/organization/members/:id/role`,
//!   `POST /api/organization/members/:id/remove`.
//! - Anonymous: `POST /api/invitations/lookup` and
//!   `POST /api/invitations/accept`.
//!
//! The organization acted on is always the signed-in user's, from the
//! session ([`CurrentUser`]); [`crate::team`] takes it first in every query.
//! Another organization's user or invitation id is a 404 like an unknown id,
//! and changes and audits nothing. Owner-only routes answer a `member` or
//! `vendor_admin` 403 before reading the body, charging a throttle or
//! writing. Every owner write re-checks the acting owner under the
//! organization row lock; role changes and removals also re-check the owner
//! count there, so an organization always keeps at least one owner.
//! Removing or demoting an owner revokes the pending invitations they sent,
//! and a re-invite revokes the address's earlier one, each in the cause's
//! transaction and audited as `organization.invitation_revoked` with a
//! `reason`. Every write shares a transaction with its audit row; the
//! invitation email is sent after commit. Error bodies are
//! `{"error": "<message>"}`.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::accounts::{self, Account, CreateAccountError, Organization, Role, User};
use crate::audit::{self, AuditEntry};
use crate::auth::{
    actor, charge_mail, ip_key, portal_link, secure, session_cookie, session_token, with_peer,
    ApiError, CurrentUser, JsonBody, MSG_DUPLICATE_EMAIL, MSG_NOT_SIGNED_IN, MSG_OWNER_ONLY,
};
use crate::team::{self, Invitation, InvitationPreview};
use crate::throttle;
use crate::AppState;

pub const MSG_UNVERIFIED_INVITER: &str = "verify your email address before inviting members";
pub const MSG_ALREADY_MEMBER: &str = "this person is already a member of your organization";
pub const MSG_LAST_OWNER: &str = "an organization must keep at least one owner";
pub const MSG_REMOVE_SELF: &str = "you cannot remove yourself from the organization";
pub const MSG_NO_MEMBER: &str = "no such member";
pub const MSG_NO_INVITATION: &str = "no such pending invitation";
pub const MSG_BAD_INVITATION: &str = "this invitation is invalid or has expired";

/// The portal page the invitation email links to.
pub const ACCEPT_INVITE_PAGE: &str = "accept-invite";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/account/profile", post(update_profile))
        .route("/api/organization", post(rename_organization))
        .route("/api/organization/members", get(list_members))
        .route("/api/organization/members/:id/role", post(change_role))
        .route("/api/organization/members/:id/remove", post(remove_member))
        .route(
            "/api/organization/invitations",
            get(list_invitations).post(invite),
        )
        .route(
            "/api/organization/invitations/:id/revoke",
            post(revoke_invitation),
        )
        .route("/api/invitations/lookup", post(lookup_invitation))
        .route("/api/invitations/accept", post(accept_invitation))
}

fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, message)
}

fn not_found(message: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, message)
}

fn conflict(message: &str) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, message)
}

/// A path id that is not a UUID names nothing: the same 404 as an unknown id
/// (not axum's plain-text 400).
fn path_id(raw: &str, message: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| not_found(message))
}

fn organization_subject(organization_id: Uuid) -> String {
    format!("organization:{organization_id}")
}

/// The throttle key charged once per invitation email of an organization.
fn organization_mail_key(organization_id: Uuid) -> String {
    format!("{}org:{organization_id}", throttle::MAIL_KEY_PREFIX)
}

/// Audit each of `revoked` (id, email) as `organization.invitation_revoked`
/// with `reason`, in the caller's transaction.
async fn audit_revoked(
    conn: &mut sqlx::PgConnection,
    user: &CurrentUser,
    revoked: &[(Uuid, String)],
    reason: &str,
) -> Result<(), ApiError> {
    let who = actor(user.user_id);
    let subject = organization_subject(user.organization_id);
    for (invitation_id, email) in revoked {
        audit::record(
            &mut *conn,
            &AuditEntry {
                actor: &who,
                action: "organization.invitation_revoked",
                subject: &subject,
                detail: json!({
                    "invitation_id": invitation_id,
                    "email": email,
                    "reason": reason,
                }),
            },
        )
        .await?;
    }
    Ok(())
}

/// Under the organization lock: 403 unless the acting user is still an owner
/// (they may have been demoted since their session was read).
async fn require_owner_locked(
    conn: &mut sqlx::PgConnection,
    user: &CurrentUser,
) -> Result<(), ApiError> {
    match team::member(&mut *conn, user.organization_id, user.user_id).await? {
        Some(u) if u.role == Role::Owner => Ok(()),
        _ => Err(ApiError::new(StatusCode::FORBIDDEN, MSG_OWNER_ONLY)),
    }
}

// ---------------------------------------------------------------------------
// Profile

#[derive(Debug, Deserialize)]
pub struct NameRequest {
    pub name: String,
}

async fn update_profile(
    State(state): State<AppState>,
    user: CurrentUser,
    JsonBody(req): JsonBody<NameRequest>,
) -> Result<Json<Account>, ApiError> {
    let name = accounts::validate_name("name", &req.name).map_err(bad_request)?;
    let mut tx = state.pool.begin().await?;
    if !team::set_user_name(&mut *tx, user.organization_id, user.user_id, &name).await? {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, MSG_NOT_SIGNED_IN));
    }
    let who = actor(user.user_id);
    // The name itself stays out of the append-only log.
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &who,
            action: "account.profile_updated",
            subject: &who,
            detail: json!({}),
        },
    )
    .await?;
    let account = accounts::account(&mut *tx, user.organization_id, user.user_id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, MSG_NOT_SIGNED_IN))?;
    tx.commit().await?;
    Ok(Json(account))
}

// ---------------------------------------------------------------------------
// Organization and members

async fn list_members(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let members = team::members(&state.pool, user.organization_id).await?;
    Ok(Json(json!({ "members": members })))
}

async fn rename_organization(
    State(state): State<AppState>,
    user: CurrentUser,
    body: Result<JsonBody<NameRequest>, ApiError>,
) -> Result<Json<Organization>, ApiError> {
    user.require_owner()?;
    let JsonBody(req) = body?;
    let name = accounts::validate_name("name", &req.name).map_err(bad_request)?;
    let mut tx = state.pool.begin().await?;
    let from = team::lock_organization(&mut *tx, user.organization_id)
        .await?
        .ok_or_else(|| ApiError::internal("signed-in user's organization vanished"))?;
    require_owner_locked(&mut tx, &user).await?;
    team::rename_organization(&mut *tx, user.organization_id, &name).await?;
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &actor(user.user_id),
            action: "organization.renamed",
            subject: &organization_subject(user.organization_id),
            detail: json!({ "from": from, "to": name }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Json(Organization {
        id: user.organization_id,
        name,
    }))
}

#[derive(Debug, Deserialize)]
pub struct RoleRequest {
    pub role: String,
}

async fn change_role(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    body: Result<JsonBody<RoleRequest>, ApiError>,
) -> Result<Json<User>, ApiError> {
    user.require_owner()?;
    let JsonBody(req) = body?;
    let role = team::assignable_role(&req.role).map_err(bad_request)?;
    let target_id = path_id(&id, MSG_NO_MEMBER)?;

    let mut tx = state.pool.begin().await?;
    team::lock_organization(&mut *tx, user.organization_id).await?;
    let Some(target) = team::member(&mut *tx, user.organization_id, target_id).await? else {
        return Err(not_found(MSG_NO_MEMBER));
    };
    if target.role == role {
        return Ok(Json(target));
    }
    // The last-owner rule first, so of two owners demoting each other at
    // once the loser hears 409, whichever check it would also fail.
    if target.role == Role::Owner && team::owner_count(&mut *tx, user.organization_id).await? <= 1 {
        return Err(conflict(MSG_LAST_OWNER));
    }
    require_owner_locked(&mut tx, &user).await?;
    team::set_role(&mut *tx, user.organization_id, target_id, role).await?;
    if target.role == Role::Owner {
        // A demoted owner's invitations must not bring in new owners or
        // members on their behalf.
        let revoked =
            team::revoke_invitations_sent_by(&mut *tx, user.organization_id, target_id).await?;
        audit_revoked(&mut tx, &user, &revoked, "sender_demoted").await?;
    }
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &actor(user.user_id),
            action: "organization.member_role_changed",
            subject: &actor(target_id),
            detail: json!({
                "organization_id": user.organization_id,
                "from": target.role,
                "to": role,
            }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Json(User { role, ..target }))
}

async fn remove_member(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    user.require_owner()?;
    let target_id = path_id(&id, MSG_NO_MEMBER)?;
    if target_id == user.user_id {
        return Err(conflict(MSG_REMOVE_SELF));
    }

    let mut tx = state.pool.begin().await?;
    team::lock_organization(&mut *tx, user.organization_id).await?;
    let Some(target) = team::member(&mut *tx, user.organization_id, target_id).await? else {
        return Err(not_found(MSG_NO_MEMBER));
    };
    if target.role == Role::Owner && team::owner_count(&mut *tx, user.organization_id).await? <= 1 {
        return Err(conflict(MSG_LAST_OWNER));
    }
    require_owner_locked(&mut tx, &user).await?;
    // Before the delete sets `invited_by` null: a removed owner must not
    // rejoin through an invitation they sent.
    let revoked =
        team::revoke_invitations_sent_by(&mut *tx, user.organization_id, target_id).await?;
    audit_revoked(&mut tx, &user, &revoked, "sender_removed").await?;
    let removed = team::remove_member(&mut *tx, user.organization_id, target_id)
        .await?
        .ok_or_else(|| not_found(MSG_NO_MEMBER))?;
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &actor(user.user_id),
            action: "organization.member_removed",
            subject: &actor(target_id),
            detail: json!({
                "organization_id": user.organization_id,
                "email": removed.email,
                "role": removed.role,
                "sessions_ended": removed.sessions_ended,
            }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Invitations (owner side)

#[derive(Debug, Deserialize)]
pub struct InviteRequest {
    pub email: String,
    pub role: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InviteResponse {
    pub invitation: Invitation,
    pub invitation_email_sent: bool,
}

async fn invite(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    user: CurrentUser,
    body: Result<JsonBody<InviteRequest>, ApiError>,
) -> Result<Response, ApiError> {
    user.require_owner()?;
    let JsonBody(req) = body?;
    let inviter = accounts::account(&state.pool, user.organization_id, user.user_id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, MSG_NOT_SIGNED_IN))?;
    if !inviter.user.email_verified {
        return Err(ApiError::new(StatusCode::FORBIDDEN, MSG_UNVERIFIED_INVITER));
    }
    let email = accounts::validate_email(&req.email).map_err(bad_request)?;
    let role = team::assignable_role(&req.role).map_err(bad_request)?;
    let peer = ip_key(peer.map(|ConnectInfo(addr)| addr));
    let org_key = organization_mail_key(user.organization_id);
    let extra = Some((org_key.as_str(), throttle::MAIL_ORG_LIMIT));
    if let Err(refused) = charge_mail(&state, &email, peer.as_deref(), extra).await? {
        return Ok(refused);
    }

    // Under the organization lock: one pending invitation per address, and
    // a just-demoted owner cannot invite. A 409 here rolls back; the charge
    // stands, as a sign-up 409's does.
    let mut tx = state.pool.begin().await?;
    team::lock_organization(&mut *tx, user.organization_id).await?;
    require_owner_locked(&mut tx, &user).await?;
    if team::is_member_email(&mut *tx, user.organization_id, &email).await? {
        return Err(conflict(MSG_ALREADY_MEMBER));
    }
    let issued =
        team::create_invitation(&mut tx, user.organization_id, &email, role, user.user_id).await?;
    audit_revoked(&mut tx, &user, &issued.replaced, "replaced").await?;
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &actor(user.user_id),
            action: "organization.member_invited",
            subject: &organization_subject(user.organization_id),
            detail: with_peer(
                json!({
                    "invitation_id": issued.invitation.id,
                    "email": email,
                    "role": role,
                }),
                peer.as_deref(),
            ),
        },
    )
    .await?;
    tx.commit().await?;

    let link = portal_link(
        &state.config.public_base_url,
        ACCEPT_INVITE_PAGE,
        &issued.token,
    );
    let sent = match state
        .mailer
        .send_invitation(
            &email,
            &inviter.organization.name,
            &inviter.user.name,
            &link,
        )
        .await
    {
        Ok(()) => true,
        Err(e) => {
            // Never the token.
            tracing::warn!(error = %e, "invitation email not sent");
            false
        }
    };
    Ok((
        StatusCode::CREATED,
        Json(InviteResponse {
            invitation: issued.invitation,
            invitation_email_sent: sent,
        }),
    )
        .into_response())
}

async fn list_invitations(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    user.require_owner()?;
    let invitations = team::pending_invitations(&state.pool, user.organization_id).await?;
    Ok(Json(json!({ "invitations": invitations })))
}

async fn revoke_invitation(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    user.require_owner()?;
    let invitation_id = path_id(&id, MSG_NO_INVITATION)?;
    let mut tx = state.pool.begin().await?;
    team::lock_organization(&mut *tx, user.organization_id).await?;
    require_owner_locked(&mut tx, &user).await?;
    let Some(email) =
        team::revoke_invitation(&mut *tx, user.organization_id, invitation_id).await?
    else {
        return Err(not_found(MSG_NO_INVITATION));
    };
    audit_revoked(&mut tx, &user, &[(invitation_id, email)], "revoked").await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Invitations (invitee side, anonymous)

#[derive(Debug, Deserialize)]
pub struct LookupRequest {
    pub token: String,
}

/// The organization, address and role of the token's pending invitation.
/// Changes nothing.
async fn lookup_invitation(
    State(state): State<AppState>,
    JsonBody(req): JsonBody<LookupRequest>,
) -> Result<Json<InvitationPreview>, ApiError> {
    team::peek_invitation(&state.pool, &req.token)
        .await?
        .map(Json)
        .ok_or_else(|| bad_request(MSG_BAD_INVITATION))
}

#[derive(Debug, Deserialize)]
pub struct AcceptRequest {
    pub token: String,
    pub name: String,
    pub password: String,
}

/// Accept an invitation: validate (an invalid name or password never burns
/// the token), peek the token and refuse a taken address before Argon2,
/// then in one transaction consume
/// it, create the user in the invitation's organization with its role and a
/// verified email, rotate the incoming session (whoever's) and sign in. A
/// taken email rolls everything back, leaving the invitation pending.
async fn accept_invitation(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    JsonBody(req): JsonBody<AcceptRequest>,
) -> Result<Response, ApiError> {
    let name = accounts::validate_name("name", &req.name).map_err(bad_request)?;
    accounts::validate_password(&req.password).map_err(bad_request)?;
    let Some(preview) = team::peek_invitation(&state.pool, &req.token).await? else {
        return Err(bad_request(MSG_BAD_INVITATION));
    };
    // A taken address 409s before Argon2, so a pending token whose address
    // has an account cannot buy unlimited hashing. The in-transaction check
    // below still covers an account created after this lookup.
    if accounts::find_user_by_email(&state.pool, &preview.email)
        .await?
        .is_some()
    {
        return Err(conflict(MSG_DUPLICATE_EMAIL));
    }
    let password_hash = accounts::hash_password(req.password).await?;
    let peer = ip_key(peer.map(|ConnectInfo(addr)| addr));

    let mut tx = state.pool.begin().await?;
    // Another request may have used the token since the peek.
    let Some(invitation) = team::consume_invitation(&mut *tx, &req.token).await? else {
        return Err(bad_request(MSG_BAD_INVITATION));
    };
    let user_id =
        match team::create_invited_user(&mut *tx, &invitation, &name, &password_hash).await {
            Ok(id) => id,
            // Dropping `tx` rolls back the consume: the invitation stays pending.
            Err(CreateAccountError::DuplicateEmail) => return Err(conflict(MSG_DUPLICATE_EMAIL)),
            Err(CreateAccountError::Db(e)) => return Err(e.into()),
        };
    let rotated = match session_token(&headers) {
        Some(old) => accounts::delete_session(&mut *tx, &old).await?.is_some(),
        None => false,
    };
    let session = accounts::create_session(&mut *tx, user_id).await?;
    let who = actor(user_id);
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &who,
            action: "organization.invitation_accepted",
            subject: &who,
            detail: with_peer(
                json!({
                    "invitation_id": invitation.id,
                    "organization_id": invitation.organization_id,
                    "role": invitation.role,
                    "rotated": rotated,
                }),
                peer.as_deref(),
            ),
        },
    )
    .await?;
    let account = accounts::account(&mut *tx, invitation.organization_id, user_id)
        .await?
        .ok_or_else(|| ApiError::internal("invited user vanished"))?;
    tx.commit().await?;

    let mut resp = (StatusCode::CREATED, Json(account)).into_response();
    resp.headers_mut()
        .insert(SET_COOKIE, session_cookie(&session, secure(&state)));
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_uuid_path_ids_are_404() {
        let e = path_id("not-a-uuid", MSG_NO_MEMBER).unwrap_err();
        assert_eq!(e.status, StatusCode::NOT_FOUND);
        assert_eq!(e.message, MSG_NO_MEMBER);
        let id = Uuid::new_v4();
        assert_eq!(path_id(&id.to_string(), MSG_NO_INVITATION).unwrap(), id);
    }
}
