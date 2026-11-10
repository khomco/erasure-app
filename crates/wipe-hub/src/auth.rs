//! `/api/auth/*`: sign-up, email verification, password sign-in, sign-out,
//! `me` and resending the verification email (portal-accounts story 1);
//! the CSRF token, sign-in throttles, session rotation and sign-out
//! everywhere (story 2); magic links, password reset and the email-send
//! throttle (story 3); changing the password while signed in (story 4).
//!
//! - Sign-in and sign-up delete the request's incoming session in the
//!   transaction that creates the new one.
//! - Every sign-in attempt is charged against `email:<address>` and
//!   `ip:<peer>` throttles ([`crate::throttle`]) before any lookup or Argon2
//!   work; a refused attempt answers 429 with `Retry-After`. A failure keeps
//!   its charge and the one reaching the limit locks the key; a success
//!   clears the email key and refunds the peer charge. The peer is axum's
//!   `ConnectInfo` (IPv6 keyed by /64); proxy headers are never trusted.
//!   Refusals are counted on the throttle row and audited once per lock as
//!   `auth.sign_in_lock_summary`, never one row each.
//! - Every route that sends email (sign-up, resend verification, magic link,
//!   password reset, invitation) first charges `mail:<peer key>` and
//!   `mail:to:<address>` ([`charge_mail`]; an invitation also charges
//!   `mail:org:<organization id>`); each charge is final, so the send that
//!   reaches a limit locks the key, and a refusal is 429 before any lookup, Argon2 or
//!   send. Locks are `auth.mail_locked`, summaries `auth.mail_lock_summary`.
//! - The magic-link and reset request routes answer 202 `{}` whether or not
//!   the account exists; the lookup, token, audit row and send all run in a
//!   spawned task, so neither the answer nor its timing depends on the
//!   account or on SMTP. A refused send writes only its throttle counters.
//! - Following a link proves mailbox control: it verifies the email and
//!   clears the email's sign-in lock. A magic link that is what verifies the
//!   email also ends the user's other sessions and makes the password
//!   unusable (a pre-registered account). A reset ends every session, and
//!   the browser's incoming one, and does not sign in. A failed use of a
//!   real (used or expired) token is audited; an unknown token is not.
//! - Both cookies are `Secure` except on an http localhost/loopback
//!   `--public-base-url` ([`secure_cookies`]).
//!
//! Every auth event is audited in the same transaction as its effect. Mail is
//! sent after commit, so an SMTP outage never rolls back an account; the
//! resend route is the recovery path. Every error body is
//! `{"error": "<message>"}`.

use axum::async_trait;
use axum::extract::rejection::JsonRejection;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Request, State};
use axum::http::header::{COOKIE, RETRY_AFTER, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::{Host, Url};
use uuid::Uuid;

use crate::accounts::{self, Account, CreateAccountError};
use crate::audit::{self, AuditEntry, AuditError};
use crate::throttle;
use crate::{csrf, AppState};

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "wipehub_session";

pub const MSG_DUPLICATE_EMAIL: &str = "an account with this email already exists";
pub const MSG_BAD_VERIFICATION: &str = "this verification link is invalid or has expired";
pub const MSG_BAD_SIGN_IN: &str = "invalid email or password";
pub const MSG_NOT_SIGNED_IN: &str = "not signed in";
pub const MSG_ALREADY_VERIFIED: &str = "this email address is already verified";
pub const MSG_MAIL_FAILED: &str = "the verification email could not be sent; try again later";
pub const MSG_LOCKED: &str = "too many failed sign-in attempts; try again later";
pub const MSG_MAIL_LOCKED: &str = "too many emails requested; try again later";
pub const MSG_BAD_MAGIC_LINK: &str = "this sign-in link is invalid or has expired";
pub const MSG_BAD_RESET: &str = "this password reset link is invalid or has expired";
pub const MSG_WRONG_CURRENT_PASSWORD: &str = "current password is incorrect";
pub const MSG_OWNER_ONLY: &str = "only an organization owner can manage the team";

/// Prefix of a password sign-in throttle key for an email address.
const SIGN_IN_EMAIL_PREFIX: &str = "email:";
/// Prefix of an email-send throttle key for a recipient address.
pub(crate) const MAIL_TO_PREFIX: &str = "mail:to:";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/auth/sign-up", post(sign_up))
        .route("/api/auth/verify-email", post(verify_email))
        .route("/api/auth/sign-in", post(sign_in))
        .route("/api/auth/sign-out", post(sign_out))
        .route("/api/auth/sign-out-everywhere", post(sign_out_everywhere))
        .route("/api/auth/csrf", get(csrf::get_token))
        .route("/api/auth/me", get(me))
        .route("/api/auth/verification-email", post(resend_verification))
        .route("/api/auth/magic-link", post(request_magic_link))
        .route("/api/auth/magic-link/sign-in", post(magic_link_sign_in))
        .route("/api/auth/password-reset", post(request_password_reset))
        .route(
            "/api/auth/password-reset/complete",
            post(complete_password_reset),
        )
        .route("/api/auth/password", post(change_password))
}

// ---------------------------------------------------------------------------
// Errors and JSON bodies

/// An API error, rendered as `{"error": message}`.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// A 500 whose cause is logged, never returned.
    pub(crate) fn internal(cause: impl std::fmt::Display) -> Self {
        tracing::error!(error = %cause, "auth request failed");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::internal(e)
    }
}

impl From<AuditError> for ApiError {
    fn from(e: AuditError) -> Self {
        Self::internal(e)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self::internal(e)
    }
}

/// `Json<T>` whose rejections are `{"error": …}` bodies: 400 for malformed
/// JSON or a missing/mistyped field, 415 without a JSON content type.
pub struct JsonBody<T>(pub T);

