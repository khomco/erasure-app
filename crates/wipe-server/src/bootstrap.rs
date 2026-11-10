//! Station start-up shared by every host.
//!
//! `wipestation serve` (`crates/wipe-cli`) and the Tauri desktop shell
//! (`apps/desktop/src-tauri`) both build their [`AppState`] here, so anything
//! a station wires at start-up — signing key, bay topology seed, topology and
//! lease stores, licence, vendor roots, concurrence policy, simulator and
//! frontend bundle — reaches both hosts at once. A host maps its own options
//! into a [`StationConfig`], calls [`build_station`], then serves the result
//! with [`crate::serve`].

use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use wipe_cert::{SigningKey, VerifyingKey};
use wipe_engine_mock::{MockBackend, MockTiming};
use wipe_fleet::FleetService;

use crate::{
    licensing::LicenseContext,
    store::{EphemeralLeaseStore, EphemeralStore, LeaseStore, StoreConfig, TopologyStore},
    AppState,
};

/// Whether, and how hard, the station advertises itself on mDNS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fleet {
    /// No advertisement (`--no-fleet`).
    Off,
    /// Advertise; failing to start mDNS fails start-up (`wipestation serve`).
    Required,
    /// Advertise if mDNS starts; otherwise report it and run without a fleet
    /// (the desktop shell).
    BestEffort,
}

/// Where the station keeps its configuration and lease state (ADR-0003,
/// ADR-0005 §3a).
#[derive(Debug, Clone)]
pub enum Persistence {
    /// Walk the store tiers: `--bay-topology` path, else
    /// `<config_dir>/bay-topology.json`, else the default config path; then
    /// the control plane, then RAM. `ephemeral_config` keeps *configuration*
    /// in RAM but still detects the lease store.
    Detect {
        control_plane_url: Option<String>,
        ephemeral_config: bool,
        /// Directory the host keeps its saved configuration and lease state
        /// in (the desktop's app-data dir). Only a store location: the file
        /// in it is never loaded as a seed, so an unreadable one is logged
        /// and ignored rather than failing start-up. `None` on the CLI.
        config_dir: Option<PathBuf>,
    },
    /// Configuration and lease state both in RAM; nothing is written to disk.
    /// `reason` is what the station reports as the cause.
    InMemory { reason: String },
}

/// Everything a host decides about the station it starts.
pub struct StationConfig {
    /// Address the host will serve on; its port is what mDNS advertises.
    pub addr: SocketAddr,
    /// Station identifier. `None`: `standalone` for stores and licensing, a
    /// random id for the fleet advertisement.
    pub station_id: Option<String>,
    /// Hostname advertised on mDNS.
    pub hostname: String,
    pub fleet: Fleet,
    /// Where the signing key seed is read or, if missing, written. `None`
    /// generates a fresh key for this run only.
    pub key_path: Option<PathBuf>,
    /// Fast mock timing (demos and CI).
    pub fast: bool,
    /// Explicit frontend bundle directory. Wins even if it does not exist
    /// (the station then serves its API-only landing page).
    pub static_dir: Option<PathBuf>,
    /// Host-specific bundle locations, tried before the shared defaults.
    pub host_static_candidates: Vec<PathBuf>,
    /// Bay-topology JSON file (`--bay-topology`); outranks `bay_profile` as
    /// the seed and, under [`Persistence::Detect`], is also the store path.
    pub bay_topology: Option<PathBuf>,
    /// Preset name or catalog model id to seed the bench from.
    pub bay_profile: Option<String>,
    pub persistence: Persistence,
    /// Vendor-issued licence file. Needs `vendor_roots`.
    pub license: Option<PathBuf>,
    /// Trusted vendor roots for the start-up licence and runtime install.
    pub vendor_roots: Vec<VerifyingKey>,
    pub concurrence_policy: wipe_common::ConcurrencePolicy,
}

impl StationConfig {
    /// A station on `addr` that advertises nothing, writes nothing, has no
    /// licence and uses default timing. Hosts override what they need.
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            station_id: None,
            hostname: "localhost".into(),
            fleet: Fleet::Off,
            key_path: None,
            fast: false,
            static_dir: None,
            host_static_candidates: Vec::new(),
            bay_topology: None,
            bay_profile: None,
            persistence: Persistence::InMemory {
                reason: "Configuration is held in memory only.".into(),
            },
            license: None,
            vendor_roots: Vec::new(),
            concurrence_policy: wipe_common::ConcurrencePolicy::None,
        }
    }
}

