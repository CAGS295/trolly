use super::{ApiURL, Endpoints};
use crate::monitor::Depth;

pub(crate) struct Binance;

impl ApiURL for Binance {
    const STREAM: &'static str = "wss://data-stream.binance.com";
    const REST: &'static str = "https://api.binance.com/api/v3";
}

impl Endpoints<Depth> for Binance {
    fn websocket_url(&self, symbol: impl AsRef<str>) -> String {
        format!(
            "{}/ws/{}@depth",
            Self::STREAM,
            symbol.as_ref().to_lowercase()
        )
    }

    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String {
        format!(
            "{}/depth?symbol={}",
            Self::REST,
            symbol.as_ref().to_uppercase()
        )
    }
}
