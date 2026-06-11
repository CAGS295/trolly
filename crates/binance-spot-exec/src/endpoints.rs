use crate::types::SpotExec;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use trolly_stream::Endpoints;

type HmacSha256 = Hmac<Sha256>;

/// Binance spot WebSocket API base URL for user-data subscriptions.
pub const WS_API_STREAM: &str = "wss://ws-api.binance.com:443/ws-api/v3";

/// Credentials used to sign [`userDataStream.subscribe.signature`](https://developers.binance.com/docs/binance-spot-api-docs/websocket-api/user-data-stream-requests).
#[derive(Clone, Debug)]
pub struct BinanceSpotUserStream {
    pub api_key: String,
    pub api_secret: String,
}

impl BinanceSpotUserStream {
    /// Build the signed `userDataStream.subscribe.signature` request payload.
    pub fn subscribe_signature_message(
        &self,
        request_id: impl AsRef<str>,
        timestamp_ms: u64,
    ) -> String {
        let signature = sign_timestamp(&self.api_secret, timestamp_ms);
        serde_json::json!({
            "id": request_id.as_ref(),
            "method": "userDataStream.subscribe.signature",
            "params": {
                "apiKey": self.api_key,
                "timestamp": timestamp_ms,
                "signature": signature,
            }
        })
        .to_string()
    }

    /// Authenticated-session variant when the connection already completed `session.logon`.
    pub fn subscribe_message(request_id: impl AsRef<str>) -> String {
        serde_json::json!({
            "id": request_id.as_ref(),
            "method": "userDataStream.subscribe",
        })
        .to_string()
    }
}

impl Endpoints<SpotExec> for BinanceSpotUserStream {
    fn websocket_url(&self) -> String {
        WS_API_STREAM.to_string()
    }

    /// User-data streams are account-scoped. The `symbols` iterator is ignored for subscription
    /// payloads; pass trading symbols to [`trolly_stream::MonitorMultiplexor::build`] for per-symbol
    /// execution handlers and include [`crate::ACCOUNT_ROUTE_ID`] for balance events.
    fn ws_subscriptions(&self, _symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_millis() as u64;

        vec![self.subscribe_signature_message("binance-spot-user-data", timestamp_ms)]
    }

    fn rest_api_url(&self, _symbol: impl AsRef<str>) -> String {
        String::new()
    }
}

fn sign_timestamp(api_secret: &str, timestamp_ms: u64) -> String {
    let payload = format!("timestamp={timestamp_ms}");
    let mut mac =
        HmacSha256::new_from_slice(api_secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribe_signature_is_deterministic_for_fixture_timestamp() {
        let creds = BinanceSpotUserStream {
            api_key: "test-key".into(),
            api_secret: "test-secret".into(),
        };

        let msg = creds.subscribe_signature_message("req-1", 1_747_385_641_636);
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();

        assert_eq!(parsed["method"], "userDataStream.subscribe.signature");
        assert_eq!(parsed["params"]["apiKey"], "test-key");
        assert_eq!(
            parsed["params"]["timestamp"].as_u64(),
            Some(1_747_385_641_636)
        );
        assert_eq!(
            parsed["params"]["signature"].as_str().unwrap().len(),
            64,
            "HMAC-SHA256 hex"
        );
    }

    #[test]
    fn websocket_url_points_at_ws_api() {
        let creds = BinanceSpotUserStream {
            api_key: String::new(),
            api_secret: String::new(),
        };
        assert_eq!(creds.websocket_url(), WS_API_STREAM);
    }
}
