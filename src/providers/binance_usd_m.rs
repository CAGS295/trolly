use super::{ApiURL, Endpoints};
use crate::monitor::Depth;

#[derive(Clone)]
pub struct BinanceUsdM;

impl ApiURL for BinanceUsdM {
    const STREAM: &'static str = "wss://fstream.binance.com";
    const REST: &'static str = "https://fapi.binance.com/fapi/v1";
}

impl Endpoints<Depth> for BinanceUsdM {
    fn websocket_url(&self) -> String {
        format!("{}/ws", Self::STREAM)
    }

    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String {
        format!(
            "{}/depth?symbol={}&limit=1000",
            Self::REST,
            symbol.as_ref().to_uppercase()
        )
    }

    fn ws_subscriptions(&self, symbols: impl Iterator<Item = impl AsRef<str>>) -> String {
        let symbols = symbols.map(|s| s.as_ref().to_lowercase() + "@depth");

        format!(
            r#"{{"method": "SUBSCRIBE", "params": {:?}, "id": 1}}"#,
            symbols.collect::<Vec<_>>()
        )
    }
}

#[cfg(test)]
mod test {
    use super::BinanceUsdM;
    use super::Endpoints;

    #[test]
    fn subscription_serialization() {
        let symbols = ["btcusdt", "ethusdt"];
        let value = BinanceUsdM.ws_subscriptions(symbols.iter());
        assert_eq!(
            value,
            r#"{"method": "SUBSCRIBE", "params": ["btcusdt@depth", "ethusdt@depth"], "id": 1}"#
        );
    }

    #[test]
    fn websocket_url() {
        let url = BinanceUsdM.websocket_url();
        assert_eq!(url, "wss://fstream.binance.com/ws");
    }

    #[test]
    fn rest_api_url() {
        let url = BinanceUsdM.rest_api_url("btcusdt");
        assert_eq!(
            url,
            "https://fapi.binance.com/fapi/v1/depth?symbol=BTCUSDT&limit=1000"
        );
    }
}
