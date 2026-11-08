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
//! - `GET /api/admin/organizations/:id/licences` — every licence of a
//!   customer organization, station and vendor-issued, newest first.
//! - `POST /api/admin/organizations/:id/licences {instance_key_id, scope,
//!   station_id?, site?, quota_erasures, features, days, reason}` — sign a
//!   licence for the organization outside any entitlement and seat (201
//!   `{licence}`); 503 [`MSG_ISSUANCE_UNAVAILABLE`] without a vendor root.
//! - `GET /api/admin/licences/:id` — `{licence, body, verification}`: the
//!   record, its stored file's decoded body, and whether that file verifies
//!   against the hub's vendor root (`no_root` when it has none).
//! - `GET /api/admin/licences/:id/download` — the stored file, byte for
//!   byte, with the customer download's headers.
//! - `POST /api/admin/licences/inspect {document}` — decode and verify a
//!   pasted licence file: `{body, verification, licence}` (the hub's record
//!   of it or `null`); 400 [`licence_admin::MSG_NOT_A_LICENCE`].
//! - `POST /api/admin/licences/:id/revoke {reason}` — mark a licence
//!   revoked: `{licence}`; 409 [`stations::MSG_ALREADY_REVOKED`] the second
//!   time. Recorded in the hub only: stations are not told.
//! - `GET /api/admin/releases` — `{releases}`: every release, newest first,
//!   every status, each with its artifacts.
//! - `POST /api/admin/releases {version, notes, channel}` — create a draft
//!   release (201 `{release}`); 409 [`release_admin::MSG_VERSION_TAKEN`].
//! - `GET /api/admin/releases/:id` — `{release}`.
//! - `GET /api/admin/releases/:id/downloads` — `{downloads}`: every recorded
//!   download of the release's files (user email, organization, file),
//!   newest first, whatever the release's status.
//! - `POST /api/admin/releases/:id/artifacts?platform=&filename=` — the raw
//!   request body (`application/octet-stream`, else 415) is one file, stored
//!   and hashed by the hub (201 `{artifact}`); over the limit 413
//!   [`release_admin::MSG_ARTIFACT_TOO_LARGE`], empty 400, a platform
//!   already there or a release not in draft 409, a store failure 503
//!   [`release_admin::MSG_STORAGE_UNAVAILABLE`].
//! - `POST /api/admin/releases/:id/artifacts/:artifact_id/remove` — remove
//!   a draft's file (`{release}`); 404 [`release_admin::MSG_NO_ARTIFACT`].
//! - `POST /api/admin/releases/:id/publish` — draft (with a file) to
//!   published (`{release}`).
//! - `POST /api/admin/releases/:id/withdraw` — draft or published to
//!   withdrawn, for good (`{release}`); files are kept.
//!
//! The organization and user routes are read-only; the lead routes also
//! change sales leads, through [`crate::lead_admin`], the offering
//! routes change offerings, through [`crate::offering_admin`], and the
//! licence routes issue and revoke licences through [`crate::stations`]
//! and read them through [`crate::licence_admin`]. A non-UUID or unknown
//! licence id is 404 [`MSG_NO_LICENCE`], writing nothing. The release
//! routes go through [`crate::release_admin`] and the hub's
//! [`crate::artifacts::ArtifactStore`]; a non-UUID or unknown release id is
//! 404 [`release_admin::MSG_NO_RELEASE`], an invalid body or upload query
//! 400 naming the field, and a refused change 409, none writing anything.
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
use axum::body::Body;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
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
use crate::artifacts::StoreError;
use crate::auth::{session_token, ApiError, JsonBody, MSG_NOT_SIGNED_IN};
use crate::lead_admin::{self, FilterParams, LeadDetail, LeadFilter, LeadList};
use crate::licence_admin::{self, InspectRequest, IssueRequest, RevokeRequest, MSG_NOT_A_LICENCE};
use crate::offering_admin::{self, NewOffering, OfferingError, OfferingList, OfferingRequest};
use crate::release_admin::{self, ReleaseDownloads, ReleaseError, ReleaseList, ReleaseRequest};
use crate::station_routes::licence_file_response;
use crate::stations::{self, MSG_ISSUANCE_UNAVAILABLE, MSG_NO_LICENCE};
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
        .route(
            "/api/admin/organizations/:id/licences",
            get(organization_licences).post(issue_licence),
        )
        .route("/api/admin/licences/inspect", post(inspect_licence))
        .route("/api/admin/licences/:id", get(licence_detail))
        .route("/api/admin/licences/:id/download", get(download_licence))
        .route("/api/admin/licences/:id/revoke", post(revoke_licence))
        .route(
            "/api/admin/releases",
            get(list_releases).post(create_release),
        )
        .route("/api/admin/releases/:id", get(release_detail))
        .route("/api/admin/releases/:id/downloads", get(release_downloads))
        .route("/api/admin/releases/:id/artifacts", post(upload_artifact))
        .route(
            "/api/admin/releases/:id/artifacts/:artifact_id/remove",
            post(remove_artifact),
        )
        .route("/api/admin/releases/:id/publish", post(publish_release))
        .route("/api/admin/releases/:id/withdraw", post(withdraw_release))
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