#[async_trait]
impl<T, S> FromRequest<S> for JsonBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(v)) => Ok(Self(v)),
            Err(rejection) => {
                let status = match rejection {
                    JsonRejection::MissingJsonContentType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    _ => StatusCode::BAD_REQUEST,
                };
                Err(ApiError::new(status, rejection.body_text()))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cookies and the signed-in user

/// `Set-Cookie` value for a new session.
pub fn session_cookie(token: &str, secure: bool) -> HeaderValue {
    cookie_header(
        SESSION_COOKIE,
        token,
        accounts::SESSION_TTL.as_secs(),
        secure,
    )
}

/// `Set-Cookie` value that clears the session cookie.
pub fn cleared_cookie(secure: bool) -> HeaderValue {
    cookie_header(SESSION_COOKIE, "", 0, secure)
}

/// `Set-Cookie` value for cookie `name`. Values are base64url tokens (or
/// empty), so the header is always valid.
pub fn cookie_header(name: &str, value: &str, max_age: u64, secure: bool) -> HeaderValue {
    let mut s = format!("{name}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}");
    if secure {
        s.push_str("; Secure");
    }
    HeaderValue::from_str(&s).expect("cookie header is ASCII")
}

/// Whether cookies carry `Secure` (ADR-0007 §3): always on https, and on http
/// unless the host is `localhost`, `*.localhost` or a loopback IP. Http on any
/// other host still gets `Secure` (browsers then drop the cookie, and
/// start-up warns).
pub fn secure_cookies(base: &Url) -> bool {
    base.scheme() == "https" || !is_local_host(base)
}

/// `localhost`, `*.localhost`, `127.0.0.0/8` or `::1` (including the
/// IPv4-mapped form).
pub fn is_local_host(base: &Url) -> bool {
    match base.host() {
        Some(Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => IpAddr::V6(ip).to_canonical().is_loopback(),
        None => false,
    }
}

pub(crate) fn secure(state: &AppState) -> bool {
    secure_cookies(&state.config.public_base_url)
}

/// The value of cookie `name` from the request's `Cookie` headers, if any
/// and non-empty. The first occurrence wins.
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(n, _)| *n == name)
        .map(|(_, value)| value.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The session token from the request's `Cookie` headers, if any.
pub fn session_token(headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, SESSION_COOKIE)
}

/// The signed-in user, from a live session cookie; 401 `not signed in`
/// otherwise. `role` is read in the query that resolves the session, so a
/// demotion or removal applies to the user's very next request.
#[derive(Debug, Clone, Copy)]
pub struct CurrentUser {
    pub user_id: Uuid,
    pub organization_id: Uuid,
    pub role: accounts::Role,
}

impl CurrentUser {
    /// 403 [`MSG_OWNER_ONLY`] unless the user is their organization's
    /// `owner` (a `member` or `vendor_admin` is refused).
    pub fn require_owner(&self) -> Result<(), ApiError> {
        if self.role == accounts::Role::Owner {
            Ok(())
        } else {
            Err(ApiError::new(StatusCode::FORBIDDEN, MSG_OWNER_ONLY))
        }
    }
}

#[async_trait]
impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let not_signed_in = || ApiError::new(StatusCode::UNAUTHORIZED, MSG_NOT_SIGNED_IN);
        let token = session_token(&parts.headers).ok_or_else(not_signed_in)?;
        let (user_id, organization_id, role) = accounts::session_user(&state.pool, &token)
            .await?
            .ok_or_else(not_signed_in)?;
        Ok(Self {
            user_id,
            organization_id,
            role,
        })
    }
}

pub(crate) fn actor(user_id: Uuid) -> String {
    format!("user:{user_id}")
}

/// `{public_base_url}/{page}?token=…`, keeping any base path.
pub fn portal_link(base: &Url, page: &str, token: &str) -> Url {
    let mut link = base.clone();
    let path = format!(
        "{}/{}",
        base.path().trim_end_matches('/'),
        page.trim_start_matches('/')
    );
    link.set_path(&path);
    link.set_fragment(None);
    link.query_pairs_mut().clear().append_pair("token", token);
    link
}

/// `{public_base_url}/verify-email?token=…`, keeping any base path.
pub fn verification_link(base: &Url, token: &str) -> Url {
    portal_link(base, "verify-email", token)
}

/// Send the verification email; `true` if the SMTP server accepted it.
async fn send_verification(state: &AppState, to: &str, token: &str) -> bool {
    let link = verification_link(&state.config.public_base_url, token);
    match state.mailer.send_verification(to, &link).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "verification email not sent");
            false
        }
    }
}

/// Which emailed link a request route issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkKind {
    MagicLink,
    PasswordReset,
}

impl LinkKind {
    fn purpose(self) -> &'static str {
        match self {
            Self::MagicLink => accounts::PURPOSE_MAGIC_LINK,
            Self::PasswordReset => accounts::PURPOSE_PASSWORD_RESET,
        }
    }

    fn ttl(self) -> std::time::Duration {
        match self {
            Self::MagicLink => accounts::MAGIC_LINK_TTL,
            Self::PasswordReset => accounts::PASSWORD_RESET_TTL,
        }
    }

    /// The portal page the emailed link opens.
    fn page(self) -> &'static str {
        match self {
            Self::MagicLink => "magic-link",
            Self::PasswordReset => "reset-password",
        }
    }

    fn requested_action(self) -> &'static str {
        match self {
            Self::MagicLink => "auth.magic_link_requested",
            Self::PasswordReset => "auth.password_reset_requested",
        }
    }
}

/// Send a magic or reset link. Runs detached from the request, so a failure
/// is logged (never the token) and goes nowhere else.
async fn send_link(state: &AppState, kind: LinkKind, to: &str, token: &str) {
    let link = portal_link(&state.config.public_base_url, kind.page(), token);
    let sent = match kind {
        LinkKind::MagicLink => state.mailer.send_magic_link(to, &link).await,
        LinkKind::PasswordReset => state.mailer.send_password_reset(to, &link).await,
    };
    if let Err(e) = sent {
        tracing::warn!(error = %e, kind = kind.purpose(), "link email not sent");
    }
}

// ---------------------------------------------------------------------------
// Handlers

#[derive(Debug, Deserialize)]
pub struct SignUpRequest {
    pub email: String,
    pub password: String,
    pub name: String,
    pub organization_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignUpResponse {
    #[serde(flatten)]
    pub account: Account,
    pub verification_email_sent: bool,
}

async fn sign_up(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    JsonBody(req): JsonBody<SignUpRequest>,
) -> Result<Response, ApiError> {
    let new =
        accounts::validate_sign_up(&req.email, &req.password, &req.name, &req.organization_name)
            .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, m))?;
    // Sign-up sends a verification email: charged before Argon2 or any write.
    let peer = ip_key(peer.map(|ConnectInfo(addr)| addr));
    if let Err(refused) = charge_mail(&state, &new.email, peer.as_deref(), None).await? {
        return Ok(refused);
    }
    let password_hash = accounts::hash_password(new.password.clone()).await?;

    let mut tx = state.pool.begin().await?;
    let account = match accounts::create_account(&mut tx, &new, &password_hash).await {
        Ok(a) => a,
        Err(CreateAccountError::DuplicateEmail) => {
            return Err(ApiError::new(StatusCode::CONFLICT, MSG_DUPLICATE_EMAIL))
        }
        Err(CreateAccountError::Db(e)) => return Err(e.into()),
    };
    let user_id = account.user.id;
    let verify_token = accounts::create_verification_token(&mut *tx, user_id).await?;
    if let Some(old) = session_token(&headers) {
        accounts::delete_session(&mut *tx, &old).await?;
    }
    let session = accounts::create_session(&mut *tx, user_id).await?;
    let who = actor(user_id);
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &who,
            action: "auth.sign_up",
            subject: &who,
            detail: json!({ "organization_id": account.organization.id }),
        },
    )
    .await?;
    tx.commit().await?;

    let sent = send_verification(&state, &account.user.email, &verify_token).await;
    let mut resp = (
        StatusCode::CREATED,
        Json(SignUpResponse {
            account,
            verification_email_sent: sent,
        }),
    )
        .into_response();
    resp.headers_mut()
        .insert(SET_COOKIE, session_cookie(&session, secure(&state)));
    Ok(resp)
}

