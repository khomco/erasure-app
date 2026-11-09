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
    attestation_statement, AttestationSigner, Capabilities, Classification, DestructMethod,
    DestructionAttestation, DestructionEvent, DestructionManifest, DestructionRecordError, Device,
    DeviceId, IdentifierSource, Intent, Job, JobSpec, ManifestState, OperatorRef,
    RecordedIdentifiers, ResolvedBayTopology, StationId, StationInfo, WipeError,
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

/// Resolve an *unsaved* topology against the devices attached right now.
///
/// The bench-setup preview needs live occupancy for a draft that has not been
/// saved, and resolution must not be re-implemented in TypeScript — the
/// matching rules have one home (ADR-0002). This is that same `resolve`,
/// exposed as a dry run: nothing is persisted and nothing is hot-reloaded.
pub async fn resolve_bay_topology(
    State(state): State<AppState>,
    Json(topology): Json<wipe_common::BayTopology>,
) -> Result<Json<ResolvedBayTopology>, ApiError> {
    let backend = state.runner.backend();
    let devices = backend.enumerate().await.map_err(api_err)?;
    Ok(Json(topology.resolve(&devices)))
}

/// Where configuration goes and whether it survives a reboot (ADR-0003).
pub async fn bay_topology_store(State(state): State<AppState>) -> Json<crate::store::StoreStatus> {
    Json(crate::store::status_of(&state.topology_store))
}

/// Enclosure models this station knows about (ADR-0004).
///
/// Served rather than bundled into the frontend so a site-local overlay takes
/// effect without rebuilding the UI, and so the builder and the bay map can
/// never disagree about what a `model_ref` means.
pub async fn enclosure_catalog(State(state): State<AppState>) -> Json<wipe_common::Catalog> {
    Json((*state.catalog).clone())
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
    state.runner.start(id).map_err(runner_err)?;
    Ok(Json(json!({"ok": true, "job_id": id})))
}

pub async fn abort_job(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.runner.abort(id).await.map_err(runner_err)?;
    Ok(Json(json!({"ok": true, "job_id": id})))
}

// ---- Record for destruction (ADR-0011 §1, first step) ------------------

/// Identifiers the operator scanned (bar code or typed) from a drive.
#[derive(Debug, Default, Deserialize)]
pub struct ScannedIdentifiers {
    #[serde(default)]
    pub asset_tag: Option<String>,
    #[serde(default)]
    pub serial: Option<String>,
}

/// What the operator attests to when recording for destruction. Text
/// fields default to empty so a missing one is refused by the
/// `DestructionAttestation` constructors with the field's name.
#[derive(Debug, Deserialize)]
pub struct AttestationFields {
    pub method: DestructMethod,
    #[serde(default)]
    pub equipment_id: String,
    #[serde(default)]
    pub location: String,
    pub operator: OperatorRef,
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub title: String,
    /// The operator ticked "I attest" against the displayed statement.
    #[serde(default)]
    pub attest: bool,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RecordForDestructionRequest {
    #[serde(flatten)]
    pub fields: AttestationFields,
    /// Present when the operator scanned the identifiers; the record's
    /// source is then `scanned`. Absent, the server reads them from the
    /// device (`source: device`).
    #[serde(default)]
    pub identifiers: Option<ScannedIdentifiers>,
}

#[derive(Debug, Deserialize)]
pub struct RecordScannedRequest {
    #[serde(flatten)]
    pub fields: AttestationFields,
    pub classification: Classification,
    #[serde(default)]
    pub drives: Vec<ScannedIdentifiers>,
}

#[derive(Debug, Serialize)]
pub struct RecordScannedResponse {
    pub job_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct StatementPreviewRequest {
    pub method: DestructMethod,
    #[serde(default)]
    pub equipment_id: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub title: String,
}

fn unprocessable(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, msg.into())
}

fn record_err(e: DestructionRecordError) -> ApiError {
    unprocessable(e.to_string())
}

fn require_attest(fields: &AttestationFields) -> Result<(), ApiError> {
    if fields.attest {
        Ok(())
    } else {
        Err(unprocessable(
            "the operator must confirm \"I attest\" against the statement; nothing was recorded",
        ))
    }
}

fn nonblank(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// Build the attested `DestructionEvent` through the story-1 constructors,
/// so blank fields and missing identifiers are refused with their names.
fn attested_event(
    fields: &AttestationFields,
    device_id: DeviceId,
    identifiers: RecordedIdentifiers,
    at: time::OffsetDateTime,
) -> Result<DestructionEvent, ApiError> {
    let signer =
        AttestationSigner::self_asserted(fields.operator.clone(), &fields.full_name, &fields.title)
            .map_err(record_err)?;
    let attestation = DestructionAttestation::new(
        fields.method,
        at,
        &fields.equipment_id,
        &fields.location,
        identifiers,
        signer,
    )
    .map_err(record_err)?;
    Ok(DestructionEvent {
        id: Uuid::new_v4(),
        device_id,
        at,
        method: fields.method,
        operator: fields.operator.clone(),
        supervisor: None,
        manifest_ref: None,
        photo_refs: Vec::new(),
        notes: nonblank(fields.notes.as_deref()),
        station_id: None,
        attestation: Some(attestation),
    })
}

/// Record an existing Job for destruction. The identifier source is decided
/// here, never claimed by the client: scanned when the request carries
/// `identifiers`, otherwise read from the device (the latest erasure
/// snapshot, else the attached device with the Job's `device_id`) beside
/// the Job's asset tag. A device-sourced record needs a device serial; an
/// asset tag alone is not "read from the device".
pub async fn record_for_destruction(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RecordForDestructionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_attest(&req.fields)?;
    let job = state
        .runner
        .get(id)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("job {id} not found")))?;
    let device_id = job
        .latest_erasure()
        .map(|e| e.device_id_from_spec())
        .unwrap_or_else(|| job.spec.device_id.clone());
    let identifiers = match req.identifiers {
        Some(scanned) => RecordedIdentifiers {
            asset_tag: scanned.asset_tag,
            serial: scanned.serial,
            source: IdentifierSource::Scanned,
        },
        None => {
            let mut serial = job
                .latest_erasure()
                .and_then(|e| nonblank(Some(&e.device_snapshot.serial)));
            if serial.is_none() {
                serial = state
                    .runner
                    .backend()
                    .enumerate()
                    .await
                    .ok()
                    .and_then(|devs| devs.into_iter().find(|d| d.id == job.spec.device_id))
                    .and_then(|d| nonblank(Some(&d.serial)));
            }
            let Some(serial) = serial else {
                return Err(unprocessable(format!(
                    "the serial of job {id}'s drive cannot be read from the device; scan the \
                     drive's asset tag or serial to record it"
                )));
            };
            RecordedIdentifiers {
                asset_tag: job.spec.asset_tag.clone(),
                serial: Some(serial),
                source: IdentifierSource::Device,
            }
        }
    };
    let dest = attested_event(
        &req.fields,
        device_id,
        identifiers,
        time::OffsetDateTime::now_utc(),
    )?;
    state
        .runner
        .record_for_destruction(id, dest)
        .map_err(runner_err)?;
    Ok(Json(json!({"ok": true, "job_id": id})))
}

