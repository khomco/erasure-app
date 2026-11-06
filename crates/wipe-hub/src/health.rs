//! `GET /api/health`: is the database reachable, and is the SMTP server?
//!
//! Each probe is bounded by [`PROBE_TIMEOUT`]. The endpoint's job is to report
//! an outage, so a probe failure is a `"down"` component, never an error.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::AppState;

/// Upper bound on each individual probe.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Component {
    Up,
    Down,
}

impl From<bool> for Component {
    fn from(up: bool) -> Self {
        if up {
            Self::Up
        } else {
            Self::Down
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthReport {
    pub status: Status,
    pub database: Component,
    pub mail: Component,
}

pub async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthReport>) {
    let (database, mail) = tokio::join!(
        probe_database(&state.pool),
        probe_smtp(&state.config.smtp_host, state.config.smtp_port),
    );
    let report = HealthReport {
        status: if database && mail {
            Status::Ok
        } else {
            Status::Degraded
        },
        database: database.into(),
        mail: mail.into(),
    };
    let code = match report.status {
        Status::Ok => StatusCode::OK,
        Status::Degraded => StatusCode::SERVICE_UNAVAILABLE,
    };
    (code, Json(report))
}

/// `SELECT 1` on the (lazy) pool.
pub async fn probe_database(pool: &PgPool) -> bool {
    match timeout(PROBE_TIMEOUT, sqlx::query("SELECT 1").execute(pool)).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "database probe failed");
            false
        }
        Err(_) => {
            tracing::warn!("database probe timed out");
            false
        }
    }
}

/// TCP connect, then read a `220` greeting.
pub async fn probe_smtp(host: &str, port: u16) -> bool {
    match timeout(PROBE_TIMEOUT, smtp_greeting(host, port)).await {
        Ok(Ok(true)) => true,
        Ok(Ok(false)) => {
            tracing::warn!(host, port, "SMTP server did not greet with 220");
            false
        }
        Ok(Err(e)) => {
            tracing::warn!(host, port, error = %e, "SMTP probe failed");
            false
        }
        Err(_) => {
            tracing::warn!(host, port, "SMTP probe timed out");
            false
        }
    }
}

async fn smtp_greeting(host: &str, port: u16) -> std::io::Result<bool> {
    let mut stream = TcpStream::connect((host, port)).await?;
    let mut buf = Vec::with_capacity(128);
    let mut chunk = [0u8; 128];
    // Enough of the first line to see the reply code.
    while buf.len() < 3 || !buf.contains(&b'\n') {
        let n = stream.read(&mut chunk).await?;
        if n == 0 || buf.len() > 1024 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let greeted = buf.starts_with(b"220");
    if greeted {
        // Polite close; the server's reply is not needed.
        let _ = stream.write_all(b"QUIT\r\n").await;
    }
    Ok(greeted)
}
