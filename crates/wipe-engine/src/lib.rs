//! Erasure engine: device-backend abstraction + job orchestration.
//!
//! The `DeviceBackend` trait is the seam between the orchestrator and the
//! hardware-touching code. On real Linux hardware we'll provide an impl
//! that issues `ioctl(SG_IO)` and NVMe admin commands directly. For tests,
//! demos, and CI we use `wipe-engine-mock`.
//!
//! The orchestrator (`JobRunner`) holds the trait object, runs the
//! NIST 800-88 Rev. 2 state machine per job, and broadcasts events.

pub mod backend;
pub mod runner;

pub use backend::*;
pub use runner::*;

pub use wipe_common as common;
