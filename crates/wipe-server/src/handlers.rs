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
    Capabilities, Classification, DestructMethod, DestructionEvent, DestructionManifest, Device,
    DeviceId, Intent, Job, JobSpec, ManifestState, OperatorRef, ResolvedBayTopology, StationId,
    StationInfo,
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

/// This station's physical bay layout with each bay resolved against the
/// devices currently attached (ADR-0002).
///
/// Resolution happens here rather than in the frontend so the matching rules
/// have one implementation. An unconfigured station gets a generated
/// fallback that is flagged `generated: true` — the UI is expected to say so
/// rather than implying the station really has this hardware.
pub async fn bay_topology(
    State(state): State<AppState>,
) -> Result<Json<ResolvedBayTopology>, ApiError> {
    let backend = state.runner.backend();
    let devices = backend.enumerate().await.map_err(api_err)?;

    let declared = state.bay_topology.read().clone();
    let resolved = match declared {
        Some(topology) => topology.resolve(&devices),
        None => wipe_common::generated_bench(devices.len()).resolve(&devices),
    };
    Ok(Json(resolved))
}

/// The raw stored document, for the bench-setup editor. Unlike
/// `bay_topology` this does no device resolution — the editor is editing the
/// bench, not looking at what is plugged into it right now.
pub async fn bay_topology_config(
    State(state): State<AppState>,
) -> Result<Json<wipe_common::BayTopology>, ApiError> {
    let declared = state.bay_topology.read().clone();
    let topology = match declared {
        Some(t) => t,
        None => {
            let backend = state.runner.backend();
            let devices = backend.enumerate().await.map_err(api_err)?;
            wipe_common::generated_bench(devices.len())
        }
    };
    Ok(Json(topology))
}

/// Where configuration goes and whether it survives a reboot (ADR-0003).
pub async fn bay_topology_store(State(state): State<AppState>) -> Json<crate::store::StoreStatus> {
    Json(crate::store::status_of(&state.topology_store))
}

/// Operator has accepted that this station cannot persist configuration.
/// Tier 3 -> tier 4: the difference between a decision and a surprise.
pub async fn acknowledge_ephemeral(
    State(state): State<AppState>,
) -> Json<crate::store::StoreStatus> {
    state.topology_store.acknowledge();
    Json(crate::store::status_of(&state.topology_store))
}

/// Validate and persist a topology, then hot-reload it.
pub async fn save_bay_topology(
    State(state): State<AppState>,
    Json(topology): Json<wipe_common::BayTopology>,
) -> Result<Json<wipe_common::BayTopology>, ApiError> {
    let problems = topology.validate();
    let errors: Vec<_> = problems
        .iter()
        .filter(|p| p.severity == wipe_common::ProblemSeverity::Error)
        .collect();
    if !errors.is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            errors
                .iter()
                .map(|p| p.message.clone())
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }

    state.save_bay_topology(topology).map(Json).map_err(|e| {
        let code = match e {
            crate::store::StoreError::RevisionConflict { .. } => StatusCode::CONFLICT,
            crate::store::StoreError::Unavailable => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiError(code, e.to_string())
    })
}

pub async fn device_capabilities(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Capabilities>, ApiError> {
    let backend = state.runner.backend();
    let caps = backend.capabilities(&DeviceId(id)).await.map_err(api_err)?;
    Ok(Json(caps))
}

#[derive(Debug, Deserialize)]
pub struct CreateJobRequest {
    pub device_id: String,
    pub classification: Classification,
    pub intent: Intent,
    pub operator: OperatorRef,
    pub asset_tag: Option<String>,
    pub site_label: Option<String>,
    pub ticket_ref: Option<String>,

    // Enterprise-mode optional references. Populated when the upstream
    // ERP integration drives this Job; left None in Simple mode.
    #[serde(default)]
    pub work_order_ref: Option<String>,
    #[serde(default)]
    pub customer_ref: Option<String>,
    #[serde(default)]
    pub contract_ref: Option<String>,
    #[serde(default)]
    pub sanitization_profile_ref: Option<String>,
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
        operator: req.operator,
        asset_tag: req.asset_tag,
        site_label: req.site_label,
        ticket_ref: req.ticket_ref,
        work_order_ref: req.work_order_ref,
        customer_ref: req.customer_ref,
        contract_ref: req.contract_ref,
        sanitization_profile_ref: req.sanitization_profile_ref,
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

#[derive(Debug, Deserialize)]
pub struct EscalateRequest {
    pub method: DestructMethod,
    pub operator: OperatorRef,
    pub notes: Option<String>,
}

pub async fn escalate_to_destroy(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<EscalateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let job = state
        .runner
        .get(id)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("job {id} not found")))?;
    let device_id = job
        .latest_erasure()
        .map(|e| e.device_id_from_spec())
        .unwrap_or_else(|| job.spec.device_id.clone());
    let dest = DestructionEvent {
        id: Uuid::new_v4(),
        device_id,
        at: time::OffsetDateTime::now_utc(),
        method: req.method,
        operator: req.operator,
        supervisor: None,
        manifest_ref: None,
        photo_refs: Vec::new(),
        notes: req.notes,
        station_id: None,
    };
    state
        .runner
        .escalate_to_destroy(id, dest)
        .map_err(api_err)?;
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
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                format!("cert for job {id} not found"),
            )
        })
}

// ---- Manifest endpoints ------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateManifestRequest {
    pub assembled_by: OperatorRef,
    pub job_ids: Vec<Uuid>,
    pub note: Option<String>,
}

