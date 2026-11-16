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
    attestation_statement, completion_statement, concurrence_statement, AttestationSigner,
    Capabilities, Classification, Concurrence, ConcurrenceKind, DestructMethod,
    DestructionAttestation, DestructionCompletion, DestructionEvent, DestructionManifest,
    DestructionRecordError, DestructionVerification, Device, DeviceId, IdentifierSource, Intent,
    Job, JobSpec, ManifestState, ManifestTransitionError, OperatorRef, RecordedIdentifiers,
    ResolvedBayTopology, StationId, StationInfo, WipeError,
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

/// Start a queued Job. 409 when the licence does not cover the start on a
/// real backend (ADR-0013 §1), with the reason and the fix; the Job stays
/// queued and nothing is broadcast.
pub async fn start_job(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.start_job(id).await.map_err(start_err)?;
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

fn require_attest(attest: bool) -> Result<(), ApiError> {
    if attest {
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
/// asset tag alone is not "read from the device". A Job never started is
/// checked against the licence once the request is valid (409 when it does
/// not cover physical destruction); a started one keeps its start's
/// decision.
pub async fn record_for_destruction(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RecordForDestructionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_attest(req.fields.attest)?;
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
        .record_for_destruction(id, dest)
        .await
        .map_err(start_err)?;
    Ok(Json(json!({"ok": true, "job_id": id})))
}

/// Record a scanned group of drives for destruction: one new Job per
/// drive (intent `destroy`, device id `scanned:<serial, else asset tag>`),
/// each with its own attestation, sharing one statement and one time.
/// All-or-nothing: a blank field, a drive with no identifier or a repeated
/// identifier is 422; a drive matching an open Job is 409 naming that Job;
/// a licence that does not cover every drive (one credit each) is 409.
pub async fn record_scanned(
    State(state): State<AppState>,
    Json(req): Json<RecordScannedRequest>,
) -> Result<Json<RecordScannedResponse>, ApiError> {
    require_attest(req.fields.attest)?;
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
    let job_ids = state.record_scanned(drives).await.map_err(start_err)?;
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

/// Assemble a destruction run from N PendingDestruction Jobs. Every Job in
/// `job_ids` must currently be in `PendingDestruction` and be on no other
/// run; a drive that was never erased may join (ADR-0011 §2). The run
/// records the concurrence the station's policy requires now
/// (`required_concurrence`). On success each Job's `manifest_id` and its
/// latest DestructionEvent's `manifest_ref` carry the run's id; nothing is
/// certified until the run destroys its Jobs.
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
    // A Job belongs to at most one live manifest, so one completion
    // decides its destruction.
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
    let manifest = DestructionManifest::new(
        req.assembled_by,
        req.job_ids.clone(),
        req.note,
        state.concurrence_policy.required_kind(),
    );
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

/// Who signs a completion or a concurrence, as story 2's attestation
/// request names them. Text fields default to empty so a missing one is
/// refused by the constructors with the field's name.
#[derive(Debug, Deserialize)]
pub struct SignerFields {
    pub operator: OperatorRef,
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub title: String,
    /// The signer ticked "I attest" against the displayed statement.
    #[serde(default)]
    pub attest: bool,
}

#[derive(Debug, Deserialize)]
pub struct CompletionRequest {
    /// The time the statement names, as the preview returned it. Omitted,
    /// the server uses now.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub completed_at: Option<time::OffsetDateTime>,
    #[serde(default)]
    pub remnants_inspected: bool,
    #[serde(default)]
    pub equipment_cycle_completed: bool,
    #[serde(default)]
    pub equipment_id: String,
    #[serde(default)]
    pub criterion: String,
    #[serde(default)]
    pub video_ref: Option<String>,
    #[serde(flatten)]
    pub signer: SignerFields,
}

#[derive(Debug, Deserialize)]
pub struct CompletionPreviewRequest {
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub completed_at: Option<time::OffsetDateTime>,
    #[serde(default)]
    pub equipment_id: String,
    #[serde(default)]
    pub criterion: String,
    #[serde(default)]
    pub video_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConcurrencePreviewRequest {
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub title: String,
}

fn manifest_not_found(id: Uuid) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, format!("manifest {id} not found"))
}

fn transition_err(e: ManifestTransitionError) -> ApiError {
    let status = if e.is_conflict() {
        StatusCode::CONFLICT
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    ApiError(status, e.to_string())
}

/// Now, to the second: the statement prints whole seconds, so a server-chosen
/// time reads back exactly as signed.
fn now_to_the_second() -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("0 is a valid nanosecond")
}

/// Every member Job must still be `PendingDestruction` before the run is
/// completed or concurred with. Refusals name the Jobs.
fn check_members(state: &AppState, ids: &[Uuid]) -> Result<(), ApiError> {
    let mut problems = Vec::new();
    for jid in ids {
        match state.runner.get(*jid) {
            None => problems.push(format!("job {jid} not found")),
            Some(job) if job.state != wipe_common::JobState::PendingDestruction => {
                problems.push(format!("job {jid} is not pending destruction"))
            }
            Some(_) => {}
        }
    }
    if !problems.is_empty() {
        return Err(ApiError(StatusCode::CONFLICT, problems.join("; ")));
    }
    Ok(())
}

/// No Job reaches `Destroyed` uncertifiable (c779361, ADR-0011 §2): before
/// any member moves, build every member's destruction record against `run`
/// exactly as it will be stored, which is what the cert generator signs.
/// Refusals are 409 naming the Jobs.
fn check_certifiable(state: &AppState, run: &DestructionManifest) -> Result<(), ApiError> {
    let problems: Vec<String> = run
        .job_ids
        .iter()
        .filter_map(|jid| match state.runner.get(*jid) {
            None => Some(format!("job {jid} not found")),
            Some(job) => wipe_cert::DestructionRecord::build(&job, run)
                .err()
                .map(|e| format!("{e}")),
        })
        .collect();
    if problems.is_empty() {
        return Ok(());
    }
    Err(ApiError(
        StatusCode::CONFLICT,
        format!(
            "cannot certify every drive on destruction run {}, so nothing changed: {}",
            run.id,
            problems.join("; ")
        ),
    ))
}

/// Apply `transition` to a copy of run `id` and commit it only if the
/// transition, the member checks and (when the run is now done) the
/// certifiability preflight and the runner's all-or-nothing move to
/// `Destroyed` all succeed. The manifests lock is held throughout, so two
/// requests cannot both pass validation, and the cert generator (which reads
/// the run on each Job's `Destroyed`) sees the run stored here.
fn advance_run(
    state: &AppState,
    id: Uuid,
    transition: impl FnOnce(&mut DestructionManifest) -> Result<(), ApiError>,
) -> Result<DestructionManifest, ApiError> {
    let mut manifests = state.manifests.write();
    let mut next = manifests
        .get(&id)
        .cloned()
        .ok_or_else(|| manifest_not_found(id))?;
    transition(&mut next)?;
    check_members(state, &next.job_ids)?;
    if next.destroys_jobs() {
        check_certifiable(state, &next)?;
        state
            .runner
            .mark_destroyed(&next.job_ids)
            .map_err(|e| ApiError(StatusCode::CONFLICT, e.to_string()))?;
    }
    manifests.insert(id, next.clone());
    Ok(next)
}

/// Confirm a destruction run's completion (ADR-0011 §1, second step). With
/// no concurrence required every member Job becomes `Destroyed` at once and
/// is certified then, asynchronously (the response may precede the
/// certificate by moments); otherwise the run awaits its concurrence.
pub async fn complete_manifest(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CompletionRequest>,
) -> Result<Json<DestructionManifest>, ApiError> {
    require_attest(req.signer.attest)?;
    let signer = AttestationSigner::self_asserted(
        req.signer.operator.clone(),
        &req.signer.full_name,
        &req.signer.title,
    )
    .map_err(record_err)?;
    let now = time::OffsetDateTime::now_utc();
    // Stored in UTC, so the stored time reads exactly as the statement prints it.
    let completed_at = req
        .completed_at
        .map(|t| t.to_offset(time::UtcOffset::UTC))
        .unwrap_or_else(now_to_the_second);
    let completion = DestructionCompletion::new(
        completed_at,
        DestructionVerification {
            remnants_inspected: req.remnants_inspected,
            equipment_cycle_completed: req.equipment_cycle_completed,
            equipment_id: req.equipment_id,
            criterion: req.criterion,
        },
        req.video_ref.as_deref(),
        signer,
    )
    .map_err(record_err)?;
    let run = advance_run(&state, id, |m| {
        // A run that is not pending is a conflict whatever the time says.
        if m.state == ManifestState::Pending {
            m.check_completion_time(completed_at, now)
                .map_err(transition_err)?;
        }
        m.complete(completion).map_err(transition_err)
    })?;
    Ok(Json(run))
}

/// Concur with a completed destruction run. The kind is the run's
/// `required_concurrence`, never the client's; the signer must differ from
/// the completion's. Every member Job then becomes `Destroyed` and is
/// certified.
pub async fn concur_manifest(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SignerFields>,
) -> Result<Json<DestructionManifest>, ApiError> {
    require_attest(req.attest)?;
    let signer = AttestationSigner::self_asserted(req.operator, &req.full_name, &req.title)
        .map_err(record_err)?;
    let run = advance_run(&state, id, |m| {
        let kind = required_kind(m)?;
        let concurrence = Concurrence::new(kind, m.id, time::OffsetDateTime::now_utc(), signer)
            .map_err(record_err)?;
        m.concur(concurrence).map_err(transition_err)
    })?;
    Ok(Json(run))
}

fn required_kind(m: &DestructionManifest) -> Result<ConcurrenceKind, ApiError> {
    m.required_concurrence
        .ok_or_else(|| transition_err(ManifestTransitionError::NoConcurrenceRequired { id: m.id }))
}

/// Render the completion statement for the operator to read before ticking
/// "I attest", with the time it names. Sending that time back with the same
/// fields stores exactly this statement.
pub async fn preview_completion_statement(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CompletionPreviewRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let run = state
        .manifests
        .read()
        .get(&id)
        .cloned()
        .ok_or_else(|| manifest_not_found(id))?;
    if run.state != ManifestState::Pending {
        return Err(transition_err(ManifestTransitionError::NotPending {
            id,
            state: run.state,
        }));
    }
    let now = time::OffsetDateTime::now_utc();
    // Stored in UTC, so the stored time reads exactly as the statement prints it.
    let completed_at = req
        .completed_at
        .map(|t| t.to_offset(time::UtcOffset::UTC))
        .unwrap_or_else(now_to_the_second);
    run.check_completion_time(completed_at, now)
        .map_err(transition_err)?;
    let statement = completion_statement(
        completed_at,
        &req.equipment_id,
        &req.criterion,
        req.video_ref.as_deref(),
    )
    .map_err(record_err)?;
    let completed_at = completed_at
        .to_offset(time::UtcOffset::UTC)
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(api_err)?;
    Ok(Json(
        json!({ "statement": statement, "completed_at": completed_at }),
    ))
}

/// Render the concurrence statement for the run's required kind. 409 when
/// the run needs no concurrence or is not awaiting it.
pub async fn preview_concurrence_statement(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ConcurrencePreviewRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let run = state
        .manifests
        .read()
        .get(&id)
        .cloned()
        .ok_or_else(|| manifest_not_found(id))?;
    let kind = required_kind(&run)?;
    if run.state != ManifestState::AwaitingConcurrence {
        return Err(transition_err(
            ManifestTransitionError::NotAwaitingConcurrence {
                id,
                state: run.state,
            },
        ));
    }
    let statement =
        concurrence_statement(kind, run.id, &req.full_name, &req.title).map_err(record_err)?;
    Ok(Json(json!({ "statement": statement })))
}

/// The station's concurrence policy, set at startup and read-only here.
pub async fn destruction_policy(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({ "concurrence_policy": state.concurrence_policy }))
}

/// The station's licence state. Read-only: it never moves the clock
/// watermark, saves lease state or records usage, so a client may poll it.
pub async fn license(State(state): State<AppState>) -> Json<crate::licensing::LicenseView> {
    let mut view = state
        .license()
        .view(time::OffsetDateTime::now_utc(), state.simulated());
    view.hub_check = state.licence_fetch.last();
    view.credit_top_up = state.credit_topup.last();
    Json(view)
}

/// Install a licence while the station runs. The body is the licence file's
/// JSON, taken as raw bytes so a body that is not UTF-8 or not JSON gets the
/// same reasoned 422 as one that is not a licence. 409 when the station trusts no vendor root;
/// 422 when the licence cannot be parsed, does not verify, or names another
/// station's key, or the file's credit grant does not verify or names
/// another station's key. On any error the installed licence is unchanged.
pub async fn install_license(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Json<crate::licensing::LicenseView>, ApiError> {
    let body = std::str::from_utf8(&body).map_err(|e| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("not a licence file: the body is not UTF-8 text: {e}"),
        )
    })?;
    state.install_license(body).map(Json).map_err(|e| {
        let code = match e {
            crate::app::LicenseInstallError::NoVendorRoot => StatusCode::CONFLICT,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        ApiError(code, e.to_string())
    })
}

/// The bench app's Register with platform panel (ADR-0008, CAP-4).
pub async fn enrollment(State(state): State<AppState>) -> Json<crate::enrollment::EnrollmentView> {
    Json(crate::enrollment::view(&state))
}

/// Start an enrollment with the hub, replacing any pending one. 400 for a
/// hub address that breaks the rule or an over-long label, 409 when the
/// station trusts no vendor root or its instance key lasts for this run only,
/// all before any hub request. Otherwise 200
/// with the attempt pending, or ended when the hub could not be reached or
/// refused: hub outcomes are view states, not HTTP errors.
pub async fn start_enrollment(
    State(state): State<AppState>,
    Json(req): Json<crate::enrollment::StartEnrollment>,
) -> Result<Json<crate::enrollment::EnrollmentView>, ApiError> {
    use crate::enrollment::StartRefusal;
    crate::enrollment::start(&state, req)
        .await
        .map(Json)
        .map_err(|e| {
            let code = match e {
                StartRefusal::NoVendorRoot | StartRefusal::KeyNotKept => StatusCode::CONFLICT,
                StartRefusal::BadOrigin(_) | StartRefusal::LabelTooLong(_) => {
                    StatusCode::BAD_REQUEST
                }
            };
            ApiError(code, e.to_string())
        })
}

/// Stop the pending enrollment, if any; the hub's code simply expires.
pub async fn cancel_enrollment(
    State(state): State<AppState>,
) -> Json<crate::enrollment::EnrollmentView> {
    Json(crate::enrollment::cancel(&state))
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

/// A licence refusal is 409 with its text; a runner refusal maps as
/// [`runner_err`].
fn start_err(e: crate::app::JobStartError) -> ApiError {
    match e {
        crate::app::JobStartError::Refused(refusal) => {
            ApiError(StatusCode::CONFLICT, refusal.message())
        }
        crate::app::JobStartError::Runner(e) => runner_err(e),
    }
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
