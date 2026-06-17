use serde::Deserialize;

use crate::endpoints::ApiCredentials;
use crate::order::PlaceOrderRequest;

/// Binance REST `POST /api/v3/order` success payload (subset).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceOrderResult {
    pub symbol: String,
    pub order_id: i64,
    pub client_order_id: String,
    pub transact_time: u64,
    pub price: String,
    pub orig_qty: String,
    pub executed_qty: String,
    pub status: String,
    pub side: String,
    #[serde(rename = "type")]
    pub order_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceOrderError {
    Http(String),
    Api { code: i64, msg: String },
    InvalidResponse(String),
}

impl std::fmt::Display for PlaceOrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(msg) => write!(f, "http error: {msg}"),
            Self::Api { code, msg } => write!(f, "binance api error {code}: {msg}"),
            Self::InvalidResponse(msg) => write!(f, "invalid response: {msg}"),
        }
    }
}

impl std::error::Error for PlaceOrderError {}

/// Signed REST client for spot order placement.
#[derive(Clone, Debug)]
pub struct SpotOrderClient {
    pub credentials: ApiCredentials,
    pub base_url: String,
    http: reqwest::Client,
}

impl SpotOrderClient {
    pub const PRODUCTION_BASE_URL: &'static str = "https://api.binance.com";

    pub fn new(credentials: ApiCredentials) -> Self {
        Self::with_base_url(credentials, Self::PRODUCTION_BASE_URL)
    }

    pub fn with_base_url(credentials: ApiCredentials, base_url: impl Into<String>) -> Self {
        Self {
            credentials,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub fn place_order_url(&self) -> String {
        format!("{}/api/v3/order", self.base_url)
    }

    pub async fn place_order(
        &self,
        request: &PlaceOrderRequest,
    ) -> Result<PlaceOrderResult, PlaceOrderError> {
        let query = request.signed_query(&self.credentials);
        let response = self
            .http
            .post(self.place_order_url())
            .header("X-MBX-APIKEY", &self.credentials.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(query)
            .send()
            .await
            .map_err(|e| PlaceOrderError::Http(e.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| PlaceOrderError::Http(e.to_string()))?;

        if !status.is_success() {
            if let Ok(err) = serde_json::from_str::<BinanceApiError>(&body) {
                return Err(PlaceOrderError::Api {
                    code: err.code,
                    msg: err.msg,
                });
            }
            return Err(PlaceOrderError::Http(format!(
                "status {}: {body}",
                status.as_u16()
            )));
        }

        serde_json::from_str(&body).map_err(|e| PlaceOrderError::InvalidResponse(e.to_string()))
    }
}

#[derive(Debug, Deserialize)]
struct BinanceApiError {
    code: i64,
    msg: String,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::order::{Side, TimeInForce};

    #[test]
    fn parse_place_order_fixture() {
        let result: PlaceOrderResult = serde_json::from_str(include_str!(
            "../tests/fixtures/place_order_response.json"
        ))
        .unwrap();
        assert_eq!(result.symbol, "BTCUSDT");
        assert_eq!(result.order_id, 28_457_393);
        assert_eq!(result.status, "NEW");
        assert_eq!(result.order_type, "LIMIT");
    }

    #[tokio::test]
    async fn place_order_posts_signed_body_to_rest_endpoint() {
        let captured = Arc::new(Mutex::new(String::new()));
        let captured_task = Arc::clone(&captured);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            *captured_task.lock().unwrap() = request.clone();

            let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
            assert!(body.contains("symbol=BTCUSDT"));
            assert!(body.contains("side=BUY"));
            assert!(body.contains("type=LIMIT"));
            assert!(body.contains("signature="));

            let response = include_str!("../tests/fixtures/place_order_response.json");
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{response}",
                response.len()
            );
            stream.write_all(http.as_bytes()).await.unwrap();
        });

        let client = SpotOrderClient::with_base_url(
            ApiCredentials {
                api_key: "test-key".into(),
                secret_key: "test-secret".into(),
            },
            format!("http://{}", addr),
        );

        let request = PlaceOrderRequest::limit(
            "BTCUSDT",
            Side::Buy,
            "0.01",
            "100",
            TimeInForce::Gtc,
        );
        let result = client.place_order(&request).await.unwrap();
        assert_eq!(result.order_id, 28_457_393);

        let raw = captured.lock().unwrap().clone();
        assert!(raw.contains("POST /api/v3/order"));
        assert!(raw.to_ascii_lowercase().contains("x-mbx-apikey: test-key"));
    }

    #[tokio::test]
    async fn place_order_surfaces_api_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let body = r#"{"code":-1013,"msg":"Invalid quantity."}"#;
            let http = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(http.as_bytes()).await.unwrap();
        });

        let client = SpotOrderClient::with_base_url(
            ApiCredentials {
                api_key: "k".into(),
                secret_key: "s".into(),
            },
            format!("http://{}", addr),
        );
        let request = PlaceOrderRequest::market("BTCUSDT", Side::Buy, "0");
        let err = client.place_order(&request).await.unwrap_err();
        assert_eq!(
            err,
            PlaceOrderError::Api {
                code: -1013,
                msg: "Invalid quantity.".into(),
            }
        );
    }
}
