//! Depth feed providers grouped by venue family.
//!
//! Binance spot lives at [`binance::spot`] (`--sources binance:SYMBOL` unchanged).

pub mod binance;
pub mod stub;

pub use binance::Binance;
pub use stub::Stub;
