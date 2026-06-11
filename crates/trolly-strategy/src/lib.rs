//! Strategy runtime: consume multi-symbol stream events and dispatch outbound messages (scaffold).

pub use trolly_stream;

/// Crate identifier for workspace wiring tests.
pub const CRATE: &str = env!("CARGO_PKG_NAME");