#[derive(Debug, Deserialize)]
pub struct VerifyEmailRequest {
    pub token: String,
}

async fn verify_email(
    State(state): State<AppState>,
    JsonBody(req): JsonBody<VerifyEmailRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let Some(user_id) = accounts::consume_verification_token(&mut tx, &req.token).await? else {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, MSG_BAD_VERIFICATION));
    };
    let who = actor(user_id);
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &who,
            action: "auth.email_verified",
            subject: &who,
            detail: json!({}),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "email_verified": true })))
}

#[derive(Debug, Deserialize)]
pub struct SignInRequest {
    pub email: String,
    pub password: String,
}

/// The throttle key for an email address: `prefix` (`email:` for sign-in,
/// `mail:to:` for sends) plus the address trimmed (as the account lookup
/// trims it) and lowercased by Postgres `lower()`, the function the lookup
/// compares with, so every spelling that reaches one account shares one key.
/// Bounded to 254 characters, so an anonymous caller cannot bloat the table
/// or the append-only log.
pub(crate) async fn email_key<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    prefix: &str,
    email: &str,
) -> Result<String, sqlx::Error> {
    // Truncate before binding so an oversized body never reaches Postgres;
    // `left()` re-bounds the result, as `lower()` can change the length.
    let bounded: String = email
        .trim()
        .chars()
        .take(accounts::EMAIL_MAX_CHARS)
        .collect();
    sqlx::query_scalar("SELECT $3 || left(lower($1), $2)")
        .bind(bounded)
        .bind(accounts::EMAIL_MAX_CHARS as i32)
        .bind(prefix)
        .fetch_one(executor)
        .await
}

/// The throttle key for a peer: IPv4 (including IPv4-mapped IPv6) keys on
/// the full address; IPv6 on its /64, which one subscriber can rotate
/// through freely. No peer, no key.
pub(crate) fn ip_key(peer: Option<SocketAddr>) -> Option<String> {
    peer.map(|addr| match addr.ip().to_canonical() {
        IpAddr::V4(v4) => format!("ip:{v4}"),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            let prefix = Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0);
            format!("ip:{prefix}/64")
        }
    })
}

/// `detail` plus `"peer"` when there is a peer key.
pub(crate) fn with_peer(mut detail: serde_json::Value, peer: Option<&str>) -> serde_json::Value {
    if let (Some(p), Some(obj)) = (peer, detail.as_object_mut()) {
        obj.insert("peer".into(), json!(p));
    }
    detail
}

/// The audit action for a summary of `key`'s refusals: email-send keys
/// (`mail:…`), sales-lead submission keys (`lead-form:…`), enrollment
/// code-entry keys (`enroll:user:…`, checked before the start prefix it
/// begins with), enrollment-start keys (`enroll:…`), station-challenge keys
/// (`station:ip:…`) and sign-in keys are
/// summarised under their own actions. [`throttle::purge_stale`] returns
/// every kind of key, so each prefix must be mapped here.
fn lock_summary_action(key: &str) -> &'static str {
    if key.starts_with(throttle::MAIL_KEY_PREFIX) {
        "auth.mail_lock_summary"
    } else if key.starts_with(throttle::LEAD_KEY_PREFIX) {
        "lead.submit_lock_summary"
    } else if key.starts_with(throttle::ENROLL_USER_KEY_PREFIX) {
        "enrollment.lookup_lock_summary"
    } else if key.starts_with(throttle::ENROLL_KEY_PREFIX) {
        "enrollment.start_lock_summary"
    } else if key.starts_with(throttle::STATION_KEY_PREFIX) {
        "station.challenge_lock_summary"
    } else {
        "auth.sign_in_lock_summary"
    }
}

/// Audit a throttle row that had refused attempts and is now gone or
/// restarted: at most one such row per lock.
pub(crate) async fn audit_lock_summary(
    conn: &mut sqlx::PgConnection,
    summary: &throttle::LockSummary,
    peer: Option<&str>,
) -> Result<(), ApiError> {
    audit::record(
        conn,
        &AuditEntry {
            actor: "anonymous",
            action: lock_summary_action(&summary.key),
            subject: &summary.key,
            detail: with_peer(summary.detail(), peer),
        },
    )
    .await?;
    Ok(())
}

/// 429 `message` with `Retry-After`.
pub(crate) fn locked_response(message: &str, retry_after_secs: u64) -> Response {
    let mut resp = ApiError::new(StatusCode::TOO_MANY_REQUESTS, message).into_response();
    resp.headers_mut()
        .insert(RETRY_AFTER, HeaderValue::from(retry_after_secs));
    resp
}

/// Charge this attempt against the peer key and then the email key, in one
/// transaction, before any lookup or Argon2 work. `Err(response)` is the
/// 429 for a refused attempt (the peer charge is given back if only the
/// email key refuses). Peer first, so a locked IP never creates `email:`
/// rows. Refusals are counted on the throttle row, not audited
/// one by one.
async fn charge_sign_in(
    state: &AppState,
    email_key: &str,
    ip_key: Option<&str>,
) -> Result<Result<(), Response>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let mut summaries = Vec::new();
    let mut refused = None;

    if let Some(ip) = ip_key {
        let ip_charge = throttle::charge(
            &mut tx,
            ip,
            throttle::SIGN_IN_IP_LIMIT,
            throttle::SIGN_IN_WINDOW,
        )
        .await?;
        summaries.extend(ip_charge.summary);
        if let throttle::Charge::Refused { retry_after_secs } = ip_charge.outcome {
            refused = Some(retry_after_secs);
        }
    }
    if refused.is_none() {
        let email = throttle::charge(
            &mut tx,
            email_key,
            throttle::SIGN_IN_EMAIL_LIMIT,
            throttle::SIGN_IN_WINDOW,
        )
        .await?;
        summaries.extend(email.summary);
        if let throttle::Charge::Refused { retry_after_secs } = email.outcome {
            if let Some(ip) = ip_key {
                throttle::refund(&mut *tx, ip).await?;
            }
            refused = Some(retry_after_secs);
        }
    }
    for s in &summaries {
        audit_lock_summary(&mut tx, s, ip_key).await?;
    }
    tx.commit().await?;
    Ok(match refused {
        Some(secs) => Err(locked_response(MSG_LOCKED, secs)),
        None => Ok(()),
    })
}

