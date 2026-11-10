//! A connected station's licence fetch (station-enrollment story 5, CAP-5,
//! ADR-0008 §2, §3). Neither route needs a session; `POST
//! /api/station/licence` is exempt from the CSRF check ([`crate::csrf`]):
//! the station has no cookie, and its instance-key signature is the
//! credential. The domain logic is [`station_link`]'s.
//!
//! - `GET /api/station/challenge` — 200 `{challenge, expires_in}`: a
//!   single-use challenge valid for [`station_link::CHALLENGE_TTL_SECS`]
//!   seconds. Charged first against `station:ip:<peer>` (IPv6 by /64): the
//!   charge is final, [`throttle::STATION_CHALLENGE_IP_LIMIT`] an hour, and
//!   the one reaching it locks the key for
//!   [`throttle::STATION_CHALLENGE_LOCKOUT`], audited once as
//!   `station.challenge_locked` (subject the throttle key) and summarised as
//!   `station.challenge_lock_summary`; refused with 429
//!   [`MSG_CHALLENGE_LOCKED`] and `Retry-After`. No peer, no charge. Issuing
//!   a challenge is not audited.
//! - `POST /api/station/licence {instance_public_key, licence_id}` — signed
//!   with [`wipe_license::station_request`] over the challenge, the hub's
//!   origin (`stripe::hub_origin(public_base_url)`), `POST`, this path and
//!   the body bytes as sent. `instance_public_key` is the station's 32-byte
//!   Ed25519 key in unpadded standard base64; `licence_id` is the id of the
//!   licence it holds, or null. In order: 401 [`MSG_UNSIGNED`] without
//!   exactly one of each signature header; 400 for a body that is not that
//!   JSON, or a key that does not decode; 401 [`MSG_KEY_ID_MISMATCH`] when
//!   the header's key id is not the key's id; 401 [`MSG_BAD_SIGNATURE`] for
//!   a signature that does not verify, or any header containing a line
//!   break (nothing is consumed); 401 [`MSG_CHALLENGE_USED`] when the
//!   challenge is unknown, expired or already used (it is consumed now);
//!   then [`station_link::fetch_licence`] in one transaction: 200
//!   `{licence}` (the stored licence document as JSON), 204 when there is
//!   nothing new, 404 [`MSG_NOT_REGISTERED`], 409 [`MSG_AMBIGUOUS_KEY`].
//!
//! Every response from both routes, errors included, carries
//! `Cache-Control: no-store` and `Pragma: no-cache`, as the enrollment
//! routes do. Errors are `{"error": message}`.

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use wipe_license::station_request::{
    self, SignedStationRequest, HEADER_CHALLENGE, HEADER_KEY_ID, HEADER_SIGNATURE,
};

use crate::audit::{self, AuditEntry};
use crate::auth::{self, ApiError};
use crate::enrollment_routes::no_store;
use crate::enrollments;
use crate::station_link::{self, Fetch};
use crate::{stripe, throttle, AppState};

/// Where a station asks for a challenge.
pub const STATION_CHALLENGE_PATH: &str = "/api/station/challenge";
/// Where a station fetches its licence.
pub const STATION_LICENCE_PATH: &str = "/api/station/licence";

pub const MSG_UNSIGNED: &str = "the request must carry one x-wipestation-key-id, one \
     x-wipestation-challenge and one x-wipestation-signature header";
pub const MSG_BAD_BODY: &str =
    "the body must be JSON {\"instance_public_key\": \"…\", \"licence_id\": \"…\" or null}";
pub const MSG_KEY_ID_MISMATCH: &str =
    "the instance key id does not match the instance public key in the body";
pub const MSG_BAD_SIGNATURE: &str =
    "the request's signature does not verify with the station's instance key";
pub const MSG_CHALLENGE_USED: &str = "challenge expired or already used";
pub const MSG_NOT_REGISTERED: &str = "this station is not registered with the platform: no \
     organization has a station with its instance key";
pub const MSG_AMBIGUOUS_KEY: &str = "this station's instance key is registered in more than one \
     organization, and the licence it holds does not say which; an owner must release it from \
     all but one";
pub const MSG_CHALLENGE_LOCKED: &str = "too many licence checks from this network; try again later";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(STATION_CHALLENGE_PATH, get(challenge))
        .route(STATION_LICENCE_PATH, post(licence))
        .layer(axum::middleware::map_response(no_store))
}

