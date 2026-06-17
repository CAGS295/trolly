use serde_json::json;
use trolly_stream::VenueEndpoints;

use crate::auth::build_subscribe_signature_params;

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
    /// Override WebSocket API host (defaults to [`Self::WS_API_URL`]).
    ws_api_url: Option<String>,
}

impl BinanceSpotUserStream {
    pub const WS_API_URL: &'static str = "wss://ws-api.binance.com:443/ws-api/v3";
    /// Demo REST base (`https://demo-api.binance.com`).
    pub const DEMO_REST_BASE: &'static str = "https://demo-api.binance.com";
    /// Demo WebSocket API (`wss://demo-ws-api.binance.com/ws-api/v3`).
    pub const DEMO_WS_API_URL: &'static str = "wss://demo-ws-api.binance.com/ws-api/v3";
    /// Demo combined market stream base (`wss://demo-stream.binance.com/ws`).
    pub const DEMO_STREAM_WS_BASE: &'static str = "wss://demo-stream.binance.com/ws";

    pub fn new(credentials: ApiCredentials) -> Self {
        Self {
            credentials,
            ws_api_url: None,
        }
    }

    /// Spot demo endpoints (see Binance spot demo general info).
    pub fn demo(credentials: ApiCredentials) -> Self {
        Self {
            credentials,
            ws_api_url: Some(Self::DEMO_WS_API_URL.into()),
        }
    }

    pub fn with_ws_api_url(mut self, url: impl Into<String>) -> Self {
        self.ws_api_url = Some(url.into());
        self
    }

    /// Read-only REST depth snapshot URL on the spot demo host.
    pub fn demo_rest_depth_url(symbol: impl AsRef<str>) -> String {
        format!(
            "{}/api/v3/depth?symbol={}&limit=1000",
            Self::DEMO_REST_BASE,
            symbol.as_ref().to_uppercase()
        )
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
        self.ws_api_url
            .clone()
            .unwrap_or_else(|| Self::WS_API_URL.into())
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
