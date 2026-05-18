use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use parking_lot::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing::{info, warn};
use uuid::Uuid;

use wipe_cert::{SignedCertificate, SigningKey};
use wipe_engine::{DeviceBackend, JobRunner};
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
    pub tool_version: String,
    /// Directory containing the built frontend (`index.html` + `assets/`).
    /// When set, the server serves the SPA at `/` with HTML5 history fallback.
    pub static_dir: Option<PathBuf>,
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
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            static_dir,
        };
        state.spawn_cert_generator();
        state
    }

    /// Watches the job runner's broadcast channel; on Completed, generates +
    /// signs a Certificate and stashes it for later retrieval.
    fn spawn_cert_generator(&self) {
        let mut rx = self.runner.subscribe();
        let state = self.clone();
        tokio::spawn(async move {
            while let Ok(update) = rx.recv().await {
                let is_completed = matches!(
                    update.event.event,
                    wipe_common::JobUpdateKind::StateChanged {
                        to: wipe_common::JobState::Completed,
                        ..
                    }
                );
                if !is_completed {
                    continue;
                }
                let Some(job) = state.runner.get(update.job_id) else {
                    continue;
                };
                let issuer = wipe_cert::CertIssuer {
                    tool_name: "wipestation".into(),
                    tool_version: state.tool_version.clone(),
                    public_key_id: state.signing_key.public_key_id(),
                };
                let validation = wipe_cert::ValidationBlock {
                    validated: false,
                    media_class: job.device_snapshot.media_type.class_label().into(),
                    validation_ref: None,
                    validation_expires: None,
                };
                let media_status = wipe_cert::MediaStatus {
                    operational: true,
                    damaged: false,
                    notes: None,
                };
                let Some(cert) =
                    wipe_cert::Certificate::from_job(&job, issuer, validation, media_status)
                else {
                    continue;
                };
                match wipe_cert::sign(cert, &state.signing_key) {
                    Ok(signed) => {
                        info!(job_id = %update.job_id, "certificate signed and stored");
                        state.certs.write().insert(update.job_id, signed);
                    }
                    Err(e) => {
                        tracing::error!(?e, job_id = %update.job_id, "failed to sign cert");
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
            "/api/devices/:id/capabilities",
            get(handlers::device_capabilities),
        )
        .route("/api/jobs", get(handlers::list_jobs).post(handlers::create_job))
        .route("/api/jobs/:id", get(handlers::get_job))
        .route("/api/jobs/:id/start", post(handlers::start_job))
        .route("/api/jobs/:id/abort", post(handlers::abort_job))
        .route("/api/jobs/:id/certificate", get(handlers::get_certificate))
        .route("/api/events", get(ws::ws_events));

    // Mount static frontend at `/` if a dist directory is configured.
    // ServeDir resolves files; for any path it can't resolve (SPA routes
    // like `/jobs/abc`), we fall back to `index.html`.
    let root = match state.static_dir.clone() {
        Some(dir) if dir.exists() => {
            let index = dir.join("index.html");
            info!(?dir, "serving frontend statically");
            let fallback = ServeDir::new(&dir).fallback(tower_http::services::ServeFile::new(index));
            Router::new().fallback_service(fallback)
        }
        Some(dir) => {
            warn!(?dir, "configured static_dir does not exist — UI will not be served");
            Router::new().fallback(api_only_landing)
        }
        None => Router::new().fallback(api_only_landing),
    };

    api.merge(root)
        .layer(CorsLayer::permissive())
        .with_state(state)
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
<code>/api/jobs</code>, <code>/api/events</code> (WebSocket), and friends.</p>
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