/// Charge one email send against `mail:<peer key>`, then
/// `mail:to:<address>`, then `extra` (a further key and its limit, such as
/// an invitation's `mail:org:<id>`), in one transaction, before any lookup,
/// Argon2 or send. `Err(response)` is the 429 for a refused send: the
/// charges this request already made on earlier keys are given back. A send
/// is final, so every charge is confirmed at once: the one reaching a limit
/// locks the key for [`throttle::MAIL_LOCKOUT`] and is audited once as
/// `auth.mail_locked`. Charges are never refunded after a send is allowed,
/// so known and unknown addresses count alike.
pub(crate) async fn charge_mail(
    state: &AppState,
    address: &str,
    peer: Option<&str>,
    extra: Option<(&str, i32)>,
) -> Result<Result<(), Response>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let address_key = email_key(&mut *tx, MAIL_TO_PREFIX, address).await?;
    let ip_key = peer.map(|p| format!("{}{p}", throttle::MAIL_KEY_PREFIX));
    let keys: Vec<(&str, i32)> = ip_key
        .as_deref()
        .map(|k| (k, throttle::MAIL_IP_LIMIT))
        .into_iter()
        .chain(std::iter::once((
            address_key.as_str(),
            throttle::MAIL_ADDRESS_LIMIT,
        )))
        .chain(extra)
        .collect();
    let mut summaries = Vec::new();
    let mut refused = None;

    for (i, &(key, limit)) in keys.iter().enumerate() {
        let charged = throttle::charge(&mut tx, key, limit, throttle::MAIL_WINDOW).await?;
        summaries.extend(charged.summary);
        if let throttle::Charge::Refused { retry_after_secs } = charged.outcome {
            for &(earlier, _) in &keys[..i] {
                throttle::refund(&mut *tx, earlier).await?;
            }
            refused = Some(retry_after_secs);
            break;
        }
    }
    if refused.is_none() {
        for &(key, limit) in &keys {
            let engaged =
                throttle::confirm_failure(&mut *tx, key, limit, throttle::MAIL_LOCKOUT).await?;
            if let Some(until) = engaged {
                audit::record(
                    &mut *tx,
                    &AuditEntry {
                        actor: "anonymous",
                        action: "auth.mail_locked",
                        subject: key,
                        detail: with_peer(json!({ "until": until }), peer),
                    },
                )
                .await?;
            }
        }
    }
    for s in &summaries {
        audit_lock_summary(&mut tx, s, peer).await?;
    }
    // Only a charged send purges, so a refused one writes nothing but its
    // own counters. Purged rows belong to other keys: no `peer` of this
    // request.
    if refused.is_none() {
        for s in throttle::purge_stale(&mut *tx).await? {
            audit_lock_summary(&mut tx, &s, None).await?;
        }
    }
    tx.commit().await?;
    Ok(match refused {
        Some(secs) => Err(locked_response(MSG_MAIL_LOCKED, secs)),
        None => Ok(()),
    })
}

async fn sign_in(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    JsonBody(req): JsonBody<SignInRequest>,
) -> Result<Response, ApiError> {
    let email_key = email_key(&state.pool, SIGN_IN_EMAIL_PREFIX, &req.email).await?;
    let ip_key = ip_key(peer.map(|ConnectInfo(addr)| addr));

    // Charged before any lookup or Argon2 work, known and unknown emails
    // alike, so parallel guesses cannot outrun the limit. A refused attempt
    // is 429 even with the right password.
    if let Err(refused) = charge_sign_in(&state, &email_key, ip_key.as_deref()).await? {
        return Ok(refused);
    }

    // From here on the charge must be confirmed or refunded. Run it as its
    // own task so a client that disconnects (axum then drops this future)
    // still gets its outcome recorded.
    tokio::spawn(finish_sign_in(state, email_key, ip_key, headers, req))
        .await
        .map_err(ApiError::internal)?
}

/// Everything after a successful charge: lookup, Argon2, then the failure or
/// success transaction.
async fn finish_sign_in(
    state: AppState,
    email_key: String,
    ip_key: Option<String>,
    headers: HeaderMap,
    req: SignInRequest,
) -> Result<Response, ApiError> {
    let peer = ip_key.as_deref();
    let keys: Vec<(&str, i32)> =
        std::iter::once((email_key.as_str(), throttle::SIGN_IN_EMAIL_LIMIT))
            .chain(peer.map(|k| (k, throttle::SIGN_IN_IP_LIMIT)))
            .collect();

    // Oversized input fails without a lookup or Argon2 work on it.
    let oversized = req.email.trim().len() > accounts::EMAIL_MAX_CHARS
        || req.password.chars().count() > accounts::PASSWORD_MAX_CHARS;
    let (candidate, ok) = if oversized {
        (None, false)
    } else {
        let candidate = accounts::find_sign_in_candidate(&state.pool, &req.email).await?;
        let ok = accounts::verify_password(
            req.password,
            candidate.as_ref().map(|c| c.password_hash.clone()),
        )
        .await;
        (candidate, ok)
    };

    let candidate = match candidate {
        Some(c) if ok => c,
        _ => {
            let reason = if oversized {
                "oversized_input"
            } else if candidate.is_some() {
                "wrong_password"
            } else {
                "unknown_email"
            };
            let mut tx = state.pool.begin().await?;
            audit::record(
                &mut *tx,
                &AuditEntry {
                    actor: "anonymous",
                    action: "auth.sign_in_failed",
                    subject: &email_key,
                    detail: with_peer(json!({ "reason": reason }), peer),
                },
            )
            .await?;
            // The failure keeps its charge; the one that finds a key at its
            // limit locks it, and only that statement audits the lock.
            for (key, limit) in &keys {
                let engaged =
                    throttle::confirm_failure(&mut *tx, key, *limit, throttle::SIGN_IN_LOCKOUT)
                        .await?;
                if let Some(until) = engaged {
                    audit::record(
                        &mut *tx,
                        &AuditEntry {
                            actor: "anonymous",
                            action: "auth.sign_in_locked",
                            subject: key,
                            detail: with_peer(json!({ "until": until }), peer),
                        },
                    )
                    .await?;
                }
            }
            // Purged rows belong to other keys: no `peer` of this request.
            for s in throttle::purge_stale(&mut *tx).await? {
                audit_lock_summary(&mut tx, &s, None).await?;
            }
            tx.commit().await?;
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, MSG_BAD_SIGN_IN));
        }
    };

    let mut tx = state.pool.begin().await?;
    // The email key is forgotten; the peer key only gets this attempt's
    // charge back, so one person signing in never unlocks an IP that is
    // spraying other accounts.
    if let Some(s) = throttle::clear(&mut *tx, &email_key).await? {
        audit_lock_summary(&mut tx, &s, peer).await?;
    }
    if let Some(ip) = peer {
        throttle::refund(&mut *tx, ip).await?;
    }
    let rotated = match session_token(&headers) {
        Some(old) => accounts::delete_session(&mut *tx, &old).await?.is_some(),
        None => false,
    };
    let session = accounts::create_session(&mut *tx, candidate.user_id).await?;
    let who = actor(candidate.user_id);
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &who,
            action: "auth.sign_in",
            subject: &who,
            detail: with_peer(json!({ "rotated": rotated }), peer),
        },
    )
    .await?;
    let account = accounts::account(&mut *tx, candidate.organization_id, candidate.user_id)
        .await?
        .ok_or_else(|| ApiError::internal("signed-in user vanished"))?;
    tx.commit().await?;

    let mut resp = Json(account).into_response();
    resp.headers_mut()
        .insert(SET_COOKIE, session_cookie(&session, secure(&state)));
    Ok(resp)
}

