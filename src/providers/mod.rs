use serde::Deserialize;

pub mod depth;

pub use depth::{
    builtin_label, init_builtin_extensions, normalize_label, register_provider_label,
    resolve_provider_label, Binance, BinanceUsdM, RPI_PREFIX, STUB_PROVIDER_LABEL,
};

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

/// Register built-in extension labels (e.g. scaffold `stub`).
pub fn init_providers() {
    init_builtin_extensions();
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
}
