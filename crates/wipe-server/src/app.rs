use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    response::IntoResponse,
    routing::{any, get, post},
    Router,
};
use parking_lot::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing::{info, warn};
use uuid::Uuid;

use wipe_cert::{SignedCertificate, SigningKey};
use wipe_engine::{DeviceBackend, JobBroadcast, JobRunner};
use wipe_fleet::FleetService;

use crate::handlers;
use crate::ws;

/// Shared state injected into every handler.
#[derive(Clone)]
pub struct AppState {
    pub runner: JobRunner,
    pub fleet: Option<Arc<FleetService>>,
    pub signing_key: Arc<SigningKey>,
    pub certs: Arc<RwLock<HashMap<Uuid, SignedCertificate>>>,
    pub manifests: Arc<RwLock<HashMap<Uuid, wipe_common::DestructionManifest>>>,
    pub tool_version: String,
    /// Directory containing the built frontend (`index.html` + `assets/`).
    /// When set, the server serves the SPA at `/` with HTML5 history fallback.
    pub static_dir: Option<PathBuf>,
    /// This station's declared physical bay layout (ADR-0002). `None` means
    /// unconfigured — the bay-topology handler then generates an
    /// explicitly-labelled fallback rather than inventing a chassis.
    ///
    /// Held behind a lock because the builder saves it at runtime and the
    /// change must take effect without a restart: a bench being configured
    /// has an operator standing at it.
    pub bay_topology: Arc<RwLock<Option<wipe_common::BayTopology>>>,
    /// Where saved configuration goes, and whether it survives reboot
    /// (ADR-0003). Detected at startup, never guessed by the operator.
    pub topology_store: Arc<dyn crate::store::TopologyStore>,
    /// A hot-plug simulator, `Some` only when the host calls
    /// [`AppState::with_simulator`]. The `/api/sim/*` routes are always
    /// mounted; their handlers answer 404 while this is `None`.
    pub simulator: Option<Arc<dyn wipe_engine::DeviceSimulator>>,
    /// Known enclosure models (ADR-0004). Bundled data, optionally corrected
    /// by a site-local overlay, served to the UI so the builder and the bay
    /// map agree on what a given `model_ref` means.
    pub catalog: Arc<wipe_common::Catalog>,
    /// Licensing state (ADR-0005). Never gates erasure — it only decides
    /// whether a certificate carries a vendor attestation chain or is marked
    /// `evaluation`.
    ///
    /// Behind a shared cell rather than a bare `Arc` because
    /// `spawn_cert_generator` captures a clone of this state at construction
    /// time. Replacing the `Arc` in a builder method afterwards would leave
    /// the already-spawned task holding the pre-licence context — which is
    /// exactly the bug that shipped the first time round: the station logged
    /// "Licensed" and then emitted evaluation certificates.
    pub license: Arc<RwLock<Arc<crate::licensing::LicenseContext>>>,
    /// Whether destruction runs need a review or a witness before their Jobs
    /// reach `Destroyed` (ADR-0011 §3). Set at startup and read-only over the
    /// API: with no sign-in or roles, a setting editable from the bench could
    /// be switched off by the operator it is meant to check. Each run copies
    /// the required kind at assembly. Never depends on licence state.
    pub concurrence_policy: wipe_common::ConcurrencePolicy,
}

impl AppState {
    pub fn new(
        backend: Arc<dyn DeviceBackend>,
        fleet: Option<Arc<FleetService>>,
        signing_key: Arc<SigningKey>,
    ) -> Self {
        Self::with_static_dir(backend, fleet, signing_key, None)
    }

    pub fn with_static_dir(
        backend: Arc<dyn DeviceBackend>,
        fleet: Option<Arc<FleetService>>,
        signing_key: Arc<SigningKey>,
        static_dir: Option<PathBuf>,
    ) -> Self {
        let runner = JobRunner::new(backend);
        let state = Self {
            runner,
            fleet,
            signing_key,
            certs: Arc::new(RwLock::new(HashMap::new())),
            manifests: Arc::new(RwLock::new(HashMap::new())),
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            static_dir,
            bay_topology: Arc::new(RwLock::new(None)),
            // Nothing persists unless a host wires a real store in. Tests and
            // embedders get an in-RAM one that has already answered the
            // "where does this go?" question, so no prompt is raised.
            topology_store: Arc::new(crate::store::EphemeralStore::new(
                "No configuration store was wired in.".into(),
                false,
            )),
            simulator: None,
            catalog: Arc::new(wipe_common::Catalog::bundled()),
            // Free tier until a host installs a licence. A station with no
            // licence still erases and still signs — the certs are just
            // marked evaluation.
            license: Arc::new(RwLock::new(Arc::new(
                crate::licensing::LicenseContext::unlicensed(
                    "standalone",
                    time::OffsetDateTime::now_utc(),
                ),
            ))),
            concurrence_policy: wipe_common::ConcurrencePolicy::None,
        };
        state.spawn_cert_generator();
        state
    }

