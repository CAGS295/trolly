mod depth;
pub mod depth_parse;
pub mod echo_depth;

pub use depth::{stream_depth_echo, DepthOutput};
pub use depth_parse::{parse_depth_bytes, parse_depth_message};
pub mod global_book;
pub mod order_book;

pub use global_book::{
    parse_book_sources, run_global_book_stream, stream_global_book, stream_global_depth_serve,
    BookSource, GlobalBookHub,
};
use clap::{Subcommand, ValueEnum};
pub use depth::{Depth, DepthConfig, DepthUpdate};

#[derive(Subcommand, Debug)]
pub enum Monitorables {
    /// Depth of the symbol.
    Depth(DepthConfig),
}

/// Depth venue for CLI `--provider` and global-book `--sources provider:SYMBOL`.
///
/// Registered labels: `binance`, `binance-usd-m`, `other` (see [`Provider::from_label`]).
/// New exchanges add a module under [`crate::providers::depth`] and extend [`Provider`].
#[derive(Clone, Copy, Debug, ValueEnum, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum Provider {
    Binance,
    BinanceUsdM,
    /// Scaffold third venue; parseable via `--sources other:SYMBOL` (no live stream yet).
    Other,
}

pub(crate) trait Monitor {
    async fn monitor(&self);
}