async fn sign_out(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    if let Some(token) = session_token(&headers) {
        let mut tx = state.pool.begin().await?;
        if let Some(user_id) = accounts::delete_session(&mut *tx, &token).await? {
            let who = actor(user_id);
            audit::record(
                &mut *tx,
                &AuditEntry {
                    actor: &who,
                    action: "auth.sign_out",
                    subject: &who,
                    detail: json!({}),
                },
            )
            .await?;
        }
        tx.commit().await?;
    }
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut()
        .insert(SET_COOKIE, cleared_cookie(secure(&state)));
    Ok(resp)
}

/// End every session of the signed-in user, this one included.
async fn sign_out_everywhere(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Response, ApiError> {
    let mut tx = state.pool.begin().await?;
    let n = accounts::delete_user_sessions(&mut *tx, user.user_id).await?;
    let who = actor(user.user_id);
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &who,
            action: "auth.sign_out_everywhere",
            subject: &who,
            detail: json!({ "sessions": n }),
        },
    )
    .await?;
    tx.commit().await?;
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut()
        .insert(SET_COOKIE, cleared_cookie(secure(&state)));
    Ok(resp)
}

async fn me(State(state): State<AppState>, user: CurrentUser) -> Result<Json<Account>, ApiError> {
    accounts::account(&state.pool, user.organization_id, user.user_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, MSG_NOT_SIGNED_IN))
}

async fn resend_verification(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    user: CurrentUser,
) -> Result<Response, ApiError> {
    let account = accounts::account(&state.pool, user.organization_id, user.user_id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, MSG_NOT_SIGNED_IN))?;
    if account.user.email_verified {
        return Err(ApiError::new(StatusCode::CONFLICT, MSG_ALREADY_VERIFIED));
    }
    let peer = ip_key(peer.map(|ConnectInfo(addr)| addr));
    if let Err(refused) = charge_mail(&state, &account.user.email, peer.as_deref(), None).await? {
        return Ok(refused);
    }
    let token = accounts::create_verification_token(&state.pool, user.user_id).await?;
    if !send_verification(&state, &account.user.email, &token).await {
        return Err(ApiError::new(StatusCode::BAD_GATEWAY, MSG_MAIL_FAILED));
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "verification_email_sent": true })),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
pub struct EmailRequest {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct CompletePasswordResetRequest {
    pub token: String,
    pub password: String,
}

/// `POST /api/auth/magic-link`: email a sign-in link, if the account exists.
async fn request_magic_link(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    JsonBody(req): JsonBody<EmailRequest>,
) -> Result<Response, ApiError> {
    request_link(state, peer, req, LinkKind::MagicLink).await
}

/// `POST /api/auth/password-reset`: email a reset link, if the account exists.
async fn request_password_reset(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    JsonBody(req): JsonBody<EmailRequest>,
) -> Result<Response, ApiError> {
    request_link(state, peer, req, LinkKind::PasswordReset).await
}

/// The two request routes. The handler only validates and charges the
/// email-send throttle, then answers 202 `{}`: the account lookup, the token,
/// the `*_requested` audit row and the send all run in a spawned task after
/// the answer is decided, so neither the response nor its timing depends on
/// whether the account exists or on SMTP. An unknown email writes nothing
/// but its throttle charges.
async fn request_link(
    state: AppState,
    peer: Option<ConnectInfo<SocketAddr>>,
    req: EmailRequest,
    kind: LinkKind,
) -> Result<Response, ApiError> {
    let email = accounts::validate_email(&req.email)
        .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, m))?;
    let peer = ip_key(peer.map(|ConnectInfo(addr)| addr));
    if let Err(refused) = charge_mail(&state, &email, peer.as_deref(), None).await? {
        return Ok(refused);
    }
    tokio::spawn(async move {
        if let Err(e) = issue_link(&state, kind, &email, peer.as_deref()).await {
            // Never the token: `issue_link` errors are database errors.
            // Logged at `warn` and never returned: the 202 is already sent.
            tracing::warn!(error = %e, kind = kind.purpose(), "link not issued");
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({}))).into_response())
}

/// The background half of [`request_link`]: if `email` has an account, issue
/// a `kind` token and audit the request in one transaction, then send the
/// link. A send failure is logged by [`send_link`].
async fn issue_link(
    state: &AppState,
    kind: LinkKind,
    email: &str,
    peer: Option<&str>,
) -> anyhow::Result<()> {
    let mut tx = state.pool.begin().await?;
    let Some((user_id, _, stored_email)) = accounts::find_user_by_email(&mut *tx, email).await?
    else {
        return Ok(());
    };
    let token = accounts::create_token(&mut *tx, user_id, kind.purpose(), kind.ttl()).await?;
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: "anonymous",
            action: kind.requested_action(),
            subject: &actor(user_id),
            detail: with_peer(json!({}), peer),
        },
    )
    .await?;
    tx.commit().await?;
    send_link(state, kind, &stored_email, &token).await;
    Ok(())
}

