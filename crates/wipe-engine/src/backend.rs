use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use wipe_common::{
    Capabilities, CommandEvidence, Device, DeviceId, Method, VerificationMethod,
    VerificationReport, WipeError, WipeResult,
};

/// Opaque handle returned by `DeviceBackend::issue` that identifies the
/// in-progress operation. Backends are free to encode their own state inside.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendHandle {
    pub id: Uuid,
    pub device: DeviceId,
    pub method: Method,
    /// Evidence captured at issuance — the command that was sent.
    pub issued_evidence: CommandEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackendProgress {
    InProgress {
        fraction: f32,
        eta_seconds: Option<u64>,
        bytes_processed: Option<u64>,
        latest_evidence: Option<CommandEvidence>,
    },
    Completed {
        final_evidence: CommandEvidence,
    },
    Failed {
        evidence: CommandEvidence,
        reason: String,
    },
}

/// Abstraction over the actual device-touching code. Implementations:
///   * `wipe-engine-mock::MockBackend` — simulated devices, used in tests/CI/demos.
///   * (future) `wipe-engine-linux` — `ioctl(SG_IO)` + NVMe admin + raw block I/O.
#[async_trait]
pub trait DeviceBackend: Send + Sync + 'static {
    /// Probe attached storage. Read-only.
    async fn enumerate(&self) -> WipeResult<Vec<Device>>;

    /// Capability probe for a single device.
    async fn capabilities(&self, id: &DeviceId) -> WipeResult<Capabilities>;

    /// If the device is in the ATA "frozen" state, issue a suspend/resume
    /// to unfreeze. No-op on devices that don't support freezing.
    async fn unfreeze(&self, id: &DeviceId) -> WipeResult<()>;

    /// Issue the sanitization command. Returns immediately with a handle;
    /// poll the handle to track progress.
    async fn issue(&self, id: &DeviceId, method: &Method) -> WipeResult<BackendHandle>;

    /// Poll an in-flight operation. The backend MAY block briefly to wait
    /// for the next progress checkpoint, but it MUST NOT block indefinitely.
    async fn poll(&self, handle: &BackendHandle) -> WipeResult<BackendProgress>;

    /// Cancel an in-flight operation if the device supports it (most
    /// firmware-driven sanitizes can't be aborted; backend returns
    /// `WipeError::InvalidState` in that case).
    async fn cancel(&self, handle: &BackendHandle) -> WipeResult<()>;

    /// Run the post-erase verification.
    async fn verify(
        &self,
        id: &DeviceId,
        method: VerificationMethod,
        samples: u32,
    ) -> WipeResult<VerificationReport>;
}

/// A convenience boxed-trait alias.
pub type DynBackend = std::sync::Arc<dyn DeviceBackend>;

/// Helper: error mapping shortcut used by both backends and the runner.
pub fn backend_err<E: std::fmt::Display>(e: E) -> WipeError {
    WipeError::Backend(e.to_string())
}

/// Simulated hot-plug, for demos, tests and the identify-mode flow.
///
/// Deliberately *not* part of [`DeviceBackend`]: attaching a drive is
/// something an operator's hands do, not something a sanitization backend
/// offers. A real backend will never implement this, and the server only
/// mounts the `/api/sim/*` routes when something does.
pub trait DeviceSimulator: Send + Sync {
    /// Plug a previously-detached device back in, or the next spare from the
    /// catalog. Returns the device that appeared.
    fn attach(&self, id: Option<&DeviceId>) -> Option<Device>;
    /// Pull a device out. Returns the device that disappeared.
    fn detach(&self, id: &DeviceId) -> Option<Device>;
    /// Devices currently unplugged but available to plug back in.
    fn detached(&self) -> Vec<Device>;
}

/// Enclosures a backend can see, for model auto-discovery (ADR-0004 §5).
///
/// Nothing implements this today: the mock has no enclosures and
/// `wipe-engine-linux` does not exist. Defined now so landing USB/PCI/SES
/// probing later is a backend change rather than a catalog-model change.
pub trait EnclosureDiscovery: Send + Sync {
    fn enclosures(&self) -> Vec<wipe_common::EnclosureIdentity>;
}

/// What a *known* enclosure model can be asked to do (ADR-0004 §6).
///
/// The UI derives affordances from [`EnclosureControl::supported`], not from
/// the catalog's `capabilities` block: the catalog says what the *model* can
/// do, this says what *this station* can currently do about it. They differ
/// whenever the Linux backend is missing, permissions are wrong, or the SES
/// device is not exposed — and showing a locate button that fails is worse
/// than not showing one.
pub trait EnclosureControl: Send + Sync {
    /// Blink the locate LED for a bay. Closes the identify-mode loop from the
    /// other end: today the operator tells us where a drive is, with this we
    /// can tell them.
    fn locate(&self, bay: &wipe_common::BayId, on: bool) -> Result<(), ControlError>;
    fn set_bay_power(&self, bay: &wipe_common::BayId, on: bool) -> Result<(), ControlError>;
    fn supported(&self) -> ControlCapabilities;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlCapabilities {
    pub locate_led: bool,
    pub per_bay_power: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// This station cannot do it — distinct from "the model cannot".
    #[error("not supported on this station: {0}")]
    Unsupported(String),
    #[error("no such bay: {0}")]
    UnknownBay(String),
    #[error("control failed: {0}")]
    Failed(String),
}
