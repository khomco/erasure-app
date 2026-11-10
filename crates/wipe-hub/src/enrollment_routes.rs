//! Station enrollment routes (station-enrollment stories 1 and 2, CAP-1 and
//! CAP-2, ADR-0008 §1).
//!
//! A station's two routes need no session, and both are exempt from the CSRF
//! check ([`crate::csrf`]): the station has no cookie, and its instance-key
//! signature is the credential.
//!
//! - `POST /api/enrollments` — a station asks to enroll. In order: verify
//!   and validate ([`enrollments::validate`], 400 `{"error": message}`);
//!   charge `enroll:ip:<peer>` (429 [`enrollments::MSG_START_LOCKED`] with
//!   `Retry-After`); store and audit ([`enrollments::start`]); answer 200
//!   `{device_code, user_code, verification_uri, verification_uri_complete,
//!   expires_in, interval}`, where `verification_uri` is
//!   `{public_base_url}/activate` and the complete one adds
//!   `?code=XXXX-XXXX`.
//! - `POST /api/enrollments/poll` — the station polls with its device code.
//!   200 `{station, licence}` once approved; otherwise 400 `{"error": code}`
//!   with an RFC 6749 §5.2 / RFC 8628 §3.5 code, plus `interval` on
//!   `slow_down` ([`enrollments::poll`]).
//!
//! The owner's three routes need a customer session and the usual CSRF
//! token. The code travels in the body, never the URL, and every action
//! re-proves it:
//!
//! - `POST /api/enrollments/lookup {code}` — 200 `{enrollment}`, the
//!   [`enrollments::PendingEnrollment`].
//! - `POST /api/enrollments/approve {code, entitlement_id}` — 201 `{station,
//!   licence}`, the `POST /api/stations` shape ([`enrollments::approve`]);
//!   `stations::register`'s refusals unchanged, the enrollment left pending.
//! - `POST /api/enrollments/deny {code}` — 200 `{}` ([`enrollments::deny`]).
//!
//! Each, in order: 403 [`enrollments::MSG_OWNER_ONLY_ENROLLMENTS`] unless the
//! caller is an organization owner (members and vendor admins alike), before
//! the body is read; approve only, 503 [`stations::MSG_ISSUANCE_UNAVAILABLE`]
//! without a vendor root; the body; a charge against `enroll:user:<user id>`
//! (429 [`enrollments::MSG_LOOKUP_LOCKED`] with `Retry-After`); the code
//! ([`enrollments::find_pending`]), 404 [`enrollments::MSG_NO_ENROLLMENT`]
//! when it matches nothing. A matching code refunds its charge and any other
//! confirms the failure: 10 failures in 15 minutes lock the user out of all
//! three for 15 minutes, audited once as `enrollment.lookup_locked` (actor
//! `user:<id>`, subject the throttle key) and summarised as
//! `enrollment.lookup_lock_summary`.
//!
//! Every response from all five routes, errors included, carries
//! `Cache-Control: no-store` and `Pragma: no-cache` (RFC 6749 §5.1).
//!
//! The start throttle is final, like the sales-lead form's: 20 per address
//! per hour, then locked for an hour, audited once as
//! `enrollment.start_locked` (subject the throttle key) and summarised as
//! `enrollment.start_lock_summary`. The peer is axum's `ConnectInfo` (IPv6 by
//! /64 for the throttle); proxy headers are never trusted. No peer, no
//! charge.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::header::{CACHE_CONTROL, PRAGMA};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use sqlx::{Connection, Postgres, Transaction};
use url::Url;
use uuid::Uuid;
use wipe_license::enrollment::{PollRequest, StartRequest};

use crate::accounts::Role;
use crate::audit::{self, AuditEntry};
use crate::auth::{self, actor, ApiError, CurrentUser, JsonBody};
use crate::enrollments::{self, EnrollmentError, FoundEnrollment, PollOutcome};
use crate::stations;
use crate::throttle;
use crate::AppState;

