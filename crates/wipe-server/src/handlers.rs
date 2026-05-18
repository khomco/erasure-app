use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use wipe_common::{
    Capabilities, Classification, Device, DeviceId, Intent, Job, JobSpec, Method, OperatorRef,
    StationId, StationInfo,
};
use crate::app::AppState;

pub async fn health() -> Json<serde_json::Value> {
    Json(json!({"ok": true, "tool": "wipestation"}))
}

pub async fn public_key(State(state): State<AppState>) -> Json<serde_json::Value> {
    let vk = state.signing_key.verifying_key();
    Json(json!({
        "public_key_id": vk.public_key_id(),
        "public_key_b64": vk.to_base64(),
        "algorithm": "ed25519"
    }))
}

pub async fn station_info(State(state): State<AppState>) -> Json<StationInfo> {
    let info = match &state.fleet {
        Some(f) => f.self_info(),
        None => placeholder_station_info(),
    };
    Json(info)
}

fn placeholder_station_info() -> StationInfo {
    use wipe_common::StationRole;
    StationInfo {
        id: StationId("standalone".into()),
        hostname: "localhost".into(),
        role: StationRole::Member,
        version: env!("CARGO_PKG_VERSION").into(),
        api_port: 0,
        started_at: time::OffsetDateTime::now_utc(),
        active_jobs: 0,
        last_seen: None,
    }
}

pub async fn list_peers(State(state): State<AppState>) -> Json<Vec<StationInfo>> {
    let peers = match &state.fleet {
        Some(f) => f.peers(),
        None => Vec::new(),
    };
    Json(peers)
}

pub async fn current_lead(State(state): State<AppState>) -> Json<serde_json::Value> {
    let lead = state.fleet.as_ref().and_then(|f| f.current_lead());
    let is_lead = state.fleet.as_ref().map(|f| f.is_lead()).unwrap_or(true);
    Json(json!({"lead": lead, "is_lead": is_lead}))
}

pub async fn list_devices(State(state): State<AppState>) -> Result<Json<Vec<Device>>, ApiError> {
    let backend = state.runner.backend();
    let devices = backend.enumerate().await.map_err(api_err)?;
    Ok(Json(devices))
}

pub async fn device_capabilities(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Capabilities>, ApiError> {
    let backend = state.runner.backend();
    let caps = backend
        .capabilities(&DeviceId(id))
        .await
        .map_err(api_err)?;
    Ok(Json(caps))
}

#[derive(Debug, Deserialize)]
pub struct CreateJobRequest {
    pub device_id: String,
    pub classification: Classification,
    pub intent: Intent,
    pub method: Option<Method>,
    #[serde(default = "default_true")]
    pub verify: bool,
    #[serde(default = "default_samples")]
    pub verify_samples: u32,
    pub operator: OperatorRef,
    pub asset_tag: Option<String>,
    pub site_label: Option<String>,
    pub ticket_ref: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_samples() -> u32 {
    8
}

#[derive(Debug, Serialize)]
pub struct CreateJobResponse {
    pub job_id: Uuid,
}

pub async fn create_job(
    State(state): State<AppState>,
    Json(req): Json<CreateJobRequest>,
) -> Result<Json<CreateJobResponse>, ApiError> {
    let spec = JobSpec {
        device_id: DeviceId(req.device_id),
        classification: req.classification,
        intent: req.intent,
        method: req.method,
        verify: req.verify,
        verify_samples: req.verify_samples,
        operator: req.operator,
        asset_tag: req.asset_tag,
        site_label: req.site_label,
        ticket_ref: req.ticket_ref,
    };
    let job_id = state.runner.create_job(spec).await.map_err(api_err)?;
    Ok(Json(CreateJobResponse { job_id }))
}

pub async fn list_jobs(State(state): State<AppState>) -> Json<Vec<Job>> {
    Json(state.runner.list())
}

pub async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Job>, ApiError> {
    state
        .runner
        .get(id)
        .map(Json)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("job {id} not found")))
}

pub async fn start_job(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.runner.start(id).map_err(api_err)?;
    Ok(Json(json!({"ok": true, "job_id": id})))
}

pub async fn abort_job(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.runner.abort(id).await.map_err(api_err)?;
    Ok(Json(json!({"ok": true, "job_id": id})))
}

pub async fn get_certificate(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<wipe_cert::SignedCertificate>, ApiError> {
    state
        .certs
        .read()
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("cert for job {id} not found")))
}

// ---- Error type ---------------------------------------------------------

pub struct ApiError(pub StatusCode, pub String);

fn api_err<E: std::fmt::Display>(e: E) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}
