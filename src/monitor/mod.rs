mod depth;
pub mod depth_parse;
pub mod echo_depth;

pub use depth::{stream_depth_echo, DepthOutput};
pub use depth_parse::{parse_depth_bytes, parse_depth_message};
pub mod aggregated_depth;
pub mod order_book;

pub use aggregated_depth::{
    canonical_depth_symbol, merge_naive_extend, run_aggregated_depth_stream,
    stream_depth_aggregated, AggregatedDepthHub,
};
use clap::{Subcommand, ValueEnum};
pub use depth::{Depth, DepthConfig, DepthUpdate};

#[derive(Subcommand, Debug)]
pub enum Monitorables {
    /// Depth of the symbol.
    Depth(DepthConfig),
}

#[derive(Clone, Debug, ValueEnum)]
#[non_exhaustive]
pub enum Provider {
    Binance,
    BinanceUsdM,
    Other,
}

pub(crate) trait Monitor {
    async fn monitor(&self);
}
