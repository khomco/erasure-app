//! Tauri 2 application shell.
//!
//! The Tauri window is a thin viewport over the same Axum API the headless
//! CLI exposes. We:
//!   1. Initialize tracing so a developer can see what's happening.
//!   2. Bind the API on 127.0.0.1:7878 (or `WIPESTATION_API_ADDR`), with
//!      saved configuration, lease state and the signing key kept in the
//!      per-user app-data directory (see [`station_config`]).
//!   3. Wait for the API to answer `/api/health`.
//!   4. Show the (initially-hidden) window pointed at the API.
//!
//! This collapses the "browser at localhost" and "Tauri window" modes onto
//! one origin — same code, same UI, same API.

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use tauri::Manager;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

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
            let resource_dist = app.path().resource_dir().ok().map(|d| d.join("dist"));
            // Machine-local, never roaming: the signing key and lease watermark
            // must not follow a user to another PC.
            let app_data_dir = app.path().app_local_data_dir().map_err(|e| e.to_string());
            tauri::async_runtime::spawn(async move {
                match bootstrap_api(api_addr, resource_dist, app_data_dir).await {
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
    use wipe_server::bootstrap::{Fleet, Persistence, StationConfig};
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

/// Build the station with the shared bootstrap (`wipe_server::bootstrap`) and
/// serve it.
async fn bootstrap_api(
    addr: SocketAddr,
    resource_dist: Option<PathBuf>,
    app_data_dir: Result<PathBuf, String>,
) -> anyhow::Result<()> {
    let data_dir = prepare_data_dir(app_data_dir);
    log_data_dir(&data_dir);
    let cfg = station_config(addr, resource_dist, &data_dir);
    let state = wipe_server::bootstrap::build_station(cfg, &mut |line| info!("{line}"))?;
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
    let parsed = url::Url::parse(url)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use wipe_server::{
        bootstrap::{build_station, Fleet, Persistence},
        StoreTier,
    };

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
            .signing_decision(time::OffsetDateTime::now_utc());
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
        let ok = reqwest_get(&format!("http://{bound}/api/sim/devices"))
            .await
            .unwrap();
        assert!(ok, "GET /api/sim/devices did not answer 200");
    }
}
