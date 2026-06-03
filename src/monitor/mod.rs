mod depth;
pub mod depth_parse;
pub mod echo_depth;

pub use depth::{stream_depth_echo, DepthOutput};
pub use depth_parse::{parse_depth_bytes, parse_depth_message};
pub mod global_book;
pub mod order_book;

pub use global_book::{
    parse_book_sources, run_global_book_stream, stream_global_book, BookSource, GlobalBookHub,
};
use clap::{Subcommand, ValueEnum};
pub use depth::{Depth, DepthConfig, DepthUpdate};

#[derive(Subcommand, Debug)]
pub enum Monitorables {
    /// Depth of the symbol.
    Depth(DepthConfig),
}

#[derive(Clone, Copy, Debug, ValueEnum, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum Provider {
    Binance,
    BinanceUsdM,
    Other,
}

pub(crate) trait Monitor {
    async fn monitor(&self);
}
