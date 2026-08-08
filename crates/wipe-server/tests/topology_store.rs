//! Tiered station-configuration store (ADR-0003).
//!
//! The interesting behaviour is the *degradation*: what a station does when it
//! cannot write, and whether it says so instead of quietly losing an
//! operator's work.

use std::sync::Arc;

use wipe_common::{arma_4u_32, dock_2bay, BayTopology};
use wipe_server::store::{
    detect_store, status_of, ControlPlaneStore, EphemeralStore, LocalFileStore, StoreConfig,
    StoreError, StoreTier, TopologyStore,
};

fn tmpdir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wipestation-store-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// --- tier 1: local file ----------------------------------------------------

#[test]
fn local_file_round_trips_a_topology() {
    let dir = tmpdir("roundtrip");
    let store = LocalFileStore::new(dir.join("bay-topology.json"));

    assert!(store.load().unwrap().is_none(), "nothing saved yet");

    let t = arma_4u_32();
    store.save(&t).unwrap();
    let back = store.load().unwrap().expect("saved topology loads");
    assert_eq!(back, t);
    assert!(store.tier().survives_reboot());
}

#[test]
fn local_file_save_leaves_no_temp_files_behind() {
    // The write is temp-file-plus-rename; a leftover .tmp would mean a crash
    // window that leaves junk beside the real config.
    let dir = tmpdir("atomic");
    let store = LocalFileStore::new(dir.join("bay-topology.json"));
    store.save(&dock_2bay()).unwrap();

    let entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries, vec!["bay-topology.json".to_string()]);
}

#[test]
fn local_file_creates_missing_directories() {
    let dir = tmpdir("nested");
    let store = LocalFileStore::new(dir.join("deep").join("deeper").join("bay-topology.json"));
    store.save(&dock_2bay()).unwrap();
    assert!(store.load().unwrap().is_some());
}

#[test]
fn a_corrupt_config_reports_a_parse_error_rather_than_panicking() {
    // ADR-0003: a bad file must never brick a station.
    let dir = tmpdir("corrupt");
    let path = dir.join("bay-topology.json");
    std::fs::write(&path, b"{ not json at all").unwrap();

    let store = LocalFileStore::new(path);
    match store.load() {
        Err(StoreError::Parse(_)) => {}
        other => panic!("expected a parse error, got {other:?}"),
    }
}

#[test]
fn probe_detects_a_writable_directory() {
    let dir = tmpdir("probe-ok");
    assert!(LocalFileStore::probe(&dir.join("bay-topology.json")).is_ok());
    // The probe must clean up after itself.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn probe_fails_on_a_read_only_directory() {
    // Stands in for the PXE / read-only-root case that motivates the tiering.
    let dir = tmpdir("probe-ro");
    let mut perms = std::fs::metadata(&dir).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o500);
    }
    std::fs::set_permissions(&dir, perms).unwrap();

    let result = LocalFileStore::probe(&dir.join("bay-topology.json"));

    // Restore so the temp dir can be cleaned up.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&dir).unwrap().permissions();
        p.set_mode(0o700);
        std::fs::set_permissions(&dir, p).unwrap();
    }

    // Running as root defeats mode bits entirely, so only assert when the
    // probe is meaningful.
    if !nix_running_as_root() {
        assert!(
            result.is_err(),
            "probe should fail on a read-only directory"
        );
    }
}

fn nix_running_as_root() -> bool {
    std::env::var("USER").map(|u| u == "root").unwrap_or(false)
}

// --- tier 2: control plane -------------------------------------------------

#[test]
fn control_plane_builds_a_station_keyed_endpoint() {
    let cp = ControlPlaneStore::new("https://hub.example.com/".into(), "bench-3".into());
    assert_eq!(
        cp.endpoint(),
        "https://hub.example.com/api/stations/bench-3/bay-topology"
    );
}

#[test]
fn control_plane_reports_unavailable_rather_than_pretending_to_save() {
    // The seam must fail honestly while the hub is unbuilt (ADR-0003 §3):
    // a stub that silently succeeded would lose an operator's work.
    let cp = ControlPlaneStore::new("https://hub.example.com".into(), "bench-3".into());
    assert!(cp.save(&dock_2bay()).is_err());
    assert!(cp.load().is_err());
}

// --- tier 4: ephemeral -----------------------------------------------------

#[test]
fn ephemeral_holds_config_for_this_boot_but_admits_it_wont_survive() {
    let store = EphemeralStore::new("Read-only root.".into(), true);
    assert!(!store.tier().survives_reboot());

    let t = arma_4u_32();
    store.save(&t).unwrap();
    assert_eq!(store.load().unwrap().unwrap(), t);

    assert!(store.detail().contains("lost when this station reboots"));
}

#[test]
fn ephemeral_awaits_an_operator_decision_until_acknowledged() {
    // Tier 3 -> tier 4. The whole point is that losing a bay map is a
    // decision the operator made, not a surprise they discover at reboot.
    let store = EphemeralStore::new("Read-only root.".into(), true);
    assert!(store.awaiting_decision());
    store.acknowledge();
    assert!(!store.awaiting_decision());
}

// --- detection -------------------------------------------------------------

