//! Customer catalog, checkout, orders, billing and payment-event routes
//! (offerings-and-payments stories 2 and 3, CAP-2, CAP-3 and CAP-5):
//!
//! - `GET /api/offerings` — any signed-in user: published offerings.
//! - `POST /api/checkout {offering_id}` — a verified owner starts a
//!   checkout: 201 `{order, checkout_url}` with the order `pending`.
//! - `GET /api/orders`, `GET /api/orders/:id` — any role: the
//!   organization's orders, newest first, or one.
//! - `POST /api/billing/portal` — an owner: `{url}` of the provider's
//!   billing page for the organization's stored customer; 409
//!   [`MSG_NO_BILLING_ACCOUNT`] before the first Stripe payment.
//! - `POST /api/payments/webhook` — the provider's events. No session and
//!   no CSRF token (the one route [`crate::csrf::require`] exempts): the
//!   configured provider's signature over the raw body is the credential.
//!   The event goes through [`payment_events::receive`] in one transaction
//!   and the answer is 200 `{"received":true}`, or 503
//!   [`MSG_EVENT_NOT_READY`] (rolled back, so the provider retries) for a
//!   paid event whose order is not committed yet.
//! - The Fake provider's hosted-page stand-in, owners only, 404 under any
//!   other provider: `GET /api/payments/fake/sessions/:id` reads the
//!   checkout's order; `POST …/:id/pay` and `POST …/:id/abandon` sign a
//!   `checkout.completed` / `checkout.expired` event, run it through
//!   `verify_event` and [`payment_events::receive`] in one transaction, and
//!   answer the order. Pay and abandon refuse a hub whose public base URL is
//!   not local (503).
//!
//! The organization is always the session's ([`CurrentUser`]). Another
//! organization's order or session id is a 404 like an unknown one, and
//! changes and audits nothing. Owner and verified-email checks run before
//! the body is read. Nothing here marks an order paid except through
//! `verify_event` → `receive` → `apply_event`. Error bodies are `{"error": "<message>"}`.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::accounts::{self, Role};
use crate::auth::{is_local_host, ApiError, CurrentUser, JsonBody, MSG_NOT_SIGNED_IN};
use crate::orders::{self, ApplyError, ApplyOutcome, OrderError};
use crate::payment_events::{self, Receipt};
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
pub const MSG_NO_BILLING_ACCOUNT: &str = "billing opens after your first payment";
pub const MSG_EVENT_NOT_READY: &str = "this payment event cannot be applied yet; retry later";

/// The provider webhook: the one mutating `/api` route without a CSRF token.
pub const PAYMENT_WEBHOOK_PATH: &str = "/api/payments/webhook";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/offerings", get(catalog))
        .route("/api/checkout", post(checkout))
        .route("/api/orders", get(list_orders))
        .route("/api/orders/:id", get(order_detail))
        .route("/api/billing/portal", post(billing_portal))
        .route(PAYMENT_WEBHOOK_PATH, post(webhook))
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
            ProviderError::NoBillingAccount => {
                ApiError::new(StatusCode::CONFLICT, MSG_NO_BILLING_ACCOUNT)
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
    let customer =
        orders::billing_customer(&state.pool, user.organization_id, state.payments.name()).await?;
    let url = state
        .payments
        .billing_portal(user.organization_id, customer.as_deref())
        .await?;
    Ok(Json(json!({ "url": url.as_str() })))
}

// ---------------------------------------------------------------------------
// Provider events

/// `POST /api/payments/webhook`: verify the raw body with the configured
/// provider (a bad signature is 400, a Fake event on a non-local hub 503),
/// then store and apply it in one transaction.
async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let event = state.payments.verify_event(&headers, &body)?;
    let mut tx = state.pool.begin().await?;
    let receipt = payment_events::receive(&mut tx, &event, &body).await?;
    if receipt == Receipt::NotReady {
        tx.rollback().await?;
        tracing::error!(
            provider = event.provider(),
            event_id = event.id(),
            session_id = event.session_id().unwrap_or("-"),
            order_ref = ?event.order_ref(),
            "a paid payment event names a hub order that has no such checkout session yet; \
             answering 503 so the provider retries"
        );
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            MSG_EVENT_NOT_READY,
        ));
    }
    tx.commit().await?;
    match &receipt {
        Receipt::Duplicate => tracing::info!(
            provider = event.provider(),
            event_id = event.id(),
            "payment event already received"
        ),
        Receipt::Recorded(outcome) => tracing::info!(
            provider = event.provider(),
            event_id = event.id(),
            event_type = event.event_type(),
            outcome = outcome.as_str(),
            "payment event received"
        ),
        Receipt::NotReady => {}
    }
    Ok(Json(json!({ "received": true })))
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
/// `session_id`, verify it through the provider, and store and apply it
/// through [`payment_events::receive`], all in one transaction holding the
/// order's row lock.
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
    let order = match payment_events::receive(&mut tx, &event, &body).await? {
        Receipt::Recorded(outcome) => match *outcome {
            ApplyOutcome::Applied(order) => Ok(order),
            other => Err(format!("{other:?}")),
        },
        other => Err(format!("{other:?}")),
    }
    .map_err(|got| {
        ApiError::internal(format!(
            "a fresh Fake event on a locked pending order was not applied: {got}"
        ))
    })?;
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
