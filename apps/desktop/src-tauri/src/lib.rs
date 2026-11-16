//! Tauri 2 application shell.
//!
//! The Tauri window is a thin viewport over the same Axum API the headless
//! CLI exposes. In `setup` we:
//!   1. Read the API address: 127.0.0.1:7878, or `WIPESTATION_API_ADDR`
//!      (`127.0.0.1:0` picks a free port).
//!   2. Bind it, *before* touching the app-data directory, so a second copy
//!      of the app (or any program already on the port) fails without
//!      touching this copy's layout, lease state or signing key.
//!   3. Build the station with the address actually bound, keeping saved
//!      configuration, lease state and the signing key in the per-user
//!      app-data directory (see [`station_config`]), and serve it.
//!   4. Create the (initially hidden) `main` window from `tauri.conf.json`
//!      (`create: false`, no URL there) pointed at the bound address, wait
//!      for `/api/health`, and show it.
//!
//! If any of 1-3 fails, the window opens instead on a small error page the
//! shell serves on a free loopback port, naming the reason. The window never
//! loads a URL this process did not bind.
//!
//! This collapses the "browser at localhost" and "Tauri window" modes onto
//! one origin — same code, same UI, same API.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use tauri::{WebviewUrl, WebviewWindowBuilder};
use tokio::net::TcpListener;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use wipe_server::{
    bootstrap::{build_station, Fleet, StationConfig},
    AppState,
};

const DEFAULT_ADDR: &str = "127.0.0.1:7878";
const API_ADDR_ENV: &str = "WIPESTATION_API_ADDR";
/// Label of the window in `tauri.conf.json` the shell creates at runtime.
const MAIN_WINDOW: &str = "main";