    /// Declare this station's physical bay layout. Passing `None` leaves the
    /// station unconfigured, which is a supported state — see ADR-0002.
    pub fn with_bay_topology(self, topology: Option<wipe_common::BayTopology>) -> Self {
        *self.bay_topology.write() = topology;
        self
    }

    /// Attach the detected configuration store and adopt whatever it already
    /// holds, so a station comes back up with the layout it was saved with.
    pub fn with_topology_store(mut self, store: Arc<dyn crate::store::TopologyStore>) -> Self {
        match store.load() {
            Ok(Some(stored)) => {
                info!(
                    tier = ?store.tier(), location = %store.location(),
                    bays = stored.bay_count(), "loaded stored bay topology"
                );
                *self.bay_topology.write() = Some(stored);
            }
            Ok(None) => {}
            Err(e) => {
                // A corrupt config must not brick the station — fall back to
                // the generated bench and let the UI show why.
                warn!(error = %e, "stored bay topology unreadable; ignoring it");
            }
        }
        self.topology_store = store;
        self
    }

    /// Merge a site-local catalog overlay over the bundled models (ADR-0004).
    ///
    /// A wrong bay count on a customer's chassis is fixable the same day
    /// rather than at the next release, which is the whole reason the overlay
    /// exists.
    pub fn with_catalog_overlay(mut self, overlay: &wipe_common::Catalog) -> Self {
        self.catalog = Arc::new(self.catalog.overlay(overlay));
        self
    }

    /// Install licensing state. Absent this the station runs free-tier.
    ///
    /// Writes *through* the shared cell so a cert generator already spawned
    /// by the constructor picks it up.
    pub fn with_license(self, license: Arc<crate::licensing::LicenseContext>) -> Self {
        *self.license.write() = license;
        self
    }

    /// Set the station's concurrence policy (default `none`). Runs
    /// assembled from now on require the policy's concurrence kind.
    pub fn with_concurrence_policy(mut self, policy: wipe_common::ConcurrencePolicy) -> Self {
        self.concurrence_policy = policy;
        self
    }

    /// Current licensing state.
    pub fn license(&self) -> Arc<crate::licensing::LicenseContext> {
        self.license.read().clone()
    }

    /// Attach a hot-plug simulator, so the always-mounted `/api/sim/*` routes
    /// answer instead of returning 404.
    pub fn with_simulator(mut self, sim: Arc<dyn wipe_engine::DeviceSimulator>) -> Self {
        self.simulator = Some(sim);
        self
    }

    /// Persist a topology and hot-reload it. Bumps `revision`, refusing a save
    /// that was based on a stale read (ADR-0003).
    pub fn save_bay_topology(
        &self,
        mut topology: wipe_common::BayTopology,
    ) -> Result<wipe_common::BayTopology, crate::store::StoreError> {
        let current_revision = self.bay_topology.read().as_ref().map(|t| t.revision);
        if let Some(stored) = current_revision {
            if topology.revision != stored {
                return Err(crate::store::StoreError::RevisionConflict {
                    stored,
                    sent: topology.revision,
                });
            }
        }
        topology.revision = topology.revision.saturating_add(1);
        topology.generated = false;
        self.topology_store.save(&topology)?;
        *self.bay_topology.write() = Some(topology.clone());
        Ok(topology)
    }