/// Record a scanned group of drives for destruction: one new Job per
/// drive (intent `destroy`, device id `scanned:<serial, else asset tag>`),
/// each with its own attestation, sharing one statement and one time.
/// All-or-nothing: a blank field, a drive with no identifier or a repeated
/// identifier is 422; a drive matching an open Job is 409 naming that Job.
pub async fn record_scanned(
    State(state): State<AppState>,
    Json(req): Json<RecordScannedRequest>,
) -> Result<Json<RecordScannedResponse>, ApiError> {
    require_attest(&req.fields)?;
    if req.drives.is_empty() {
        return Err(unprocessable("scan at least one drive to record"));
    }
    let at = time::OffsetDateTime::now_utc();
    let mut seen_tags: Vec<String> = Vec::new();
    let mut seen_serials: Vec<String> = Vec::new();
    let mut drives = Vec::with_capacity(req.drives.len());
    for (i, scanned) in req.drives.iter().enumerate() {
        let tag = nonblank(scanned.asset_tag.as_deref());
        let serial = nonblank(scanned.serial.as_deref());
        if let Some(t) = &tag {
            if seen_tags.iter().any(|o| o.eq_ignore_ascii_case(t)) {
                return Err(unprocessable(format!("asset tag {t} is listed twice")));
            }
            seen_tags.push(t.clone());
        }
        if let Some(s) = &serial {
            if seen_serials.iter().any(|o| o.eq_ignore_ascii_case(s)) {
                return Err(unprocessable(format!("serial {s} is listed twice")));
            }
            seen_serials.push(s.clone());
        }
        let Some(key) = serial.clone().or_else(|| tag.clone()) else {
            return Err(unprocessable(format!(
                "drive {}: an asset tag or a serial is required",
                i + 1
            )));
        };
        let device_id = DeviceId(format!("scanned:{key}"));
        let identifiers = RecordedIdentifiers {
            asset_tag: tag.clone(),
            serial,
            source: IdentifierSource::Scanned,
        };
        let ev = attested_event(&req.fields, device_id.clone(), identifiers, at)?;
        let spec = JobSpec {
            device_id,
            classification: req.classification,
            intent: Intent::Destroy,
            operator: req.fields.operator.clone(),
            asset_tag: tag,
            site_label: None,
            ticket_ref: None,
            work_order_ref: None,
            customer_ref: None,
            contract_ref: None,
            sanitization_profile_ref: None,
        };
        drives.push((spec, ev));
    }
    let job_ids = state
        .runner
        .record_scanned_for_destruction(drives)
        .await
        .map_err(runner_err)?;
    Ok(Json(RecordScannedResponse { job_ids }))
}

