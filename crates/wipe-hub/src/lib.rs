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
pub mod audit;
pub mod auth;
pub mod config;
pub mod db;
pub mod health;
pub mod mail;

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
        Ok(Self {
            config: Arc::new(config),
            pool,
            mailer,
        })
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/health", get(health::health))
        .merge(auth::routes())
        // Unknown API paths are JSON 404s, portal or not: a typo in a client
        // must never get `index.html` back with a 200.
        .route("/api", any(api_not_found))
        .route("/api/", any(api_not_found))
        .route("/api/*rest", any(api_not_found));

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
    axum::serve(listener, router(state)).await?;
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
        "wipe-hub listening"
    );
    serve_on(listener, state).await
}