/// Where a station starts enrolling.
pub const ENROLLMENT_START_PATH: &str = "/api/enrollments";
/// Where a station polls its enrollment.
pub const ENROLLMENT_POLL_PATH: &str = "/api/enrollments/poll";
/// Where an owner looks up a pending enrollment by its user code.
pub const ENROLLMENT_LOOKUP_PATH: &str = "/api/enrollments/lookup";
/// Where an owner approves an enrollment.
pub const ENROLLMENT_APPROVE_PATH: &str = "/api/enrollments/approve";
/// Where an owner denies an enrollment.
pub const ENROLLMENT_DENY_PATH: &str = "/api/enrollments/deny";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(ENROLLMENT_START_PATH, post(start))
        .route(ENROLLMENT_POLL_PATH, post(poll))
        .route(ENROLLMENT_LOOKUP_PATH, post(lookup))
        .route(ENROLLMENT_APPROVE_PATH, post(approve))
        .route(ENROLLMENT_DENY_PATH, post(deny))
        .layer(axum::middleware::map_response(no_store))
}

/// `Cache-Control: no-store` and `Pragma: no-cache` on every response from
/// these routes, success or error (RFC 6749 §5.1, inherited by RFC 8628):
/// start carries the device code, poll and approve the licence, and lookup
/// what a pending enrollment's code names.
pub(crate) async fn no_store(mut resp: Response) -> Response {
    let headers = resp.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    resp
}

impl From<EnrollmentError> for ApiError {
    fn from(e: EnrollmentError) -> Self {
        ApiError::internal(e)
    }
}

/// `{base}/activate`, keeping any base path, with `?code=XXXX-XXXX` when
/// `code` (undashed) is given.
pub fn activation_link(base: &Url, code: Option<&str>) -> Url {
    let mut link = base.clone();
    let path = format!(
        "{}/{}",
        base.path().trim_end_matches('/'),
        enrollments::ACTIVATE_PAGE
    );
    link.set_path(&path);
    link.set_fragment(None);
    link.set_query(None);
    if let Some(code) = code {
        link.query_pairs_mut()
            .append_pair("code", &enrollments::format_user_code(code));
    }
    link
}

async fn start(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    JsonBody(req): JsonBody<StartRequest>,
) -> Result<Response, ApiError> {
    let new = enrollments::validate(&req).map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, m))?;
    let addr = peer.map(|ConnectInfo(addr)| addr);
    let peer_key = auth::ip_key(addr);
    if let Err(refused) = charge_start(&state, peer_key.as_deref()).await? {
        return Ok(refused);
    }
    let requested_from = addr.map(|a| a.ip().to_canonical().to_string());

    let mut tx = state.pool.begin().await?;
    let started = enrollments::start(
        &mut tx,
        &new,
        requested_from.as_deref(),
        peer_key.as_deref(),
    )
    .await?;
    tx.commit().await?;

    let base = &state.config.public_base_url;
    Ok(Json(json!({
        "device_code": started.device_code,
        "user_code": enrollments::format_user_code(&started.user_code),
        "verification_uri": activation_link(base, None).as_str(),
        "verification_uri_complete": activation_link(base, Some(&started.user_code)).as_str(),
        "expires_in": enrollments::EXPIRES_IN_SECS,
        "interval": enrollments::POLL_INTERVAL_SECS,
    }))
    .into_response())
}

/// Charge one start against `enroll:<peer key>`. The charge is final: the
/// one reaching [`throttle::ENROLL_IP_LIMIT`] locks the key for
/// [`throttle::ENROLL_LOCKOUT`], audited once as `enrollment.start_locked`.
/// `Err(response)` is the 429 for a refused start. No peer, no charge.
async fn charge_start(
    state: &AppState,
    peer: Option<&str>,
) -> Result<Result<(), Response>, ApiError> {
    let Some(peer) = peer else {
        return Ok(Ok(()));
    };
    let key = format!("{}{peer}", throttle::ENROLL_KEY_PREFIX);
    let mut tx = state.pool.begin().await?;
    let charged = throttle::charge(
        &mut tx,
        &key,
        throttle::ENROLL_IP_LIMIT,
        throttle::ENROLL_WINDOW,
    )
    .await?;
    let refused = match charged.outcome {
        throttle::Charge::Refused { retry_after_secs } => Some(retry_after_secs),
        throttle::Charge::Charged => {
            let engaged = throttle::confirm_failure(
                &mut *tx,
                &key,
                throttle::ENROLL_IP_LIMIT,
                throttle::ENROLL_LOCKOUT,
            )
            .await?;
            if let Some(until) = engaged {
                audit::record(
                    &mut *tx,
                    &AuditEntry {
                        actor: "anonymous",
                        action: "enrollment.start_locked",
                        subject: &key,
                        detail: json!({ "until": until }),
                    },
                )
                .await?;
            }
            None
        }
    };
    if let Some(s) = &charged.summary {
        auth::audit_lock_summary(&mut tx, s, Some(peer)).await?;
    }
    tx.commit().await?;
    Ok(match refused {
        Some(secs) => Err(auth::locked_response(enrollments::MSG_START_LOCKED, secs)),
        None => Ok(()),
    })
}