#[test]
fn detection_picks_local_file_when_the_path_is_writable() {
    let dir = tmpdir("detect-local");
    let store = detect_store(&StoreConfig {
        explicit_path: Some(dir.join("bay-topology.json")),
        control_plane_url: None,
        station_id: "s1".into(),
    });
    assert_eq!(store.tier(), StoreTier::LocalFile);
    assert!(!store.awaiting_decision());
}

#[test]
fn detection_falls_to_ephemeral_and_asks_when_nothing_can_persist() {
    // No writable path, no control plane: the station still works, but it
    // must raise the question rather than silently running ephemeral.
    let store = detect_store(&StoreConfig {
        explicit_path: Some(std::path::PathBuf::from(
            "/proc/definitely/not/writable/x.json",
        )),
        control_plane_url: None,
        station_id: "s1".into(),
    });
    assert_eq!(store.tier(), StoreTier::Ephemeral);
    assert!(
        store.awaiting_decision(),
        "operator must be prompted, not silently downgraded"
    );

    let status = status_of(&(store as Arc<dyn TopologyStore>));
    assert!(!status.survives_reboot);
    assert!(status.needs_operator_decision);
}

#[test]
fn an_unreachable_control_plane_degrades_to_ephemeral_and_names_the_endpoint() {
    let store = detect_store(&StoreConfig {
        explicit_path: Some(std::path::PathBuf::from(
            "/proc/definitely/not/writable/x.json",
        )),
        control_plane_url: Some("https://hub.example.com".into()),
        station_id: "s1".into(),
    });
    assert_eq!(store.tier(), StoreTier::Ephemeral);
    assert!(store.awaiting_decision());
    assert!(
        store.detail().contains("hub.example.com"),
        "the operator should be told which control plane failed, got: {}",
        store.detail()
    );
}

#[test]
fn a_saved_topology_survives_a_restart_on_the_local_tier() {
    // The end-to-end promise of tier 1, exercised through detection rather
    // than by constructing the backend directly.
    let dir = tmpdir("restart");
    let cfg = StoreConfig {
        explicit_path: Some(dir.join("bay-topology.json")),
        control_plane_url: None,
        station_id: "s1".into(),
    };

    let mut saved: BayTopology = arma_4u_32();
    saved.label = "Bench 7".into();
    detect_store(&cfg).save(&saved).unwrap();

    // "Reboot": a fresh detection over the same config.
    let after = detect_store(&cfg)
        .load()
        .unwrap()
        .expect("config came back");
    assert_eq!(after.label, "Bench 7");
    assert_eq!(after.bay_count(), 32);
}

// --- identify-mode bindings round-trip through the store -------------------

#[test]
fn path_bindings_learned_by_identify_mode_persist_and_resolve() {
    // What identify mode writes: a bay bound to a *port*, not a position.
    // The store must carry that through a restart, and the binding must keep
    // pointing at the same port when a different drive is plugged into it.
    use wipe_common::{BayBinding, BusType, Device, DeviceId, MediaType};

    let dir = tmpdir("identify");
    let cfg = StoreConfig {
        explicit_path: Some(dir.join("bay-topology.json")),
        control_plane_url: None,
        station_id: "s1".into(),
    };

    let mut t = dock_2bay();
    t.auto_fill_unbound = false;
    t.enclosures[0].banks[0].bays[1].binding = BayBinding::Path {
        path: "/dev/sdb".into(),
    };
    detect_store(&cfg).save(&t).unwrap();

    let after: BayTopology = detect_store(&cfg).load().unwrap().expect("reloads");
    let bay2 = &after.enclosures[0].banks[0].bays[1];
    assert_eq!(
        bay2.binding,
        BayBinding::Path {
            path: "/dev/sdb".into()
        }
    );

    let drive = |id: &str, serial: &str, path: &str| Device {
        id: DeviceId::from(id),
        vendor: "TestCo".into(),
        model: "TM-1".into(),
        serial: serial.into(),
        wwn: None,
        capacity_bytes: 1_000,
        media_type: MediaType::SsdSata,
        bus: BusType::Sata,
        firmware: None,
        removable: false,
        block_size: 512,
        path: path.into(),
    };

    // Original drive in that port lands in bay 2.
    let r = after.resolve(&[drive("dev-a", "SN-A", "/dev/sdb")]);
    assert_eq!(r.occupancy.len(), 1);
    assert_eq!(r.occupancy[0].device_id.0, "dev-a");

    // Swap a different drive into the same port: the bay keeps its identity.
    // This is the whole reason identify mode binds by path rather than serial.
    let r = after.resolve(&[drive("dev-b", "SN-B", "/dev/sdb")]);
    assert_eq!(r.occupancy.len(), 1);
    assert_eq!(r.occupancy[0].bay_id, bay2.id);
    assert_eq!(r.occupancy[0].device_id.0, "dev-b");

    // Pull it out entirely and the bay reads empty rather than adopting
    // whatever else happens to be attached.
    let r = after.resolve(&[drive("dev-c", "SN-C", "/dev/sdz")]);
    assert!(r.occupancy.is_empty());
    assert_eq!(r.unplaced_devices.len(), 1);
}
