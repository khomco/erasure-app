//! Customer catalog, checkout, orders and billing routes
//! (offerings-and-payments story 2, CAP-2 and CAP-5):
//!
//! - `GET /api/offerings` — any signed-in user: published offerings.
//! - `POST /api/checkout {offering_id}` — a verified owner starts a
//!   checkout: 201 `{order, checkout_url}` with the order `pending`.
//! - `GET /api/orders`, `GET /api/orders/:id` — any role: the
//!   organization's orders, newest first, or one.
//! - `POST /api/billing/portal` — an owner: `{url}` of the provider's
//!   billing page.
//! - The Fake provider's hosted-page stand-in, owners only, 404 under any
//!   other provider: `GET /api/payments/fake/sessions/:id` reads the
//!   checkout's order; `POST …/:id/pay` and `POST …/:id/abandon` sign a
//!   `checkout.completed` / `checkout.expired` event, run it through
//!   `verify_event` and [`orders::apply_event`] in one transaction, and
//!   answer the order. Pay and abandon refuse a hub whose public base URL is
//!   not local (503).
//!
//! The organization is always the session's ([`CurrentUser`]). Another
//! organization's order or session id is a 404 like an unknown one, and
//! changes and audits nothing. Owner and verified-email checks run before
//! the body is read. Nothing here marks an order paid except through
//! `verify_event` → `apply_event`. Error bodies are `{"error": "<message>"}`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::accounts::{self, Role};
use crate::auth::{is_local_host, ApiError, CurrentUser, JsonBody, MSG_NOT_SIGNED_IN};
use crate::orders::{self, ApplyError, OrderError};
use crate::payments::{
    Fake, ProviderError, FAKE_CHECKOUT_COMPLETED, FAKE_CHECKOUT_EXPIRED, MSG_FAKE_LOCAL_ONLY,
};
use crate::AppState;

pub const MSG_OWNER_ONLY_BUY: &str = "only an organization owner can buy";
pub const MSG_OWNER_ONLY_BILLING: &str = "only an organization owner can manage billing";
pub const MSG_UNVERIFIED_BUYER: &str = "verify your email address before buying";
pub const MSG_NOT_FOR_SALE: &str = "this offering is not for sale";
pub const MSG_NO_ORDER: &str = "no such order";
pub const MSG_NO_CHECKOUT: &str = "no such checkout";
pub const MSG_CHECKOUT_ENDED: &str = "this checkout has already ended";
pub const MSG_PROVIDER_FAILED: &str = "the payment provider could not be reached; try again later";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/offerings", get(catalog))
        .route("/api/checkout", post(checkout))
        .route("/api/orders", get(list_orders))
        .route("/api/orders/:id", get(order_detail))
        .route("/api/billing/portal", post(billing_portal))
        .route("/api/payments/fake/sessions/:id", get(fake_session))
        .route("/api/payments/fake/sessions/:id/pay", post(fake_pay))
        .route(
            "/api/payments/fake/sessions/:id/abandon",
            post(fake_abandon),
        )
}

fn forbidden(message: &str) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, message)
}

fn not_found(message: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, message)
}

fn require_owner(user: &CurrentUser, message: &str) -> Result<(), ApiError> {
    if user.role == Role::Owner {
        Ok(())
    } else {
        Err(forbidden(message))
    }
}

impl From<ProviderError> for ApiError {
    fn from(e: ProviderError) -> Self {
        match e {
            ProviderError::Unavailable(message) => {
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, message)
            }
            ProviderError::Rejected(message) => {
                tracing::warn!(reason = message, "payment event rejected");
                ApiError::new(StatusCode::BAD_REQUEST, message)
            }
            other => {
                tracing::error!(error = %other, "payment provider request failed");
                ApiError::new(StatusCode::BAD_GATEWAY, MSG_PROVIDER_FAILED)
            }
        }
    }
}

impl From<OrderError> for ApiError {
    fn from(e: OrderError) -> Self {
        match e {
            OrderError::NotForSale => not_found(MSG_NOT_FOR_SALE),
            OrderError::Provider(p) => p.into(),
            OrderError::Db(e) => e.into(),
            OrderError::Audit(e) => e.into(),
        }
    }
}