/// Build the station a host serves.
///
/// Start-up lines go to `report` as they happen, so a later failure still
/// leaves the earlier lines visible. The mock backend is always attached as
/// the hot-plug simulator, which is what identify mode is driven by.
pub fn build_station(cfg: StationConfig, report: &mut dyn FnMut(String)) -> Result<AppState> {
    let StationConfig {
        addr,
        station_id,
        hostname,
        fleet,
        key_path,
        fast,
        static_dir,
        host_static_candidates,
        bay_topology: bay_topology_path,
        bay_profile,
        persistence,
        license: license_path,
        vendor_roots,
        concurrence_policy,
    } = cfg;

    let seed_topology = load_bay_topology(bay_topology_path.clone(), bay_profile, report)?;
    let instance_key_kept = key_path.is_some();
    let signing_key = Arc::new(load_or_create_signing_key(key_path)?);
    report(format!(
        "signing key public id: {}",
        signing_key.verifying_key().public_key_id()
    ));
    report(format!(
        "signing key public b64: {}",
        signing_key.verifying_key().to_base64()
    ));

    let backend = Arc::new(MockBackend::with_catalog(
        wipe_engine_mock::default_devices_public(),
        if fast {
            MockTiming::fast()
        } else {
            MockTiming::default()
        },
    ));

    let resolved_station_id = station_id.clone().unwrap_or_else(|| "standalone".into());

    let fleet = match fleet {
        Fleet::Off => None,
        Fleet::Required | Fleet::BestEffort => {
            use wipe_common::{StationId, StationInfo, StationRole};
            let id = station_id
                .map(StationId)
                .unwrap_or_else(StationId::new_random);
            let info = StationInfo {
                id,
                hostname,
                role: StationRole::Member,
                version: env!("CARGO_PKG_VERSION").into(),
                api_port: addr.port(),
                started_at: time::OffsetDateTime::now_utc(),
                active_jobs: 0,
                last_seen: None,
            };
            start_fleet(fleet, report, || Ok(FleetService::start(info)?))?.map(Arc::new)
        }
    };

    let resolved_dist = resolve_frontend_dist(static_dir, &host_static_candidates);
    match &resolved_dist {
        Some(p) => report(format!("serving frontend from {}", p.display())),
        None => report("no frontend bundle found — UI will show an API-only landing page".into()),
    }

    // Work out where saved configuration goes before serving, so the tier is
    // in the startup log next to everything else an operator checks (ADR-0003).
    let (store, lease_store, settings_path) = match persistence {
        Persistence::Detect {
            control_plane_url,
            ephemeral_config,
            config_dir,
        } => {
            let store_cfg = StoreConfig {
                explicit_path: bay_topology_path
                    .or_else(|| config_dir.map(|d| d.join("bay-topology.json"))),
                control_plane_url,
                station_id: resolved_station_id.clone(),
            };
            let (topology, lease) = serve_stores(&store_cfg, ephemeral_config);
            (
                topology,
                lease,
                Some(crate::store::station_settings_path(&store_cfg)),
            )
        }
        Persistence::InMemory { reason } => {
            let topology: Arc<dyn TopologyStore> =
                Arc::new(EphemeralStore::new(reason.clone(), false));
            let lease: Arc<dyn LeaseStore> = Arc::new(EphemeralLeaseStore::new(reason));
            (topology, lease, None)
        }
    };
    let store_status = crate::store::status_of(&store);
    report(format!(
        "config store: {:?} at {} — {}",
        store_status.tier, store_status.location, store_status.detail
    ));
    if store_status.needs_operator_decision {
        report(
            "  ^ the UI will ask the operator to point at a control plane or accept the loss"
                .into(),
        );
    }

    // ADR-0005: a licence only decides how certificates are *marked*. A
    // station with none still erases and still signs. The roots are kept
    // whether or not a licence is given: they are also what a licence
    // installed at runtime must verify against (ADR-0008 §4).
    let license_ctx = build_license_context(
        license_path,
        &vendor_roots,
        &signing_key.public_key_id(),
        &resolved_station_id,
        lease_store,
        report,
    )?;
    report(lease_store_line(&license_ctx.lease_persistence()));
    report(lease_counts_line(&license_ctx.lease_state()));
    report(runtime_install_line(vendor_roots.len()));
    report(hub_line(settings_path.as_deref()));

    let state = AppState::with_static_dir(backend.clone(), fleet, signing_key, resolved_dist)
        .with_station_id(resolved_station_id)
        .with_settings_path(settings_path)
        .with_instance_key_kept(instance_key_kept);
    let mut state = apply_licensing(state, license_ctx, vendor_roots)
        .with_topology_store(store)
        .with_concurrence_policy(concurrence_policy)
        // The mock can fake hot-plug, which is what identify mode is driven
        // by. The /api/sim/* routes are always mounted; without this call
        // they answer 404.
        .with_simulator(backend);
    // A --bay-topology file or --bay-profile seeds the bench only when the
    // store had nothing; a saved layout is the operator's and outranks it.
    if state.bay_topology.read().is_none() {
        state = state.with_bay_topology(seed_topology);
    }
    // The 24-hour licence check (ADR-0008 §3), here so both hosts get it.
    // It never gates a start: it only installs a renewed licence and reports.
    crate::licence_fetch::spawn(&state);
    report(format!(
        "destruction concurrence policy: {}",
        serde_json::to_value(concurrence_policy)?
            .as_str()
            .unwrap_or_default()
    ));
    Ok(state)
}

/// Start mDNS advertisement under `mode`. A failure fails start-up under
/// [`Fleet::Required`]; under [`Fleet::BestEffort`] it is reported and the
/// station runs without a fleet. [`Fleet::Off`] never calls `start`.
fn start_fleet<F>(
    mode: Fleet,
    report: &mut dyn FnMut(String),
    start: impl FnOnce() -> Result<F>,
) -> Result<Option<F>> {
    match mode {
        Fleet::Off => Ok(None),
        Fleet::Required => start().map(Some),
        Fleet::BestEffort => match start() {
            Ok(f) => Ok(Some(f)),
            Err(e) => {
                report(format!("mDNS fleet disabled: {e}"));
                Ok(None)
            }
        },
    }
}

