//! Station and licence routes (licence-delivery stories 1 and 2, CAP-1, CAP-2,
//! CAP-3, CAP-5):
//!
//! - `GET /api/stations` — any role: `{stations}`, the organization's
//!   stations (released ones last) with each one's latest licence and
//!   `renewal_available`.
//! - `POST /api/stations {entitlement_id, instance_key_id, station_id?,
//!   label?, site?}` — an owner registers a station: 201 `{station,
//!   licence}`. 503 [`MSG_ISSUANCE_UNAVAILABLE`] when the hub has no vendor
//!   root; the other refusals are [`stations::register`]'s.
//! - `POST /api/stations/:id/release` — an owner: 200 `{station}`, its seat
//!   free and its licence superseded; 409
//!   [`stations::MSG_ALREADY_RELEASED`] the
//!   second time.
//! - `POST /api/stations/:id/reissue` — an owner issues a station whose
//!   `renewal_available` is true a renewed licence to its entitlement's
//!   current `term_end`: 201 `{station, licence}`, the old licence superseded
//!   (its file still verifies offline until its own `not_after`). 503
//!   [`MSG_ISSUANCE_UNAVAILABLE`] when the hub has no vendor root; the other
//!   refusals are [`stations::reissue`]'s.
//! - `GET /api/licences/:id/download` — any role: the stored licence file,
//!   byte for byte, as `application/json` with `Content-Disposition:
//!   attachment`; audited. A revoked licence is 409
//!   [`stations::MSG_LICENCE_REVOKED`] and a vendor-issued one 404: those
//!   reach a customer only through the vendor.
//! - `POST /api/stations/:id/reissue` refuses a station whose latest
//!   licence is revoked (409 [`stations::MSG_LICENCE_REVOKED`]).
//! - `GET /api/licensing/root` — any role: `{root: {public_key_id,
//!   public_key_b64} | null}`, the vendor root public key a station passes as
//!   `--vendor-root-b64` and an auditor verifies against.
//!
//! Every route needs a session, and the organization is always the
//! session's: another organization's station, licence or entitlement is a 404
//! like an unknown one. Owner checks run before the body is read.

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use uuid::Uuid;

use crate::accounts::Role;
use crate::auth::{ApiError, CurrentUser, JsonBody};
use crate::stations::{
    self, RegisterRequest, StationError, MSG_ISSUANCE_UNAVAILABLE, MSG_NO_LICENCE, MSG_NO_STATION,
    MSG_OWNER_ONLY_STATIONS,
};
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/stations", get(list).post(register))
        .route("/api/stations/:id/release", post(release))
        .route("/api/stations/:id/reissue", post(reissue))
        .route("/api/licences/:id/download", get(download))
        .route("/api/licensing/root", get(root))
}

impl From<StationError> for ApiError {
    fn from(e: StationError) -> Self {
        match e {
            StationError::BadRequest(m) => ApiError::new(StatusCode::BAD_REQUEST, m),
            StationError::NotFound(m) => ApiError::new(StatusCode::NOT_FOUND, m),
            StationError::Conflict(m) => ApiError::new(StatusCode::CONFLICT, m),
            StationError::Issue(m) => ApiError::internal(m),
            StationError::Db(e) => e.into(),
            StationError::Audit(e) => e.into(),
        }
    }
}

fn require_owner(user: &CurrentUser) -> Result<(), ApiError> {
    if user.role == Role::Owner {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            MSG_OWNER_ONLY_STATIONS,
        ))
    }
}

async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stations = stations::list(&state.pool, user.organization_id).await?;
    Ok(Json(json!({ "stations": stations })))
}

async fn register(
    State(state): State<AppState>,
    user: CurrentUser,
    body: Result<JsonBody<RegisterRequest>, ApiError>,
) -> Result<Response, ApiError> {
    require_owner(&user)?;
    let Some(vendor) = state.config.vendor_root.as_ref() else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            MSG_ISSUANCE_UNAVAILABLE,
        ));
    };
    let JsonBody(req) = body?;
    let new = req.validate()?;
    let mut tx = state.pool.begin().await?;
    let registered = stations::register(
        &mut tx,
        &vendor.root,
        user.organization_id,
        user.user_id,
        &new,
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "station": registered.station,
            "licence": registered.licence,
        })),
    )
        .into_response())
}

async fn release(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_owner(&user)?;
    let id =
        Uuid::parse_str(&id).map_err(|_| ApiError::new(StatusCode::NOT_FOUND, MSG_NO_STATION))?;
    let mut tx = state.pool.begin().await?;
    let station = stations::release(&mut tx, user.organization_id, user.user_id, id).await?;
    tx.commit().await?;
    Ok(Json(json!({ "station": station })))
}

async fn reissue(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    require_owner(&user)?;
    let Some(vendor) = state.config.vendor_root.as_ref() else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            MSG_ISSUANCE_UNAVAILABLE,
        ));
    };
    let id =
        Uuid::parse_str(&id).map_err(|_| ApiError::new(StatusCode::NOT_FOUND, MSG_NO_STATION))?;
    let mut tx = state.pool.begin().await?;
    let reissued = stations::reissue(
        &mut tx,
        &vendor.root,
        user.organization_id,
        user.user_id,
        id,
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "station": reissued.station,
            "licence": reissued.licence,
        })),
    )
        .into_response())
}

async fn download(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, MSG_NO_LICENCE);
    let id = Uuid::parse_str(&id).map_err(|_| not_found())?;
    let mut tx = state.pool.begin().await?;
    let document = stations::licence_document(&mut tx, user.organization_id, user.user_id, id)
        .await?
        .ok_or_else(not_found)?;
    tx.commit().await?;
    licence_file_response(id, document)
}

/// Licence `id`'s stored file as a download: `application/json`,
/// `Content-Disposition: attachment`, `Cache-Control: no-store`. The
/// customer and the admin download share it.
pub(crate) fn licence_file_response(id: Uuid, document: String) -> Result<Response, ApiError> {
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"licence-{id}.json\""))
        .map_err(ApiError::internal)?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            ),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        document,
    )
        .into_response())
}

async fn root(State(state): State<AppState>, _user: CurrentUser) -> Json<serde_json::Value> {
    let root = state.config.vendor_root.as_ref().map(|r| {
        json!({
            "public_key_id": r.root.public_key_id(),
            "public_key_b64": r.root.verifying_key().to_base64(),
        })
    });
    Json(json!({ "root": root }))
}
