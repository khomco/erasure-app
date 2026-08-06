//! Station configuration store (ADR-0003).
//!
//! Evidence never touches local storage. *Configuration* — today just the bay
//! topology — does, through a backend the station picks for itself:
//!
//! 1. [`LocalFileStore`]   — the config path is writable. Atomic write.
//! 2. [`ControlPlaneStore`] — read-only root, but a hub is configured.
//! 3. *(operator prompt)*  — nowhere to persist; ask, don't guess.
//! 4. [`EphemeralStore`]   — acknowledged: works this boot, lost on reboot.
//!
//! The tier is *detected*, not configured. An operator should not have to know
//! whether the root filesystem they booted is writable.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use wipe_common::BayTopology;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("no writable location for station configuration")]
    Unavailable,
    #[error(
        "configuration was changed by someone else (stored revision {stored}, you sent {sent})"
    )]
    RevisionConflict { stored: u32, sent: u32 },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not parse stored configuration: {0}")]
    Parse(String),
    #[error("control plane unreachable: {0}")]
    ControlPlane(String),
}

/// Which backend is in play. Exposed over the API because "why did my layout
/// vanish?" is a question an operator will ask, and the answer should not be
/// buried in a log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreTier {
    /// Writable local file. Survives reboot.
    LocalFile,
    /// Pushed to a control plane keyed by station id. Survives reboot centrally.
    ControlPlane,
    /// Nothing is writable and no control plane is configured. The station is
    /// fully usable; configuration is lost on reboot.
    Ephemeral,
}

impl StoreTier {
    pub fn survives_reboot(self) -> bool {
        !matches!(self, Self::Ephemeral)
    }
}

/// What the UI needs to tell the operator the truth about persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreStatus {
    pub tier: StoreTier,
    pub survives_reboot: bool,
    /// Human-readable location: a path, a URL, or "memory".
    pub location: String,
    /// Set when the station could not persist and has not been told what to do
    /// about it — tier 3. The UI is expected to prompt rather than silently
    /// running ephemeral.
    pub needs_operator_decision: bool,
    /// Why the station ended up on this tier. Shown verbatim to the operator.
    pub detail: String,
}

pub trait TopologyStore: Send + Sync {
    fn tier(&self) -> StoreTier;
    fn location(&self) -> String;
    /// Why this tier was chosen. Written for an operator, not a log reader.
    fn detail(&self) -> String;
    fn load(&self) -> Result<Option<BayTopology>, StoreError>;
    fn save(&self, topology: &BayTopology) -> Result<(), StoreError>;

    /// Tier 3: the station could not persist and no one has said what to do
    /// about it. Backends that *can* persist have answered by existing.
    fn awaiting_decision(&self) -> bool {
        false
    }

    /// Operator has accepted that configuration will not survive reboot.
    fn acknowledge(&self) {}
}

// ---------------------------------------------------------------------------
// Tier 1 — local file
// ---------------------------------------------------------------------------

pub struct LocalFileStore {
    path: PathBuf,
}

impl LocalFileStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Can we actually write here?
    ///
    /// Deliberately a write probe rather than an inference from permissions or
    /// mount flags: read-only NFS, overlayfs and a full disk all look writable
    /// to a metadata check and are not.
    pub fn probe(path: &Path) -> Result<(), std::io::Error> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir)?;
        let probe = dir.join(format!(".wipestation-probe-{}", std::process::id()));
        std::fs::write(&probe, b"probe")?;
        std::fs::remove_file(&probe)?;
        Ok(())
    }
}

impl TopologyStore for LocalFileStore {
    fn tier(&self) -> StoreTier {
        StoreTier::LocalFile
    }

    fn location(&self) -> String {
        self.path.display().to_string()
    }

    fn detail(&self) -> String {
        format!(
            "Saved to {} on this station. Survives reboot.",
            self.path.display()
        )
    }

