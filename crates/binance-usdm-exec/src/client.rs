use serde::Deserialize;

use crate::endpoints::ApiCredentials;
use crate::order::PlaceOrderRequest;

/// Binance REST `POST /fapi/v1/order` success payload (subset).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceOrderResult {
    pub symbol: String,
    pub order_id: i64,
    pub client_order_id: String,
    pub update_time: u64,
    pub price: String,
    pub orig_qty: String,
    pub executed_qty: String,
    pub status: String,
    pub side: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub position_side: String,
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

/// Signed REST client for USDM order placement.
#[derive(Clone, Debug)]
pub struct UsdmOrderClient {
    pub credentials: ApiCredentials,
    pub base_url: String,
    http: reqwest::Client,
}

impl UsdmOrderClient {
    pub const PRODUCTION_BASE_URL: &'static str = "https://fapi.binance.com";

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
        format!("{}/fapi/v1/order", self.base_url)
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

/// REST client for USDM `listenKey` lifecycle (`POST` / `PUT` / `DELETE /fapi/v1/listenKey`).
#[derive(Clone, Debug)]
pub struct UsdmListenKeyClient {
    pub credentials: ApiCredentials,
    pub base_url: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenKeyError {
    Http(String),
    Api { code: i64, msg: String },
    InvalidResponse(String),
}

impl std::fmt::Display for ListenKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(msg) => write!(f, "http error: {msg}"),
            Self::Api { code, msg } => write!(f, "binance api error {code}: {msg}"),
            Self::InvalidResponse(msg) => write!(f, "invalid response: {msg}"),
        }
    }
}

impl std::error::Error for ListenKeyError {}

#[derive(Debug, Deserialize)]
struct ListenKeyResponse {
    #[serde(rename = "listenKey")]
    listen_key: String,
}

impl UsdmListenKeyClient {
    pub fn new(credentials: ApiCredentials) -> Self {
        Self::with_base_url(credentials, UsdmOrderClient::PRODUCTION_BASE_URL)
    }

    pub fn demo(credentials: ApiCredentials) -> Self {
        Self::with_base_url(credentials, crate::endpoints::UsdmUserDataStream::DEMO_REST_BASE)
    }

    pub fn with_base_url(credentials: ApiCredentials, base_url: impl Into<String>) -> Self {
        Self {
            credentials,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    fn listen_key_url(&self) -> String {
        format!("{}/fapi/v1/listenKey", self.base_url)
    }

    pub async fn create_listen_key(&self) -> Result<String, ListenKeyError> {
        let response = self
            .http
            .post(self.listen_key_url())
            .header("X-MBX-APIKEY", &self.credentials.api_key)
            .send()
            .await
            .map_err(|e| ListenKeyError::Http(e.to_string()))?;
        Self::parse_listen_key_response(response).await
    }

    pub async fn keepalive_listen_key(&self) -> Result<(), ListenKeyError> {
        let response = self
            .http
            .put(self.listen_key_url())
            .header("X-MBX-APIKEY", &self.credentials.api_key)
            .send()
            .await
            .map_err(|e| ListenKeyError::Http(e.to_string()))?;
        Self::parse_empty_ok_response(response).await
    }

    pub async fn close_listen_key(&self) -> Result<(), ListenKeyError> {
        let response = self
            .http
            .delete(self.listen_key_url())
            .header("X-MBX-APIKEY", &self.credentials.api_key)
            .send()
            .await
            .map_err(|e| ListenKeyError::Http(e.to_string()))?;
        Self::parse_empty_ok_response(response).await
    }

    async fn parse_listen_key_response(
        response: reqwest::Response,
    ) -> Result<String, ListenKeyError> {
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| ListenKeyError::Http(e.to_string()))?;

        if !status.is_success() {
            if let Ok(err) = serde_json::from_str::<BinanceApiError>(&body) {
                return Err(ListenKeyError::Api {
                    code: err.code,
                    msg: err.msg,
                });
            }
            return Err(ListenKeyError::Http(format!(
                "status {}: {body}",
                status.as_u16()
            )));
        }

        serde_json::from_str::<ListenKeyResponse>(&body)
            .map(|parsed| parsed.listen_key)
            .map_err(|e| ListenKeyError::InvalidResponse(e.to_string()))
    }

    async fn parse_empty_ok_response(response: reqwest::Response) -> Result<(), ListenKeyError> {
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| ListenKeyError::Http(e.to_string()))?;

        if status.is_success() {
            return Ok(());
        }

        if let Ok(err) = serde_json::from_str::<BinanceApiError>(&body) {
            return Err(ListenKeyError::Api {
                code: err.code,
                msg: err.msg,
            });
        }

        Err(ListenKeyError::Http(format!(
            "status {}: {body}",
            status.as_u16()
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::order::{PositionSide, Side, TimeInForce};

    #[test]
    fn parse_place_order_fixture() {
        let result: PlaceOrderResult = serde_json::from_str(include_str!(
            "../tests/fixtures/place_order_response.json"
        ))
        .unwrap();
        assert_eq!(result.symbol, "BTCUSDT");
        assert_eq!(result.order_id, 22_542_179);
        assert_eq!(result.status, "NEW");
        assert_eq!(result.order_type, "LIMIT");
        assert_eq!(result.position_side, "LONG");
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
            assert!(body.contains("positionSide=LONG"));
            assert!(body.contains("signature="));

            let response = include_str!("../tests/fixtures/place_order_response.json");
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{response}",
                response.len()
            );
            stream.write_all(http.as_bytes()).await.unwrap();
        });

        let client = UsdmOrderClient::with_base_url(
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
        )
        .with_position_side(PositionSide::Long);
        let result = client.place_order(&request).await.unwrap();
        assert_eq!(result.order_id, 22_542_179);

        let raw = captured.lock().unwrap().clone();
        assert!(raw.contains("POST /fapi/v1/order"));
        assert!(raw.to_ascii_lowercase().contains("x-mbx-apikey: test-key"));
    }

    #[tokio::test]
    async fn listen_key_create_keepalive_close_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let method_log = Arc::new(Mutex::new(Vec::new()));
        let method_log_task = Arc::clone(&method_log);

        tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let method = request
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                method_log_task.lock().unwrap().push(method);

                let (status, body) = if request.contains("POST") {
                    (
                        "200 OK",
                        r#"{"listenKey":"demo-listen-key-123"}"#.to_string(),
                    )
                } else {
                    ("200 OK", "{}".to_string())
                };
                let http = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(http.as_bytes()).await.unwrap();
            }
        });

        let client = UsdmListenKeyClient::with_base_url(
            ApiCredentials {
                api_key: "demo-key".into(),
                secret_key: "demo-secret".into(),
            },
            format!("http://{}", addr),
        );

        let key = client.create_listen_key().await.unwrap();
        assert_eq!(key, "demo-listen-key-123");
        client.keepalive_listen_key().await.unwrap();
        client.close_listen_key().await.unwrap();

        let methods = method_log.lock().unwrap().clone();
        assert_eq!(methods, vec!["POST", "PUT", "DELETE"]);
    }

    #[tokio::test]
    async fn place_order_surfaces_api_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let body = r#"{"code":-1111,"msg":"Invalid quantity."}"#;
            let http = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(http.as_bytes()).await.unwrap();
        });

        let client = UsdmOrderClient::with_base_url(
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
                code: -1111,
                msg: "Invalid quantity.".into(),
            }
        );
    }
}