impl From<ApplyError> for ApiError {
    fn from(e: ApplyError) -> Self {
        match e {
            ApplyError::Db(e) => e.into(),
            ApplyError::Audit(e) => e.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Catalog and checkout

async fn catalog(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let offerings = orders::catalog(&state.pool).await?;
    Ok(Json(json!({ "offerings": offerings })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutRequest {
    pub offering_id: Uuid,
}

async fn checkout(
    State(state): State<AppState>,
    user: CurrentUser,
    body: Result<JsonBody<CheckoutRequest>, ApiError>,
) -> Result<Response, ApiError> {
    require_owner(&user, MSG_OWNER_ONLY_BUY)?;
    let buyer = accounts::account(&state.pool, user.organization_id, user.user_id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, MSG_NOT_SIGNED_IN))?;
    if !buyer.user.email_verified {
        return Err(forbidden(MSG_UNVERIFIED_BUYER));
    }
    let JsonBody(req) = body?;
    let mut tx = state.pool.begin().await?;
    let started = orders::create(
        &mut tx,
        state.payments.as_ref(),
        user.organization_id,
        user.user_id,
        req.offering_id,
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "order": started.order,
            "checkout_url": started.checkout_url.as_str(),
        })),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Orders and billing

async fn list_orders(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let orders = orders::list(&state.pool, user.organization_id).await?;
    Ok(Json(json!({ "orders": orders })))
}

async fn order_detail(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = Uuid::parse_str(&id).map_err(|_| not_found(MSG_NO_ORDER))?;
    let order = orders::get(&state.pool, user.organization_id, id)
        .await?
        .ok_or_else(|| not_found(MSG_NO_ORDER))?;
    Ok(Json(json!({ "order": order })))
}

async fn billing_portal(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_owner(&user, MSG_OWNER_ONLY_BILLING)?;
    let url = state.payments.billing_portal(user.organization_id).await?;
    Ok(Json(json!({ "url": url.as_str() })))
}

// ---------------------------------------------------------------------------
// The Fake provider's hosted-page stand-in

/// Owners only, and only while the Fake provider is in use.
fn fake_checkout_access(state: &AppState, user: &CurrentUser) -> Result<(), ApiError> {
    require_owner(user, MSG_OWNER_ONLY_BUY)?;
    if state.payments.name() != "fake" {
        return Err(not_found(MSG_NO_CHECKOUT));
    }
    Ok(())
}

async fn fake_session(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    fake_checkout_access(&state, &user)?;
    let mut conn = state.pool.acquire().await?;
    let order = orders::by_session(&mut conn, user.organization_id, "fake", &session_id)
        .await?
        .ok_or_else(|| not_found(MSG_NO_CHECKOUT))?;
    Ok(Json(json!({ "order": order })))
}

/// Sign a Fake `event_type` event for the organization's pending checkout
/// `session_id`, verify it through the provider and apply it, all in one
/// transaction holding the order's row lock.
async fn fake_finish(
    state: &AppState,
    user: &CurrentUser,
    session_id: &str,
    event_type: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    fake_checkout_access(state, user)?;
    if !is_local_host(&state.config.public_base_url) {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            MSG_FAKE_LOCAL_ONLY,
        ));
    }
    let mut tx = state.pool.begin().await?;
    let order = orders::by_session(&mut tx, user.organization_id, "fake", session_id)
        .await?
        .ok_or_else(|| not_found(MSG_NO_CHECKOUT))?;
    if order.status != "pending" {
        return Err(ApiError::new(StatusCode::CONFLICT, MSG_CHECKOUT_ENDED));
    }
    let (body, signature) = Fake::signed_event(event_type, session_id);
    let event = state
        .payments
        .verify_event(&Fake::signature_headers(&signature), &body)?;
    let order = orders::apply_event(&mut tx, &event)
        .await?
        .ok_or_else(|| ApiError::internal("a locked pending order vanished"))?;
    tx.commit().await?;
    Ok(Json(json!({ "order": order })))
}

async fn fake_pay(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    fake_finish(&state, &user, &session_id, FAKE_CHECKOUT_COMPLETED).await
}

async fn fake_abandon(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    fake_finish(&state, &user, &session_id, FAKE_CHECKOUT_EXPIRED).await
}
