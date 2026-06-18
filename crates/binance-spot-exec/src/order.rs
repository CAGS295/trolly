//! Signed spot order placement via Binance REST `POST /api/v3/order`.
//!
//! Order fills and rejects are reconciled by the existing `executionReport`
//! user-data stream path — this module only submits the order.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::auth::{current_timestamp_ms, sign_hmac_sha256_hex, signed_params_payload};
use crate::endpoints::ApiCredentials;

/// Base URL for the Binance production REST API.
pub const REST_BASE_URL: &str = "https://api.binance.com";

/// Order side.
#[derive(Debug, Clone, PartialEq)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl fmt::Display for OrderSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderSide::Buy => write!(f, "BUY"),
            OrderSide::Sell => write!(f, "SELL"),
        }
    }
}

impl FromStr for OrderSide {
    type Err = OrderError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "BUY" => Ok(OrderSide::Buy),
            "SELL" => Ok(OrderSide::Sell),
            _ => Err(OrderError::InvalidSide(s.to_string())),
        }
    }
}

/// Order type.
#[derive(Debug, Clone, PartialEq)]
pub enum OrderType {
    Market,
    Limit,
}

impl fmt::Display for OrderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderType::Market => write!(f, "MARKET"),
            OrderType::Limit => write!(f, "LIMIT"),
        }
    }
}

/// Time-in-force for LIMIT orders.
#[derive(Debug, Clone, PartialEq)]
pub enum TimeInForce {
    GoodTillCancel,
    ImmediateOrCancel,
    FillOrKill,
}

impl fmt::Display for TimeInForce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimeInForce::GoodTillCancel => write!(f, "GTC"),
            TimeInForce::ImmediateOrCancel => write!(f, "IOC"),
            TimeInForce::FillOrKill => write!(f, "FOK"),
        }
    }
}

/// Builder for a Binance spot order.
#[derive(Debug, Clone)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub quantity: String,
    /// Required for LIMIT orders.
    pub price: Option<String>,
    /// Defaults to GTC for LIMIT orders.
    pub time_in_force: Option<TimeInForce>,
}

impl OrderRequest {
    /// Create a MARKET order.
    pub fn market(
        symbol: impl Into<String>,
        side: OrderSide,
        quantity: impl Into<String>,
    ) -> Self {
        Self {
            symbol: symbol.into(),
            side,
            order_type: OrderType::Market,
            quantity: quantity.into(),
            price: None,
            time_in_force: None,
        }
    }

    /// Create a LIMIT GTC order.
    pub fn limit(
        symbol: impl Into<String>,
        side: OrderSide,
        quantity: impl Into<String>,
        price: impl Into<String>,
    ) -> Self {
        Self {
            symbol: symbol.into(),
            side,
            order_type: OrderType::Limit,
            quantity: quantity.into(),
            price: Some(price.into()),
            time_in_force: Some(TimeInForce::GoodTillCancel),
        }
    }

    /// Override time-in-force (useful to switch to IOC/FOK).
    pub fn with_time_in_force(mut self, tif: TimeInForce) -> Self {
        self.time_in_force = Some(tif);
        self
    }

    /// Build a signed `BTreeMap` of query parameters ready for Binance REST.
    ///
    /// `timestamp_ms` is injected so callers can supply a fixed value in tests.
    pub fn to_signed_params(
        &self,
        credentials: &ApiCredentials,
        timestamp_ms: u64,
    ) -> BTreeMap<String, String> {
        let mut params = BTreeMap::new();
        params.insert("quantity".into(), self.quantity.clone());
        params.insert("side".into(), self.side.to_string());
        params.insert("symbol".into(), self.symbol.clone());
        params.insert("timestamp".into(), timestamp_ms.to_string());
        params.insert("type".into(), self.order_type.to_string());
        if let Some(price) = &self.price {
            params.insert("price".into(), price.clone());
        }
        if let Some(tif) = &self.time_in_force {
            params.insert("timeInForce".into(), tif.to_string());
        }
        let payload = signed_params_payload(&params);
        let signature = sign_hmac_sha256_hex(&credentials.secret_key, &payload);
        params.insert("signature".into(), signature);
        params
    }
}