async fn challenge(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
) -> Result<Response, ApiError> {
    let peer_key = auth::ip_key(peer.map(|ConnectInfo(addr)| addr));
    if let Err(refused) = charge_challenge(&state, peer_key.as_deref()).await? {
        return Ok(refused);
    }
    let mut tx = state.pool.begin().await?;
    let challenge = station_link::issue_challenge(&mut tx, peer_key.as_deref()).await?;
    tx.commit().await?;
    Ok(Json(json!({
        "challenge": challenge,
        "expires_in": station_link::CHALLENGE_TTL_SECS,
    }))
    .into_response())
}

/// Charge one challenge against `station:<peer key>`, final, as
/// `enrollment_routes` charges an enrollment start. `Err(response)` is the
/// 429. No peer, no charge.
async fn charge_challenge(
    state: &AppState,
    peer: Option<&str>,
) -> Result<Result<(), Response>, ApiError> {
    let Some(peer) = peer else {
        return Ok(Ok(()));
    };
    let key = format!("{}{peer}", throttle::STATION_KEY_PREFIX);
    let mut tx = state.pool.begin().await?;
    let charged = throttle::charge(
        &mut tx,
        &key,
        throttle::STATION_CHALLENGE_IP_LIMIT,
        throttle::STATION_CHALLENGE_WINDOW,
    )
    .await?;
    let refused = match charged.outcome {
        throttle::Charge::Refused { retry_after_secs } => Some(retry_after_secs),
        throttle::Charge::Charged => {
            let engaged = throttle::confirm_failure(
                &mut *tx,
                &key,
                throttle::STATION_CHALLENGE_IP_LIMIT,
                throttle::STATION_CHALLENGE_LOCKOUT,
            )
            .await?;
            if let Some(until) = engaged {
                audit::record(
                    &mut *tx,
                    &AuditEntry {
                        actor: "anonymous",
                        action: "station.challenge_locked",
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
        Some(secs) => Err(auth::locked_response(MSG_CHALLENGE_LOCKED, secs)),
        None => Ok(()),
    })
}

/// `{instance_public_key, licence_id}`.
#[derive(Debug, Deserialize)]
struct LicenceRequest {
    instance_public_key: String,
    licence_id: Option<String>,
}

/// Exactly one value of header `name`, as text.
fn one_header(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut values = headers.get_all(name).iter();
    match (values.next(), values.next()) {
        (Some(v), None) => v.to_str().ok().map(str::to_string),
        _ => None,
    }
}

fn unauthorized(message: &str) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, message)
}

async fn licence(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let signed = match (
        one_header(&headers, HEADER_KEY_ID),
        one_header(&headers, HEADER_CHALLENGE),
        one_header(&headers, HEADER_SIGNATURE),
    ) {
        (Some(instance_key_id), Some(challenge), Some(signature_b64)) => SignedStationRequest {
            instance_key_id,
            challenge,
            signature_b64,
        },
        _ => return Err(unauthorized(MSG_UNSIGNED)),
    };
    let req: LicenceRequest = serde_json::from_slice(&body)
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, MSG_BAD_BODY))?;
    let key = wipe_license::enrollment::decode_public_key(&req.instance_public_key)
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, enrollments::MSG_BAD_PUBLIC_KEY))?;
    let origin = stripe::hub_origin(&state.config.public_base_url);
    match station_request::verify(&key, &signed, &origin, "POST", STATION_LICENCE_PATH, &body) {
        Ok(()) => {}
        Err(station_request::StationRequestError::KeyIdMismatch) => {
            return Err(unauthorized(MSG_KEY_ID_MISMATCH))
        }
        Err(_) => return Err(unauthorized(MSG_BAD_SIGNATURE)),
    }

    // Consumed in a statement of its own, before the fetch: a challenge is
    // spent by its first validly signed use, whatever happens next.
    if !station_link::consume_challenge(&state.pool, &signed.challenge).await? {
        return Err(unauthorized(MSG_CHALLENGE_USED));
    }
    let root = state.config.vendor_root.as_ref().map(|v| &v.root);
    let mut tx = state.pool.begin().await?;
    let fetched = station_link::fetch_licence(
        &mut tx,
        root,
        &signed.instance_key_id,
        req.licence_id.as_deref(),
    )
    .await?;
    tx.commit().await?;
    Ok(match fetched {
        Fetch::Licence(licence) => Json(json!({ "licence": licence })).into_response(),
        Fetch::Current => StatusCode::NO_CONTENT.into_response(),
        Fetch::NotFound => ApiError::new(StatusCode::NOT_FOUND, MSG_NOT_REGISTERED).into_response(),
        Fetch::Ambiguous => ApiError::new(StatusCode::CONFLICT, MSG_AMBIGUOUS_KEY).into_response(),
    })
}