/// Clear `user_id`'s password sign-in lock: an emailed link proves control
/// of the mailbox. Audits the lock's summary, if it had refusals.
async fn clear_sign_in_lock(
    conn: &mut sqlx::PgConnection,
    email: &str,
    peer: Option<&str>,
) -> Result<(), ApiError> {
    let key = email_key(&mut *conn, SIGN_IN_EMAIL_PREFIX, email).await?;
    if let Some(s) = throttle::clear(&mut *conn, &key).await? {
        audit_lock_summary(conn, &s, peer).await?;
    }
    Ok(())
}

/// Audit a failed use of an emailed link as `action` (`{reason: "used" |
/// "expired"}`), but only when `raw` matches a stored `purpose` token: an
/// unknown or malformed token writes nothing, so these rows are bounded by
/// the links issued, which the email-send throttle limits.
async fn audit_token_failure(
    state: &AppState,
    raw: &str,
    purpose: &str,
    action: &str,
    peer: Option<&str>,
) -> Result<(), ApiError> {
    if let Some((user_id, reason)) = accounts::token_failure(&state.pool, raw, purpose).await? {
        audit::record(
            &state.pool,
            &AuditEntry {
                actor: "anonymous",
                action,
                subject: &actor(user_id),
                detail: with_peer(json!({ "reason": reason }), peer),
            },
        )
        .await?;
    }
    Ok(())
}

/// `POST /api/auth/magic-link/sign-in`: use a magic link. Signs in exactly
/// like a password sign-in (rotating the incoming session), but neither the
/// sign-in throttle nor its lock applies: the link is the way back in for a
/// locked-out user.
///
/// If this sign-in is what verifies the email, whoever set the password
/// never proved they own the mailbox (the account may have been
/// pre-registered by someone else), so the same transaction also ends every
/// other session of the user and makes the password unusable until a reset.
async fn magic_link_sign_in(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    JsonBody(req): JsonBody<TokenRequest>,
) -> Result<Response, ApiError> {
    let peer = ip_key(peer.map(|ConnectInfo(addr)| addr));
    let peer = peer.as_deref();
    let mut tx = state.pool.begin().await?;
    let Some(user_id) =
        accounts::consume_token(&mut *tx, &req.token, accounts::PURPOSE_MAGIC_LINK).await?
    else {
        drop(tx);
        audit_token_failure(
            &state,
            &req.token,
            accounts::PURPOSE_MAGIC_LINK,
            "auth.magic_link_failed",
            peer,
        )
        .await?;
        return Err(ApiError::new(StatusCode::BAD_REQUEST, MSG_BAD_MAGIC_LINK));
    };
    accounts::retire_tokens(&mut *tx, user_id, &[accounts::PURPOSE_MAGIC_LINK]).await?;
    let email_verified = accounts::mark_email_verified(&mut tx, user_id).await?;
    let (organization_id, email) = accounts::find_user_by_id(&mut *tx, user_id)
        .await?
        .ok_or_else(|| ApiError::internal("magic-link user vanished"))?;
    clear_sign_in_lock(&mut tx, &email, peer).await?;
    // The incoming session, whoever it belongs to.
    let rotated = match session_token(&headers) {
        Some(old) => accounts::delete_session(&mut *tx, &old).await?.is_some(),
        None => false,
    };
    let sessions_ended = if email_verified {
        accounts::set_password_hash(&mut *tx, user_id, accounts::UNUSABLE_PASSWORD_HASH).await?;
        accounts::delete_user_sessions(&mut *tx, user_id).await?
    } else {
        0
    };
    let session = accounts::create_session(&mut *tx, user_id).await?;
    let who = actor(user_id);
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &who,
            action: "auth.magic_link_sign_in",
            subject: &who,
            detail: with_peer(
                json!({
                    "rotated": rotated,
                    "email_verified": email_verified,
                    "sessions_ended": sessions_ended,
                    "password_cleared": email_verified,
                }),
                peer,
            ),
        },
    )
    .await?;
    let account = accounts::account(&mut *tx, organization_id, user_id)
        .await?
        .ok_or_else(|| ApiError::internal("signed-in user vanished"))?;
    tx.commit().await?;

    let mut resp = Json(account).into_response();
    resp.headers_mut()
        .insert(SET_COOKIE, session_cookie(&session, secure(&state)));
    Ok(resp)
}

