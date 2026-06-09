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

/// Book routing identity for `--sources provider:SYMBOL` (includes unknown venues).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Provider {
    Binance,
    BinanceUsdM,
    /// Third-party venue registered by label until wired with endpoints.
    Custom(String),
}

impl Provider {
    pub fn label(&self) -> &str {
        match self {
            Provider::Binance => "binance",
            Provider::BinanceUsdM => "binance-usd-m",
            Provider::Custom(label) => label.as_str(),
        }
    }

    pub fn from_label(label: &str) -> Self {
        match label.trim().to_ascii_lowercase().as_str() {
            "binance" => Provider::Binance,
            "binance-usd-m" | "binance_usd_m" | "binanceusdm" => Provider::BinanceUsdM,
            other => Provider::Custom(other.to_string()),
        }
    }
}

/// CLI-selectable providers for single-venue depth echo / serve.
#[derive(Clone, Copy, Debug, ValueEnum, Eq, PartialEq, Hash)]
pub enum SelectableProvider {
    Binance,
    BinanceUsdM,
}

impl From<SelectableProvider> for Provider {
    fn from(value: SelectableProvider) -> Self {
        match value {
            SelectableProvider::Binance => Provider::Binance,
            SelectableProvider::BinanceUsdM => Provider::BinanceUsdM,
        }
    }
}

pub(crate) trait Monitor {
    async fn monitor(&self);
}
