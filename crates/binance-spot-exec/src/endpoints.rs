use serde_json::json;
use trolly_stream::VenueEndpoints;

use crate::auth::build_subscribe_signature_params;

/// API credentials for signed user-data stream subscription (WebSocket API only).
#[derive(Clone, Debug)]
pub struct ApiCredentials {
    pub api_key: String,
    pub secret_key: String,
}

/// Production spot REST host (`/api/v3/...`).
pub const SPOT_REST_BASE_URL: &str = "https://api.binance.com/api";

/// Binance spot **demo** hosts ([demo mode general info](https://developers.binance.com/docs/binance-spot-api-docs/demo-mode/general-info)).
pub const SPOT_DEMO_REST_BASE_URL: &str = "https://demo-api.binance.com/api";
pub const SPOT_DEMO_WS_API_URL: &str = "wss://demo-ws-api.binance.com/ws-api/v3";
pub const SPOT_DEMO_MARKET_STREAM_URL: &str = "wss://demo-stream.binance.com/ws";

/// REST depth snapshot URL for spot (production or demo base).
pub fn spot_depth_rest_url(base: &str, symbol: impl AsRef<str>, limit: u32) -> String {
    format!(
        "{}/v3/depth?symbol={}&limit={}",
        base.trim_end_matches('/'),
        symbol.as_ref().to_uppercase(),
        limit
    )
}

/// Binance spot user-data stream endpoints (WebSocket API, no REST trading).
#[derive(Clone, Debug)]
pub struct BinanceSpotUserStream {
    pub credentials: ApiCredentials,
    ws_api_url: &'static str,
}

impl BinanceSpotUserStream {
    pub const WS_API_URL: &'static str = "wss://ws-api.binance.com:443/ws-api/v3";

    pub fn new(credentials: ApiCredentials) -> Self {
        Self {
            credentials,
            ws_api_url: Self::WS_API_URL,
        }
    }

    /// Demo/testnet WebSocket API (`wss://demo-ws-api.binance.com/ws-api/v3`).
    pub fn demo(credentials: ApiCredentials) -> Self {
        Self {
            credentials,
            ws_api_url: SPOT_DEMO_WS_API_URL,
        }
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
        self.ws_api_url.into()
    }

    /// User-data is account-wide; `symbols` are ignored for subscribe payloads.
    /// Symbols are used by [`SpotExecHandler`] / multiplexor routing only.
    fn ws_subscriptions(&self, _symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        vec![self.subscribe_request_json()]
    }

    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String {
        if self.ws_api_url == SPOT_DEMO_WS_API_URL {
            spot_depth_rest_url(SPOT_DEMO_REST_BASE_URL, symbol, 100)
        } else {
            spot_depth_rest_url(SPOT_REST_BASE_URL, symbol, 100)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trolly_stream::VenueEndpoints;

    #[test]
    fn demo_user_stream_uses_demo_ws_api_url() {
        let stream = BinanceSpotUserStream::demo(ApiCredentials {
            api_key: "k".into(),
            secret_key: "s".into(),
        });
        assert_eq!(
            stream.websocket_url(),
            "wss://demo-ws-api.binance.com/ws-api/v3"
        );
        assert!(stream
            .rest_api_url("btcusdt")
            .starts_with("https://demo-api.binance.com/api/v3/depth"));
    }

    #[test]
    fn production_user_stream_uses_production_ws_api_url() {
        let stream = BinanceSpotUserStream::new(ApiCredentials {
            api_key: "k".into(),
            secret_key: "s".into(),
        });
        assert_eq!(
            stream.websocket_url(),
            "wss://ws-api.binance.com:443/ws-api/v3"
        );
    }
}