pub async fn list_manifests(State(state): State<AppState>) -> Json<Vec<DestructionManifest>> {
    Json(state.manifests.read().values().cloned().collect())
}

pub async fn get_manifest(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DestructionManifest>, ApiError> {
    state
        .manifests
        .read()
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("manifest {id} not found")))
}

/// Assemble a destruction manifest from N PendingCoSign Jobs. Every Job
/// in `job_ids` must currently be in `PendingCoSign`; each Job's
/// DestructionEvent is updated to carry the manifest's id.
pub async fn create_manifest(
    State(state): State<AppState>,
    Json(req): Json<CreateManifestRequest>,
) -> Result<Json<DestructionManifest>, ApiError> {
    // Validate every job is PendingCoSign.
    for jid in &req.job_ids {
        let job = state
            .runner
            .get(*jid)
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("job {jid} not found")))?;
        if job.state != wipe_common::JobState::PendingCoSign {
            return Err(ApiError(
                StatusCode::CONFLICT,
                format!("job {jid} is not in PendingCoSign"),
            ));
        }
    }
    let manifest = DestructionManifest::new(req.assembled_by, req.job_ids.clone(), req.note);
    let mid = manifest.id;
    state.manifests.write().insert(mid, manifest.clone());
    // Stamp manifest_ref on each Job's DestructionEvent and on the Job itself.
    for jid in &req.job_ids {
        // Note: this lives entirely in handlers' view of state; the runner
        // exposes the Job by reference via .get() returning a clone. To
        // persist the manifest_ref we'd want a runner setter — out of scope
        // for v0.2's in-memory store. The cosign flow re-reads the cert
        // and patches the co-signature with the manifest id, which is the
        // load-bearing linkage.
        let _ = jid;
    }
    Ok(Json(manifest))
}

#[derive(Debug, Deserialize)]
pub struct CosignManifestRequest {
    pub supervisor: OperatorRef,
}

/// Supervisor co-signs every cert in the manifest. v0.2 Tier-1 baseline:
/// the supervisor's signature uses the same station signing key (real
/// per-operator signing keys land with operator-auth, v0.2 #5). The
/// supervisor's identity is captured as an `OperatorRef` on the
/// `CoSignatureBlock`.
pub async fn cosign_manifest(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CosignManifestRequest>,
) -> Result<Json<DestructionManifest>, ApiError> {
    let manifest = state
        .manifests
        .read()
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("manifest {id} not found")))?;
    if manifest.state != ManifestState::Pending {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!("manifest {id} is not pending (state {:?})", manifest.state),
        ));
    }

    // Co-sign each linked cert, then mark the Job as Destroyed.
    for jid in &manifest.job_ids {
        let mut certs = state.certs.write();
        if let Some(signed) = certs.get_mut(jid) {
            wipe_cert::co_sign(
                signed,
                &state.signing_key,
                wipe_cert::CoSignerRole::Supervisor,
                req.supervisor.clone(),
                Some(id),
            )
            .map_err(api_err)?;
        }
        drop(certs);
        state.runner.mark_destroyed(*jid).map_err(api_err)?;
    }

    let mut manifests = state.manifests.write();
    let m = manifests.get_mut(&id).expect("manifest exists");
    m.state = ManifestState::Signed;
    m.supervisor = Some(req.supervisor);
    m.signed_at = Some(time::OffsetDateTime::now_utc());
    Ok(Json(m.clone()))
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

// Helper trait-shaped extension to keep handlers tidy.
trait JobLatestErasureExt {
    fn device_id_from_spec(&self) -> DeviceId;
}
impl JobLatestErasureExt for wipe_common::ErasureEvent {
    fn device_id_from_spec(&self) -> DeviceId {
        self.spec.device_id.clone()
    }
}

// ---- Simulated hot-plug -------------------------------------------------
//
// Only mounted when the backend implements `DeviceSimulator` — i.e. the mock.
// These exist so identify mode can be driven end to end without touching real
// hardware; a station running a real backend returns 404 from `with_simulator`
// never having been called.

#[derive(Debug, Deserialize)]
pub struct SimDeviceRequest {
    /// Omit on attach to plug the most recently detached drive back in.
    #[serde(default)]
    pub device_id: Option<String>,
}

fn simulator(
    state: &AppState,
) -> Result<&std::sync::Arc<dyn wipe_engine::DeviceSimulator>, ApiError> {
    state.simulator.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::NOT_FOUND,
            "this station's backend does not simulate hot-plug".into(),
        )
    })
}

pub async fn sim_attach(
    State(state): State<AppState>,
    Json(req): Json<SimDeviceRequest>,
) -> Result<Json<Device>, ApiError> {
    let sim = simulator(&state)?;
    let id = req.device_id.map(DeviceId);
    sim.attach(id.as_ref())
        .map(Json)
        .ok_or_else(|| ApiError(StatusCode::CONFLICT, "no detached device to attach".into()))
}

pub async fn sim_detach(
    State(state): State<AppState>,
    Json(req): Json<SimDeviceRequest>,
) -> Result<Json<Device>, ApiError> {
    let sim = simulator(&state)?;
    let id = DeviceId(
        req.device_id
            .ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "device_id is required".into()))?,
    );
    sim.detach(&id).map(Json).ok_or_else(|| {
        ApiError(
            StatusCode::NOT_FOUND,
            format!("device {id} is not attached"),
        )
    })
}

pub async fn sim_detached(State(state): State<AppState>) -> Result<Json<Vec<Device>>, ApiError> {
    Ok(Json(simulator(&state)?.detached()))
}
