mod depth;
mod order_book;

use clap::{Subcommand, ValueEnum};
pub use depth::Depth;
use depth::DepthConfig;

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
