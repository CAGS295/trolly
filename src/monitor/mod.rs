use async_trait::async_trait;
use clap::{Subcommand, ValueEnum};

mod depth;
mod order_book;

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

#[async_trait]
pub(crate) trait Monitor {
    async fn monitor(&self);
}
