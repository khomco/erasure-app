//! The sales-lead routes (lead-capture story 1):
//!
//! - `POST /api/leads` — the marketing site's early-access and contact
//!   forms, cross-origin through [`crate::cors`] and the usual CSRF check.
//!   In order: validate (400); charge `lead-form:ip:<peer>` (429
//!   [`leads::MSG_SUBMIT_LOCKED`] with `Retry-After`); a filled honeypot
//!   answers 202 `{}` and does nothing else; charge the email-send throttle
//!   ([`auth::charge_mail`], its own 429); store the lead, the submission
//!   and the audit row in one transaction; answer 202 `{}`; send the
//!   confirmation from a spawned task, unless the lead is unsubscribed
//!   (`suppressed`): an unsubscribe sticks, and no email ever goes to it.
//!   The email-send throttle is charged either way, so neither the answer
//!   nor a throttle refusal reveals whether an address is known or
//!   unsubscribed, and a send failure is only logged.
//!
//! The per-IP throttle's audit rows (`lead.submit_locked` with the throttle
//! key as subject, `lead.submit_lock_summary` adding `peer`) keep the peer IP
//! like every other throttle row; they are never linked to a lead id.
//! - `POST /api/leads/unsubscribe` — the emailed link's portal page. Any of a
//!   lead's tokens unsubscribes it: 200 `{"unsubscribed": true}`, also when
//!   it already was; a malformed or unknown token is 404
//!   [`leads::MSG_BAD_UNSUBSCRIBE`].
//!
//! The peer is axum's `ConnectInfo` (IPv6 by /64); proxy headers are never
//! trusted. Error bodies are `{"error": "<message>"}`.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::accounts;
use crate::audit::{self, AuditEntry};
use crate::auth::{self, ApiError, JsonBody};
use crate::leads::{self, LeadRequest, LeadStoreError};
use crate::throttle;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/leads", post(submit))
        .route("/api/leads/unsubscribe", post(unsubscribe))
}

impl From<LeadStoreError> for ApiError {
    fn from(e: LeadStoreError) -> Self {
        ApiError::internal(e)
    }
}

fn accepted() -> Response {
    (StatusCode::ACCEPTED, Json(json!({}))).into_response()
}

async fn submit(
    State(state): State<AppState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    JsonBody(req): JsonBody<LeadRequest>,
) -> Result<Response, ApiError> {
    let new = leads::validate(&req).map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, m))?;
    let peer = auth::ip_key(peer.map(|ConnectInfo(addr)| addr));
    if let Err(refused) = charge_submission(&state, peer.as_deref()).await? {
        return Ok(refused);
    }
    if req.is_honeypot() {
        return Ok(accepted());
    }
    if let Err(refused) = auth::charge_mail(&state, &new.email, peer.as_deref(), None).await? {
        return Ok(refused);
    }
    let mut tx = state.pool.begin().await?;
    let submitted = leads::submit(&mut tx, &new).await?;
    tx.commit().await?;

    if submitted.suppressed {
        // Unsubscribed: stored and audited, never emailed.
        return Ok(accepted());
    }
    let kind = new.kind;
    let to = new.email;
    tokio::spawn(async move {
        let link = auth::portal_link(
            &state.config.public_base_url,
            "unsubscribe",
            &submitted.unsubscribe_token,
        );
        if let Err(e) = state.mailer.send_lead_confirmation(&to, kind, &link).await {
            // Never the token. Logged and never returned: the 202 is sent.
            tracing::warn!(
                error = %e,
                lead = %submitted.lead_id,
                "lead confirmation email not sent"
            );
        }
    });
    Ok(accepted())
}

/// Charge one submission against `lead-form:<peer key>`. The charge is final: the
/// one reaching [`throttle::LEAD_IP_LIMIT`] locks the key for
/// [`throttle::LEAD_LOCKOUT`], audited once as `lead.submit_locked`.
/// `Err(response)` is the 429 for a refused submission. No peer, no charge.
async fn charge_submission(
    state: &AppState,
    peer: Option<&str>,
) -> Result<Result<(), Response>, ApiError> {
    let Some(peer) = peer else {
        return Ok(Ok(()));
    };
    let key = format!("{}{peer}", throttle::LEAD_KEY_PREFIX);
    let mut tx = state.pool.begin().await?;
    let charged = throttle::charge(
        &mut tx,
        &key,
        throttle::LEAD_IP_LIMIT,
        throttle::LEAD_WINDOW,
    )
    .await?;
    let refused = match charged.outcome {
        throttle::Charge::Refused { retry_after_secs } => Some(retry_after_secs),
        throttle::Charge::Charged => {
            let engaged = throttle::confirm_failure(
                &mut *tx,
                &key,
                throttle::LEAD_IP_LIMIT,
                throttle::LEAD_LOCKOUT,
            )
            .await?;
            if let Some(until) = engaged {
                audit::record(
                    &mut *tx,
                    &AuditEntry {
                        actor: "anonymous",
                        action: "lead.submit_locked",
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
        Some(secs) => Err(auth::locked_response(leads::MSG_SUBMIT_LOCKED, secs)),
        None => Ok(()),
    })
}

#[derive(Debug, Deserialize)]
struct UnsubscribeRequest {
    #[serde(default)]
    token: String,
}

async fn unsubscribe(
    State(state): State<AppState>,
    JsonBody(req): JsonBody<UnsubscribeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let not_valid = || ApiError::new(StatusCode::NOT_FOUND, leads::MSG_BAD_UNSUBSCRIBE);
    let hash = accounts::hash_token(&req.token).ok_or_else(not_valid)?;
    let mut tx = state.pool.begin().await?;
    leads::unsubscribe(&mut tx, &hash)
        .await?
        .ok_or_else(not_valid)?;
    tx.commit().await?;
    Ok(Json(json!({ "unsubscribed": true })))
}
