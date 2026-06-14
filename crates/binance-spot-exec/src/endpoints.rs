use serde_json::json;
use trolly_stream::VenueEndpoints;

use crate::auth::build_subscribe_signature_params;

/// Demo WebSocket API ([Spot Demo general info](https://developers.binance.com/docs/binance-spot-api-docs/demo-mode/general-info)).
pub const DEMO_WS_API_URL: &str = "wss://demo-ws-api.binance.com/ws-api/v3";
/// Demo market streams base (map from production `wss://stream.binance.com/ws`).
pub const DEMO_MARKET_STREAM_URL: &str = "wss://demo-stream.binance.com/ws";

/// API credentials for signed user-data stream subscription (WebSocket API only).
#[derive(Clone, Debug)]
pub struct ApiCredentials {
    pub api_key: String,
    pub secret_key: String,
}

/// Binance spot user-data stream endpoints (WebSocket API, no REST trading).
#[derive(Clone, Debug)]
pub struct BinanceSpotUserStream {
    pub credentials: ApiCredentials,
    ws_api_url: String,
}

impl BinanceSpotUserStream {
    pub const WS_API_URL: &'static str = "wss://ws-api.binance.com:443/ws-api/v3";

    pub fn new(credentials: ApiCredentials) -> Self {
        Self {
            credentials,
            ws_api_url: Self::WS_API_URL.into(),
        }
    }

    /// User-data subscribe against the spot **demo** WebSocket API host.
    pub fn demo(credentials: ApiCredentials) -> Self {
        Self {
            credentials,
            ws_api_url: DEMO_WS_API_URL.into(),
        }
    }

    pub fn ws_api_url(&self) -> &str {
        &self.ws_api_url
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
        self.ws_api_url.clone()
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