async fn poll(
    State(state): State<AppState>,
    JsonBody(req): JsonBody<PollRequest>,
) -> Result<Response, ApiError> {
    let mut tx = state.pool.begin().await?;
    let outcome = enrollments::poll(&mut tx, &req).await?;
    tx.commit().await?;
    Ok(match outcome {
        PollOutcome::Collected { station, licence } => {
            Json(json!({ "station": station, "licence": licence })).into_response()
        }
        PollOutcome::SlowDown { interval } => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "slow_down", "interval": interval })),
        )
            .into_response(),
        other => {
            let code = other.error_code().unwrap_or("invalid_grant");
            (StatusCode::BAD_REQUEST, Json(json!({ "error": code }))).into_response()
        }
    })
}

// ---------------------------------------------------------------------------
// An owner's code entry

/// `{code}`: the user code as the owner typed it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeRequest {
    pub code: String,
}

/// `{code, entitlement_id}`: the code and the entitlement whose seat the
/// station takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveRequest {
    pub code: String,
    pub entitlement_id: Uuid,
}

/// 403 [`enrollments::MSG_OWNER_ONLY_ENROLLMENTS`] unless `user` is an
/// organization owner.
fn require_owner(user: &CurrentUser) -> Result<(), ApiError> {
    if user.role == Role::Owner {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            enrollments::MSG_OWNER_ONLY_ENROLLMENTS,
        ))
    }
}

/// The user's code-entry throttle key, `enroll:user:<user id>`.
fn user_key(user: &CurrentUser) -> String {
    format!("{}{}", throttle::ENROLL_USER_KEY_PREFIX, user.user_id)
}

/// Charge one code entry against `key`. `Err(response)` is the 429 while
/// the key is locked or full.
async fn charge_code(state: &AppState, key: &str) -> Result<Result<(), Response>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let charged = throttle::charge(
        &mut tx,
        key,
        throttle::ENROLL_LOOKUP_LIMIT,
        throttle::ENROLL_LOOKUP_WINDOW,
    )
    .await?;
    if let Some(s) = &charged.summary {
        auth::audit_lock_summary(&mut tx, s, None).await?;
    }
    tx.commit().await?;
    Ok(match charged.outcome {
        throttle::Charge::Refused { retry_after_secs } => Err(auth::locked_response(
            enrollments::MSG_LOOKUP_LOCKED,
            retry_after_secs,
        )),
        throttle::Charge::Charged => Ok(()),
    })
}