/// The frontend bundle a host serves.
///
/// An explicit directory wins unchanged, even if it does not exist (the
/// station then serves its API-only landing page). Otherwise the first of
/// `host_candidates`, then [`default_frontend_candidates`], that holds an
/// `index.html`, canonicalized. `None` if none does.
pub fn resolve_frontend_dist(
    explicit: Option<PathBuf>,
    host_candidates: &[PathBuf],
) -> Option<PathBuf> {
    if explicit.is_some() {
        return explicit;
    }
    first_frontend_dist(
        host_candidates
            .iter()
            .cloned()
            .chain(default_frontend_candidates()),
    )
}

/// The first of `candidates` holding an `index.html`, canonicalized.
pub fn first_frontend_dist(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .find(|p| has_index_html(p))
        .map(|p| p.canonicalize().unwrap_or(p))
}

/// Where a built frontend usually is, for either host:
///   * `<cwd>/apps/desktop/dist`
///   * `<cwd>/dist`
///   * `<exe-dir>/static`
///   * `<exe-dir>/../apps/desktop/dist`, up to four levels up
pub fn default_frontend_candidates() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("apps/desktop/dist"));
        candidates.push(cwd.join("dist"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("static"));
            candidates.push(parent.join("../apps/desktop/dist"));
            candidates.push(parent.join("../../apps/desktop/dist"));
            candidates.push(parent.join("../../../apps/desktop/dist"));
            candidates.push(parent.join("../../../../apps/desktop/dist"));
        }
    }
    candidates
}

fn has_index_html(p: &Path) -> bool {
    p.join("index.html").is_file()
}

/// Load the station's bay topology from an explicit file or a named preset.
///
/// A file that names an unknown schema version, or a preset name we don't
/// know, is a hard error: rendering the wrong bay map is worse than
/// rendering none, because the operator's reason to trust it is that they
/// stop double-checking against the metal.
fn load_bay_topology(
    path: Option<PathBuf>,
    profile: Option<String>,
    report: &mut dyn FnMut(String),
) -> Result<Option<wipe_common::BayTopology>> {
    if let Some(path) = path {
        // Since ADR-0003 this path is read *and* written, so "not there yet"
        // is a first run rather than a misconfiguration. A file that exists
        // but is wrong is still a hard error.
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report(format!(
                    "bay topology: {} does not exist yet — it will be created on first save",
                    path.display()
                ));
                return Ok(None);
            }
            Err(e) => {
                return Err(e).with_context(|| format!("reading bay topology {}", path.display()))
            }
        };
        let topology: wipe_common::BayTopology = serde_json::from_str(&raw)
            .with_context(|| format!("parsing bay topology {}", path.display()))?;
        if topology.schema_version != wipe_common::BAY_TOPOLOGY_SCHEMA_VERSION {
            return Err(anyhow!(
                "bay topology {} declares schema_version {}, but this build understands {}",
                path.display(),
                topology.schema_version,
                wipe_common::BAY_TOPOLOGY_SCHEMA_VERSION
            ));
        }
        let dupes = topology.duplicate_bay_ids();
        if !dupes.is_empty() {
            return Err(anyhow!(
                "bay topology {} has duplicate bay ids: {}",
                path.display(),
                dupes
                    .iter()
                    .map(|b| b.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        report(format!(
            "bay topology: {} ({} bays) from {}",
            topology.label,
            topology.bay_count(),
            path.display()
        ));
        return Ok(Some(topology));
    }

    if let Some(name) = profile {
        let (topology, source) = resolve_profile(&name)?;
        report(format!(
            "bay topology: {} ({} bays) from {source} `{name}`",
            topology.label,
            topology.bay_count()
        ));
        return Ok(Some(topology));
    }

    report("bay topology: not configured — bench will be shown as unconfigured".into());
    Ok(None)
}

/// Resolve a `--bay-profile` name: a built-in preset, or a catalog model id.
///
/// The catalog is a source of defaults (ADR-0004), so a station should be able
/// to start from one without an operator hand-writing the same JSON the
/// builder would have produced. Either way the bays start unbound — the
/// catalog knows the shape of a chassis, never which drive is in which bay.
pub fn resolve_profile(name: &str) -> Result<(wipe_common::BayTopology, &'static str)> {
    if let Some(topology) = wipe_common::preset(name) {
        return Ok((topology, "preset"));
    }

    let catalog = wipe_common::Catalog::bundled();
    if let Some(model) = catalog.get(&wipe_common::ModelId(name.to_string())) {
        return Ok((
            wipe_common::BayTopology {
                schema_version: wipe_common::BAY_TOPOLOGY_SCHEMA_VERSION,
                label: model.display_name(),
                generated: false,
                revision: 0,
                auto_fill_unbound: true,
                enclosures: vec![model.expand("enc1")],
            },
            "catalog model",
        ));
    }

    Err(anyhow!(
        "unknown bay profile `{name}` — presets: {}; catalog models: {}",
        wipe_common::preset_names().join(", "),
        catalog
            .models
            .iter()
            .map(|m| m.id.0.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Read the station's Ed25519 seed from `path`, or create it there.
///
/// A created seed is written crash-safely and never overwrites an existing
/// file, and on Unix it is readable by its owner only (`0o600`): it is the key
/// every certificate this station signs is checked against. An existing file
/// that is unreadable or not a 32-byte seed fails start-up with its path
/// named, and is left untouched.
fn load_or_create_signing_key(path: Option<PathBuf>) -> Result<SigningKey> {
    let Some(p) = path else {
        return Ok(SigningKey::generate());
    };
    // `exists()` would report an unreadable key (EACCES) as absent and send
    // it down the create path; only a definite NotFound creates.
    match fs::symlink_metadata(&p) {
        Ok(_) => read_signing_key(&p),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => create_signing_key(&p),
        Err(e) => Err(e).with_context(|| format!("reading signing key {}", p.display())),
    }
}

fn read_signing_key(p: &Path) -> Result<SigningKey> {
    let bytes = fs::read(p).with_context(|| format!("reading signing key {}", p.display()))?;
    let arr: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
        anyhow!(
            "{} is not a 32-byte Ed25519 seed (got {})",
            p.display(),
            bytes.len()
        )
    })?;
    Ok(SigningKey::from_seed(arr))
}

/// Generate a seed and create `p` holding it. If another process created `p`
/// first, its key is read instead, so both sign with the same key.
fn create_signing_key(p: &Path) -> Result<SigningKey> {
    let key = SigningKey::generate();
    match write_new_secret(p, &key.0.to_bytes()) {
        Ok(()) => Ok(key),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => read_signing_key(p),
        Err(e) => Err(e).with_context(|| format!("writing signing key {}", p.display())),
    }
}

/// Create `path` holding `bytes`, owner-only on Unix, without ever exposing a
/// partly written file or replacing an existing one.
///
/// The bytes go to a temp file beside `path` (created exclusively, synced),
/// which is then hard-linked into place: the link fails with `AlreadyExists`
/// if `path` now exists. The temp file is removed either way. On a filesystem
/// without hard links (FAT/exFAT, some SMB/NFS) the link fails otherwise, and
/// the bytes are written straight to `path` with [`create_exclusive`].
fn write_new_secret(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    let mut tmp_name = name.to_os_string();
    tmp_name.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = path.with_file_name(tmp_name);
    // A leftover from a crashed run with the same pid would make the
    // exclusive create below fail; it never holds a key anyone uses.
    let _ = fs::remove_file(&tmp);

    let result = (|| {
        create_exclusive(&tmp, bytes).map_err(|e| {
            // Only the final step may report `AlreadyExists` (the race).
            std::io::Error::new(
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    std::io::ErrorKind::Other
                } else {
                    e.kind()
                },
                e,
            )
        })?;
        match fs::hard_link(&tmp, path) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                create_exclusive(path, bytes)
            }
            other => other,
        }
    })();
    let _ = fs::remove_file(&tmp);
    result?;
    // Make the new directory entry durable too; best effort.
    #[cfg(unix)]
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = fs::File::open(dir).and_then(|d| d.sync_all());
    }
    Ok(())
}

