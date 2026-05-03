mod depth;
pub mod aggregated_depth;
pub mod order_book;

use clap::{Subcommand, ValueEnum};
pub use aggregated_depth::{
    merge_naive_extend, stream_depth_aggregated, AggregatedDepthHub,
};
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
