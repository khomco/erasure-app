//! The admin area's routes (portal-accounts story 5; sales leads,
//! lead-capture story 2; offerings, offerings-and-payments story 1):
//!
//! - `GET /api/admin/organizations?q=` — search customer organizations by
//!   name or by their users' emails and names.
//! - `GET /api/admin/users?q=` — search customer users by email or name.
//! - `GET /api/admin/organizations/:id` — one customer organization and its
//!   users.
//! - `GET /api/admin/leads?segment=&status=&from=&to=` — sales leads,
//!   filtered ([`lead_admin::LeadFilter`]), at most 100 plus `truncated`.
//! - `GET /api/admin/leads/:id` — one lead, its submissions and its notes.
//! - `POST /api/admin/leads/:id/status {status}` — set the lead's status.
//! - `POST /api/admin/leads/:id/notes {body}` — add a note (201).
//! - `POST /api/admin/leads/:id/erase` — delete the lead, its submissions
//!   and its notes.
//! - `POST /api/admin/leads/export {segment?, status?, from?, to?}` — every
//!   matching lead as a CSV download. A CSRF-protected POST, not a GET, so
//!   no other site can start a download of personal data.
//! - `GET /api/admin/offerings` — every offering, drafts first.
//! - `POST /api/admin/offerings {offering}` — create a draft offering (201).
//! - `GET /api/admin/offerings/:id` — one offering.
//! - `POST /api/admin/offerings/:id {offering}` — edit it (a full body).
//! - `POST /api/admin/offerings/:id/publish` — draft to published.
//! - `POST /api/admin/offerings/:id/retire` — published to retired.
//!
//! The organization and user routes are read-only; the lead routes also
//! change sales leads, through [`crate::lead_admin`], and the offering
//! routes change offerings, through [`crate::offering_admin`].
//!
//! Only a vendor admin gets in ([`VendorAdmin`]'s extractor, the first
//! extractor of every handler): no session is 401 `not signed in`; any
//! other session (an owner, a member, a `vendor_admin`-role user outside the
//! vendor organization, or one whose email is unverified) is 403
//! [`MSG_ADMIN_ONLY`]. The check runs before the path, query or body is read
//! or any data is touched, and is read live, so a change applies to the
//! next request. Each handler runs one transaction in which the data layer
//! reads or writes and records its audit row. A bad `q` is 400
//! [`admin::MSG_BAD_QUERY`], a bad lead filter 400 [`MSG_BAD_FILTER`], an
//! unknown, non-UUID or vendor organization id 404 [`MSG_NO_ORGANIZATION`]
//! and an unknown, non-UUID or erased lead id 404 [`MSG_NO_LEAD`]; none
//! writes an audit row. An invalid offering body is 400 naming the field,
//! an unknown or non-UUID offering id 404 [`MSG_NO_OFFERING`], and a
//! refused offering change (a duplicate name, a frozen term, a wrong
//! lifecycle step) 409; none writes an audit row either. Error bodies are `{"error": "<message>"}`.

use axum::async_trait;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::accounts;
use crate::admin::{
    self, AdminOrganizationDetail, AdminReadError, OrganizationSearch, UserSearch, VendorAdmin,
};
use crate::auth::{session_token, ApiError, JsonBody, MSG_NOT_SIGNED_IN};
use crate::lead_admin::{self, FilterParams, LeadDetail, LeadFilter, LeadList};
use crate::offering_admin::{self, NewOffering, OfferingError, OfferingList, OfferingRequest};
use crate::AppState;

pub const MSG_ADMIN_ONLY: &str = "only vendor admins can use the admin area";
pub const MSG_NO_ORGANIZATION: &str = "no such organization";
pub const MSG_NO_LEAD: &str = "no such sales lead";
pub const MSG_NO_OFFERING: &str = "no such offering";
pub const MSG_BAD_FILTER: &str = "segment must be one of itad, enterprise-it, government, other, \
     none; status one of new, contacted, qualified, closed; from and to dates as YYYY-MM-DD, \
     from not after to";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/organizations", get(search_organizations))
        .route("/api/admin/organizations/:id", get(organization_detail))
        .route("/api/admin/users", get(search_users))
        .route("/api/admin/leads", get(list_leads))
        .route("/api/admin/leads/export", post(export_leads))
        .route("/api/admin/leads/:id", get(lead_detail))
        .route("/api/admin/leads/:id/status", post(set_lead_status))
        .route("/api/admin/leads/:id/notes", post(add_lead_note))
        .route("/api/admin/leads/:id/erase", post(erase_lead))
        .route(
            "/api/admin/offerings",
            get(list_offerings).post(create_offering),
        )
        .route(
            "/api/admin/offerings/:id",
            get(offering_detail).post(update_offering),
        )
        .route("/api/admin/offerings/:id/publish", post(publish_offering))
        .route("/api/admin/offerings/:id/retire", post(retire_offering))
}

impl From<AdminReadError> for ApiError {
    fn from(e: AdminReadError) -> Self {
        ApiError::internal(e)
    }
}

impl From<OfferingError> for ApiError {
    fn from(e: OfferingError) -> Self {
        match e {
            OfferingError::Conflict(m) => ApiError::new(StatusCode::CONFLICT, m),
            e => ApiError::internal(e),
        }
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

// ---------------------------------------------------------------------------
// Sales leads

fn bad_filter() -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, MSG_BAD_FILTER)
}

fn no_lead() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, MSG_NO_LEAD)
}

/// The lead id from the path, or 404.
fn lead_id(id: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(id).map_err(|_| no_lead())
}