/// `POST /api/auth/password-reset/complete`: set a new password with a reset
/// link. Validates the password first (an invalid one never burns the
/// token), peeks the token before any Argon2 work, then in one transaction
/// consumes it, sets the hash, ends every session of the user and the
/// request's incoming session (whoever it belongs to: its cookie is cleared),
/// verifies the email and clears its sign-in lock. The user is not signed
/// in: they sign in with the new password.
async fn complete_password_reset(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    JsonBody(req): JsonBody<CompletePasswordResetRequest>,
) -> Result<Response, ApiError> {
    accounts::validate_password(&req.password)
        .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, m))?;
    let peer = ip_key(peer.map(|ConnectInfo(addr)| addr));
    let peer = peer.as_deref();
    let bad_link = || async {
        audit_token_failure(
            &state,
            &req.token,
            accounts::PURPOSE_PASSWORD_RESET,
            "auth.password_reset_failed",
            peer,
        )
        .await?;
        Err::<Response, _>(ApiError::new(StatusCode::BAD_REQUEST, MSG_BAD_RESET))
    };
    if accounts::peek_token(&state.pool, &req.token, accounts::PURPOSE_PASSWORD_RESET)
        .await?
        .is_none()
    {
        return bad_link().await;
    }
    let password_hash = accounts::hash_password(req.password.clone()).await?;

    let mut tx = state.pool.begin().await?;
    // Another request may have used the token since the peek.
    let Some(user_id) =
        accounts::consume_token(&mut *tx, &req.token, accounts::PURPOSE_PASSWORD_RESET).await?
    else {
        drop(tx);
        return bad_link().await;
    };
    accounts::set_password_hash(&mut *tx, user_id, &password_hash).await?;
    accounts::retire_tokens(
        &mut *tx,
        user_id,
        &[
            accounts::PURPOSE_PASSWORD_RESET,
            accounts::PURPOSE_MAGIC_LINK,
        ],
    )
    .await?;
    let sessions = accounts::delete_user_sessions(&mut *tx, user_id).await?;
    // Counted above if it was this user's; otherwise another user's session
    // in this browser, ended because its cookie is about to be cleared.
    if let Some(old) = session_token(&headers) {
        accounts::delete_session(&mut *tx, &old).await?;
    }
    let email_verified = accounts::mark_email_verified(&mut tx, user_id).await?;
    let (_, email) = accounts::find_user_by_id(&mut *tx, user_id)
        .await?
        .ok_or_else(|| ApiError::internal("reset user vanished"))?;
    clear_sign_in_lock(&mut tx, &email, peer).await?;
    let who = actor(user_id);
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &who,
            action: "auth.password_reset",
            subject: &who,
            detail: with_peer(
                json!({ "sessions": sessions, "email_verified": email_verified }),
                peer,
            ),
        },
    )
    .await?;
    tx.commit().await?;

    let mut resp = Json(json!({ "password_reset": true })).into_response();
    resp.headers_mut()
        .insert(SET_COOKIE, cleared_cookie(secure(&state)));
    Ok(resp)
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// `POST /api/auth/password`: change the signed-in user's password. The new
/// password is validated first (nothing is charged for an invalid one). The
/// attempt is then charged against the sign-in throttle (`email:<the user's
/// address>` and `ip:<peer>`) exactly like a sign-in, since the current
/// password check is otherwise an unthrottled guessing oracle for whoever
/// holds the session. A wrong current password keeps its charge and may lock
/// the email key; the right one sets the new hash, retires outstanding reset
/// and sign-in links, ends every session of the user (this one included) and
/// starts a new one for this browser.
async fn change_password(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    user: CurrentUser,
    JsonBody(req): JsonBody<ChangePasswordRequest>,
) -> Result<Response, ApiError> {
    accounts::validate_password(&req.new_password)
        .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, m))?;
    let account = accounts::account(&state.pool, user.organization_id, user.user_id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, MSG_NOT_SIGNED_IN))?;
    let email_key = email_key(&state.pool, SIGN_IN_EMAIL_PREFIX, &account.user.email).await?;
    let ip_key = ip_key(peer.map(|ConnectInfo(addr)| addr));
    if let Err(refused) = charge_sign_in(&state, &email_key, ip_key.as_deref()).await? {
        return Ok(refused);
    }
    // The charge must be confirmed or refunded even if the client
    // disconnects: run the rest as its own task, like `finish_sign_in`.
    tokio::spawn(finish_password_change(state, user, email_key, ip_key, req))
        .await
        .map_err(ApiError::internal)?
}

