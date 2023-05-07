use super::{ApiURL, Endpoints};
use crate::monitor::Depth;

pub(crate) struct Binance;

impl ApiURL for Binance {
    const STREAM: &'static str = "wss://data-stream.binance.com";
    const REST: &'static str = "https://api.binance.com/api/v3";
}

impl Endpoints<Depth> for Binance {
    fn websocket_url() -> String {
        format!("{}/ws", Self::STREAM)
    }

    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String {
        format!(
            "{}/depth?symbol={}",
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
    use super::Binance;
    use super::Endpoints;

    #[test]
    fn subscription_serialization() {
        let symbols = vec!["a", "b"];
        let value = Binance.ws_subscriptions(symbols.iter());
        assert_eq!(
            value,
            r#"{"method": "SUBSCRIBE", "params": ["a@depth", "b@depth"], "id": 1}"#
        );
    }
}