async fn list_leads(
    State(state): State<AppState>,
    admin: VendorAdmin,
    params: Result<Query<FilterParams>, QueryRejection>,
) -> Result<Json<LeadList>, ApiError> {
    let Query(params) = params.map_err(|_| bad_filter())?;
    let filter = LeadFilter::parse(&params).map_err(|_| bad_filter())?;
    let mut tx = state.pool.begin().await?;
    let found = lead_admin::list(&mut tx, &admin, &filter).await?;
    tx.commit().await?;
    Ok(Json(found))
}

/// The CSV is built from rows read in the transaction that audits the
/// export, and sent only after it commits: no export leaves unaudited. A
/// body that is not a JSON object of optional strings with known keys is
/// [`MSG_BAD_FILTER`], never an unfiltered export.
async fn export_leads(
    State(state): State<AppState>,
    admin: VendorAdmin,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Ok(Json(body @ serde_json::Value::Object(_))) = body else {
        return Err(bad_filter());
    };
    let params: FilterParams = serde_json::from_value(body).map_err(|_| bad_filter())?;
    let filter = LeadFilter::parse(&params).map_err(|_| bad_filter())?;
    let mut tx = state.pool.begin().await?;
    let export = lead_admin::export_rows(&mut tx, &admin, &filter).await?;
    tx.commit().await?;
    let disposition = format!("attachment; filename=\"sales-leads-{}.csv\"", export.date);
    Ok((
        [
            (CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (CONTENT_DISPOSITION, disposition),
            (CACHE_CONTROL, "no-store".to_string()),
        ],
        lead_admin::to_csv(&export.leads),
    )
        .into_response())
}

async fn lead_detail(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Json<LeadDetail>, ApiError> {
    let id = lead_id(&id)?;
    let mut tx = state.pool.begin().await?;
    let detail = lead_admin::detail(&mut tx, &admin, id)
        .await?
        .ok_or_else(no_lead)?;
    tx.commit().await?;
    Ok(Json(detail))
}

#[derive(Debug, Deserialize)]
struct StatusRequest {
    #[serde(default)]
    status: String,
}

async fn set_lead_status(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
    JsonBody(req): JsonBody<StatusRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = lead_id(&id)?;
    let status = lead_admin::validate_status(&req.status)
        .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, m))?;
    let mut tx = state.pool.begin().await?;
    let lead = lead_admin::set_status(&mut tx, &admin, id, status)
        .await?
        .ok_or_else(no_lead)?;
    tx.commit().await?;
    Ok(Json(json!({ "lead": lead })))
}

#[derive(Debug, Deserialize)]
struct NoteRequest {
    #[serde(default)]
    body: String,
}

async fn add_lead_note(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
    JsonBody(req): JsonBody<NoteRequest>,
) -> Result<Response, ApiError> {
    let id = lead_id(&id)?;
    let body = lead_admin::validate_note(&req.body)
        .map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, m))?;
    let mut tx = state.pool.begin().await?;
    let note = lead_admin::add_note(&mut tx, &admin, id, &body)
        .await?
        .ok_or_else(no_lead)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(json!({ "note": note }))).into_response())
}

async fn erase_lead(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = lead_id(&id)?;
    let mut tx = state.pool.begin().await?;
    lead_admin::erase(&mut tx, &admin, id)
        .await?
        .ok_or_else(no_lead)?;
    tx.commit().await?;
    Ok(Json(json!({})))
}

// ---------------------------------------------------------------------------
// Offerings

fn no_offering() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, MSG_NO_OFFERING)
}

/// The offering id from the path, or 404.
fn offering_id(id: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(id).map_err(|_| no_offering())
}

/// A validated offering body, or 400 naming the field.
fn offering_of(req: &OfferingRequest) -> Result<NewOffering, ApiError> {
    offering_admin::validate(req).map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, m))
}

async fn list_offerings(
    State(state): State<AppState>,
    admin: VendorAdmin,
) -> Result<Json<OfferingList>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let found = offering_admin::list(&mut tx, &admin).await?;
    tx.commit().await?;
    Ok(Json(found))
}

async fn create_offering(
    State(state): State<AppState>,
    admin: VendorAdmin,
    JsonBody(req): JsonBody<OfferingRequest>,
) -> Result<Response, ApiError> {
    let new = offering_of(&req)?;
    let mut tx = state.pool.begin().await?;
    let offering = offering_admin::create(&mut tx, &admin, &new).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(json!({ "offering": offering }))).into_response())
}

async fn offering_detail(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = offering_id(&id)?;
    let mut tx = state.pool.begin().await?;
    let offering = offering_admin::detail(&mut tx, &admin, id)
        .await?
        .ok_or_else(no_offering)?;
    tx.commit().await?;
    Ok(Json(json!({ "offering": offering })))
}

async fn update_offering(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
    JsonBody(req): JsonBody<OfferingRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = offering_id(&id)?;
    let new = offering_of(&req)?;
    let mut tx = state.pool.begin().await?;
    let offering = offering_admin::update(&mut tx, &admin, id, &new)
        .await?
        .ok_or_else(no_offering)?;
    tx.commit().await?;
    Ok(Json(json!({ "offering": offering })))
}

async fn publish_offering(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = offering_id(&id)?;
    let mut tx = state.pool.begin().await?;
    let offering = offering_admin::publish(&mut tx, &admin, id)
        .await?
        .ok_or_else(no_offering)?;
    tx.commit().await?;
    Ok(Json(json!({ "offering": offering })))
}

async fn retire_offering(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = offering_id(&id)?;
    let mut tx = state.pool.begin().await?;
    let offering = offering_admin::retire(&mut tx, &admin, id)
        .await?
        .ok_or_else(no_offering)?;
    tx.commit().await?;
    Ok(Json(json!({ "offering": offering })))
}