pub fn run() {
    // Init logging unconditionally so the Tauri window's stderr is informative.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();

    // Lossy, so a non-UTF-8 value is rejected by name rather than ignored.
    let api_addr =
        api_addr_from_env(std::env::var_os(API_ADDR_ENV).map(|v| v.to_string_lossy().into_owned()));
    info!(?api_addr, "starting wipestation desktop shell");

    tauri::Builder::default()
        .setup(move |app| {
            use tauri::Manager;
            let resource_dist = app.path().resource_dir().ok().map(|d| d.join("dist"));
            // Machine-local, never roaming: the signing key and lease watermark
            // must not follow a user to another PC.
            let app_data_dir = app.path().app_local_data_dir().map_err(|e| e.to_string());
            let build = desktop_station(resource_dist, app_data_dir, Fleet::BestEffort);

            // An error page that cannot be served fails setup: Tauri exits
            // non-zero rather than open a window on nothing.
            let started = tauri::async_runtime::block_on(start_or_explain(api_addr, build))?;
            let url = started.url.clone();
            let ready = started.station.is_some();
            if let Some((state, listener)) = started.station {
                tauri::async_runtime::spawn(async move {
                    match wipe_server::serve_on(state, listener).await {
                        Ok(()) => info!("api server exited"),
                        Err(e) => error!(?e, "api server crashed"),
                    }
                });
            }

            let window = main_window_config(&app.config().app.windows, url.clone())?;
            let window = WebviewWindowBuilder::from_config(app.handle(), &window)?.build()?;

            if !ready {
                let _ = window.show();
                let _ = window.set_focus();
                return Ok(());
            }
            // Reveal the window once the station answers.
            tauri::async_runtime::spawn(async move {
                if wait_for_ready(&url, Duration::from_secs(15)).await {
                    info!("api ready — showing window");
                    let _ = window.show();
                    let _ = window.set_focus();
                } else {
                    warn!("api did not answer /api/health within timeout — showing window anyway");
                    let _ = window.show();
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// The API address from `WIPESTATION_API_ADDR`'s value; unset is the
/// default. A value that is not a socket address is an error naming it,
/// never a silent fallback to the default.
fn api_addr_from_env(value: Option<String>) -> Result<SocketAddr, String> {
    match value {
        None => Ok(DEFAULT_ADDR
            .parse()
            .expect("DEFAULT_ADDR is a socket address")),
        Some(v) => v.parse().map_err(|e| {
            format!(
                "{API_ADDR_ENV} is set to {v:?}, which is not an address and port such as \
                 {DEFAULT_ADDR} ({e}). Correct or unset it and start Wipestation again."
            )
        }),
    }
}

/// Bind `addr`, then build the station with the address actually bound (its
/// port is what mDNS advertises). Nothing runs `build` unless the bind
/// succeeded, so a busy port writes nothing to the app-data directory. No
/// other port is tried.
async fn start_station(
    addr: SocketAddr,
    build: impl FnOnce(SocketAddr) -> anyhow::Result<AppState>,
) -> Result<(AppState, TcpListener), String> {
    let listener = TcpListener::bind(addr).await.map_err(|e| {
        format!(
            "Wipestation could not listen on {addr}: {e}. If another copy of Wipestation \
             is already running, use that copy's window instead: do not start a second copy \
             on another address, because every copy shares one app-data directory (bench \
             layout, lease state and signing key). If another program is using the address, \
             quit it, or set {API_ADDR_ENV} to another address ({API_ADDR_ENV}=127.0.0.1:0 \
             picks a free port), and start Wipestation again."
        )
    })?;
    let bound = listener
        .local_addr()
        .map_err(|e| format!("Wipestation could not read the address it bound ({addr}): {e}"))?;
    let state = build(bound).map_err(|e| format!("{e:#}"))?;
    Ok((state, listener))
}

/// The `main` window's configuration from `tauri.conf.json`, pointed at
/// `url` (always one built by [`window_url`]).
fn main_window_config(
    windows: &[tauri::utils::config::WindowConfig],
    url: url::Url,
) -> Result<tauri::utils::config::WindowConfig, String> {
    let mut window = windows
        .iter()
        .find(|w| w.label == MAIN_WINDOW)
        .cloned()
        .ok_or_else(|| format!("tauri.conf.json has no `{MAIN_WINDOW}` window"))?;
    window.url = WebviewUrl::External(url);
    Ok(window)
}

/// What the window opens on: the station's address with the station to
/// serve there, or the start-up error page's address and no station.
struct Started {
    url: url::Url,
    station: Option<(AppState, TcpListener)>,
}

/// Start the station on `api_addr`, or, if the address is invalid, the bind
/// fails or the station cannot be built, serve the error page naming why.
/// Errs only if the error page itself cannot be served.
async fn start_or_explain(
    api_addr: Result<SocketAddr, String>,
    build: impl FnOnce(SocketAddr) -> anyhow::Result<AppState>,
) -> std::io::Result<Started> {
    let started = match api_addr {
        Ok(addr) => start_station(addr, build).await,
        Err(reason) => Err(reason),
    };
    match started {
        Ok((state, listener)) => {
            let bound = listener.local_addr()?;
            info!(%bound, "api bound");
            Ok(Started {
                url: window_url(bound),
                station: Some((state, listener)),
            })
        }
        Err(reason) => {
            error!(%reason, "station did not start; showing the start-up error page");
            let page = serve_startup_error(reason).await?;
            Ok(Started {
                url: window_url(page),
                station: None,
            })
        }
    }
}

/// The URL the window loads for a listener bound at `addr`. An unspecified
/// address (`0.0.0.0`, `::`) is reached over loopback on the same port.
fn window_url(addr: SocketAddr) -> url::Url {
    let ip = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    url::Url::parse(&format!("http://{}/", SocketAddr::new(ip, addr.port())))
        .expect("a socket address forms an http URL")
}

/// The page the window shows when the station did not start. `reason` is
/// HTML-escaped.
fn startup_error_page(reason: &str) -> String {
    let reason = html_escape(reason);
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Wipestation could not start</title>
<style>body{{font-family:ui-sans-serif,system-ui,sans-serif;background:#020617;color:#e2e8f0;padding:2rem;max-width:46rem;margin:auto}}
pre{{background:#0f172a;padding:1rem;border-radius:.375rem;white-space:pre-wrap;word-break:break-word;color:#fca5a5}}
h1{{margin-top:0}}</style>
</head><body>
<h1>Wipestation could not start</h1>
<p>The station did not start, so no drives can be erased from this window.</p>
<pre>{reason}</pre>
<p>Close this window, fix the cause above, and start Wipestation again.</p>
</body></html>
"#
    )
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Serve the start-up error page on a free loopback port and return it.
/// Every path, `/api/health` included, answers 503 with the page, so
/// nothing mistakes it for a running station.
async fn serve_startup_error(reason: String) -> std::io::Result<SocketAddr> {
    serve_startup_error_at(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), reason).await
}

async fn serve_startup_error_at(bind: SocketAddr, reason: String) -> std::io::Result<SocketAddr> {
    use axum::http::{header, StatusCode};
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let page: std::sync::Arc<str> = startup_error_page(&reason).into();
    let app = axum::Router::new().fallback(move || {
        let page = page.clone();
        async move {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [
                    (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                page.to_string(),
            )
        }
    });
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            error!(?e, "start-up error page stopped");
        }
    });
    Ok(addr)
}

/// File names the desktop keeps in its app-data directory. The topology and
/// lease names are the store's own (ADR-0003, ADR-0005 §3a).
const BAY_TOPOLOGY_FILE: &str = "bay-topology.json";
const SIGNING_KEY_FILE: &str = "station.key";

/// What the desktop can do with its per-user app-data directory
/// (`app.path().app_local_data_dir()`, identifier `dev.wipestation.desktop`):
/// the machine-local app-data dir (`%LOCALAPPDATA%` on Windows, not the
/// roaming profile), so the signing key and lease state stay on this PC.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DataDir {
    /// Resolved, created and passed the write probe.
    Writable(PathBuf),
    /// Resolved, but could not be created or written; the reason.
    Unwritable(PathBuf, String),
    /// Tauri could not resolve it; the error.
    Unavailable(String),
}

/// Create the app-data directory and check it with the store's own write
/// probe (`LocalFileStore::probe`), so the verdict matches what the store
/// will find when it detects its tier.
fn prepare_data_dir(resolved: Result<PathBuf, String>) -> DataDir {
    let dir = match resolved {
        Ok(dir) => dir,
        Err(e) => return DataDir::Unavailable(e),
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return DataDir::Unwritable(dir, format!("cannot create it: {e}"));
    }
    match wipe_server::store::LocalFileStore::probe(&dir.join(BAY_TOPOLOGY_FILE)) {
        Ok(()) => DataDir::Writable(dir),
        Err(e) => DataDir::Unwritable(dir, format!("cannot write to it: {e}")),
    }
}

fn log_data_dir(data_dir: &DataDir) {
    match data_dir {
        DataDir::Writable(dir) => info!(dir = %dir.display(), "app-data directory ready"),
        DataDir::Unwritable(dir, reason) => warn!(
            dir = %dir.display(), %reason,
            "app-data directory not writable; configuration cannot be saved (the UI asks the \
             operator to acknowledge with \"Continue without saving\"), lease state is held \
             in memory, and the signing key is fresh for this run"
        ),
        DataDir::Unavailable(reason) => warn!(
            %reason,
            "no app-data directory; configuration and lease state are held in memory only, \
             and the signing key is fresh for this run"
        ),
    }
}

/// The desktop's station: best-effort mDNS, a random fleet station id per
/// launch, no licence. The resource dir's `dist` is tried before the shared
/// bundle locations.
///
/// Persistence follows `data_dir`:
///   * writable: store tiers detected at `<dir>/bay-topology.json` (lease
///     state beside it) and the signing seed kept in `<dir>/station.key`;
///   * unwritable: the same detection, which falls to ADR-0003 tier 3 (the
///     UI warns that configuration cannot be saved and the operator
///     acknowledges with "Continue without saving"), and a fresh key for this
///     run;
///   * unavailable: everything in memory and a fresh key.
fn station_config(
    addr: SocketAddr,
    resource_dist: Option<PathBuf>,
    data_dir: &DataDir,
) -> wipe_server::bootstrap::StationConfig {
    use wipe_server::bootstrap::Persistence;
    let detect_in = |dir: &PathBuf| Persistence::Detect {
        control_plane_url: None,
        ephemeral_config: false,
        config_dir: Some(dir.clone()),
    };
    let (persistence, key_path) = match data_dir {
        DataDir::Writable(dir) => (detect_in(dir), Some(dir.join(SIGNING_KEY_FILE))),
        DataDir::Unwritable(dir, _) => (detect_in(dir), None),
        DataDir::Unavailable(e) => (
            Persistence::InMemory {
                reason: format!(
                    "The desktop app could not find its app-data directory ({e}), so it keeps \
                     configuration and licence lease state in memory only, and its \
                     certificate signing key is fresh each launch."
                ),
            },
            None,
        ),
    };
    StationConfig {
        hostname: hostname_or_default(),
        fleet: Fleet::BestEffort,
        host_static_candidates: resource_dist.into_iter().collect(),
        persistence,
        key_path,
        ..StationConfig::new(addr)
    }
}

/// The desktop's station builder, given the address the shell bound:
/// prepare the app-data directory, then build through the shared bootstrap
/// (`wipe_server::bootstrap`). It runs only after a successful bind. Tests
/// pass `Fleet::Off` so they advertise nothing on the LAN.
fn desktop_station(
    resource_dist: Option<PathBuf>,
    app_data_dir: Result<PathBuf, String>,
    fleet: Fleet,
) -> impl FnOnce(SocketAddr) -> anyhow::Result<AppState> {
    move |addr| {
        let data_dir = prepare_data_dir(app_data_dir);
        log_data_dir(&data_dir);
        let cfg = desktop_config(addr, resource_dist, &data_dir, fleet);
        build_station(cfg, &mut |line| info!("{line}"))
    }
}

/// The desktop's [`StationConfig`] for the address the shell bound (its
/// port is what mDNS advertises), with `fleet` as given.
fn desktop_config(
    bound: SocketAddr,
    resource_dist: Option<PathBuf>,
    data_dir: &DataDir,
    fleet: Fleet,
) -> StationConfig {
    StationConfig {
        fleet,
        ..station_config(bound, resource_dist, data_dir)
    }
}

fn hostname_or_default() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "wipestation-desktop".into())
}

async fn wait_for_ready(base: &url::Url, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    let Ok(url) = base.join("api/health") else {
        return false;
    };
    loop {
        // A peer that accepts but never closes must not hold the loop past
        // its deadline; a timed-out attempt counts as not ready.
        let attempt = tokio::time::timeout_at(deadline, reqwest_get(&url)).await;
        if let Ok(Ok(true)) = attempt {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Whether `GET url` answers 200.
async fn reqwest_get(url: &url::Url) -> Result<bool, std::io::Error> {
    Ok(http_get(url).await?.status_line.starts_with("HTTP/1.1 200"))
}

/// A response read by [`http_get`]. Production reads only the status line;
/// the tests read the rest.
#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
struct RawResponse {
    status_line: String,
    /// Header lines, as sent.
    headers: Vec<String>,
    body: String,
}

impl RawResponse {
    #[cfg(test)]
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find_map(|line| {
            let (n, v) = line.split_once(':')?;
            n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }
}

/// Hand-rolled, no dependency: open a TCP connection, send a minimal GET
/// with `Connection: close` and read the whole response. Chunked bodies are
/// returned undecoded.
async fn http_get(url: &url::Url) -> Result<RawResponse, std::io::Error> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let invalid = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, m.to_string());
    let host = url.host_str().ok_or_else(|| invalid("no host"))?;
    let addrs = url.socket_addrs(|| Some(80))?;
    let mut stream = tokio::net::TcpStream::connect(&*addrs).await?;
    // RFC 7230 §5.4: a non-default port is part of the Host header.
    let host = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n",
        url.path()
    );
    stream.write_all(req.as_bytes()).await?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await?;
    let raw = String::from_utf8_lossy(&raw);
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default().to_string();
    Ok(RawResponse {
        status_line,
        headers: lines.map(str::to_string).collect(),
        body: body.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wipe_server::{bootstrap::Persistence, StoreTier};

    fn addr() -> SocketAddr {
        "127.0.0.1:7878".parse().unwrap()
    }

    fn tmpdir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("wipestation-desktop-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_desktop_station_prefers_its_resource_dist_and_has_no_licence() {
        let dist = PathBuf::from("/resources/dist");
        let data = DataDir::Writable(PathBuf::from("/data"));
        let cfg = station_config(addr(), Some(dist.clone()), &data);
        assert_eq!(cfg.addr, addr());
        assert_eq!(cfg.fleet, Fleet::BestEffort);
        assert!(cfg.station_id.is_none());
        assert!(cfg.bay_topology.is_none());
        assert!(cfg.license.is_none());
        assert!(cfg.vendor_roots.is_empty());
        assert!(cfg.static_dir.is_none());
        assert_eq!(cfg.host_static_candidates.first(), Some(&dist));

        assert!(station_config(addr(), None, &data)
            .host_static_candidates
            .is_empty());
    }

    #[test]
    fn a_writable_data_dir_detects_stores_there_and_keeps_the_key_there() {
        let dir = PathBuf::from("/data");
        let cfg = station_config(addr(), None, &DataDir::Writable(dir.clone()));
        match cfg.persistence {
            Persistence::Detect {
                control_plane_url,
                ephemeral_config,
                config_dir,
            } => {
                assert!(control_plane_url.is_none());
                assert!(!ephemeral_config);
                assert_eq!(config_dir, Some(dir.clone()));
            }
            other => panic!("expected Detect, got {other:?}"),
        }
        assert_eq!(cfg.key_path, Some(dir.join("station.key")));
    }

    #[test]
    fn an_unwritable_data_dir_still_detects_but_uses_a_fresh_key() {
        let dir = PathBuf::from("/data");
        let cfg = station_config(
            addr(),
            None,
            &DataDir::Unwritable(dir.clone(), "read-only".into()),
        );
        match cfg.persistence {
            Persistence::Detect { config_dir, .. } => assert_eq!(config_dir, Some(dir)),
            other => panic!("expected Detect, got {other:?}"),
        }
        assert!(cfg.key_path.is_none());
    }

    #[test]
    fn no_data_dir_keeps_everything_in_memory_and_says_why() {
        let cfg = station_config(
            addr(),
            None,
            &DataDir::Unavailable("no home directory".into()),
        );
        match cfg.persistence {
            Persistence::InMemory { reason } => {
                assert!(reason.contains("no home directory"), "{reason}");
                assert!(
                    reason.contains("signing key is fresh each launch"),
                    "{reason}"
                );
            }
            other => panic!("expected InMemory, got {other:?}"),
        }
        assert!(cfg.key_path.is_none());
    }

    #[test]
    fn prepare_data_dir_creates_a_missing_dir_and_passes_errors_through() {
        let dir = tmpdir("prepare").join("nested");
        assert_eq!(
            prepare_data_dir(Ok(dir.clone())),
            DataDir::Writable(dir.clone())
        );
        assert!(dir.is_dir());
        assert_eq!(
            prepare_data_dir(Err("unknown path".into())),
            DataDir::Unavailable("unknown path".into())
        );
    }

    #[test]
    fn a_file_where_the_data_dir_should_be_is_unwritable() {
        let parent = tmpdir("not-a-dir");
        std::fs::create_dir_all(&parent).unwrap();
        let file = parent.join("app-data");
        std::fs::write(&file, b"not a directory").unwrap();
        match prepare_data_dir(Ok(file.clone())) {
            DataDir::Unwritable(dir, reason) => {
                assert_eq!(dir, file);
                assert!(!reason.is_empty());
            }
            other => panic!("expected Unwritable, got {other:?}"),
        }
    }

    /// An app-data "dir" that cannot be created (a file stands there) still
    /// starts a station: the store falls to ADR-0003 tier 3, lease state is
    /// RAM-only and the key is fresh for the run.
    #[tokio::test]
    async fn an_unwritable_data_dir_starts_a_station_that_asks_the_operator() {
        let parent = tmpdir("unwritable-build");
        std::fs::create_dir_all(&parent).unwrap();
        let file = parent.join("app-data");
        std::fs::write(&file, b"not a directory").unwrap();
        let data = prepare_data_dir(Ok(file.clone()));
        assert!(matches!(data, DataDir::Unwritable(..)), "{data:?}");
        let first = assert_starts_unsaved(&data);
        let second = assert_starts_unsaved(&data);
        assert_ne!(
            first.signing_key.public_key_id(),
            second.signing_key.public_key_id(),
            "an unwritable app-data dir signs each run with a fresh key"
        );
    }

    /// No app-data dir at all: everything in memory, and the station says
    /// its signing key is fresh each launch.
    #[tokio::test]
    async fn an_unavailable_data_dir_starts_an_in_memory_station() {
        let state = build_desktop_station(&DataDir::Unavailable("no home directory".into()));
        assert_eq!(state.topology_store.tier(), StoreTier::Ephemeral);
        let lease = state.license().lease_persistence();
        assert_eq!(lease.tier, StoreTier::Ephemeral);
        let detail = state.topology_store.detail();
        assert!(detail.contains("no home directory"), "{detail}");
        assert!(
            detail.contains("signing key is fresh each launch"),
            "{detail}"
        );
    }

    /// A desktop station built from `station_config` with the fleet off, so
    /// tests advertise nothing on the LAN.
    fn build_desktop_station(data: &DataDir) -> wipe_server::AppState {
        let cfg = wipe_server::bootstrap::StationConfig {
            fleet: Fleet::Off,
            ..station_config("127.0.0.1:0".parse().unwrap(), None, data)
        };
        build_station(cfg, &mut |_| {}).unwrap()
    }

    /// What an unwritable app-data dir must still start: Ephemeral topology
    /// awaiting the operator's "Continue without saving", RAM-only lease
    /// state. Returns the station.
    fn assert_starts_unsaved(data: &DataDir) -> wipe_server::AppState {
        let state = build_desktop_station(data);
        assert_eq!(state.topology_store.tier(), StoreTier::Ephemeral);
        assert!(state.topology_store.awaiting_decision());
        assert_eq!(
            state.license().lease_persistence().tier,
            StoreTier::Ephemeral
        );
        state
    }

    /// An existing directory the write probe rejects (not a failed create).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_read_only_data_dir_fails_the_probe_and_still_starts_a_station() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmpdir("read-only");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        // Give the dir back its write bit however the test ends.
        struct Restore<'a>(&'a std::path::Path);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o700));
            }
        }
        let _restore = Restore(&dir);
        // Root ignores the mode bits; nothing to test there.
        let probe = dir.join("root-check");
        if std::fs::write(&probe, b"").is_ok() {
            let _ = std::fs::remove_file(&probe);
            eprintln!("skipped: running as a user that can write a 0o500 directory");
            return;
        }
        let data = prepare_data_dir(Ok(dir.clone()));
        match &data {
            DataDir::Unwritable(d, reason) => {
                assert_eq!(d, &dir);
                assert!(reason.starts_with("cannot write to it"), "{reason}");
            }
            other => panic!("expected Unwritable, got {other:?}"),
        }
        assert_starts_unsaved(&data);
        assert!(!dir.join("station.key").exists());
    }

    /// Layout, lease state and signing key survive a relaunch.
    #[tokio::test]
    async fn a_relaunched_desktop_station_keeps_its_layout_lease_and_key() {
        let dir = tmpdir("relaunch");
        let data = prepare_data_dir(Ok(dir.clone()));
        assert_eq!(data, DataDir::Writable(dir.clone()));

        let first = build_desktop_station(&data);
        assert!(first.bay_topology.read().is_none());
        assert_eq!(first.topology_store.tier(), StoreTier::LocalFile);
        let mut layout = wipe_common::preset("dock-2bay").unwrap();
        layout.label = "Saved on the desktop".into();
        first.save_bay_topology(layout).unwrap();
        let decision = first
            .license()
            .decide_start(
                &wipe_server::StartRequest {
                    method: None,
                    drives: 1,
                    simulated: first.simulated(),
                },
                time::OffsetDateTime::now_utc(),
            )
            .unwrap();
        assert!(decision.evaluation);
        first.license().record_evaluation_erasure();
        let first_lease = first.license().lease_state();
        assert_eq!(first_lease.evaluation_erasures, 1);
        let first_key_id = first.signing_key.public_key_id();
        drop(first);

        let second = build_desktop_station(&data);
        let reloaded = second.bay_topology.read().clone().unwrap();
        assert_eq!(reloaded.label, "Saved on the desktop");
        assert_eq!(first_key_id, second.signing_key.public_key_id());
        assert_eq!(second.topology_store.tier(), StoreTier::LocalFile);
        assert_eq!(
            second.license().lease_persistence().tier,
            StoreTier::LocalFile
        );
        assert_eq!(
            second.license().lease_state().evaluation_erasures,
            first_lease.evaluation_erasures
        );
        assert!(dir.join("station.key").is_file());
        assert!(dir.join("bay-topology.json").is_file());
        assert!(dir.join("lease-state.json").is_file());
    }

    /// CAP-3 on the desktop's own config: the mock simulator answers.
    #[tokio::test]
    async fn the_desktop_station_serves_the_simulator() {
        let dir = tmpdir("sim");
        let data = prepare_data_dir(Ok(dir));
        let state = build_desktop_station(&data);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, wipe_server::router(state))
                .await
                .unwrap();
        });
        let url = window_url(bound).join("api/sim/devices").unwrap();
        let ok = reqwest_get(&url).await.unwrap();
        assert!(ok, "GET /api/sim/devices did not answer 200");
    }

    // ---- start-up: bind, then build; the window follows the bound address ----

    /// The production station builder with the fleet off.
    fn quiet_station(dir: &std::path::Path) -> impl FnOnce(SocketAddr) -> anyhow::Result<AppState> {
        desktop_station(None, Ok(dir.to_path_buf()), Fleet::Off)
    }

    /// Assert `url` serves the start-up error page and return its body.
    async fn assert_error_page(url: &url::Url) -> String {
        let res = http_get(url).await.unwrap();
        assert!(
            res.status_line.starts_with("HTTP/1.1 503"),
            "{}",
            res.status_line
        );
        assert_eq!(res.header("content-type"), Some("text/html; charset=utf-8"));
        assert_eq!(res.header("cache-control"), Some("no-store"));
        assert!(
            res.body.contains("Wipestation could not start"),
            "{}",
            res.body
        );
        res.body
    }

    #[test]
    fn the_api_address_comes_from_the_env_or_the_default_and_never_falls_back() {
        assert_eq!(api_addr_from_env(None), Ok(addr()));
        assert_eq!(
            api_addr_from_env(Some("127.0.0.1:0".into())),
            Ok("127.0.0.1:0".parse().unwrap())
        );
        assert_eq!(
            api_addr_from_env(Some("[::1]:9000".into())),
            Ok("[::1]:9000".parse().unwrap())
        );
        for bad in ["nonsense", "", "127.0.0.1", "localhost:7878"] {
            let e = api_addr_from_env(Some(bad.into())).unwrap_err();
            assert!(e.contains("WIPESTATION_API_ADDR"), "{e}");
            assert!(e.contains(&format!("{bad:?}")), "{e}");
        }
    }

    #[test]
    fn the_window_url_is_the_bound_address() {
        let url = |a: &str| window_url(a.parse().unwrap()).to_string();
        assert_eq!(url("127.0.0.1:7878"), "http://127.0.0.1:7878/");
        assert_eq!(url("127.0.0.1:54321"), "http://127.0.0.1:54321/");
        assert_eq!(url("[::1]:7878"), "http://[::1]:7878/");
        assert_eq!(url("0.0.0.0:7878"), "http://127.0.0.1:7878/");
        assert_eq!(url("[::]:7878"), "http://[::1]:7878/");
        assert_eq!(url("192.168.1.5:80"), "http://192.168.1.5/");
    }

    /// Tauri must create no window before `setup` gives it the bound URL.
    #[test]
    fn tauri_conf_creates_no_window_before_setup() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let windows = conf["app"]["windows"].as_array().unwrap();
        let main = windows
            .iter()
            .find(|w| w["label"] == MAIN_WINDOW)
            .expect("a `main` window");
        assert_eq!(main["create"], serde_json::Value::Bool(false));
        assert!(main.get("url").is_none(), "{main}");
        assert_eq!(main["visible"], serde_json::Value::Bool(false));
    }

    /// Port 0: the station is built with, and served on, the port actually
    /// bound, and the window URL answers `/api/health`.
    #[tokio::test]
    async fn port_zero_builds_and_serves_the_station_on_the_bound_port() {
        let dir = tmpdir("port-zero");
        let seen = std::sync::Arc::new(parking_lot::Mutex::new(None));
        let build = {
            let seen = seen.clone();
            let inner = quiet_station(&dir);
            move |a: SocketAddr| {
                *seen.lock() = Some(a);
                inner(a)
            }
        };
        let started = start_or_explain(api_addr_from_env(Some("127.0.0.1:0".into())), build)
            .await
            .unwrap();
        let (state, listener) = started.station.expect("a station");
        let bound = listener.local_addr().unwrap();
        assert_ne!(bound.port(), 0);
        assert_eq!(*seen.lock(), Some(bound));
        assert_eq!(started.url, window_url(bound));
        assert_eq!(
            started.url.as_str(),
            format!("http://127.0.0.1:{}/", bound.port())
        );
        tokio::spawn(wipe_server::serve_on(state, listener));
        let health = started.url.join("api/health").unwrap();
        let res = http_get(&health).await.unwrap();
        assert!(res.status_line.starts_with("HTTP/1.1 200"), "{res:?}");
        assert!(wait_for_ready(&started.url, Duration::from_secs(5)).await);
    }

    /// A busy port: no other port is tried, nothing touches the app-data dir,
    /// and the window URL serves an error naming the address and the env var.
    #[tokio::test]
    async fn a_busy_port_shows_the_error_and_writes_nothing() {
        let occupant = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let busy = occupant.local_addr().unwrap();
        let dir = tmpdir("busy");

        let reason = match start_station(busy, quiet_station(&dir)).await {
            Ok(_) => panic!("bound a busy port"),
            Err(reason) => reason,
        };
        assert!(reason.contains(&busy.to_string()), "{reason}");
        assert!(reason.contains("WIPESTATION_API_ADDR"), "{reason}");
        assert!(reason.contains("127.0.0.1:0"), "{reason}");
        assert!(reason.contains("use that copy's window"), "{reason}");

        let started = start_or_explain(Ok(busy), quiet_station(&dir))
            .await
            .unwrap();
        assert!(started.station.is_none());
        assert_ne!(started.url, window_url(busy));
        let body = assert_error_page(&started.url).await;
        assert!(body.contains(&busy.to_string()), "{body}");
        assert!(body.contains("WIPESTATION_API_ADDR"), "{body}");
        assert!(!dir.exists(), "the app-data dir was created");
    }

    /// An unparsable `WIPESTATION_API_ADDR`: no bind, no build; the error
    /// page names the rejected value.
    #[tokio::test]
    async fn a_bad_api_addr_shows_the_rejected_value() {
        let built = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let build = {
            let built = built.clone();
            move |_: SocketAddr| -> anyhow::Result<AppState> {
                built.store(true, std::sync::atomic::Ordering::SeqCst);
                anyhow::bail!("must not build")
            }
        };
        let started = start_or_explain(api_addr_from_env(Some("nonsense".into())), build)
            .await
            .unwrap();
        assert!(started.station.is_none());
        assert!(!built.load(std::sync::atomic::Ordering::SeqCst));
        let body = assert_error_page(&started.url).await;
        assert!(body.contains("&quot;nonsense&quot;"), "{body}");
    }

    /// A corrupt signing key fails the build; the error page names the key
    /// file, which is left as it was.
    #[tokio::test]
    async fn a_corrupt_key_shows_the_build_error_and_keeps_the_key() {
        let dir = tmpdir("corrupt-key");
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("station.key");
        std::fs::write(&key, b"short").unwrap();

        let reason = match start_station("127.0.0.1:0".parse().unwrap(), quiet_station(&dir)).await
        {
            Ok(_) => panic!("built a station on a corrupt key"),
            Err(reason) => reason,
        };
        assert!(reason.contains("station.key"), "{reason}");

        let started = start_or_explain(Ok("127.0.0.1:0".parse().unwrap()), quiet_station(&dir))
            .await
            .unwrap();
        assert!(started.station.is_none());
        let body = assert_error_page(&started.url).await;
        assert!(body.contains("station.key"), "{body}");
        assert_eq!(std::fs::read(&key).unwrap(), b"short");
    }

    /// The error page answers every path with 503, escapes the reason and is
    /// never the React bundle.
    #[tokio::test]
    async fn the_error_page_escapes_the_reason_and_answers_503_everywhere() {
        let reason = r#"<script>alert('x')</script> & "quoted""#;
        let page = startup_error_page(reason);
        assert!(page
            .contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; &amp; &quot;quoted&quot;"));
        assert!(!page.contains("<script"), "{page}");

        let addr = serve_startup_error(reason.into()).await.unwrap();
        assert!(addr.ip().is_loopback());
        let base = window_url(addr);
        for path in ["", "api/health", "index.html", "assets/app.js"] {
            let body = assert_error_page(&base.join(path).unwrap()).await;
            assert!(!body.contains("<script"), "{path}: {body}");
            assert!(body.contains("&lt;script&gt;"), "{path}: {body}");
        }
        assert!(!wait_for_ready(&base, Duration::from_millis(200)).await);
    }

    /// If even the error page cannot be bound, the error comes back (setup
    /// returns it and Tauri exits non-zero).
    #[tokio::test]
    async fn an_unservable_error_page_is_an_error() {
        let occupant = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let busy = occupant.local_addr().unwrap();
        assert!(serve_startup_error_at(busy, "reason".into()).await.is_err());
    }

    /// The real `tauri.conf.json` windows.
    fn conf_windows() -> Vec<tauri::utils::config::WindowConfig> {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        serde_json::from_value(conf["app"]["windows"].clone()).unwrap()
    }

    #[test]
    fn the_main_window_is_pointed_at_the_given_url_and_keeps_its_config() {
        let windows = conf_windows();
        let conf_main = windows.iter().find(|w| w.label == MAIN_WINDOW).unwrap();
        let station = window_url("127.0.0.1:54321".parse().unwrap());
        let page = window_url("127.0.0.1:61000".parse().unwrap());
        for url in [station, page] {
            let w = main_window_config(&windows, url.clone()).unwrap();
            assert_eq!(w.url, WebviewUrl::External(url));
            assert_eq!(w.label, MAIN_WINDOW);
            assert_eq!(w.title, conf_main.title);
            assert_eq!(w.title, "Wipestation");
            assert_eq!((w.width, w.height), (conf_main.width, conf_main.height));
            assert!(!w.visible);
            assert!(!w.create);
        }
        let others: Vec<_> = windows
            .into_iter()
            .filter(|w| w.label != MAIN_WINDOW)
            .collect();
        let e = main_window_config(&others, window_url(addr())).unwrap_err();
        assert!(e.contains("main"), "{e}");
    }

    #[test]
    fn the_desktop_config_uses_the_bound_address_and_the_given_fleet() {
        let bound: SocketAddr = "127.0.0.1:54321".parse().unwrap();
        let data = DataDir::Unavailable("none".into());
        for fleet in [Fleet::Off, Fleet::BestEffort] {
            let cfg = desktop_config(bound, None, &data, fleet);
            assert_eq!(cfg.addr, bound);
            assert_eq!(cfg.fleet, fleet);
        }
    }

    /// A peer that accepts and never answers does not hold the wait past
    /// its deadline.
    #[tokio::test]
    async fn waiting_for_a_silent_peer_ends_at_the_deadline() {
        let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = window_url(silent.local_addr().unwrap());
        let held = tokio::spawn(async move {
            let mut conns = Vec::new();
            while let Ok((c, _)) = silent.accept().await {
                conns.push(c);
            }
        });
        let started = std::time::Instant::now();
        assert!(!wait_for_ready(&base, Duration::from_millis(300)).await);
        assert!(started.elapsed() < Duration::from_secs(2));
        held.abort();
    }
}