async fn finish_password_change(
    state: AppState,
    user: CurrentUser,
    email_key: String,
    ip_key: Option<String>,
    req: ChangePasswordRequest,
) -> Result<Response, ApiError> {
    let peer = ip_key.as_deref();
    let who = actor(user.user_id);
    let stored = accounts::password_hash(&state.pool, user.organization_id, user.user_id).await?;
    // Oversized input fails without Argon2 work on it. An unusable hash
    // (`!unusable`) never verifies, after the same Argon2 cost.
    let ok = req.current_password.chars().count() <= accounts::PASSWORD_MAX_CHARS
        && stored.is_some()
        && accounts::verify_password(req.current_password, stored).await;

    if !ok {
        let mut tx = state.pool.begin().await?;
        audit::record(
            &mut *tx,
            &AuditEntry {
                actor: &who,
                action: "auth.password_change_failed",
                subject: &who,
                detail: with_peer(json!({ "reason": "wrong_password" }), peer),
            },
        )
        .await?;
        let keys = std::iter::once((email_key.as_str(), throttle::SIGN_IN_EMAIL_LIMIT))
            .chain(peer.map(|k| (k, throttle::SIGN_IN_IP_LIMIT)));
        for (key, limit) in keys {
            let engaged =
                throttle::confirm_failure(&mut *tx, key, limit, throttle::SIGN_IN_LOCKOUT).await?;
            if let Some(until) = engaged {
                audit::record(
                    &mut *tx,
                    &AuditEntry {
                        actor: "anonymous",
                        action: "auth.sign_in_locked",
                        subject: key,
                        detail: with_peer(json!({ "until": until }), peer),
                    },
                )
                .await?;
            }
        }
        for s in throttle::purge_stale(&mut *tx).await? {
            audit_lock_summary(&mut tx, &s, None).await?;
        }
        tx.commit().await?;
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            MSG_WRONG_CURRENT_PASSWORD,
        ));
    }

    let password_hash = accounts::hash_password(req.new_password).await?;
    let mut tx = state.pool.begin().await?;
    accounts::set_password_hash(&mut *tx, user.user_id, &password_hash).await?;
    accounts::retire_tokens(
        &mut *tx,
        user.user_id,
        &[
            accounts::PURPOSE_PASSWORD_RESET,
            accounts::PURPOSE_MAGIC_LINK,
        ],
    )
    .await?;
    let sessions_ended = accounts::delete_user_sessions(&mut *tx, user.user_id).await?;
    let session = accounts::create_session(&mut *tx, user.user_id).await?;
    if let Some(s) = throttle::clear(&mut *tx, &email_key).await? {
        audit_lock_summary(&mut tx, &s, peer).await?;
    }
    if let Some(ip) = peer {
        throttle::refund(&mut *tx, ip).await?;
    }
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &who,
            action: "auth.password_changed",
            subject: &who,
            detail: with_peer(json!({ "sessions_ended": sessions_ended }), peer),
        },
    )
    .await?;
    let account = accounts::account(&mut *tx, user.organization_id, user.user_id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, MSG_NOT_SIGNED_IN))?;
    tx.commit().await?;

    let mut resp = Json(account).into_response();
    resp.headers_mut()
        .insert(SET_COOKIE, session_cookie(&session, secure(&state)));
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_attributes() {
        let c = session_cookie("tok", false);
        let c = c.to_str().unwrap();
        assert_eq!(
            c,
            "wipehub_session=tok; HttpOnly; SameSite=Lax; Path=/; Max-Age=1209600"
        );
        assert_eq!(
            session_cookie("tok", true).to_str().unwrap(),
            "wipehub_session=tok; HttpOnly; SameSite=Lax; Path=/; Max-Age=1209600; Secure"
        );
        assert_eq!(
            cleared_cookie(false).to_str().unwrap(),
            "wipehub_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
        );
        assert_eq!(
            cookie_header("other", "v", 5, true).to_str().unwrap(),
            "other=v; HttpOnly; SameSite=Lax; Path=/; Max-Age=5; Secure"
        );
    }

    #[test]
    fn secure_cookie_policy() {
        for (base, want) in [
            ("http://127.0.0.1:7900", false),
            ("http://127.8.9.10", false),
            ("http://localhost", false),
            ("http://LOCALHOST:5174", false),
            ("http://portal.localhost", false),
            ("http://[::1]:7900", false),
            ("http://[::ffff:127.0.0.1]", false),
            ("http://hub.example.com", true),
            ("http://10.0.0.5", true),
            ("http://localhost.example.com", true),
            ("http://[::2]", true),
            ("https://localhost", true),
            ("https://127.0.0.1", true),
            ("https://hub.example.com", true),
        ] {
            let url = Url::parse(base).unwrap();
            assert_eq!(secure_cookies(&url), want, "{base}");
        }
    }

    #[test]
    fn ip_keys() {
        let key = |a: &str| ip_key(Some(a.parse().unwrap()));
        assert_eq!(ip_key(None), None);
        assert_eq!(key("192.0.2.1:5000").as_deref(), Some("ip:192.0.2.1"));
        assert_eq!(
            key("[::ffff:192.0.2.1]:5000").as_deref(),
            Some("ip:192.0.2.1")
        );
        assert_eq!(
            key("[2001:db8:1:2:aaaa:bbbb:cccc:dddd]:443").as_deref(),
            Some("ip:2001:db8:1:2::/64")
        );
        assert_eq!(
            key("[2001:db8:1:2::1]:1"),
            key("[2001:db8:1:2:ffff:ffff:ffff:ffff]:2")
        );
        assert_ne!(key("[2001:db8:1:2::1]:1"), key("[2001:db8:1:3::1]:1"));
        assert_eq!(key("[::1]:7900").as_deref(), Some("ip:::/64"));
    }

    #[test]
    fn peer_is_added_to_object_details() {
        assert_eq!(
            with_peer(json!({"rotated": true}), Some("ip:192.0.2.1")),
            json!({"rotated": true, "peer": "ip:192.0.2.1"})
        );
        assert_eq!(with_peer(json!({"a": 1}), None), json!({"a": 1}));
    }

    #[test]
    fn session_token_is_found_among_cookies() {
        let mut h = HeaderMap::new();
        h.append(
            COOKIE,
            HeaderValue::from_static("a=1; wipehub_session=abc ; b=2"),
        );
        assert_eq!(session_token(&h).as_deref(), Some("abc"));
        let mut h = HeaderMap::new();
        h.append(COOKIE, HeaderValue::from_static("x=1"));
        h.append(COOKIE, HeaderValue::from_static("wipehub_session=def"));
        assert_eq!(session_token(&h).as_deref(), Some("def"));
        let mut h = HeaderMap::new();
        h.append(COOKIE, HeaderValue::from_static("wipehub_session="));
        assert_eq!(session_token(&h), None);
        assert_eq!(session_token(&HeaderMap::new()), None);
        let mut h = HeaderMap::new();
        h.append(
            COOKIE,
            HeaderValue::from_static("wipehub_csrf=c; wipehub_session=s"),
        );
        assert_eq!(cookie_value(&h, "wipehub_csrf").as_deref(), Some("c"));
        assert_eq!(cookie_value(&h, "wipehub_session").as_deref(), Some("s"));
        assert_eq!(cookie_value(&h, "wipehub"), None);
    }

    #[test]
    fn portal_links_keep_base_path() {
        let base = Url::parse("http://127.0.0.1:7900").unwrap();
        assert_eq!(
            portal_link(&base, "magic-link", "abc-_").as_str(),
            "http://127.0.0.1:7900/magic-link?token=abc-_"
        );
        let base = Url::parse("https://example.com/portal/?x=1#frag").unwrap();
        assert_eq!(
            portal_link(&base, "reset-password", "t").as_str(),
            "https://example.com/portal/reset-password?token=t"
        );
        assert_eq!(LinkKind::MagicLink.page(), "magic-link");
        assert_eq!(LinkKind::PasswordReset.page(), "reset-password");
    }

    #[test]
    fn lock_summaries_are_audited_under_their_keys_action() {
        assert_eq!(
            lock_summary_action("mail:to:a@b.co"),
            "auth.mail_lock_summary"
        );
        assert_eq!(
            lock_summary_action("mail:ip:192.0.2.1"),
            "auth.mail_lock_summary"
        );
        assert_eq!(
            lock_summary_action("email:a@b.co"),
            "auth.sign_in_lock_summary"
        );
        assert_eq!(
            lock_summary_action("ip:192.0.2.1"),
            "auth.sign_in_lock_summary"
        );
        // An address that merely contains `mail:` is still a sign-in key.
        assert_eq!(
            lock_summary_action("email:mail:x@b.co"),
            "auth.sign_in_lock_summary"
        );
        assert_eq!(
            lock_summary_action("lead-form:ip:192.0.2.1"),
            "lead.submit_lock_summary"
        );
        assert_eq!(
            lock_summary_action("lead-form:ip:2001:db8::/64"),
            "lead.submit_lock_summary"
        );
        assert_eq!(
            lock_summary_action("email:lead-form:x@b.co"),
            "auth.sign_in_lock_summary"
        );
        assert_eq!(
            lock_summary_action("enroll:ip:192.0.2.1"),
            "enrollment.start_lock_summary"
        );
        assert_eq!(
            lock_summary_action("enroll:ip:2001:db8::/64"),
            "enrollment.start_lock_summary"
        );
        assert_eq!(
            lock_summary_action("email:enroll:x@b.co"),
            "auth.sign_in_lock_summary"
        );
        assert_eq!(
            lock_summary_action("enroll:user:00000000-0000-0000-0000-000000000000"),
            "enrollment.lookup_lock_summary"
        );
        assert_eq!(
            lock_summary_action("email:enroll:user:x@b.co"),
            "auth.sign_in_lock_summary"
        );
        assert_eq!(
            lock_summary_action("station:ip:192.0.2.1"),
            "station.challenge_lock_summary"
        );
        assert_eq!(
            lock_summary_action("station:ip:2001:db8::/64"),
            "station.challenge_lock_summary"
        );
        assert_eq!(
            lock_summary_action("email:station:x@b.co"),
            "auth.sign_in_lock_summary"
        );
    }

    #[test]
    fn verification_link_keeps_base_path() {
        let base = Url::parse("http://127.0.0.1:7900").unwrap();
        assert_eq!(
            verification_link(&base, "abc-_").as_str(),
            "http://127.0.0.1:7900/verify-email?token=abc-_"
        );
        let base = Url::parse("https://example.com/portal/").unwrap();
        assert_eq!(
            verification_link(&base, "t").as_str(),
            "https://example.com/portal/verify-email?token=t"
        );
    }
}
