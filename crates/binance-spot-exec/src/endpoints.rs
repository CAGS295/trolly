use serde_json::json;
use trolly_stream::VenueEndpoints;

use crate::auth::build_subscribe_signature_params;

/// Binance spot demo REST host ([demo general info](https://developers.binance.com/docs/binance-spot-api-docs/demo-mode/general-info)).
pub const DEMO_REST_BASE_URL: &str = "https://demo-api.binance.com";
/// Binance spot demo WebSocket API (signed user-data subscribe).
pub const DEMO_WS_API_URL: &str = "wss://demo-ws-api.binance.com/ws-api/v3";
/// Binance spot demo public market streams base.
pub const DEMO_STREAM_BASE_URL: &str = "wss://demo-stream.binance.com/ws";

/// Demo REST depth snapshot URL for a symbol.
pub fn demo_depth_rest_url(symbol: impl AsRef<str>) -> String {
    format!(
        "{}/api/v3/depth?symbol={}&limit=1000",
        DEMO_REST_BASE_URL.trim_end_matches('/'),
        symbol.as_ref().to_uppercase()
    )
}

/// API credentials for signed user-data stream subscription (WebSocket API only).
#[derive(Clone, Debug)]
pub struct ApiCredentials {
    pub api_key: String,
    pub secret_key: String,
}

/// Binance spot user-data stream endpoints and REST base URL metadata.
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

    /// User-data stream wired to Binance spot **demo** WebSocket API hosts.
    pub fn demo(credentials: ApiCredentials) -> Self {
        Self {
            credentials,
            ws_api_url: DEMO_WS_API_URL.into(),
        }
    }

    pub fn with_ws_api_url(credentials: ApiCredentials, ws_api_url: impl Into<String>) -> Self {
        Self {
            credentials,
            ws_api_url: ws_api_url.into(),
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
        crate::order::DEFAULT_REST_BASE_URL.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trolly_stream::VenueEndpoints;

    #[test]
    fn demo_depth_rest_url_builds_expected_path() {
        assert_eq!(
            demo_depth_rest_url("btcusdt"),
            "https://demo-api.binance.com/api/v3/depth?symbol=BTCUSDT&limit=1000"
        );
    }

    #[test]
    fn demo_user_stream_uses_demo_ws_api_host() {
        let stream = BinanceSpotUserStream::demo(ApiCredentials {
            api_key: "k".into(),
            secret_key: "s".into(),
        });
        assert_eq!(stream.websocket_url(), DEMO_WS_API_URL);
    }
}
