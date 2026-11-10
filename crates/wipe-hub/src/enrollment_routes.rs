//! A station's enrollment routes (station-enrollment story 1, CAP-1,
//! ADR-0008 §1). Neither needs a session, and both are exempt from the CSRF
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
//! Every response from both routes, errors included, carries
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
use serde_json::json;
use url::Url;
use wipe_license::enrollment::{PollRequest, StartRequest};

use crate::audit::{self, AuditEntry};
use crate::auth::{self, ApiError, JsonBody};
use crate::enrollments::{self, EnrollmentError, PollOutcome};
use crate::throttle;
use crate::AppState;

/// Where a station starts enrolling.
pub const ENROLLMENT_START_PATH: &str = "/api/enrollments";
/// Where a station polls its enrollment.
pub const ENROLLMENT_POLL_PATH: &str = "/api/enrollments/poll";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(ENROLLMENT_START_PATH, post(start))
        .route(ENROLLMENT_POLL_PATH, post(poll))
        .layer(axum::middleware::map_response(no_store))
}

/// `Cache-Control: no-store` and `Pragma: no-cache` on every response from
/// these routes, success or error (RFC 6749 §5.1, inherited by RFC 8628):
/// start carries the device code and poll the licence.
async fn no_store(mut resp: Response) -> Response {
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
