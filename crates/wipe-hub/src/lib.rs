//! `wipe-hub`: the vendor-side service for accounts, payments and licence
//! delivery (ADR-0007). It serves `/api/*` from one Axum origin.
//!
//! Dependency rule (ADR-0007 §1): this crate may depend on `wipe-common`,
//! `wipe-cert` and `wipe-license` only, and no station crate may depend on it.
//! A station never needs the hub to erase, sign or verify.

pub mod config;
pub mod health;

use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use sqlx::postgres::{PgPool, PgPoolOptions};
use tokio::net::TcpListener;
use tracing::info;

pub use config::{startup_warnings, Args, Config, ConfigError, PaymentProvider};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: PgPool,
}

impl AppState {
    /// Build state with a lazy pool: nothing connects until first use, so the
    /// hub starts (and reports `database: down`) while Postgres is down.
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new()
            .connect_lazy(&config.database_url)
            .map_err(|e| anyhow::anyhow!("invalid --database-url / WIPE_HUB_DATABASE_URL: {e}"))?;
        Ok(Self {
            config: Arc::new(config),
            pool,
        })
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health::health))
        .with_state(state)
}

/// Serve on an already-bound listener (tests bind port 0).
pub async fn serve_on(listener: TcpListener, state: AppState) -> anyhow::Result<()> {
    axum::serve(listener, router(state)).await?;
    Ok(())
}

pub async fn serve(config: Config) -> anyhow::Result<()> {
    let addr = config.addr;
    let state = AppState::new(config)?;
    let listener = TcpListener::bind(addr).await?;
    info!(
        %addr,
        public_base_url = %state.config.public_base_url,
        payment_provider = ?state.config.payment_provider,
        "wipe-hub listening"
    );
    serve_on(listener, state).await
}