/// ACK response from Binance `POST /api/v3/order`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderAck {
    pub symbol: String,
    #[serde(rename = "orderId")]
    pub order_id: i64,
    pub status: String,
    #[serde(rename = "clientOrderId")]
    pub client_order_id: String,
}

/// Errors from order placement.
#[derive(Debug)]
pub enum OrderError {
    /// HTTP transport or JSON decode error.
    Http(String),
    /// Binance rejected the order (negative error code in response body).
    Api { code: i64, msg: String },
    /// Unrecognised order side string.
    InvalidSide(String),
}

impl fmt::Display for OrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderError::Http(e) => write!(f, "HTTP error: {e}"),
            OrderError::Api { code, msg } => write!(f, "Binance API error {code}: {msg}"),
            OrderError::InvalidSide(s) => write!(f, "invalid order side: {s}"),
        }
    }
}

impl std::error::Error for OrderError {}

/// Abstraction over the HTTP transport for order placement (enables test mocking).
pub trait OrderClient {
    fn post_order(
        &self,
        params: BTreeMap<String, String>,
        api_key: &str,
    ) -> impl std::future::Future<Output = Result<OrderAck, OrderError>> + Send;
}

/// Submit a signed order via `POST /api/v3/order`.
///
/// The returned [`OrderAck`] is the exchange acknowledgement.  
/// Subsequent fills and rejects arrive via the `executionReport` user-data stream
/// and are handled by the existing [`crate::handler::SpotExecHandler`].
pub async fn place_order<C: OrderClient>(
    client: &C,
    credentials: &ApiCredentials,
    request: OrderRequest,
) -> Result<OrderAck, OrderError> {
    let timestamp_ms = current_timestamp_ms();
    let params = request.to_signed_params(credentials, timestamp_ms);
    client.post_order(params, &credentials.api_key).await
}

// ----- Live HTTP client -----

/// [`OrderClient`] backed by `reqwest` that calls the live Binance REST endpoint.
pub struct HttpOrderClient {
    inner: reqwest::Client,
    base_url: String,
}

impl HttpOrderClient {
    /// Target the production Binance REST endpoint.
    pub fn new() -> Self {
        Self::with_base_url(REST_BASE_URL)
    }

    /// Target an arbitrary base URL (useful for integration tests pointing at a local mock server).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            inner: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }
}

impl Default for HttpOrderClient {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderClient for HttpOrderClient {
    async fn post_order(
        &self,
        params: BTreeMap<String, String>,
        api_key: &str,
    ) -> Result<OrderAck, OrderError> {
        let url = format!("{}/api/v3/order", self.base_url);
        let response = self
            .inner
            .post(&url)
            .header("X-MBX-APIKEY", api_key)
            .form(&params)
            .send()
            .await
            .map_err(|e| OrderError::Http(e.to_string()))?;

        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|e| OrderError::Http(e.to_string()))?;

        if let Some(code) = value.get("code").and_then(|c| c.as_i64()) {
            if code < 0 {
                let msg = value
                    .get("msg")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string();
                return Err(OrderError::Api { code, msg });
            }
        }

        serde_json::from_value(value).map_err(|e| OrderError::Http(e.to_string()))
    }
}

// ----- Test mock client -----

/// [`OrderClient`] that returns a pre-set JSON response without making network calls.
///
/// Intended for unit tests only.
#[cfg(test)]
pub(crate) struct MockOrderClient {
    pub response: serde_json::Value,
}

