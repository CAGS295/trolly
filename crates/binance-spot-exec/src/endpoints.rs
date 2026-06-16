use serde_json::json;
use trolly_stream::VenueEndpoints;

use crate::auth::build_subscribe_signature_params;

/// API credentials for signed Binance spot REST and WebSocket API calls.
#[derive(Clone, Debug)]
pub struct ApiCredentials {
    pub api_key: String,
    pub secret_key: String,
}

/// Binance spot REST trading endpoints.
#[derive(Clone, Debug)]
pub struct BinanceSpotRest;

impl BinanceSpotRest {
    pub const PRODUCTION_URL: &'static str = "https://api.binance.com";
    pub const TESTNET_URL: &'static str = "https://testnet.binance.vision";
    /// Spot demo REST host ([demo mode general info](https://developers.binance.com/docs/binance-spot-api-docs/demo-mode/general-info)).
    pub const DEMO_URL: &'static str = "https://demo-api.binance.com";

    pub fn demo_depth_url(symbol: impl AsRef<str>, limit: u16) -> String {
        format!(
            "{}/api/v3/depth?symbol={}&limit={}",
            Self::DEMO_URL,
            symbol.as_ref().to_uppercase(),
            limit
        )
    }
}

/// Binance spot user-data stream endpoints (WebSocket API, no REST trading).
#[derive(Clone, Debug)]
pub struct BinanceSpotUserStream {
    pub credentials: ApiCredentials,
}

impl BinanceSpotUserStream {
    pub const WS_API_URL: &'static str = "wss://ws-api.binance.com:443/ws-api/v3";
    /// Spot demo WebSocket API ([demo mode general info](https://developers.binance.com/docs/binance-spot-api-docs/demo-mode/general-info)).
    pub const DEMO_WS_API_URL: &'static str = "wss://demo-ws-api.binance.com/ws-api/v3";
    /// Spot demo market streams host (map from production `stream.binance.com`).
    pub const DEMO_STREAM_URL: &'static str = "wss://demo-stream.binance.com/ws";

    pub fn new(credentials: ApiCredentials) -> Self {
        Self { credentials }
    }

    pub fn subscribe_request_json(&self) -> String {
        let params = build_subscribe_signature_params(
            &self.credentials.api_key,
            &self.credentials.secret_key,
        );
        json!({
            "id": "binance-spot-exec-subscribe",
            "method": "userDataStream.subscribe.signature",
            "params": params,
        })
        .to_string()
    }
}

impl VenueEndpoints for BinanceSpotUserStream {
    fn websocket_url(&self) -> String {
        Self::WS_API_URL.into()
    }

    /// User-data is account-wide; `symbols` are ignored for subscribe payloads.
    /// Symbols are used by [`SpotExecHandler`] / multiplexor routing only.
    fn ws_subscriptions(&self, _symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        vec![self.subscribe_request_json()]
    }

    fn rest_api_url(&self, _symbol: impl AsRef<str>) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_depth_url() {
        assert_eq!(
            BinanceSpotRest::demo_depth_url("btcusdt", 500),
            "https://demo-api.binance.com/api/v3/depth?symbol=BTCUSDT&limit=500"
        );
    }
}
