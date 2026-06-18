mod depth;
pub mod depth_parse;
pub mod echo_depth;

pub use depth::{stream_depth_echo, DepthOutput};
pub use depth_parse::{parse_depth_bytes, parse_depth_message};
pub mod global_book;
pub mod order_book;

pub use depth::{Depth, DepthConfig, DepthUpdate};
use clap::{Subcommand, ValueEnum};

pub use global_book::{
    run_global_book_stream, stream_global_book, stream_global_depth_serve, GlobalBookHub,
};

#[derive(Subcommand, Debug)]
pub enum Monitorables {
    /// Depth of the symbol.
    Depth(DepthConfig),
}

/// Depth venue for CLI `--provider` and global-book `--sources provider:SYMBOL`.
///
/// Registered labels: `binance`, `binance-usd-m`, `other` (see [`Provider::from_label`]).
/// New exchanges add a module under [`crate::providers::depth`] and extend [`Provider`].
#[derive(Clone, Copy, Debug, ValueEnum, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum Provider {
    Binance,
    BinanceUsdM,
    /// Scaffold third venue (`stub:SYMBOL`); see [`crate::providers::Stub`].
    Stub,
    Other,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::Binance => "binance",
            Provider::BinanceUsdM => "binance-usd-m",
            Provider::Stub => "stub",
            Provider::Other => "other",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "binance" => Some(Provider::Binance),
            "binance-usd-m" | "binance_usd_m" | "binanceusdm" => Some(Provider::BinanceUsdM),
            "stub" => Some(Provider::Stub),
            _ => None,
        }
    }
}

/// One depth feed: exchange provider + subscription symbol (may include venue-specific prefixes).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BookSource {
    pub provider: Provider,
    pub symbol: String,
}

impl BookSource {
    pub fn new(provider: Provider, symbol: impl Into<String>) -> Self {
        Self {
            provider,
            symbol: symbol.into().trim().to_uppercase(),
        }
    }

    /// Unique routing / factory key (`binance:BTCUSDT`, `binance-usd-m:RPI:BTCUSDC`, …).
    pub fn stream_id(&self) -> String {
        format!("{}:{}", self.provider.label(), self.symbol)
    }

    /// Instrument identity for cross-source merge (uppercase symbol only).
    pub fn canonical_instrument(&self) -> String {
        self.symbol.clone()
    }

    /// Parse `provider:SYMBOL` (e.g. `binance:BTCUSDT`, `binance-usd-m:RPI:BTCUSDC`, `stub:BTCUSDT`).
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let (provider_label, symbol) = raw
            .split_once(':')
            .ok_or_else(|| format!("expected provider:SYMBOL, got {raw:?}"))?;
        let provider = Provider::from_label(provider_label)
            .ok_or_else(|| format!("unknown provider {provider_label:?}"))?;
        if symbol.trim().is_empty() {
            return Err(format!("missing symbol in {raw:?}"));
        }
        Ok(Self::new(provider, symbol))
    }
}

/// Parse a comma-separated list of `provider:SYMBOL` entries.
pub fn parse_book_sources(list: &str) -> Result<Vec<BookSource>, String> {
    let mut out = Vec::new();
    for part in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        out.push(BookSource::parse(part)?);
    }
    if out.is_empty() {
        return Err("at least one book source is required".into());
    }
    Ok(out)
}

pub(crate) trait Monitor {
    async fn monitor(&self);
}

#[cfg(test)]
mod book_source_tests {
    use super::{parse_book_sources, BookSource, Provider};

    #[test]
    fn parse_book_source_spot() {
        let s = BookSource::parse("binance:btcusdt").unwrap();
        assert_eq!(s.provider, Provider::Binance);
        assert_eq!(s.symbol, "BTCUSDT");
        assert_eq!(s.stream_id(), "binance:BTCUSDT");
        assert_eq!(s.canonical_instrument(), "BTCUSDT");
    }

    #[test]
    fn parse_book_source_usdm_rpi() {
        let s = BookSource::parse("binance-usd-m:RPI:BTCUSDC").unwrap();
        assert_eq!(s.stream_id(), "binance-usd-m:RPI:BTCUSDC");
        assert_eq!(s.canonical_instrument(), "RPI:BTCUSDC");
    }

    #[test]
    fn parse_book_source_stub_venue() {
        let s = BookSource::parse("stub:ethusdt").unwrap();
        assert_eq!(s.provider, Provider::Stub);
        assert_eq!(s.symbol, "ETHUSDT");
        assert_eq!(s.stream_id(), "stub:ETHUSDT");
    }

    #[test]
    fn parse_book_sources_list() {
        let v = parse_book_sources("binance:BTCUSDT,binance-usd-m:BTCUSDT,stub:BTCUSDT").unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].provider, Provider::Binance);
        assert_eq!(v[0].canonical_instrument(), "BTCUSDT");
        assert_eq!(v[1].provider, Provider::BinanceUsdM);
        assert_eq!(v[1].canonical_instrument(), "BTCUSDT");
        assert_eq!(v[2].provider, Provider::Stub);
        assert_eq!(v[2].stream_id(), "stub:BTCUSDT");
    }

    #[test]
    fn parse_book_sources_rejects_unknown_provider() {
        assert!(parse_book_sources("unknown:BTCUSDT").is_err());
    }
}