/// Create `path` exclusively (`AlreadyExists` if it exists), owner-only on
/// Unix, write `bytes` and sync. A file this call created but could not fill
/// is removed, so a failed write never leaves a short seed behind.
fn create_exclusive(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    if written.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
    }
    written
}

/// The topology store and the lease store a detecting station runs with.
///
/// `--ephemeral-config` keeps *configuration* in RAM; lease state is detected
/// whatever it says, because lease state is licensing enforcement, not
/// configuration (ADR-0005 §3a). That only stops this flag from refilling a
/// quota: deleting or moving `lease-state.json`, or pointing `--bay-topology`
/// or `WIPESTATION_CONFIG_DIR` at an empty directory, still starts a fresh
/// lease — the speed bump ADR-0005 §3a admits.
fn serve_stores(
    cfg: &StoreConfig,
    force_ephemeral: bool,
) -> (Arc<dyn TopologyStore>, Arc<dyn LeaseStore>) {
    let topology: Arc<dyn TopologyStore> = if force_ephemeral {
        Arc::new(EphemeralStore::new(
            "Started with --ephemeral-config.".into(),
            false,
        ))
    } else {
        crate::store::detect_store(cfg)
    };
    (topology, crate::store::detect_lease_store(cfg))
}

/// The start-up line saying where lease state lives and whether it survives
/// reboot. Same shape as the `config store:` line above it.
fn lease_store_line(lease: &crate::LeasePersistence) -> String {
    format!(
        "lease store: {:?} at {} — {}",
        lease.tier, lease.location, lease.detail
    )
}

/// The start-up line giving the local erasure counts (ADR-0005 §5): licensed
/// erasures under the licence the stored count belongs to (which, on an
/// unlicensed start, is the last licence seen, if any), and evaluation
/// erasures over the station's life. A count only — no limit or warning is
/// attached to it.
fn lease_counts_line(lease: &wipe_license::LeaseState) -> String {
    let licensed = match &lease.license_id {
        Some(id) => format!(
            "{} licensed erasure(s) under licence {id}",
            lease.erasures_used
        ),
        None => "no licensed erasures (no licence seen)".to_string(),
    };
    format!(
        "lease counts: {licensed}, {} evaluation erasure(s) on this station",
        lease.evaluation_erasures
    )
}

/// Give the station its start-up licence context and the vendor roots a
/// licence installed at runtime (`POST /api/license`) is checked against.
fn apply_licensing(
    state: AppState,
    license_ctx: LicenseContext,
    vendor_roots: Vec<VerifyingKey>,
) -> AppState {
    state
        .with_license(Arc::new(license_ctx))
        .with_vendor_roots(vendor_roots)
}

