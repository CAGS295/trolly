use std::collections::BTreeMap;
use std::fmt;

use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::{Deserialize, Serialize};

use crate::auth::{append_signature, current_timestamp_ms, signed_params_payload};
use crate::endpoints::ApiCredentials;

/// Binance spot REST base URL (production).
pub const DEFAULT_REST_BASE_URL: &str = "https://api.binance.com";

/// Order side for `POST /api/v3/order`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }
}

impl fmt::Display for OrderSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Order type for `POST /api/v3/order`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderType {
    Market,
    Limit,
}

impl OrderType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Market => "MARKET",
            Self::Limit => "LIMIT",
        }
    }
}

impl fmt::Display for OrderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Time-in-force for limit orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TimeInForce {
    #[default]
    Gtc,
    Ioc,
    Fok,
}

impl TimeInForce {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gtc => "GTC",
            Self::Ioc => "IOC",
            Self::Fok => "FOK",
        }
    }
}

impl fmt::Display for TimeInForce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Request parameters for a spot order placement call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotOrderRequest {
    pub symbol: String,
    pub side: OrderSide,
    pub quantity: String,
    pub order_type: OrderType,
    pub price: Option<String>,
    pub time_in_force: TimeInForce,
}

impl SpotOrderRequest {
    pub fn market(symbol: impl Into<String>, side: OrderSide, quantity: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into().to_uppercase(),
            side,
            quantity: quantity.into(),
            order_type: OrderType::Market,
            price: None,
            time_in_force: TimeInForce::Gtc,
        }
    }

    pub fn limit(
        symbol: impl Into<String>,
        side: OrderSide,
        quantity: impl Into<String>,
        price: impl Into<String>,
    ) -> Self {
        Self::limit_with_tif(symbol, side, quantity, price, TimeInForce::Gtc)
    }

    pub fn limit_with_tif(
        symbol: impl Into<String>,
        side: OrderSide,
        quantity: impl Into<String>,
        price: impl Into<String>,
        time_in_force: TimeInForce,
    ) -> Self {
        Self {
            symbol: symbol.into().to_uppercase(),
            side,
            quantity: quantity.into(),
            order_type: OrderType::Limit,
            price: Some(price.into()),
            time_in_force,
        }
    }
}

/// Binance REST order placement response (subset of fields used by callers/tests).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotOrderResponse {
    pub symbol: String,
    pub order_id: u64,
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

#[derive(Debug)]
pub enum OrderError {
    Http(reqwest::Error),
    Api { status: u16, body: String },
    InvalidSide(String),
    InvalidLimit(String),
}

impl fmt::Display for OrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(err) => write!(f, "http error: {err}"),
            Self::Api { status, body } => write!(f, "binance api error ({status}): {body}"),
            Self::InvalidSide(side) => write!(f, "invalid order side: {side}"),
            Self::InvalidLimit(msg) => write!(f, "invalid limit order: {msg}"),
        }
    }
}

impl std::error::Error for OrderError {}

impl From<reqwest::Error> for OrderError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

pub fn parse_order_side(side: &str) -> Result<OrderSide, OrderError> {
    match side.to_ascii_uppercase().as_str() {
        "BUY" => Ok(OrderSide::Buy),
        "SELL" => Ok(OrderSide::Sell),
        other => Err(OrderError::InvalidSide(other.into())),
    }
}

pub fn build_signed_order_form(
    request: &SpotOrderRequest,
    credentials: &ApiCredentials,
    timestamp_ms: u64,
) -> Result<BTreeMap<String, String>, OrderError> {
    if request.order_type == OrderType::Limit && request.price.is_none() {
        return Err(OrderError::InvalidLimit(
            "limit orders require price".into(),
        ));
    }

    let mut params = BTreeMap::new();
    params.insert("symbol".into(), request.symbol.clone());
    params.insert("side".into(), request.side.to_string());
    params.insert("type".into(), request.order_type.to_string());
    params.insert("quantity".into(), request.quantity.clone());
    if let Some(price) = &request.price {
        params.insert("price".into(), price.clone());
        params.insert("timeInForce".into(), request.time_in_force.to_string());
    }
    params.insert("timestamp".into(), timestamp_ms.to_string());
    append_signature(&credentials.secret_key, &mut params);
    Ok(params)
}

