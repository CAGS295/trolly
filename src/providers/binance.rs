use super::{ApiURL, Endpoints};
use crate::monitor::Depth;

#[derive(Clone)]
pub struct Binance;

impl ApiURL for Binance {
    const STREAM: &'static str = "wss://stream.binance.com:9443";
    const REST: &'static str = "https://api.binance.com/api/v3";
}

impl Endpoints<Depth> for Binance {
    fn websocket_url(&self) -> String {
        format!("{}/ws", Self::STREAM)
    }

    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String {
        format!(
            "{}/depth?symbol={}&limit=5000",
            Self::REST,
            symbol.as_ref().to_uppercase()
        )
    }

    fn ws_subscriptions(&self, symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        let symbols: Vec<_> = symbols.map(|s| s.as_ref().to_lowercase() + "@depth").collect();

        vec![format!(
            r#"{{"method": "SUBSCRIBE", "params": {:?}, "id": 1}}"#,
            symbols
        )]
    }
}

#[cfg(test)]
mod test {
    use super::Binance;
    use super::Endpoints;

    #[test]
    fn subscription_serialization() {
        let symbols = ["a", "b"];
        let msgs = Binance.ws_subscriptions(symbols.iter());
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0],
            r#"{"method": "SUBSCRIBE", "params": ["a@depth", "b@depth"], "id": 1}"#
        );
    }
}