/// The pending enrollment `code` names, locked `FOR UPDATE` in `tx`, with
/// the charge on `key` refunded in `tx`; otherwise the failure confirmed
/// (the one that locks the key audited as `enrollment.lookup_locked`), `tx`
/// committed and 404 [`enrollments::MSG_NO_ENROLLMENT`]. Everything runs on
/// `tx`'s one connection: a handler never holds a connection while waiting
/// for another from the pool, which concurrent requests would exhaust.
async fn match_code(
    mut tx: Transaction<'static, Postgres>,
    user: &CurrentUser,
    key: &str,
    code: &str,
) -> Result<(Transaction<'static, Postgres>, FoundEnrollment), ApiError> {
    if let Some(found) = enrollments::find_pending(&mut tx, code).await? {
        throttle::refund(&mut *tx, key).await?;
        return Ok((tx, found));
    }
    let engaged = throttle::confirm_failure(
        &mut *tx,
        key,
        throttle::ENROLL_LOOKUP_LIMIT,
        throttle::ENROLL_LOOKUP_LOCKOUT,
    )
    .await?;
    if let Some(until) = engaged {
        audit::record(
            &mut *tx,
            &AuditEntry {
                actor: &actor(user.user_id),
                action: "enrollment.lookup_locked",
                subject: key,
                detail: json!({ "until": until }),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Err(ApiError::new(
        StatusCode::NOT_FOUND,
        enrollments::MSG_NO_ENROLLMENT,
    ))
}

async fn lookup(
    State(state): State<AppState>,
    user: CurrentUser,
    body: Result<JsonBody<CodeRequest>, ApiError>,
) -> Result<Response, ApiError> {
    require_owner(&user)?;
    let JsonBody(req) = body?;
    let key = user_key(&user);
    if let Err(refused) = charge_code(&state, &key).await? {
        return Ok(refused);
    }
    let tx = state.pool.begin().await?;
    let (tx, found) = match_code(tx, &user, &key, &req.code).await?;
    tx.commit().await?;
    Ok(Json(json!({ "enrollment": found.enrollment })).into_response())
}

async fn approve(
    State(state): State<AppState>,
    user: CurrentUser,
    body: Result<JsonBody<ApproveRequest>, ApiError>,
) -> Result<Response, ApiError> {
    require_owner(&user)?;
    let Some(vendor) = state.config.vendor_root.as_ref() else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            stations::MSG_ISSUANCE_UNAVAILABLE,
        ));
    };
    let JsonBody(req) = body?;
    let key = user_key(&user);
    if let Err(refused) = charge_code(&state, &key).await? {
        return Ok(refused);
    }
    let tx = state.pool.begin().await?;
    let (mut tx, found) = match_code(tx, &user, &key, &req.code).await?;
    // The approval runs in a savepoint: a refusal rolls back to it, leaving
    // the enrollment pending, and the right code's refund is still committed.
    let mut approval = Connection::begin(&mut *tx).await?;
    let result = enrollments::approve(
        &mut approval,
        &vendor.root,
        user.organization_id,
        user.user_id,
        &found,
        req.entitlement_id,
    )
    .await;
    let registered = match result {
        Ok(registered) => {
            approval.commit().await?;
            tx.commit().await?;
            registered
        }
        Err(refused) => {
            approval.rollback().await?;
            tx.commit().await?;
            return Err(refused.into());
        }
    };
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "station": registered.station,
            "licence": registered.licence,
        })),
    )
        .into_response())
}

async fn deny(
    State(state): State<AppState>,
    user: CurrentUser,
    body: Result<JsonBody<CodeRequest>, ApiError>,
) -> Result<Response, ApiError> {
    require_owner(&user)?;
    let JsonBody(req) = body?;
    let key = user_key(&user);
    if let Err(refused) = charge_code(&state, &key).await? {
        return Ok(refused);
    }
    let tx = state.pool.begin().await?;
    let (mut tx, found) = match_code(tx, &user, &key, &req.code).await?;
    enrollments::deny(&mut tx, user.user_id, &found).await?;
    tx.commit().await?;
    Ok(Json(json!({})).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_links_keep_the_base_path() {
        let base = Url::parse("http://127.0.0.1:7900").unwrap();
        assert_eq!(
            activation_link(&base, None).as_str(),
            "http://127.0.0.1:7900/activate"
        );
        assert_eq!(
            activation_link(&base, Some("BCDFGHJK")).as_str(),
            "http://127.0.0.1:7900/activate?code=BCDF-GHJK"
        );
        let base = Url::parse("https://example.com/portal/?x=1#frag").unwrap();
        assert_eq!(
            activation_link(&base, Some("BCDFGHJK")).as_str(),
            "https://example.com/portal/activate?code=BCDF-GHJK"
        );
        assert_eq!(
            activation_link(&base, None).as_str(),
            "https://example.com/portal/activate"
        );
    }
}