/// Whether `POST /api/license` can install a licence on this station.
fn runtime_install_line(roots: usize) -> String {
    if roots == 0 {
        "runtime licence install: disabled — no --vendor-root-b64, so POST /api/license is refused"
            .to_string()
    } else {
        format!(
            "runtime licence install: enabled (POST /api/license, checked against {roots} vendor \
             root(s)); an installed licence lasts until restart"
        )
    }
}

/// The start-up line naming the hub `wipestation enroll` saved in the
/// station settings (ADR-0008 §5). Only reported: an unreadable or invalid
/// file never fails start-up. `None`: the station keeps nothing on disk.
fn hub_line(settings_path: Option<&Path>) -> String {
    const NONE: &str = "hub: none — run wipestation enroll --hub <origin> to register this station";
    let Some(path) = settings_path else {
        return NONE.to_string();
    };
    match crate::store::load_station_settings(path) {
        Ok(Some(crate::store::StationSettings {
            hub_origin: Some(raw),
        })) => match crate::hub_client::HubOrigin::parse(&raw) {
            Ok(origin) => format!(
                "hub: {origin} (from {}); checked for a renewed licence at start-up and every 24 \
                 hours",
                path.display()
            ),
            Err(e) => format!(
                "hub: none — {} holds a hub address this station refuses ({e}); run wipestation \
                 enroll --hub <origin> to register this station",
                path.display()
            ),
        },
        Ok(_) => NONE.to_string(),
        Err(e) => format!(
            "hub: none — {} could not be read ({e}); run wipestation enroll --hub <origin> to \
             register this station",
            path.display()
        ),
    }
}

