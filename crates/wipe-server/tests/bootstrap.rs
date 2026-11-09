//! The shared station bootstrap both hosts start from: `wipestation serve`
//! and the Tauri desktop shell call `wipe_server::bootstrap::build_station`.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Mutex,
};

use wipe_server::{
    bootstrap::{build_station, resolve_frontend_dist, Fleet, Persistence, StationConfig},
    router, StoreTier, TopologyStore,
};

/// Tests that set `WIPESTATION_CONFIG_DIR` hold this, so they do not see each
/// other's value.
static ENV: Mutex<()> = Mutex::new(());

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wipestation-bootstrap-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn addr() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

/// Mirrors the desktop shell's posture (`apps/desktop/src-tauri/src/lib.rs`)
/// for a writable app-data dir `dir`, minus mDNS: `Fleet::Off`, so tests
/// advertise nothing on the LAN. The best-effort fleet handling is covered by
/// `start_fleet`'s unit test.
fn desktop_config(dir: &Path) -> StationConfig {
    StationConfig {
        hostname: "wipestation-desktop".into(),
        fleet: Fleet::Off,
        key_path: Some(dir.join("station.key")),
        persistence: Persistence::Detect {
            control_plane_url: None,
            ephemeral_config: false,
            config_dir: Some(dir.to_path_buf()),
        },
        ..StationConfig::new(addr())
    }
}

/// The desktop's fallback when it has no app-data directory.
fn in_memory_config() -> StationConfig {
    StationConfig {
        hostname: "wipestation-desktop".into(),
        fleet: Fleet::Off,
        persistence: Persistence::InMemory {
            reason: "The desktop app could not find its app-data directory (test), so it \
                     keeps configuration and licence lease state in memory only."
                .into(),
        },
        ..StationConfig::new(addr())
    }
}

fn build(cfg: StationConfig) -> (wipe_server::AppState, Vec<String>) {
    let mut lines = Vec::new();
    let state = build_station(cfg, &mut |l| lines.push(l)).unwrap();
    (state, lines)
}

fn list(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect()
}

#[tokio::test]
async fn a_key_file_is_created_once_and_reused() {
    let dir = tmpdir("key");
    let key = dir.join("station.key");
    let cfg = || StationConfig {
        key_path: Some(key.clone()),
        ..StationConfig::new(addr())
    };

    let (first, _) = build(cfg());
    assert!(key.is_file());
    let (second, _) = build(cfg());
    assert_eq!(
        first.signing_key.public_key_id(),
        second.signing_key.public_key_id()
    );

    std::fs::write(&key, b"too short").unwrap();
    let err = build_station(cfg(), &mut |_| {}).err().unwrap();
    assert!(err.to_string().contains("32-byte Ed25519 seed"), "{err}");
}

#[tokio::test]
async fn the_in_memory_fallback_keeps_everything_in_memory() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmpdir("in-memory");
    // Point default detection here: an in-memory station must not use it.
    std::env::set_var("WIPESTATION_CONFIG_DIR", &dir);
    let (state, lines) = build(in_memory_config());
    std::env::remove_var("WIPESTATION_CONFIG_DIR");

    assert_eq!(state.topology_store.tier(), StoreTier::Ephemeral);
    assert_eq!(
        state.license().lease_persistence().tier,
        StoreTier::Ephemeral
    );
    assert!(state.simulator.is_some());
    assert!(state.vendor_roots.is_empty());
    assert!(list(&dir).is_empty(), "wrote {:?}", list(&dir));
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("config store: Ephemeral")),
        "{lines:?}"
    );
}

#[tokio::test]
async fn a_station_built_with_the_desktop_config_serves_the_simulator() {
    let dir = tmpdir("desktop-sim");
    let (state, _) = build(desktop_config(&dir));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bound = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    let r = reqwest::get(format!("http://{bound}/api/sim/devices"))
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

/// The desktop's app-data dir holds the layout, lease state and key across
/// launches, and is not the CLI's default config dir.
#[tokio::test]
async fn a_config_dir_station_keeps_its_layout_lease_and_key_across_builds() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmpdir("config-dir");
    let cli_dir = tmpdir("config-dir-cli-default");
    std::env::set_var("WIPESTATION_CONFIG_DIR", &cli_dir);
    let built = build_station(desktop_config(&dir), &mut |_| {});
    let first = built.unwrap();

    assert_eq!(first.topology_store.tier(), StoreTier::LocalFile);
    assert_eq!(
        first.topology_store.location(),
        dir.join("bay-topology.json").display().to_string()
    );
    assert_eq!(
        first.license().lease_persistence().location,
        dir.join("lease-state.json").display().to_string()
    );
    let mut layout = wipe_common::preset("dock-2bay").unwrap();
    layout.label = "Saved in the app-data dir".into();
    first.save_bay_topology(layout).unwrap();
    first
        .license()
        .signing_decision(time::OffsetDateTime::now_utc());
    let first_key_id = first.signing_key.public_key_id();
    drop(first);

    let second = build_station(desktop_config(&dir), &mut |_| {});
    std::env::remove_var("WIPESTATION_CONFIG_DIR");
    let second = second.unwrap();

    assert_eq!(
        second.bay_topology.read().as_ref().unwrap().label,
        "Saved in the app-data dir"
    );
    assert_eq!(first_key_id, second.signing_key.public_key_id());
    assert_eq!(
        second.license().lease_persistence().tier,
        StoreTier::LocalFile
    );
    assert!(dir.join("lease-state.json").is_file());
    assert!(list(&cli_dir).is_empty(), "wrote {:?}", list(&cli_dir));
}