impl From<ReleaseError> for ApiError {
    fn from(e: ReleaseError) -> Self {
        match e {
            ReleaseError::NotFound(m) => ApiError::new(StatusCode::NOT_FOUND, m),
            ReleaseError::Conflict(m) => ApiError::new(StatusCode::CONFLICT, m),
            ReleaseError::Store(StoreError::TooLarge(_)) => ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                release_admin::MSG_ARTIFACT_TOO_LARGE,
            ),
            ReleaseError::Store(StoreError::Empty) => {
                ApiError::new(StatusCode::BAD_REQUEST, release_admin::MSG_EMPTY_ARTIFACT)
            }
            ReleaseError::Store(StoreError::Source(cause)) => {
                tracing::warn!(%cause, "an artifact upload was interrupted");
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    release_admin::MSG_UPLOAD_INTERRUPTED,
                )
            }
            ReleaseError::Store(cause) => {
                // Disk full, permissions: logged for the operator, never shown.
                tracing::error!(error = %cause, "artifact storage failed");
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    release_admin::MSG_STORAGE_UNAVAILABLE,
                )
            }
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

// ---------------------------------------------------------------------------
// Licences

fn no_organization() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, MSG_NO_ORGANIZATION)
}

fn no_licence() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, MSG_NO_LICENCE)
}

/// The licence id from the path, or 404.
fn licence_id(id: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(id).map_err(|_| no_licence())
}

fn bad_request(m: String) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, m)
}

async fn organization_licences(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = Uuid::parse_str(&id).map_err(|_| no_organization())?;
    let mut tx = state.pool.begin().await?;
    let licences = licence_admin::list_for_organization(&mut tx, &admin, id)
        .await?
        .ok_or_else(no_organization)?;
    tx.commit().await?;
    Ok(Json(json!({ "licences": licences })))
}

async fn issue_licence(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
    body: Result<JsonBody<IssueRequest>, ApiError>,
) -> Result<Response, ApiError> {
    let Some(vendor) = state.config.vendor_root.as_ref() else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            MSG_ISSUANCE_UNAVAILABLE,
        ));
    };
    let id = Uuid::parse_str(&id).map_err(|_| no_organization())?;
    let JsonBody(req) = body?;
    let new = req.validate().map_err(bad_request)?;
    let mut tx = state.pool.begin().await?;
    let licence_id = stations::vendor_issue(&mut tx, &vendor.root, &admin, id, &new).await?;
    let licence = licence_admin::get(&mut tx, licence_id)
        .await?
        .ok_or_else(|| ApiError::internal("an issued licence vanished"))?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(json!({ "licence": licence }))).into_response())
}

async fn licence_detail(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Json<licence_admin::LicenceDetail>, ApiError> {
    let id = licence_id(&id)?;
    let root = state.config.vendor_root.as_ref().map(|v| &v.root);
    let mut tx = state.pool.begin().await?;
    let detail = licence_admin::detail(&mut tx, &admin, root, id)
        .await?
        .ok_or_else(no_licence)?;
    tx.commit().await?;
    Ok(Json(detail))
}

async fn download_licence(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = licence_id(&id)?;
    let mut tx = state.pool.begin().await?;
    let document = licence_admin::document(&mut tx, &admin, id)
        .await?
        .ok_or_else(no_licence)?;
    tx.commit().await?;
    licence_file_response(id, document)
}

/// Any body that is not `{document: "<text>"}` is [`MSG_NOT_A_LICENCE`].
async fn inspect_licence(
    State(state): State<AppState>,
    admin: VendorAdmin,
    body: Result<JsonBody<InspectRequest>, ApiError>,
) -> Result<Json<licence_admin::Inspected>, ApiError> {
    let JsonBody(req) =
        body.map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, MSG_NOT_A_LICENCE))?;
    let root = state.config.vendor_root.as_ref().map(|v| &v.root);
    let mut tx = state.pool.begin().await?;
    let inspected = licence_admin::inspect_document(&mut tx, &admin, root, &req.document).await?;
    tx.commit().await?;
    Ok(Json(inspected))
}

