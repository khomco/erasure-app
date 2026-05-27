//! Shared domain types for the wipestation product.
//!
//! These types are deliberately serde-friendly and have no engine/runtime deps —
//! they cross the boundary between the engine, server, cert, and frontend.

pub mod device;
pub mod method;
pub mod erasure_event;
pub mod job;
pub mod evidence;
pub mod operator;
pub mod fleet;
pub mod error;
pub(crate) mod serde_hex_opt;

pub use device::*;
pub use method::*;
pub use erasure_event::*;
pub use job::*;
pub use evidence::*;
pub use operator::*;
pub use fleet::*;
pub use error::*;
