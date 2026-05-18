//! Tauri 2 application shell.
//!
//! The Tauri window is a thin viewport over the same Axum API the headless
//! CLI exposes. We:
//!   1. Initialize tracing so a developer can see what's happening.
//!   2. Bind the API on 127.0.0.1:7878 (or `WIPESTATION_API_ADDR`).
//!   3. Wait for the API to answer `/api/health`.
//!   4. Show the (initially-hidden) window pointed at the API.
//!
//! This collapses the "browser at localhost" and "Tauri window" modes onto
//! one origin — same code, same UI, same API.

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use tauri::Manager;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use wipe_cert::SigningKey;
use wipe_engine_mock::{MockBackend, MockTiming};
use wipe_fleet::FleetService;

const DEFAULT_ADDR: &str = "127.0.0.1:7878";

pub fn run() {
    // Init logging unconditionally so the Tauri window's stderr is informative.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();

    let api_addr: SocketAddr = std::env::var("WIPESTATION_API_ADDR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| DEFAULT_ADDR.parse().unwrap());
    let api_url = format!("http://{api_addr}");
    info!(%api_addr, "starting wipestation desktop shell");

    tauri::Builder::default()
        .setup(move |app| {
            // Spawn the API in the background.
            let api_url_for_setup = api_url.clone();
            let static_dir = detect_static_dir(app.handle());
            tauri::async_runtime::spawn(async move {
                match bootstrap_api(api_addr, static_dir).await {
                    Ok(_) => info!("api server exited"),
                    Err(e) => tracing::error!(?e, "api server failed to start or crashed"),
                }
            });

            // Wait for the API to become ready, then reveal the main window.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if wait_for_ready(&api_url_for_setup, Duration::from_secs(15)).await {
                    info!("api ready — showing window");
                    if let Some(win) = handle.get_webview_window("main") {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                } else {
                    warn!("api did not come up within timeout — showing window anyway");
                    if let Some(win) = handle.get_webview_window("main") {
                        let _ = win.show();
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![api_url_cmd])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[tauri::command]
fn api_url_cmd() -> String {
    std::env::var("WIPESTATION_API_ADDR")
        .map(|s| format!("http://{s}"))
        .unwrap_or_else(|_| format!("http://{DEFAULT_ADDR}"))
}

/// Locate the frontend bundle. In dev (running via `pnpm tauri dev`) the
/// `tauri dev` process puts the built dist next to the binary; the
/// resource resolver normally handles this. As a robust fallback we walk
/// a few common paths.
fn detect_static_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    if let Ok(resource) = app.path().resource_dir() {
        let p = resource.join("dist");
        if p.join("index.html").is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        for rel in [
            "../apps/desktop/dist",
            "../../apps/desktop/dist",
            "../../../apps/desktop/dist",
            "../../../../apps/desktop/dist",
        ] {
            let p = exe.parent()?.join(rel);
            if p.join("index.html").is_file() {
                return Some(p);
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join("apps/desktop/dist");
        if p.join("index.html").is_file() {
            return Some(p);
        }
    }
    warn!("could not locate frontend dist — Tauri window will show the API-only landing page");
    None
}

async fn bootstrap_api(addr: SocketAddr, static_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        MockTiming::default(),
    ));
    let signing_key = Arc::new(SigningKey::generate());

    let info_msg = wipe_common::StationInfo {
        id: wipe_common::StationId::new_random(),
        hostname: hostname_or_default(),
        role: wipe_common::StationRole::Member,
        version: env!("CARGO_PKG_VERSION").into(),
        api_port: addr.port(),
        started_at: time::OffsetDateTime::now_utc(),
        active_jobs: 0,
        last_seen: None,
    };
    // mDNS is best-effort — don't fail the app if it can't start.
    let fleet = match FleetService::start(info_msg) {
        Ok(f) => Some(Arc::new(f)),
        Err(e) => {
            warn!(?e, "mDNS fleet disabled");
            None
        }
    };

    let state = wipe_server::AppState::with_static_dir(backend, fleet, signing_key, static_dir);
    info!(%addr, "binding API");
    wipe_server::serve(state, addr).await?;
    Ok(())
}

fn hostname_or_default() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "wipestation-desktop".into())
}

async fn wait_for_ready(api_url: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    let url = format!("{api_url}/api/health");
    loop {
        match reqwest_get(&url).await {
            Ok(true) => return true,
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn reqwest_get(url: &str) -> Result<bool, std::io::Error> {
    // Hand-rolled, no dependency: open a TCP connection, send a minimal
    // GET, look for a 200 in the first line.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let parsed = url::Url::parse(url).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no host"))?;
    let port = parsed.port().unwrap_or(80);
    let path = parsed.path();
    let mut stream = tokio::net::TcpStream::connect((host, port)).await?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await?;
    let mut buf = [0u8; 64];
    let n = stream.read(&mut buf).await?;
    let line = String::from_utf8_lossy(&buf[..n]);
    Ok(line.starts_with("HTTP/1.1 200"))
}
