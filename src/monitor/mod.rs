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
use clap::Subcommand;
pub use depth::{Depth, DepthConfig, DepthUpdate};

#[derive(Subcommand, Debug)]
pub enum Monitorables {
    /// Depth of the symbol.
    Depth(DepthConfig),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum Provider {
    Binance,
    BinanceUsdM,
    /// Label from `--sources` that is not yet wired to a depth endpoint.
    Registered(String),
}

impl Provider {
    /// Parse a CLI provider label (`binance`, `binance-usd-m`, or any future venue).
    pub fn from_label(label: &str) -> Self {
        match label.trim().to_ascii_lowercase().as_str() {
            "binance" => Provider::Binance,
            "binance-usd-m" | "binance_usd_m" | "binanceusdm" => Provider::BinanceUsdM,
            other => Provider::Registered(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Provider::Binance => "binance",
            Provider::BinanceUsdM => "binance-usd-m",
            Provider::Registered(label) => label.as_str(),
        }
    }

    pub fn is_known(&self) -> bool {
        matches!(self, Provider::Binance | Provider::BinanceUsdM)
    }
}

impl clap::ValueEnum for Provider {
    fn value_variants<'a>() -> &'a [Self] {
        &[Provider::Binance, Provider::BinanceUsdM]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        match self {
            Provider::Binance => Some(clap::builder::PossibleValue::new("binance")),
            Provider::BinanceUsdM => Some(clap::builder::PossibleValue::new("binance-usd-m")),
            Provider::Registered(_) => None,
        }
    }
}

pub(crate) trait Monitor {
    async fn monitor(&self);
}