#[cfg(test)]
impl OrderClient for MockOrderClient {
    async fn post_order(
        &self,
        _params: BTreeMap<String, String>,
        _api_key: &str,
    ) -> Result<OrderAck, OrderError> {
        if let Some(code) = self.response.get("code").and_then(|c| c.as_i64()) {
            if code < 0 {
                let msg = self
                    .response
                    .get("msg")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string();
                return Err(OrderError::Api { code, msg });
            }
        }
        serde_json::from_value(self.response.clone())
            .map_err(|e| OrderError::Http(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_credentials() -> ApiCredentials {
        ApiCredentials {
            api_key: "test-api-key".into(),
            secret_key: "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j".into(),
        }
    }

    #[test]
    fn market_order_params_include_required_fields() {
        let credentials = test_credentials();
        let request = OrderRequest::market("BTCUSDT", OrderSide::Buy, "0.001");
        let params = request.to_signed_params(&credentials, 1_000_000);

        assert_eq!(params["symbol"], "BTCUSDT");
        assert_eq!(params["side"], "BUY");
        assert_eq!(params["type"], "MARKET");
        assert_eq!(params["quantity"], "0.001");
        assert_eq!(params["timestamp"], "1000000");
        assert!(params.contains_key("signature"), "signature must be present");
        assert!(!params.contains_key("price"), "market order must not include price");
        assert!(!params.contains_key("timeInForce"), "market order must not include timeInForce");
    }

    #[test]
    fn limit_order_params_include_price_and_tif() {
        let credentials = test_credentials();
        let request = OrderRequest::limit("ETHUSDT", OrderSide::Sell, "1.0", "3000.00");
        let params = request.to_signed_params(&credentials, 1_000_000);

        assert_eq!(params["symbol"], "ETHUSDT");
        assert_eq!(params["side"], "SELL");
        assert_eq!(params["type"], "LIMIT");
        assert_eq!(params["price"], "3000.00");
        assert_eq!(params["timeInForce"], "GTC");
        assert!(params.contains_key("signature"));
    }

    #[test]
    fn signature_matches_hmac_sha256_of_sorted_params() {
        let credentials = test_credentials();
        let request = OrderRequest::market("BTCUSDT", OrderSide::Buy, "0.001");
        let params = request.to_signed_params(&credentials, 1_000_000);

        let signature = params["signature"].clone();
        let mut params_without_sig = params.clone();
        params_without_sig.remove("signature");
        let payload = signed_params_payload(&params_without_sig);
        let expected = sign_hmac_sha256_hex(&credentials.secret_key, &payload);
        assert_eq!(signature, expected);
    }

    #[test]
    fn with_time_in_force_overrides_default_gtc() {
        let request = OrderRequest::limit("BTCUSDT", OrderSide::Buy, "0.001", "50000")
            .with_time_in_force(TimeInForce::ImmediateOrCancel);
        assert_eq!(request.time_in_force, Some(TimeInForce::ImmediateOrCancel));
    }

    #[test]
    fn order_side_from_str_is_case_insensitive() {
        assert_eq!("buy".parse::<OrderSide>().unwrap(), OrderSide::Buy);
        assert_eq!("SELL".parse::<OrderSide>().unwrap(), OrderSide::Sell);
        assert!("invalid".parse::<OrderSide>().is_err());
    }

    #[tokio::test]
    async fn place_order_with_mock_client_success() {
        let credentials = test_credentials();
        let client = MockOrderClient {
            response: serde_json::json!({
                "symbol": "BTCUSDT",
                "orderId": 12345,
                "status": "NEW",
                "clientOrderId": "abc123"
            }),
        };
        let request = OrderRequest::market("BTCUSDT", OrderSide::Buy, "0.001");
        let ack = place_order(&client, &credentials, request).await.unwrap();

        assert_eq!(ack.symbol, "BTCUSDT");
        assert_eq!(ack.order_id, 12345);
        assert_eq!(ack.status, "NEW");
        assert_eq!(ack.client_order_id, "abc123");
    }

    #[tokio::test]
    async fn place_order_with_mock_client_api_reject() {
        let credentials = test_credentials();
        let client = MockOrderClient {
            response: serde_json::json!({
                "code": -1121,
                "msg": "Invalid symbol."
            }),
        };
        let request = OrderRequest::market("INVALID", OrderSide::Buy, "0.001");
        let err = place_order(&client, &credentials, request)
            .await
            .unwrap_err();

        assert!(
            matches!(err, OrderError::Api { code: -1121, .. }),
            "expected Api error -1121, got {err:?}"
        );
    }

    #[tokio::test]
    async fn place_order_limit_with_mock_client() {
        let credentials = test_credentials();
        let client = MockOrderClient {
            response: serde_json::json!({
                "symbol": "ETHUSDT",
                "orderId": 99,
                "status": "NEW",
                "clientOrderId": "limit-001"
            }),
        };
        let request = OrderRequest::limit("ETHUSDT", OrderSide::Sell, "1.0", "3000.00");
        let ack = place_order(&client, &credentials, request).await.unwrap();

        assert_eq!(ack.symbol, "ETHUSDT");
        assert_eq!(ack.order_id, 99);
    }
}
