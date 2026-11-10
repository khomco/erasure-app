//! Axum HTTP + WebSocket server for the wipestation.
//!
//! Same routes are used by:
//!   * Tauri frontend (when running in standalone mode, talks to localhost)
//!   * Operator tablet console (talks to a remote station's API port)
//!   * Hub (future) for cross-site visibility.

pub mod app;
pub mod bootstrap;
pub mod enrollment;
pub mod handlers;
pub mod hub_client;
pub mod licensing;
pub mod store;
pub mod ws;

pub use app::*;
pub use licensing::{LicenseContext, LicenseCustomer, LicenseView, SigningDecision};
pub use store::{
    detect_lease_store, lease_state_path, EphemeralLeaseStore, LeasePersistence, LeaseStore,
    LocalFileLeaseStore, StoreStatus, StoreTier, TopologyStore,
};