    fn load(&self) -> Result<Option<BayTopology>, StoreError> {
        match std::fs::read_to_string(&self.path) {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| StoreError::Parse(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, topology: &BayTopology) -> Result<(), StoreError> {
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir)?;
        // Write beside the target and rename: a half-written topology is worse
        // than no topology, and rename is atomic within a filesystem.
        let tmp = dir.join(format!(
            ".{}.tmp-{}",
            self.path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "bay-topology.json".into()),
            std::process::id()
        ));
        let body =
            serde_json::to_string_pretty(topology).map_err(|e| StoreError::Parse(e.to_string()))?;
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tier 2 — control plane (client-side seam)
// ---------------------------------------------------------------------------

/// Pushes configuration to a central control plane keyed by station id, so a
/// station with no writable storage still gets its layout back after a reboot.
///
/// **The hub does not exist yet** (CONTEXT §11 v0.3 #7). This is a real seam,
/// not a stub: with no endpoint configured it reports itself unavailable and
/// the station falls to the operator prompt. Nothing here pretends to succeed.
///
/// Wire contract, deliberately boring:
///   `GET  {base}/api/stations/{station_id}/bay-topology`
///   `PUT  {base}/api/stations/{station_id}/bay-topology`
/// carrying the same document the local file holds.
///
/// Discovery is config-first (`--control-plane-url`). mDNS, and "the elected
/// Lead is the control plane", are the obvious later options — both wait on the
/// Lead having differentiated responsibilities at all (CONTEXT §12).
pub struct ControlPlaneStore {
    base_url: String,
    station_id: String,
}

impl ControlPlaneStore {
    pub fn new(base_url: String, station_id: String) -> Self {
        Self {
            base_url,
            station_id,
        }
    }

    pub fn endpoint(&self) -> String {
        format!(
            "{}/api/stations/{}/bay-topology",
            self.base_url.trim_end_matches('/'),
            self.station_id
        )
    }

    /// Is the configured control plane actually reachable?
    ///
    /// No HTTP client is wired in yet, so this is honest about not knowing
    /// rather than optimistically claiming the tier.
    pub fn probe(&self) -> Result<(), StoreError> {
        Err(StoreError::ControlPlane(
            "control-plane transport is not implemented yet (hub is v0.3 #7)".into(),
        ))
    }
}

impl TopologyStore for ControlPlaneStore {
    fn tier(&self) -> StoreTier {
        StoreTier::ControlPlane
    }

    fn location(&self) -> String {
        self.endpoint()
    }

    fn detail(&self) -> String {
        format!(
            "Saved centrally to {} for station `{}`.",
            self.base_url, self.station_id
        )
    }

    fn load(&self) -> Result<Option<BayTopology>, StoreError> {
        self.probe().map(|_| None)
    }

    fn save(&self, _topology: &BayTopology) -> Result<(), StoreError> {
        self.probe()
    }
}

// ---------------------------------------------------------------------------
// Tier 4 — ephemeral
// ---------------------------------------------------------------------------

/// In-RAM only. Fully functional for this boot; the UI is expected to say so.
pub struct EphemeralStore {
    held: RwLock<Option<BayTopology>>,
    reason: String,
    /// True until an operator has acknowledged that config will not persist.
    /// Tier 3 in ADR-0003: the difference between a decision and a surprise.
    awaiting_decision: RwLock<bool>,
}

impl EphemeralStore {
    pub fn new(reason: String, awaiting_decision: bool) -> Self {
        Self {
            held: RwLock::new(None),
            reason,
            awaiting_decision: RwLock::new(awaiting_decision),
        }
    }
}

impl TopologyStore for EphemeralStore {
    fn tier(&self) -> StoreTier {
        StoreTier::Ephemeral
    }

    fn location(&self) -> String {
        "memory (this boot only)".into()
    }

    fn detail(&self) -> String {
        format!(
            "{} Configuration works normally but is lost when this station reboots. \
             Use Export to keep a copy.",
            self.reason
        )
    }

    fn load(&self) -> Result<Option<BayTopology>, StoreError> {
        Ok(self.held.read().clone())
    }

    fn save(&self, topology: &BayTopology) -> Result<(), StoreError> {
        *self.held.write() = Some(topology.clone());
        Ok(())
    }

    fn awaiting_decision(&self) -> bool {
        *self.awaiting_decision.read()
    }

    fn acknowledge(&self) {
        *self.awaiting_decision.write() = false;
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// How the station was told to find its configuration.
#[derive(Debug, Clone, Default)]
pub struct StoreConfig {
    /// Explicit `--bay-topology <path>`. Read *and* written.
    pub explicit_path: Option<PathBuf>,
    /// `--control-plane-url`.
    pub control_plane_url: Option<String>,
    pub station_id: String,
}

/// Default config path when none was given: `$WIPESTATION_CONFIG_DIR`, else the
/// platform config dir, else `./`.
pub fn default_config_path() -> PathBuf {
    if let Ok(dir) = std::env::var("WIPESTATION_CONFIG_DIR") {
        return PathBuf::from(dir).join("bay-topology.json");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("wipestation")
            .join("bay-topology.json");
    }
    PathBuf::from("bay-topology.json")
}

/// Walk the tiers and return the first backend that can actually hold a
/// configuration. Never fails — the floor is ephemeral.
pub fn detect_store(cfg: &StoreConfig) -> Arc<dyn TopologyStore> {
    let path = cfg
        .explicit_path
        .clone()
        .unwrap_or_else(default_config_path);

    // Tier 1 — is the config path genuinely writable?
    match LocalFileStore::probe(&path) {
        Ok(()) => {
            tracing::info!(path = %path.display(), "config store: local file");
            return Arc::new(LocalFileStore::new(path));
        }
        Err(e) => {
            tracing::warn!(
                path = %path.display(), error = %e,
                "config store: local path not writable, trying control plane"
            );
        }
    }

    // Tier 2 — a control plane, if one is configured and reachable.
    if let Some(url) = cfg.control_plane_url.clone() {
        let cp = ControlPlaneStore::new(url.clone(), cfg.station_id.clone());
        match cp.probe() {
            Ok(()) => {
                tracing::info!(url = %url, "config store: control plane");
                return Arc::new(cp);
            }
            Err(e) => {
                tracing::warn!(url = %url, error = %e, "config store: control plane unavailable");
                return Arc::new(EphemeralStore::new(
                    format!("This station has no writable storage, and the configured control plane at {url} could not be used ({e})."),
                    true,
                ));
            }
        }
    }

    // Tier 3 — nowhere to persist and nothing configured. Hold the decision
    // open so the UI prompts rather than quietly running ephemeral.
    tracing::warn!("config store: ephemeral — awaiting operator decision");
    Arc::new(EphemeralStore::new(
        format!(
            "This station has no writable storage ({}) and no control plane is configured.",
            path.display()
        ),
        true,
    ))
}

/// Build a [`StoreStatus`] for the API.
pub fn status_of(store: &Arc<dyn TopologyStore>) -> StoreStatus {
    StoreStatus {
        tier: store.tier(),
        survives_reboot: store.tier().survives_reboot(),
        location: store.location(),
        needs_operator_decision: store.awaiting_decision(),
        detail: store.detail(),
    }
}
