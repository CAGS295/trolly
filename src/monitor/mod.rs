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

#[derive(Clone, Copy, Debug, ValueEnum, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum Provider {
    Binance,
    BinanceUsdM,
    Other,
}

impl Provider {
    /// Resolve a built-in venue label (`binance`, `binance-usd-m`, …).
    pub fn from_builtin_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "binance" => Some(Provider::Binance),
            "binance-usd-m" | "binance_usd_m" | "binanceusdm" => Some(Provider::BinanceUsdM),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        crate::providers::builtin_label(self)
    }
}

pub(crate) trait Monitor {
    async fn monitor(&self);
}