/// Build the station's licensing state from `--license` / `--vendor-root-b64`.
///
/// A licence that fails to verify, or that names another station's key, is a
/// hard error at startup rather than a silent downgrade: the operator asked
/// for licensed operation and needs to know they did not get it. Running with
/// *no* licence at all is a different, supported thing — free tier.
fn build_license_context(
    license_path: Option<PathBuf>,
    roots: &[VerifyingKey],
    station_key_id: &str,
    station_id: &str,
    lease_store: Arc<dyn LeaseStore>,
    report: &mut dyn FnMut(String),
) -> Result<LicenseContext> {
    let now = time::OffsetDateTime::now_utc();
    let Some(path) = license_path else {
        report("licence: none — free tier, certificates will be marked evaluation".into());
        return Ok(LicenseContext::open(station_id, None, lease_store, now));
    };

    if roots.is_empty() {
        return Err(anyhow!(
            "--license needs --vendor-root-b64: an unverified licence proves nothing"
        ));
    }
    let license = wipe_license::load_license(&path).map_err(|e| anyhow!("{e}"))?;
    wipe_license::check_installable(&license, station_key_id, roots)
        .map_err(|p| anyhow!("cannot install {}: {p}", path.display()))?;

    let ent = &license.body.entitlements;
    report(format!(
        "licence: {} for {} ({}) — valid {} to {}",
        license.body.license_id,
        ent.customer_name,
        ent.customer_id,
        ent.not_before.date(),
        ent.not_after.date()
    ));

    let chain = wipe_license::AttestationChain::new(license);
    let ctx = LicenseContext::open(station_id, Some(chain), lease_store, now);
    report(format!("  {}", ctx.status(now).operator_message()));
    Ok(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> impl FnMut(String) {
        |_| {}
    }

    #[test]
    fn default_frontend_candidates_are_tried_in_order() {
        let cwd = std::env::current_dir().unwrap();
        let exe_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_owned();
        assert_eq!(
            default_frontend_candidates(),
            vec![
                cwd.join("apps/desktop/dist"),
                cwd.join("dist"),
                exe_dir.join("static"),
                exe_dir.join("../apps/desktop/dist"),
                exe_dir.join("../../apps/desktop/dist"),
                exe_dir.join("../../../apps/desktop/dist"),
                exe_dir.join("../../../../apps/desktop/dist"),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_created_signing_key_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = lease_tmpdir("key-mode");
        let path = dir.join("station.key");
        load_or_create_signing_key(Some(path.clone())).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode {mode:o}");
    }

    #[test]
    fn creating_a_signing_key_leaves_no_temp_file_behind() {
        let dir = lease_tmpdir("key-tmp");
        let path = dir.join("station.key");
        load_or_create_signing_key(Some(path.clone())).unwrap();
        // A losing create (the key now exists) cleans up too.
        assert!(write_new_secret(&path, &[0u8; 32]).is_err());
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["station.key".to_string()]);
    }

    #[test]
    fn an_existing_signing_key_is_never_overwritten() {
        let dir = lease_tmpdir("key-keep");
        let path = dir.join("station.key");
        let key = load_or_create_signing_key(Some(path.clone())).unwrap();
        let bytes = fs::read(&path).unwrap();
        let err = write_new_secret(&path, &[0u8; 32]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let again = load_or_create_signing_key(Some(path)).unwrap();
        assert_eq!(key.public_key_id(), again.public_key_id());
    }

    /// Another process created the key between our `exists()` check and our
    /// create: its key is read and used, not an error and not replaced.
    #[test]
    fn losing_the_create_race_reads_the_winners_key() {
        let dir = lease_tmpdir("key-race");
        let path = dir.join("station.key");
        let winner = SigningKey::generate();
        fs::write(&path, winner.0.to_bytes()).unwrap();
        let ours = create_signing_key(&path).unwrap();
        assert_eq!(ours.public_key_id(), winner.public_key_id());
        assert_eq!(fs::read(&path).unwrap(), winner.0.to_bytes());
    }

    #[test]
    fn a_signing_key_read_error_names_its_path() {
        let dir = lease_tmpdir("key-read-err");
        let path = dir.join("station.key");
        fs::create_dir(&path).unwrap();
        let err = load_or_create_signing_key(Some(path.clone()))
            .err()
            .unwrap();
        let msg = format!("{err:#}");
        assert!(msg.contains(&path.display().to_string()), "{msg}");
        assert!(path.is_dir(), "the unreadable key is left in place");
    }

    #[test]
    fn a_wrong_length_signing_key_names_its_path_and_is_left_untouched() {
        let dir = lease_tmpdir("key-short");
        let path = dir.join("station.key");
        fs::write(&path, b"short").unwrap();
        let err = load_or_create_signing_key(Some(path.clone()))
            .err()
            .unwrap();
        let msg = format!("{err:#}");
        assert!(msg.contains(&path.display().to_string()), "{msg}");
        assert!(msg.contains("got 5"), "{msg}");
        assert_eq!(fs::read(&path).unwrap(), b"short");
    }

    /// The no-hard-link fallback: an exclusive, owner-only write of the final
    /// path that never replaces an existing file.
    #[test]
    fn the_no_hard_link_fallback_creates_exclusively_and_owner_only() {
        let dir = lease_tmpdir("key-fallback");
        let path = dir.join("station.key");
        create_exclusive(&path, &[7u8; 32]).unwrap();
        assert_eq!(fs::read(&path).unwrap(), [7u8; 32]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "mode {mode:o}");
        }
        let err = create_exclusive(&path, &[0u8; 32]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), [7u8; 32]);
    }

    /// A key whose metadata cannot be read (EACCES on its directory) is a
    /// read error naming the path, not a create attempt.
    #[cfg(unix)]
    #[test]
    fn an_unstatable_signing_key_is_a_read_error_not_a_create() {
        use std::os::unix::fs::PermissionsExt;
        let dir = lease_tmpdir("key-eacces");
        let locked = dir.join("locked");
        fs::create_dir(&locked).unwrap();
        let path = locked.join("station.key");
        fs::write(&path, [1u8; 32]).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let result = load_or_create_signing_key(Some(path.clone()));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        if fs::metadata(&path).is_ok() && result.is_ok() {
            // Root ignores the mode bits; nothing to test there.
            return;
        }
        let msg = format!("{:#}", result.err().unwrap());
        assert!(msg.contains("reading signing key"), "{msg}");
        assert!(msg.contains(&path.display().to_string()), "{msg}");
    }

    #[test]
    fn a_signing_key_write_error_names_its_path() {
        let path = std::env::temp_dir()
            .join(format!("wipestation-no-such-dir-{}", std::process::id()))
            .join("station.key");
        let err = load_or_create_signing_key(Some(path.clone()))
            .err()
            .unwrap();
        let msg = format!("{err:#}");
        assert!(msg.contains("writing signing key"), "{msg}");
        assert!(msg.contains(&path.display().to_string()), "{msg}");
    }

    #[test]
    fn an_mdns_failure_fails_a_required_fleet_and_is_reported_under_best_effort() {
        let fail = || -> Result<()> { Err(anyhow!("no multicast")) };

        let mut lines = Vec::new();
        assert!(start_fleet(Fleet::Required, &mut |l| lines.push(l), fail).is_err());

        let started = start_fleet(Fleet::BestEffort, &mut |l| lines.push(l), fail).unwrap();
        assert!(started.is_none());
        assert_eq!(lines, vec!["mDNS fleet disabled: no multicast".to_string()]);

        let off = start_fleet(Fleet::Off, &mut quiet(), || -> Result<()> {
            panic!("Off must not start mDNS")
        });
        assert!(off.unwrap().is_none());
    }

    /// The station keeps its vendor roots even with no licence, so a licence
    /// can be installed at runtime.
    #[tokio::test]
    async fn serve_without_a_licence_still_trusts_its_vendor_roots_for_runtime_install() {
        let root = wipe_license::VendorRoot::generate();
        let station = |roots: Vec<VerifyingKey>| {
            let backend = Arc::new(MockBackend::with_catalog(
                wipe_engine_mock::default_devices_public(),
                MockTiming::fast(),
            ));
            let state = AppState::new(backend, None, Arc::new(SigningKey::generate()));
            let store: Arc<dyn LeaseStore> = Arc::new(EphemeralLeaseStore::new("test".into()));
            let ctx = build_license_context(
                None,
                &roots,
                "ed25519:test",
                "station-1",
                store,
                &mut quiet(),
            )
            .unwrap();
            apply_licensing(state, ctx, roots)
        };
        let licence_for = |state: &AppState| {
            let now = time::OffsetDateTime::now_utc();
            let ent = wipe_license::Entitlements {
                customer_id: "c".into(),
                customer_name: "C".into(),
                quota: wipe_license::Quota::Unlimited,
                scope: wipe_license::Scope::Site {
                    site_id: "s".into(),
                },
                not_before: now - time::Duration::days(1),
                not_after: now + time::Duration::days(30),
                features: vec![],
                allowed_methods: wipe_license::AllowedMethods::All,
                machine_binding: None,
            };
            let licence = root
                .issue("lic-rt", state.signing_key.public_key_id(), ent, now)
                .unwrap();
            serde_json::to_string(&licence).unwrap()
        };

        let state = station(vec![root.verifying_key()]);
        assert!(state.install_license(&licence_for(&state)).is_ok());

        let state = station(Vec::new());
        assert_eq!(
            state.install_license(&licence_for(&state)),
            Err(crate::LicenseInstallError::NoVendorRoot)
        );
    }

    fn lease_tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wipestation-bootstrap-lease-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store_cfg_in(dir: &Path) -> StoreConfig {
        StoreConfig {
            explicit_path: Some(dir.join("bay-topology.json")),
            control_plane_url: None,
            station_id: "station-1".into(),
        }
    }

    #[test]
    fn ephemeral_config_keeps_topology_in_ram_but_still_persists_lease_state() {
        let dir = lease_tmpdir("ephemeral-config");
        let cfg = store_cfg_in(&dir);

        let (topology, lease) = serve_stores(&cfg, true);
        assert_eq!(topology.tier(), crate::StoreTier::Ephemeral);
        assert_eq!(lease.tier(), crate::StoreTier::LocalFile);
        assert_eq!(
            lease.location(),
            dir.join("lease-state.json").display().to_string()
        );

        let (topology, lease) = serve_stores(&cfg, false);
        assert_eq!(topology.tier(), crate::StoreTier::LocalFile);
        assert_eq!(lease.tier(), crate::StoreTier::LocalFile);
    }

    #[test]
    fn an_unlicensed_station_saves_its_lease_after_a_signing_decision() {
        let dir = lease_tmpdir("unlicensed");
        let path = dir.join("lease-state.json");
        let store = Arc::new(crate::LocalFileLeaseStore::new(path.clone()));
        let ctx =
            build_license_context(None, &[], "ed25519:test", "station-1", store, &mut quiet())
                .unwrap();
        assert_eq!(ctx.lease_persistence().tier, crate::StoreTier::LocalFile);

        let decision = ctx.signing_decision(time::OffsetDateTime::now_utc());
        assert!(decision.evaluation);
        let saved: wipe_license::LeaseState =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved.erasures_used, 0);

        let line = lease_store_line(&ctx.lease_persistence());
        assert!(line.starts_with("lease store: LocalFile at "), "{line}");
        assert!(line.contains(&path.display().to_string()), "{line}");
    }

    #[test]
    fn the_start_up_counts_line_reports_an_unlicensed_stations_evaluation_erasures() {
        let dir = lease_tmpdir("counts-line");
        let path = dir.join("lease-state.json");
        let mut seeded = wipe_license::LeaseState::new(time::OffsetDateTime::now_utc());
        seeded.evaluation_erasures = 3;
        let store = Arc::new(crate::LocalFileLeaseStore::new(path));
        LeaseStore::save(store.as_ref(), &seeded).unwrap();
        let ctx =
            build_license_context(None, &[], "ed25519:test", "station-1", store, &mut quiet())
                .unwrap();
        assert_eq!(
            lease_counts_line(&ctx.lease_state()),
            "lease counts: no licensed erasures (no licence seen), 3 evaluation erasure(s) on \
             this station"
        );
    }

    #[test]
    fn the_start_up_counts_line_names_the_licence_a_stored_count_belongs_to() {
        let dir = lease_tmpdir("counts-line-licence");
        let path = dir.join("lease-state.json");
        let mut seeded = wipe_license::LeaseState::new(time::OffsetDateTime::now_utc());
        seeded.erasures_used = 5;
        seeded.evaluation_erasures = 2;
        seeded.license_id = Some("lic-old".into());
        let store = Arc::new(crate::LocalFileLeaseStore::new(path));
        LeaseStore::save(store.as_ref(), &seeded).unwrap();
        // Unlicensed start: the stored count is kept, and named by its licence.
        let ctx =
            build_license_context(None, &[], "ed25519:test", "station-1", store, &mut quiet())
                .unwrap();
        assert_eq!(
            lease_counts_line(&ctx.lease_state()),
            "lease counts: 5 licensed erasure(s) under licence lic-old, 2 evaluation \
             erasure(s) on this station"
        );
    }

    #[test]
    fn a_licensed_station_starts_from_its_saved_lease_count() {
        let dir = lease_tmpdir("licensed");
        let now = time::OffsetDateTime::now_utc();
        let root = wipe_license::VendorRoot::generate();
        let ent = wipe_license::Entitlements {
            customer_id: "c".into(),
            customer_name: "C".into(),
            quota: wipe_license::Quota::Count { erasures: 10 },
            scope: wipe_license::Scope::Site {
                site_id: "s".into(),
            },
            not_before: now - time::Duration::days(1),
            not_after: now + time::Duration::days(30),
            features: vec![],
            allowed_methods: wipe_license::AllowedMethods::All,
            machine_binding: None,
        };
        let license = root.issue("lic-cli", "ed25519:test", ent, now).unwrap();
        let license_path = dir.join("licence.json");
        wipe_license::save_license(&license_path, &license).unwrap();

        let lease_path = dir.join("lease-state.json");
        let mut seeded = wipe_license::LeaseState::new(now - time::Duration::hours(1));
        seeded.erasures_used = 2;
        seeded.license_id = Some("lic-cli".into());
        let store = Arc::new(crate::LocalFileLeaseStore::new(lease_path));
        LeaseStore::save(store.as_ref(), &seeded).unwrap();

        let ctx = build_license_context(
            Some(license_path),
            &[root.verifying_key()],
            "ed25519:test",
            "station-1",
            store,
            &mut quiet(),
        )
        .unwrap();
        assert_eq!(ctx.lease_state().erasures_used, 2);
        assert_eq!(ctx.lease_persistence().tier, crate::StoreTier::LocalFile);
    }

    #[cfg(unix)]
    #[test]
    fn serve_stores_on_a_read_only_directory_runs_the_lease_in_ram_and_says_so() {
        use std::os::unix::fs::PermissionsExt;
        let dir = lease_tmpdir("read-only");
        let set = |mode| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode));
        set(0o500).unwrap();
        let probe = dir.join(".mode-probe");
        if std::fs::write(&probe, b"x").is_ok() {
            let _ = std::fs::remove_file(&probe);
            set(0o700).unwrap();
            return; // mode bits do not stop this user (root)
        }
        let (_, lease) = serve_stores(&store_cfg_in(&dir), false);
        set(0o700).unwrap();
        assert_eq!(lease.tier(), crate::StoreTier::Ephemeral);
        let ctx =
            build_license_context(None, &[], "ed25519:test", "station-1", lease, &mut quiet())
                .unwrap();
        let line = lease_store_line(&ctx.lease_persistence());
        assert!(
            line.contains("quota count and clock-rollback protection reset on reboot"),
            "{line}"
        );
    }

    #[test]
    fn the_start_up_line_says_what_a_ram_only_lease_loses_on_reboot() {
        let store = Arc::new(EphemeralLeaseStore::new(
            "This station cannot write /ro.".into(),
        ));
        let ctx =
            build_license_context(None, &[], "ed25519:test", "station-1", store, &mut quiet())
                .unwrap();
        let line = lease_store_line(&ctx.lease_persistence());
        assert!(line.starts_with("lease store: Ephemeral at "), "{line}");
        assert!(
            line.contains("quota count and clock-rollback protection reset on reboot"),
            "{line}"
        );
    }

    #[test]
    fn the_hub_line_names_a_saved_hub_and_its_file() {
        let none = "hub: none — run wipestation enroll --hub <origin> to register this station";
        assert_eq!(hub_line(None), none);
        let dir = lease_tmpdir("hub-line");
        let path = crate::store::station_settings_path(&store_cfg_in(&dir));
        assert_eq!(hub_line(Some(&path)), none, "no file yet");
        crate::store::save_station_settings(
            &path,
            &crate::store::StationSettings {
                hub_origin: Some("https://hub.example".into()),
            },
        )
        .unwrap();
        assert_eq!(
            hub_line(Some(&path)),
            format!(
                "hub: https://hub.example (from {}); checked for a renewed licence at start-up \
                 and every 24 hours",
                path.display()
            )
        );
        fs::write(&path, "{}").unwrap();
        assert_eq!(hub_line(Some(&path)), none, "no address saved");
    }

    #[tokio::test]
    async fn an_invalid_settings_file_is_reported_in_the_hub_line_not_fatal() {
        let dir = lease_tmpdir("hub-line-bad");
        let path = crate::store::station_settings_path(&store_cfg_in(&dir));
        fs::write(&path, "not json").unwrap();
        let line = hub_line(Some(&path));
        assert!(line.starts_with("hub: none — "), "{line}");
        assert!(line.contains("could not be read"), "{line}");
        assert!(line.contains(&path.display().to_string()), "{line}");

        fs::write(&path, r#"{"hub_origin":"http://hub.example"}"#).unwrap();
        let line = hub_line(Some(&path));
        assert!(
            line.contains("holds a hub address this station refuses"),
            "{line}"
        );

        // build_station reports the line and still starts.
        let mut cfg = StationConfig::new("127.0.0.1:0".parse().unwrap());
        cfg.persistence = Persistence::Detect {
            control_plane_url: None,
            ephemeral_config: true,
            config_dir: Some(dir.clone()),
        };
        let mut lines = Vec::new();
        build_station(cfg, &mut |l| lines.push(l)).unwrap();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("hub: none — ") && l.contains(&path.display().to_string())),
            "{lines:#?}"
        );

        // A saved hub is named with its file.
        fs::write(&path, r#"{"hub_origin":"https://hub.example"}"#).unwrap();
        let mut cfg = StationConfig::new("127.0.0.1:0".parse().unwrap());
        cfg.persistence = Persistence::Detect {
            control_plane_url: None,
            ephemeral_config: true,
            config_dir: Some(dir.clone()),
        };
        let mut lines = Vec::new();
        build_station(cfg, &mut |l| lines.push(l)).unwrap();
        let want = format!(
            "hub: https://hub.example (from {}); checked for a renewed licence at start-up and \
             every 24 hours",
            path.display()
        );
        assert!(lines.contains(&want), "{lines:#?}");
    }

    #[test]
    fn a_licence_without_vendor_roots_is_refused() {
        let store: Arc<dyn LeaseStore> = Arc::new(EphemeralLeaseStore::new("test".into()));
        let err = build_license_context(
            Some(PathBuf::from("/nonexistent/licence.json")),
            &[],
            "ed25519:test",
            "station-1",
            store,
            &mut quiet(),
        )
        .err()
        .unwrap();
        assert!(
            err.to_string()
                .starts_with("--license needs --vendor-root-b64"),
            "{err}"
        );
    }
}
