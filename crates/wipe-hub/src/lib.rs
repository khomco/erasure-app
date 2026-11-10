//! `wipe-hub`: the vendor-side service for accounts, payments and licence
//! delivery (ADR-0007). It serves `/api/*` and the built portal
//! (`apps/portal`, ADR-0006 §1) from one Axum origin: `/api/*` paths that
//! match no route are JSON 404s and never fall through to the portal,
//! `/assets/*` has no fallback, and any other unknown path gets the portal's
//! `index.html` (SPA fallback).
//!
//! Dependency rule (ADR-0007 §1): this crate may depend on `wipe-common`,
//! `wipe-cert` and `wipe-license` only, and no station crate may depend on it.
//! A station never needs the hub to erase, sign or verify.

pub mod accounts;
pub mod admin;
pub mod admin_routes;
pub mod artifacts;
pub mod audit;
pub mod auth;
pub mod config;
pub mod cors;
pub mod csrf;
pub mod db;
pub mod deployment;
pub mod download_routes;
pub mod downloads;
pub mod enrollment_routes;
pub mod enrollments;
pub mod entitlements;
pub mod health;
pub mod lead_admin;
pub mod lead_routes;
pub mod leads;
pub mod licence_admin;
pub mod mail;
pub mod offering_admin;
pub mod order_routes;
pub mod orders;
pub mod payment_events;
pub mod payments;
pub mod release_admin;
pub mod station_link;
pub mod station_link_routes;
pub mod station_routes;
pub mod stations;
pub mod stripe;
pub mod team;
pub mod team_routes;
pub mod throttle;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{any, get};
use axum::{Json, Router};
use sqlx::postgres::{PgPool, PgPoolOptions};
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};
use tracing::info;

pub use config::{startup_warnings, Args, Config, ConfigError, PaymentProvider};
use mail::Mailer;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: PgPool,
    pub mailer: Mailer,
    /// The payment provider `--payment-provider` selects (ADR-0007 §4).
    pub payments: Arc<dyn payments::PaymentProvider>,
    /// Where release files are kept (ADR-0007 §5): `--artifact-dir`.
    pub artifacts: Arc<dyn artifacts::ArtifactStore>,
    /// Signs and verifies download links with a key drawn at start, so a
    /// restart voids outstanding links ([`downloads::DownloadSigner`]).
    pub download_signer: Arc<downloads::DownloadSigner>,
}

impl AppState {
    /// Build state with a lazy pool: nothing connects until first use, so the
    /// hub starts (and reports `database: down`) while Postgres is down.
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new()
            .connect_lazy(&config.database_url)
            .map_err(|e| anyhow::anyhow!("invalid --database-url / WIPE_HUB_DATABASE_URL: {e}"))?;
        // Build the sign-in timing equaliser before the first request.
        accounts::prepare_dummy_hash();
        let mailer = Mailer::new(
            &config.smtp_host,
            config.smtp_port,
            config.mail_from.clone(),
        );
        let payments = payments::provider_for(&config)?;
        let artifacts = artifacts::store_for(config.artifact_dir.clone());
        Ok(Self {
            config: Arc::new(config),
            pool,
            mailer,
            payments,
            artifacts,
            download_signer: Arc::new(downloads::DownloadSigner::random()),
        })
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/health", get(health::health))
        .route("/api/deployment", get(deployment::deployment))
        .merge(auth::routes())
        .merge(team_routes::routes())
        .merge(admin_routes::routes())
        .merge(lead_routes::routes())
        .merge(order_routes::routes())
        .merge(station_routes::routes())
        .merge(download_routes::routes())
        .merge(enrollment_routes::routes())
        .merge(station_link_routes::routes())
        // Unknown API paths are JSON 404s, portal or not: a typo in a client
        // must never get `index.html` back with a 200.
        .route("/api", any(api_not_found))
        .route("/api/", any(api_not_found))
        .route("/api/*rest", any(api_not_found))
        // After the catch-alls, so unknown `/api` paths are covered too: every
        // mutating `/api` request needs the double-submit CSRF token.
        .layer(axum::middleware::from_fn(csrf::require))
        // Outside the CSRF check, so the marketing site can read its 403s
        // too; it touches only `cors::SITE_PATHS` from `--site-origin`.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            cors::site_cors,
        ));

    let site = match &state.config.portal_dir {
        Some(dir) => {
            // Vite fingerprints asset names, so a stale tab asking for an old
            // hash must get a 404, never HTML parsed as JS: no fallback here.
            let assets = ServeDir::new(dir.join("assets"));
            let spa = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
            Router::new()
                .nest_service("/assets", assets)
                .fallback_service(spa)
        }
        None => Router::new().fallback(portal_not_built),
    };

    api.merge(site).with_state(state)
}

async fn api_not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "not found"})),
    )
}

async fn portal_not_built() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        format!(
            "not found: the portal is not served by this hub. Build it with `{}` \
             or pass --portal-dir.\n",
            config::PORTAL_BUILD_HINT
        ),
    )
}

/// Serve on an already-bound listener (tests bind port 0).
pub async fn serve_on(listener: TcpListener, state: AppState) -> anyhow::Result<()> {
    // Peer addresses feed the per-IP sign-in throttle.
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Apply migrations (failing fast if Postgres is unreachable), then bind and
/// serve. [`serve_on`] does not migrate.
pub async fn serve(config: Config) -> anyhow::Result<()> {
    let addr = config.addr;
    let state = AppState::new(config)?;
    // Migrate before binding: a hub must never serve an unmigrated schema.
    db::migrate_url(&state.config.database_url).await?;
    let listener = TcpListener::bind(addr).await?;
    info!(
        %addr,
        public_base_url = %state.config.public_base_url,
        payment_provider = ?state.config.payment_provider,
        portal_dir = ?state.config.portal_dir,
        site_origin = %state.config.site_origin,
        artifact_dir = %state.config.artifact_dir.display(),
        "wipe-hub listening"
    );
    serve_on(listener, state).await
}
