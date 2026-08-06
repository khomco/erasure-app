//! Axum HTTP + WebSocket server for the wipestation.
//!
//! Same routes are used by:
//!   * Tauri frontend (when running in standalone mode, talks to localhost)
//!   * Operator tablet console (talks to a remote station's API port)
//!   * Hub (future) for cross-site visibility.

pub mod app;
pub mod handlers;
pub mod store;
pub mod ws;

pub use app::*;
pub use store::{StoreStatus, StoreTier, TopologyStore};