async fn revoke_licence(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
    body: Result<JsonBody<RevokeRequest>, ApiError>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = licence_id(&id)?;
    let JsonBody(req) = body?;
    let reason = licence_admin::validate_reason("reason", &req.reason).map_err(bad_request)?;
    let mut tx = state.pool.begin().await?;
    stations::revoke(&mut tx, &admin, id, &reason).await?;
    let licence = licence_admin::get(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::internal("a revoked licence vanished"))?;
    tx.commit().await?;
    Ok(Json(json!({ "licence": licence })))
}

// ---------------------------------------------------------------------------
// Releases

fn no_release() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, release_admin::MSG_NO_RELEASE)
}

/// The release id from the path, or 404.
fn release_id(id: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(id).map_err(|_| no_release())
}

async fn list_releases(
    State(state): State<AppState>,
    admin: VendorAdmin,
) -> Result<Json<ReleaseList>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let found = release_admin::list(&mut tx, &admin).await?;
    tx.commit().await?;
    Ok(Json(found))
}

async fn create_release(
    State(state): State<AppState>,
    admin: VendorAdmin,
    JsonBody(req): JsonBody<ReleaseRequest>,
) -> Result<Response, ApiError> {
    let new = release_admin::validate(&req).map_err(bad_request)?;
    let mut tx = state.pool.begin().await?;
    let release = release_admin::create(&mut tx, &admin, &new).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(json!({ "release": release }))).into_response())
}

async fn release_detail(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = release_id(&id)?;
    let mut tx = state.pool.begin().await?;
    let release = release_admin::detail(&mut tx, &admin, id).await?;
    tx.commit().await?;
    Ok(Json(json!({ "release": release })))
}

async fn release_downloads(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Json<ReleaseDownloads>, ApiError> {
    let id = release_id(&id)?;
    let mut tx = state.pool.begin().await?;
    let found = release_admin::downloads(&mut tx, &admin, id).await?;
    tx.commit().await?;
    Ok(Json(found))
}

/// The upload's query string. Unknown keys are refused.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadParams {
    platform: Option<String>,
    filename: Option<String>,
}

const MSG_BAD_UPLOAD_QUERY: &str =
    "the upload takes exactly two query parameters: platform and filename";
const MSG_NOT_OCTET_STREAM: &str = "the upload body must be the raw file, sent as \
     Content-Type: application/octet-stream";

/// Whether the request's content type is `application/octet-stream`
/// (parameters allowed).
fn is_octet_stream(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/octet-stream"))
}

/// The raw body is the file: streamed to the store (never buffered whole),
/// hashed and counted on the way, then recorded. The body is the last
/// extractor and is read only after the admin, path, query and content
/// type checks pass.
async fn upload_artifact(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
    params: Result<Query<UploadParams>, QueryRejection>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let id = release_id(&id)?;
    let Query(params) =
        params.map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, MSG_BAD_UPLOAD_QUERY))?;
    let platform = release_admin::validate_platform(params.platform.as_deref().unwrap_or(""))
        .map_err(bad_request)?;
    let filename = release_admin::validate_filename(params.filename.as_deref().unwrap_or(""))
        .map_err(bad_request)?;
    if !is_octet_stream(&headers) {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            MSG_NOT_OCTET_STREAM,
        ));
    }
    // A declared length over the limit is refused before a byte is read;
    // the limit is enforced on the bytes themselves either way.
    let declared = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > state.config.artifact_max_bytes) {
        return Err(
            ReleaseError::Store(StoreError::TooLarge(state.config.artifact_max_bytes)).into(),
        );
    }
    use futures::TryStreamExt;
    let reader =
        tokio_util::io::StreamReader::new(body.into_data_stream().map_err(std::io::Error::other));
    let artifact = release_admin::upload(
        &state.pool,
        state.artifacts.as_ref(),
        state.config.artifact_max_bytes,
        &admin,
        id,
        &platform,
        &filename,
        reader,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "artifact": artifact }))).into_response())
}

async fn remove_artifact(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path((id, artifact_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = release_id(&id)?;
    let artifact_id = Uuid::parse_str(&artifact_id)
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, release_admin::MSG_NO_ARTIFACT))?;
    let release = release_admin::remove_artifact(
        &state.pool,
        state.artifacts.as_ref(),
        &admin,
        id,
        artifact_id,
    )
    .await?;
    Ok(Json(json!({ "release": release })))
}

async fn publish_release(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = release_id(&id)?;
    let mut tx = state.pool.begin().await?;
    let release = release_admin::publish(&mut tx, &admin, id).await?;
    tx.commit().await?;
    Ok(Json(json!({ "release": release })))
}

async fn withdraw_release(
    State(state): State<AppState>,
    admin: VendorAdmin,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = release_id(&id)?;
    let mut tx = state.pool.begin().await?;
    let release = release_admin::withdraw(&mut tx, &admin, id).await?;
    tx.commit().await?;
    Ok(Json(json!({ "release": release })))
}
