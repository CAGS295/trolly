use serde_json::json;
use trolly_stream::VenueEndpoints;

use crate::auth::build_subscribe_signature_params;

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
}

impl BinanceSpotUserStream {
    pub const WS_API_URL: &'static str = "wss://ws-api.binance.com:443/ws-api/v3";

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
        crate::order::DEFAULT_REST_BASE_URL.into()
    }
}