/// Render the "recorded for destruction" statement for the operator to read
/// before ticking "I attest". The text is rendered only in Rust; a record
/// with the same inputs stores exactly this.
pub async fn preview_attestation_statement(
    Json(req): Json<StatementPreviewRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let statement = attestation_statement(
        req.method,
        &req.full_name,
        &req.title,
        &req.equipment_id,
        &req.location,
    )
    .map_err(record_err)?;
    Ok(Json(json!({ "statement": statement })))
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

/// Assemble a destruction manifest from N PendingDestruction Jobs. Every Job
/// in `job_ids` must currently be in `PendingDestruction` and on no other
/// manifest. On success each Job's `manifest_id` and its latest
/// DestructionEvent's `manifest_ref` carry the manifest's id; the stored
/// certificates are untouched (the co-signature block records the manifest).
pub async fn create_manifest(
    State(state): State<AppState>,
    Json(req): Json<CreateManifestRequest>,
) -> Result<Json<DestructionManifest>, ApiError> {
    if req.job_ids.is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "a manifest needs at least one job".into(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    if let Some(dup) = req.job_ids.iter().find(|j| !seen.insert(**j)) {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("job {dup} is listed twice"),
        ));
    }
    // A Job belongs to at most one live manifest. Without this, the second
    // co-sign appends a second supervisor signature to an already-final cert
    // and then fails half-way through `mark_destroyed`.
    {
        let manifests = state.manifests.read();
        for jid in &req.job_ids {
            if let Some(other) = manifests
                .values()
                .find(|m| m.state != ManifestState::Rejected && m.job_ids.contains(jid))
            {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    format!("job {jid} is already on manifest {}", other.id),
                ));
            }
        }
    }
    // Validate every job is PendingDestruction.
    for jid in &req.job_ids {
        let job = state
            .runner
            .get(*jid)
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("job {jid} not found")))?;
        if job.state != wipe_common::JobState::PendingDestruction {
            return Err(ApiError(
                StatusCode::CONFLICT,
                format!("job {jid} is not pending destruction"),
            ));
        }
    }
    let manifest = DestructionManifest::new(req.assembled_by, req.job_ids.clone(), req.note);
    let mid = manifest.id;
    // Stamp the manifest on each Job and its latest DestructionEvent before
    // storing the manifest. The preflight above scans stored manifests while
    // the runner checks `Job::manifest_id`; the two agree today because
    // nothing sets a manifest `Rejected`. So a runner refusal here means a
    // concurrent request changed a Job after the preflight: another assembly
    // linked it first, or its state moved on (e.g. it was aborted). The
    // runner stamped nothing and no manifest is stored.
    state
        .runner
        .link_to_manifest(mid, &req.job_ids)
        .map_err(|e| ApiError(StatusCode::CONFLICT, e.to_string()))?;
    state.manifests.write().insert(mid, manifest.clone());
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

    // Validate the whole manifest before mutating anything, so a refusal
    // leaves every cert and Job exactly as it was.
    //
    // A Job with no certificate must never be finalised as Destroyed: that
    // would record a destruction the station cannot attest to. This happens
    // when a Job is recorded for destruction before any erasure activity existed (the cert
    // generator has no device snapshot to certify). Refuse, and leave the
    // manifest pending so the gap stays visible rather than silent.
    {
        let certs = state.certs.read();
        let mut problems = Vec::new();
        for jid in &manifest.job_ids {
            match state.runner.get(*jid) {
                None => problems.push(format!("job {jid} not found")),
                Some(job) if job.state != wipe_common::JobState::PendingDestruction => {
                    problems.push(format!("job {jid} is not pending destruction"))
                }
                Some(_) if !certs.contains_key(jid) => problems.push(format!(
                    "job {jid} has no certificate; it cannot be certified as destroyed"
                )),
                Some(_) => {}
            }
        }
        if !problems.is_empty() {
            return Err(ApiError(StatusCode::CONFLICT, problems.join("; ")));
        }
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
        state.runner.mark_destroyed(*jid).map_err(runner_err)?;
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

/// Map a `JobRunner` refusal to a status: an unknown Job is 404, a
/// transition the Job's state does not allow is 409, anything else
/// (backend, I/O) stays 500. The body keeps the runner's message.
fn runner_err(e: WipeError) -> ApiError {
    let status = match e {
        WipeError::JobNotFound(_) => StatusCode::NOT_FOUND,
        WipeError::InvalidState(_) => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    ApiError(status, e.to_string())
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
// Always mounted. They answer only when the host called
// `AppState::with_simulator`, and return 404 otherwise. These exist so identify
// mode can be driven end to end without touching real hardware.

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