pub fn signed_order_form_body(
    request: &SpotOrderRequest,
    credentials: &ApiCredentials,
    timestamp_ms: u64,
) -> Result<String, OrderError> {
    Ok(signed_params_payload(&build_signed_order_form(
        request,
        credentials,
        timestamp_ms,
    )?))
}

/// Signed REST client for `POST /api/v3/order`.
#[derive(Clone, Debug)]
pub struct SpotOrderClient {
    pub base_url: String,
    pub credentials: ApiCredentials,
    http: reqwest::Client,
}

impl SpotOrderClient {
    pub fn new(base_url: impl Into<String>, credentials: ApiCredentials) -> Self {
        Self {
            base_url: base_url.into(),
            credentials,
            http: reqwest::Client::new(),
        }
    }

    pub fn with_http_client(
        base_url: impl Into<String>,
        credentials: ApiCredentials,
        http: reqwest::Client,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            credentials,
            http,
        }
    }

    pub fn order_url(&self) -> String {
        format!("{}/api/v3/order", self.base_url.trim_end_matches('/'))
    }

    pub async fn place_order(
        &self,
        request: SpotOrderRequest,
    ) -> Result<SpotOrderResponse, OrderError> {
        let timestamp_ms = current_timestamp_ms();
        let body = signed_order_form_body(&request, &self.credentials, timestamp_ms)?;
        let headers = order_headers(&self.credentials.api_key)?;

        let response = self
            .http
            .post(self.order_url())
            .headers(headers)
            .body(body)
            .send()
            .await?;

        parse_order_response(response.status().as_u16(), response.text().await?)
    }
}

fn order_headers(api_key: &str) -> Result<HeaderMap, OrderError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "X-MBX-APIKEY",
        HeaderValue::from_str(api_key)
            .map_err(|_| OrderError::InvalidLimit("invalid api key header".into()))?,
    );
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    Ok(headers)
}

fn parse_order_response(status: u16, text: String) -> Result<SpotOrderResponse, OrderError> {
    if status >= 400 {
        return Err(OrderError::Api { status, body: text });
    }

    serde_json::from_str(&text).map_err(|err| OrderError::Api {
        status,
        body: format!("invalid json: {err}; body={text}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_order_form_includes_price_and_time_in_force() {
        let request = SpotOrderRequest::limit("btcusdt", OrderSide::Buy, "0.01", "100");
        let creds = ApiCredentials {
            api_key: "key".into(),
            secret_key: "secret".into(),
        };
        let params = build_signed_order_form(&request, &creds, 1_747_385_641_636).unwrap();
        assert_eq!(params.get("symbol"), Some(&"BTCUSDT".to_string()));
        assert_eq!(params.get("side"), Some(&"BUY".to_string()));
        assert_eq!(params.get("type"), Some(&"LIMIT".to_string()));
        assert_eq!(params.get("quantity"), Some(&"0.01".to_string()));
        assert_eq!(params.get("price"), Some(&"100".to_string()));
        assert_eq!(params.get("timeInForce"), Some(&"GTC".to_string()));
        assert_eq!(params.get("timestamp"), Some(&"1747385641636".to_string()));
        assert!(params.contains_key("signature"));
    }

    #[test]
    fn market_order_form_omits_price_and_time_in_force() {
        let request = SpotOrderRequest::market("ETHUSDT", OrderSide::Sell, "1.5");
        let creds = ApiCredentials {
            api_key: "key".into(),
            secret_key: "secret".into(),
        };
        let params = build_signed_order_form(&request, &creds, 42).unwrap();
        assert_eq!(params.get("type"), Some(&"MARKET".to_string()));
        assert!(!params.contains_key("price"));
        assert!(!params.contains_key("timeInForce"));
    }

    #[test]
    fn signature_matches_hmac_payload() {
        let request = SpotOrderRequest::limit("BTCUSDT", OrderSide::Buy, "0.01", "100");
        let creds = ApiCredentials {
            api_key: "test-key".into(),
            secret_key: "test-secret".into(),
        };
        let params = build_signed_order_form(&request, &creds, 1_000).unwrap();
        let expected = crate::auth::sign_hmac_sha256_hex(
            "test-secret",
            "price=100&quantity=0.01&side=BUY&symbol=BTCUSDT&timeInForce=GTC&timestamp=1000&type=LIMIT",
        );
        assert_eq!(params.get("signature"), Some(&expected));
    }
}
