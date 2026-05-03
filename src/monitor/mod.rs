mod depth;
pub mod depth_parse;
pub mod echo_depth;
pub mod order_book;

use clap::{Subcommand, ValueEnum};
pub use depth::{stream_depth_echo, Depth, DepthConfig, DepthOutput, DepthUpdate};
pub use depth_parse::{parse_depth_bytes, parse_depth_message};

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