/// A `config_dir` file is a store location, not a seed: a corrupt one is
/// logged and ignored, as on the CLI's default path, rather than failing.
#[tokio::test]
async fn a_corrupt_layout_in_the_config_dir_does_not_fail_the_build() {
    let dir = tmpdir("config-dir-corrupt");
    std::fs::write(dir.join("bay-topology.json"), "{ not json").unwrap();
    let (state, lines) = build(desktop_config(&dir));
    assert!(state.bay_topology.read().is_none());
    assert_eq!(state.topology_store.tier(), StoreTier::LocalFile);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("bay topology: not configured")),
        "{lines:?}"
    );
}

#[tokio::test]
async fn a_saved_layout_outranks_a_profile_seed() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmpdir("saved-vs-seed");
    let mut saved = wipe_common::preset("dock-2bay").unwrap();
    saved.label = "Saved by the operator".into();
    wipe_server::store::LocalFileStore::new(dir.join("bay-topology.json"))
        .save(&saved)
        .unwrap();

    std::env::set_var("WIPESTATION_CONFIG_DIR", &dir);
    let mut lines = Vec::new();
    let built = build_station(
        StationConfig {
            bay_profile: Some("arma-4u-32".into()),
            persistence: Persistence::Detect {
                control_plane_url: None,
                ephemeral_config: false,
                config_dir: None,
            },
            ..StationConfig::new(addr())
        },
        &mut |l| lines.push(l),
    );
    std::env::remove_var("WIPESTATION_CONFIG_DIR");
    let state = built.unwrap();

    assert_eq!(state.topology_store.tier(), StoreTier::LocalFile);
    let topology = state.bay_topology.read().clone().unwrap();
    assert_eq!(topology.label, "Saved by the operator");
}

#[tokio::test]
async fn a_licence_without_vendor_roots_fails_the_build() {
    let err = build_station(
        StationConfig {
            license: Some(PathBuf::from("/nonexistent/licence.json")),
            ..StationConfig::new(addr())
        },
        &mut |_| {},
    )
    .err()
    .unwrap();
    assert!(
        err.to_string()
            .starts_with("--license needs --vendor-root-b64"),
        "{err}"
    );
}

#[test]
fn frontend_bundle_resolution_order() {
    let dir = tmpdir("frontend");
    let host = dir.join("resources/dist");
    std::fs::create_dir_all(&host).unwrap();
    std::fs::write(host.join("index.html"), "<html></html>").unwrap();

    // An explicit directory wins unchanged, even when it does not exist.
    let missing = dir.join("no-such-dir");
    assert_eq!(
        resolve_frontend_dist(Some(missing.clone()), std::slice::from_ref(&host)),
        Some(missing)
    );
    // A host candidate with an index.html wins over the shared defaults.
    assert_eq!(
        resolve_frontend_dist(None, std::slice::from_ref(&host)),
        Some(host.canonicalize().unwrap())
    );
    // Candidates without an index.html are skipped; none found is None.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(
        wipe_server::bootstrap::first_frontend_dist(vec![empty, dir.join("absent")]),
        None
    );
}

/// `wipestation serve` prints these lines, in this order, exactly as it did
/// before the bootstrap was shared.
#[tokio::test]
async fn a_cli_station_reports_its_start_up_lines_in_order() {
    let dir = tmpdir("cli-lines");
    let (_, lines) = build(StationConfig {
        bay_topology: Some(dir.join("bay-topology.json")),
        static_dir: Some(dir.join("no-ui")),
        persistence: Persistence::Detect {
            control_plane_url: None,
            ephemeral_config: false,
            config_dir: None,
        },
        ..StationConfig::new(addr())
    });
    let expected = [
        "bay topology: ",
        "signing key public id: ed25519:",
        "signing key public b64: ",
        "serving frontend from ",
        "config store: LocalFile at ",
        "licence: none — free tier, certificates will be marked evaluation",
        "lease store: LocalFile at ",
        "lease counts: ",
        "runtime licence install: disabled",
        "destruction concurrence policy: none",
    ];
    assert_eq!(lines.len(), expected.len(), "{lines:#?}");
    for (line, prefix) in lines.iter().zip(expected) {
        assert!(
            line.starts_with(prefix),
            "{line:?} should start with {prefix:?}"
        );
    }
}
