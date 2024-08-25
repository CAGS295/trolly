mod depth;
mod order_book;

use clap::{Subcommand, ValueEnum};
use depth::DepthConfig;
pub use depth::{Depth, DepthUpdate, DepthHandler};

#[derive(Subcommand, Debug)]
pub enum Monitorables {
    /// Depth of the symbol.
    Depth(DepthConfig),
}

#[derive(Clone, Debug, ValueEnum)]
#[non_exhaustive]
pub enum Provider {
    Binance,
    Other,
}

pub(crate) trait Monitor {
    async fn monitor(&self);
}
