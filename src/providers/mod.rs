use serde::Deserialize;

mod binance_usd_m;
pub mod depth;

pub use depth::binance::spot::Binance;
pub use binance_usd_m::{BinanceUsdM, RPI_PREFIX};

use crate::monitor::Provider;

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

/// Whether `label` from `--sources provider:SYMBOL` has a wired depth implementation.
pub fn is_registered_depth_label(label: &str) -> bool {
    Provider::from_label(label).is_known()
}

#[derive(Deserialize, PartialEq, Debug)]
pub struct NullResponse {
    id: u64,
    pub result: Option<String>,
}

#[cfg(test)]
mod test {
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
    fn depth_registry_knows_binance_venues() {
        assert!(super::is_registered_depth_label("binance"));
        assert!(super::is_registered_depth_label("binance-usd-m"));
        assert!(!super::is_registered_depth_label("coinbase"));
    }
}
