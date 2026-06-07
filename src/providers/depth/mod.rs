//! Depth feed venues for `--sources provider:SYMBOL`.
//!
//! Layout: `depth::<exchange>::<market>` (e.g. [`binance::spot`]).
//! Top-level siblings like [`super::BinanceUsdM`] remain until migrated (see `.todo`).

pub mod binance;
pub mod other;

pub use binance::Binance;
pub use other::Other;

/// CLI / global-book labels accepted by [`crate::monitor::Provider::from_label`].
pub const REGISTERED_LABELS: &[&str] = &["binance", "binance-usd-m", "other"];
