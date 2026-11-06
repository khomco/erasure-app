//! `/api/auth/*`: sign-up, email verification, password sign-in, sign-out,
//! `me` and resending the verification email (portal-accounts story 1).
//!
//! Every auth event is audited in the same transaction as its effect. Mail is
//! sent after commit, so an SMTP outage never rolls back an account; the
//! resend route is the recovery path. Every error body is
//! `{"error": "<message>"}`.

use axum::async_trait;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, FromRequestParts, Request, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;
use uuid::Uuid;

use crate::accounts::{self, Account, CreateAccountError};
use crate::audit::{self, AuditEntry, AuditError};
use crate::AppState;

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "wipehub_session";

pub const MSG_DUPLICATE_EMAIL: &str = "an account with this email already exists";
pub const MSG_BAD_VERIFICATION: &str = "this verification link is invalid or has expired";
pub const MSG_BAD_SIGN_IN: &str = "invalid email or password";
pub const MSG_NOT_SIGNED_IN: &str = "not signed in";
pub const MSG_ALREADY_VERIFIED: &str = "this email address is already verified";
pub const MSG_MAIL_FAILED: &str = "the verification email could not be sent; try again later";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/auth/sign-up", post(sign_up))
        .route("/api/auth/verify-email", post(verify_email))
        .route("/api/auth/sign-in", post(sign_in))
        .route("/api/auth/sign-out", post(sign_out))
        .route("/api/auth/me", get(me))
        .route("/api/auth/verification-email", post(resend_verification))
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
    fn internal(cause: impl std::fmt::Display) -> Self {
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
    cookie_header(token, accounts::SESSION_TTL.as_secs(), secure)
}

/// `Set-Cookie` value that clears the session cookie.
pub fn cleared_cookie(secure: bool) -> HeaderValue {
    cookie_header("", 0, secure)
}

fn cookie_header(value: &str, max_age: u64, secure: bool) -> HeaderValue {
    let mut s =
        format!("{SESSION_COOKIE}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}");
    if secure {
        s.push_str("; Secure");
    }
    // Token is base64url, so the value is always a valid header.
    HeaderValue::from_str(&s).expect("cookie header is ASCII")
}

fn secure_cookies(state: &AppState) -> bool {
    state.config.public_base_url.scheme() == "https"
}

/// The session token from the request's `Cookie` headers, if any.
pub fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The signed-in user, from a live session cookie; 401 `not signed in`
/// otherwise.
#[derive(Debug, Clone, Copy)]
pub struct CurrentUser {
    pub user_id: Uuid,
    pub organization_id: Uuid,
}

#[async_trait]
impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let not_signed_in = || ApiError::new(StatusCode::UNAUTHORIZED, MSG_NOT_SIGNED_IN);
        let token = session_token(&parts.headers).ok_or_else(not_signed_in)?;
        let (user_id, organization_id) = accounts::session_user(&state.pool, &token)
            .await?
            .ok_or_else(not_signed_in)?;
        Ok(Self {
            user_id,
            organization_id,
        })
    }
}

fn actor(user_id: Uuid) -> String {
    format!("user:{user_id}")
}

/// `{public_base_url}/verify-email?token=…`, keeping any base path.
pub fn verification_link(base: &Url, token: &str) -> Url {
    let mut link = base.clone();
    let path = format!("{}/verify-email", base.path().trim_end_matches('/'));
    link.set_path(&path);
    link.set_fragment(None);
    link.query_pairs_mut().clear().append_pair("token", token);
    link
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
    JsonBody(req): JsonBody<SignUpRequest>,
) -> Result<Response, ApiError> {
    let new =
        accounts::validate_sign_up(&req.email, &req.password, &req.name, &req.organization_name)
            .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, m))?;
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
        .insert(SET_COOKIE, session_cookie(&session, secure_cookies(&state)));
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

async fn sign_in(
    State(state): State<AppState>,
    JsonBody(req): JsonBody<SignInRequest>,
) -> Result<Response, ApiError> {
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
            // Bounded, so an anonymous caller cannot bloat the append-only log.
            let email: String = req
                .email
                .trim()
                .to_lowercase()
                .chars()
                .take(accounts::EMAIL_MAX_CHARS)
                .collect();
            let subject = format!("email:{email}");
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
                    subject: &subject,
                    detail: json!({ "reason": reason }),
                },
            )
            .await?;
            tx.commit().await?;
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, MSG_BAD_SIGN_IN));
        }
    };

    let mut tx = state.pool.begin().await?;
    let session = accounts::create_session(&mut *tx, candidate.user_id).await?;
    let who = actor(candidate.user_id);
    audit::record(
        &mut *tx,
        &AuditEntry {
            actor: &who,
            action: "auth.sign_in",
            subject: &who,
            detail: json!({}),
        },
    )
    .await?;
    let account = accounts::account(&mut *tx, candidate.organization_id, candidate.user_id)
        .await?
        .ok_or_else(|| ApiError::internal("signed-in user vanished"))?;
    tx.commit().await?;

    let mut resp = Json(account).into_response();
    resp.headers_mut()
        .insert(SET_COOKIE, session_cookie(&session, secure_cookies(&state)));
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
        .insert(SET_COOKIE, cleared_cookie(secure_cookies(&state)));
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
    user: CurrentUser,
) -> Result<Response, ApiError> {
    let account = accounts::account(&state.pool, user.organization_id, user.user_id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, MSG_NOT_SIGNED_IN))?;
    if account.user.email_verified {
        return Err(ApiError::new(StatusCode::CONFLICT, MSG_ALREADY_VERIFIED));
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
        assert!(session_cookie("tok", true)
            .to_str()
            .unwrap()
            .ends_with("; Secure"));
        assert!(cleared_cookie(false)
            .to_str()
            .unwrap()
            .contains("Max-Age=0"));
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
