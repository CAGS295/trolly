use serde::Deserialize;

mod binance_usd_m;
pub mod depth;

pub use binance_usd_m::{BinanceUsdM, RPI_PREFIX};
pub use depth::binance::spot::Binance;
pub use depth::other::Other;
pub use depth::REGISTERED_LABELS;

/// A [Provider] must implement this trait for [net] to know where to pull the data from.
pub trait Endpoints<Monitorable> {
    fn websocket_url(&self) -> String;
    fn ws_subscriptions(&self, symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String>;
    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String;
}

trait ApiURL {
    const STREAM: &'static str;
    const REST: &'static str;
}

#[derive(Deserialize, PartialEq, Debug)]
pub struct NullResponse {
    id: u64,
    pub result: Option<String>,
}

#[cfg(test)]
mod test {
    use super::depth::REGISTERED_LABELS;
    use super::NullResponse;

    #[test]
    fn deserialize_empty_response() {
        let body = r#"{"result":null,"id":1}"#;
        let expected: NullResponse = serde_json::from_str(body).unwrap();
        assert_eq!(
            NullResponse {
                id: 1,
                result: None
            },
            expected
        );
    }

    #[test]
    fn registered_labels_include_binance_and_other() {
        assert!(REGISTERED_LABELS.contains(&"binance"));
        assert!(REGISTERED_LABELS.contains(&"binance-usd-m"));
        assert!(REGISTERED_LABELS.contains(&"other"));
    }
}
