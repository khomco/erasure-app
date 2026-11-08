//! The admin area's routes (portal-accounts story 5), all GET and read-only:
//!
//! - `GET /api/admin/organizations?q=` — search customer organizations by
//!   name or by their users' emails and names.
//! - `GET /api/admin/users?q=` — search customer users by email or name.
//! - `GET /api/admin/organizations/:id` — one customer organization and its
//!   users.
//!
//! Only a vendor admin gets in ([`VendorAdmin`]'s extractor): no session is
//! 401 `not signed in`; any other session (an owner, a member, a
//! `vendor_admin`-role user outside the vendor organization, or one whose
//! email is unverified) is 403 [`MSG_ADMIN_ONLY`]. The check runs before
//! the query string is read or any data is touched, and is read live, so a
//! change applies to the next request. Each handler runs one transaction in
//! which [`crate::admin`] reads and writes its audit row. A bad `q` is 400
//! [`admin::MSG_BAD_QUERY`] and an unknown, non-UUID or vendor organization
//! id is 404 [`MSG_NO_ORGANIZATION`]; neither writes an audit row. Error
//! bodies are `{"error": "<message>"}`.

use axum::async_trait;
use axum::extract::rejection::QueryRejection;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::accounts;
use crate::admin::{
    self, AdminOrganizationDetail, AdminReadError, OrganizationSearch, UserSearch, VendorAdmin,
};
use crate::auth::{session_token, ApiError, MSG_NOT_SIGNED_IN};
use crate::AppState;

pub const MSG_ADMIN_ONLY: &str = "only vendor admins can use the admin area";
pub const MSG_NO_ORGANIZATION: &str = "no such organization";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/organizations", get(search_organizations))
        .route("/api/admin/organizations/:id", get(organization_detail))
        .route("/api/admin/users", get(search_users))
}

impl From<AdminReadError> for ApiError {
    fn from(e: AdminReadError) -> Self {
        ApiError::internal(e)
    }
}

#[async_trait]
impl FromRequestParts<AppState> for VendorAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let not_signed_in = || ApiError::new(StatusCode::UNAUTHORIZED, MSG_NOT_SIGNED_IN);
        let token = session_token(&parts.headers).ok_or_else(not_signed_in)?;
        let (user_id, _, _) = accounts::session_user(&state.pool, &token)
            .await?
            .ok_or_else(not_signed_in)?;
        // Role, vendor organization and verified email, read together and
        // live: none of them is taken from the session lookup above.
        let is_admin: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM users u JOIN organizations o ON o.id = u.organization_id \
             WHERE u.id = $1 AND u.role = 'vendor_admin' AND o.is_vendor \
               AND u.email_verified_at IS NOT NULL)",
        )
        .bind(user_id)
        .fetch_one(&state.pool)
        .await?;
        if is_admin {
            Ok(VendorAdmin { user_id })
        } else {
            Err(ApiError::new(StatusCode::FORBIDDEN, MSG_ADMIN_ONLY))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
}

/// The validated `q`, or 400.
fn query_of(params: Result<Query<SearchParams>, QueryRejection>) -> Result<String, ApiError> {
    let bad = || ApiError::new(StatusCode::BAD_REQUEST, admin::MSG_BAD_QUERY);
    let Query(params) = params.map_err(|_| bad())?;
    admin::validate_query(params.q.as_deref()).map_err(|_| bad())
}

// `admin` precedes the query in each signature: extractors run in order, so
// the admin check comes before the query string is read.

async fn search_organizations(
    State(state): State<AppState>,
    admin: VendorAdmin,
    params: Result<Query<SearchParams>, QueryRejection>,
) -> Result<Json<OrganizationSearch>, ApiError> {
    let q = query_of(params)?;
    let mut tx = state.pool.begin().await?;
    let found = admin::search_organizations(&mut tx, &admin, &q).await?;
    tx.commit().await?;
    Ok(Json(found))
}

async fn search_users(
    State(state): State<AppState>,
    admin: VendorAdmin,
    params: Result<Query<SearchParams>, QueryRejection>,
) -> Result<Json<UserSearch>, ApiError> {
    let q = query_of(params)?;
    let mut tx = state.pool.begin().await?;
    let found = admin::search_users(&mut tx, &admin, &q).await?;
    tx.commit().await?;
    Ok(Json(found))
}

async fn organization_detail(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Json<AdminOrganizationDetail>, ApiError> {
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, MSG_NO_ORGANIZATION);
    let id = Uuid::parse_str(&id).map_err(|_| not_found())?;
    let mut tx = state.pool.begin().await?;
    let detail = admin::organization_detail(&mut tx, &admin, id)
        .await?
        .ok_or_else(not_found)?;
    tx.commit().await?;
    Ok(Json(detail))
}