    /// Watches the job runner's broadcast channel. On `Erased`, generates +
    /// signs a Certificate. On `PendingDestruction` we generate the cert and
    /// hold it; completing the destruction run (and any required
    /// concurrence) later moves the Job to `Destroyed` without touching the
    /// cert (see `handlers::complete_manifest`). No co-signature is appended.
    fn spawn_cert_generator(&self) {
        let mut rx = self.runner.subscribe();
        let state = self.clone();
        tokio::spawn(async move {
            while let Ok(b) = rx.recv().await {
                let (job_id, new_state) = match b {
                    JobBroadcast::JobStateChanged { job_id, to, .. } => (job_id, to),
                    _ => continue,
                };
                let needs_cert = matches!(
                    new_state,
                    wipe_common::JobState::Erased | wipe_common::JobState::PendingDestruction
                );
                if !needs_cert {
                    continue;
                }
                let Some(job) = state.runner.get(job_id) else {
                    continue;
                };
                let Some(device) = job.latest_erasure().map(|e| e.device_snapshot.clone()) else {
                    tracing::warn!(%job_id, "no erasure activity; skipping cert generation");
                    continue;
                };
                let issuer = wipe_cert::CertIssuer {
                    tool_name: "wipestation".into(),
                    tool_version: state.tool_version.clone(),
                    public_key_id: state.signing_key.public_key_id(),
                };
                let validation = wipe_cert::ValidationBlock {
                    validated: false,
                    media_class: device.media_type.class_label().into(),
                    validation_ref: None,
                    validation_expires: None,
                };
                let media_status = wipe_cert::MediaStatus {
                    operational: !matches!(new_state, wipe_common::JobState::PendingDestruction),
                    damaged: false,
                    notes: None,
                };
                // For PendingDestruction we temporarily synthesize a Destroyed
                // disposition on the cert so the document is meaningful;
                // completing the destruction run finalises the Job to Destroyed.
                let mut job_for_cert = job.clone();
                if new_state == wipe_common::JobState::PendingDestruction {
                    job_for_cert.state = wipe_common::JobState::Destroyed;
                    if job_for_cert.ended_at.is_none() {
                        job_for_cert.ended_at = Some(time::OffsetDateTime::now_utc());
                    }
                }
                // A DestructionEvent's `manifest_ref` is stamped on the live
                // Job at assembly and never belongs in the signed bytes. The
                // generator runs after the record request has returned, so a
                // manifest assembled in that gap would otherwise be signed in;
                // clear it so the certificate does not depend on request timing.
                for activity in &mut job_for_cert.activities {
                    if let wipe_common::JobActivity::Destruction(ev) = activity {
                        ev.manifest_ref = None;
                    }
                }
                // ADR-0005: licensing decides how the cert is *marked*, never
                // whether the erasure happened. An unlicensed, expired or
                // quota-exhausted station still signs — the cert just says so.
                let decision = state
                    .license()
                    .signing_decision(time::OffsetDateTime::now_utc());
                let Some(cert) = wipe_cert::Certificate::from_job(
                    &job_for_cert,
                    issuer,
                    validation,
                    media_status,
                    decision.evaluation,
                ) else {
                    tracing::warn!(%job_id, "Certificate::from_job returned None");
                    continue;
                };
                match wipe_cert::sign(cert, &state.signing_key) {
                    Ok(mut signed) => {
                        if let Some(attestation) = decision.attestation {
                            wipe_cert::attach_attestation(&mut signed, attestation);
                        } else {
                            tracing::info!(
                                %job_id, status = ?decision.status,
                                "certificate marked evaluation (no valid licence)"
                            );
                        }
                        if !decision.evaluation {
                            state.license().record_erasure();
                        }
                        info!(%job_id, ?new_state, evaluation = decision.evaluation,
                              "certificate signed and stored");
                        state.certs.write().insert(job_id, signed);
                    }
                    Err(e) => {
                        tracing::error!(?e, %job_id, "failed to sign cert");
                    }
                }
            }
        });
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/health", get(handlers::health))
        .route("/api/station", get(handlers::station_info))
        .route("/api/public_key", get(handlers::public_key))
        .route("/api/fleet/peers", get(handlers::list_peers))
        .route("/api/fleet/lead", get(handlers::current_lead))
        .route("/api/devices", get(handlers::list_devices))
        .route(
            "/api/bay-topology",
            get(handlers::bay_topology).put(handlers::save_bay_topology),
        )
        .route(
            "/api/bay-topology/config",
            get(handlers::bay_topology_config),
        )
        .route(
            "/api/bay-topology/resolve",
            post(handlers::resolve_bay_topology),
        )
        .route("/api/bay-topology/store", get(handlers::bay_topology_store))
        .route("/api/enclosure-catalog", get(handlers::enclosure_catalog))
        .route(
            "/api/bay-topology/store/acknowledge",
            post(handlers::acknowledge_ephemeral),
        )
        .route(
            "/api/devices/:id/capabilities",
            get(handlers::device_capabilities),
        )
        .route(
            "/api/jobs",
            get(handlers::list_jobs).post(handlers::create_job),
        )
        .route("/api/jobs/:id", get(handlers::get_job))
        .route("/api/jobs/:id/start", post(handlers::start_job))
        .route("/api/jobs/:id/abort", post(handlers::abort_job))
        .route(
            "/api/jobs/:id/record-for-destruction",
            post(handlers::record_for_destruction),
        )
        .route("/api/destruction/records", post(handlers::record_scanned))
        .route(
            "/api/destruction/statement",
            post(handlers::preview_attestation_statement),
        )
        .route("/api/jobs/:id/certificate", get(handlers::get_certificate))
        .route(
            "/api/manifests",
            get(handlers::list_manifests).post(handlers::create_manifest),
        )
        .route("/api/manifests/:id", get(handlers::get_manifest))
        .route(
            "/api/manifests/:id/completion",
            post(handlers::complete_manifest),
        )
        .route(
            "/api/manifests/:id/completion/statement",
            post(handlers::preview_completion_statement),
        )
        .route(
            "/api/manifests/:id/concurrence",
            post(handlers::concur_manifest),
        )
        .route(
            "/api/manifests/:id/concurrence/statement",
            post(handlers::preview_concurrence_statement),
        )
        .route("/api/destruction/policy", get(handlers::destruction_policy))
        .route("/api/sim/devices/attach", post(handlers::sim_attach))
        .route("/api/sim/devices/detach", post(handlers::sim_detach))
        .route("/api/sim/devices", get(handlers::sim_detached))
        .route("/api/events", get(ws::ws_events))
        // An unknown API path is a JSON 404, never the frontend or the
        // API-only landing page: a client calling a removed route (such as
        // the old `escalate-to-destroy`) must see that it is gone.
        .route("/api/*rest", any(unknown_api_route));

    // Mount static frontend at `/` if a dist directory is configured.
    // ServeDir resolves files; for any path it can't resolve (SPA routes
    // like `/jobs/abc`), we fall back to `index.html`.
    let root = match state.static_dir.clone() {
        Some(dir) if dir.exists() => {
            let index = dir.join("index.html");
            info!(?dir, "serving frontend statically");
            let fallback =
                ServeDir::new(&dir).fallback(tower_http::services::ServeFile::new(index));
            Router::new().fallback_service(fallback)
        }
        Some(dir) => {
            warn!(
                ?dir,
                "configured static_dir does not exist — UI will not be served"
            );
            Router::new().fallback(api_only_landing)
        }
        None => Router::new().fallback(api_only_landing),
    };

    api.merge(root)
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn unknown_api_route(uri: axum::http::Uri) -> impl IntoResponse {
    (
        axum::http::StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({ "error": format!("no API route {}", uri.path()) })),
    )
}

/// Friendly response when the binary is run without a frontend bundle attached.
async fn api_only_landing() -> impl IntoResponse {
    let body = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Wipestation API</title>
<style>body{font-family:ui-sans-serif,system-ui,sans-serif;background:#020617;color:#e2e8f0;padding:2rem;max-width:46rem;margin:auto}
code{background:#0f172a;padding:.15rem .35rem;border-radius:.25rem;color:#a5b4fc}
h1{margin-top:0}a{color:#818cf8}</style>
</head><body>
<h1>Wipestation API</h1>
<p>The HTTP API is running, but no frontend bundle was attached.</p>
<p>To serve the web UI, start with <code>--static-dir &lt;path-to-dist&gt;</code>,
or build the frontend and rerun:</p>
<pre><code>cd apps/desktop &amp;&amp; pnpm install &amp;&amp; pnpm build
wipestation serve --static-dir apps/desktop/dist</code></pre>
<p>API routes are at <code>/api/health</code>, <code>/api/devices</code>,
<code>/api/jobs</code>, <code>/api/manifests</code>, <code>/api/events</code>
(WebSocket), and friends.</p>
</body></html>"#;
    axum::response::Html(body)
}

pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "wipestation api listening");
    axum::serve(listener, app).await?;
    Ok(())
}
