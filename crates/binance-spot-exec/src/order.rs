//! Signed REST order placement (`POST /api/v3/order`).

use std::collections::BTreeMap;
use std::fmt;

use reqwest::blocking::Client as BlockingClient;
use reqwest::Client as AsyncClient;
use serde::{Deserialize, Serialize};

use crate::auth::{sign_params, signed_params_payload};
use crate::endpoints::ApiCredentials;

pub const ORDER_PATH: &str = "/api/v3/order";
pub const DEFAULT_REST_BASE: &str = "https://api.binance.com";
pub const DEMO_REST_BASE: &str = "https://demo-api.binance.com";

/// Demo REST depth snapshot URL ([Spot Demo general info](https://developers.binance.com/docs/binance-spot-api-docs/demo-mode/general-info)).
pub fn demo_depth_rest_url(symbol: impl AsRef<str>) -> String {
    format!(
        "{}/api/v3/depth?symbol={}&limit=1000",
        DEMO_REST_BASE,
        symbol.as_ref().to_uppercase()
    )
}

/// Order side for spot placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    pub fn parse(value: &str) -> Result<Self, SpotOrderError> {
        match value.to_ascii_uppercase().as_str() {
            "BUY" => Ok(Self::Buy),
            "SELL" => Ok(Self::Sell),
            other => Err(SpotOrderError::InvalidSide(other.into())),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }
}

/// Supported spot order types for outbound placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    Market,
    Limit,
}

impl OrderType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Market => "MARKET",
            Self::Limit => "LIMIT",
        }
    }
}

/// Time-in-force for limit orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeInForce {
    Gtc,
    Ioc,
    Fok,
}

impl TimeInForce {
    pub fn parse(value: &str) -> Result<Self, SpotOrderError> {
        match value.to_ascii_uppercase().as_str() {
            "GTC" => Ok(Self::Gtc),
            "IOC" => Ok(Self::Ioc),
            "FOK" => Ok(Self::Fok),
            other => Err(SpotOrderError::InvalidTimeInForce(other.into())),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Gtc => "GTC",
            Self::Ioc => "IOC",
            Self::Fok => "FOK",
        }
    }
}

/// Normalized new-order request (market or limit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOrderRequest {
    pub symbol: String,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub quantity: String,
    pub price: Option<String>,
    pub time_in_force: Option<TimeInForce>,
}

impl NewOrderRequest {
    pub fn market(symbol: impl Into<String>, side: OrderSide, quantity: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            side,
            order_type: OrderType::Market,
            quantity: quantity.into(),
            price: None,
            time_in_force: None,
        }
    }

    pub fn limit(
        symbol: impl Into<String>,
        side: OrderSide,
        quantity: impl Into<String>,
        price: impl Into<String>,
        time_in_force: TimeInForce,
    ) -> Self {
        Self {
            symbol: symbol.into(),
            side,
            order_type: OrderType::Limit,
            quantity: quantity.into(),
            price: Some(price.into()),
            time_in_force: Some(time_in_force),
        }
    }

    pub fn from_strategy_order(
        symbol: impl Into<String>,
        side: &str,
        quantity: impl Into<String>,
        price: Option<String>,
    ) -> Result<Self, SpotOrderError> {
        let side = OrderSide::parse(side)?;
        let quantity = quantity.into();
        let symbol = symbol.into();
        match price {
            Some(price) => Ok(Self::limit(
                symbol,
                side,
                quantity,
                price,
                TimeInForce::Gtc,
            )),
            None => Ok(Self::market(symbol, side, quantity)),
        }
    }

    pub fn to_unsigned_params(&self) -> Result<BTreeMap<String, String>, SpotOrderError> {
        let mut params = BTreeMap::new();
        params.insert("symbol".into(), self.symbol.to_ascii_uppercase());
        params.insert("side".into(), self.side.as_str().into());
        params.insert("type".into(), self.order_type.as_str().into());
        params.insert("quantity".into(), self.quantity.clone());

        match self.order_type {
            OrderType::Market => {}
            OrderType::Limit => {
                let price = self
                    .price
                    .as_ref()
                    .ok_or(SpotOrderError::MissingLimitPrice)?;
                let tif = self
                    .time_in_force
                    .ok_or(SpotOrderError::MissingTimeInForce)?;
                params.insert("price".into(), price.clone());
                params.insert("timeInForce".into(), tif.as_str().into());
            }
        }

        Ok(params)
    }

    pub fn to_signed_params(&self, secret_key: &str) -> Result<BTreeMap<String, String>, SpotOrderError> {
        Ok(sign_params(secret_key, self.to_unsigned_params()?))
    }

    pub fn signed_query_string(&self, secret_key: &str) -> Result<String, SpotOrderError> {
        Ok(signed_params_payload(&self.to_signed_params(secret_key)?))
    }
}

/// Acknowledgement from `POST /api/v3/order`. Fills reconcile via user-data `executionReport`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewOrderResponse {
    pub symbol: String,
    pub order_id: u64,
    pub client_order_id: String,
    pub transact_time: u64,
    pub price: String,
    pub orig_qty: String,
    pub executed_qty: String,
    pub status: String,
    #[serde(default)]
    pub order_list_id: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct BinanceApiError {
    code: i64,
    msg: String,
}

/// Errors from outbound order placement.
#[derive(Debug)]
pub enum SpotOrderError {
    InvalidSide(String),
    InvalidTimeInForce(String),
    MissingLimitPrice,
    MissingTimeInForce,
    UnsupportedOutbound(trolly_strategy::OutboundMessage),
    Http(reqwest::Error),
    Api { code: i64, msg: String },
    InvalidResponse(String),
}

impl fmt::Display for SpotOrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSide(value) => write!(f, "invalid order side: {value}"),
            Self::InvalidTimeInForce(value) => write!(f, "invalid time in force: {value}"),
            Self::MissingLimitPrice => write!(f, "limit order missing price"),
            Self::MissingTimeInForce => write!(f, "limit order missing time in force"),
            Self::UnsupportedOutbound(message) => {
                write!(f, "unsupported outbound message for spot order egress: {message:?}")
            }
            Self::Http(err) => write!(f, "http error: {err}"),
            Self::Api { code, msg } => write!(f, "binance api error {code}: {msg}"),
            Self::InvalidResponse(msg) => write!(f, "invalid order response: {msg}"),
        }
    }
}

impl std::error::Error for SpotOrderError {}

impl From<reqwest::Error> for SpotOrderError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

/// Async REST client for signed spot order placement.
#[derive(Clone, Debug)]
pub struct SpotOrderClient {
    credentials: ApiCredentials,
    rest_base: String,
    client: AsyncClient,
}

impl SpotOrderClient {
    pub fn new(credentials: ApiCredentials) -> Self {
        Self::with_rest_base(credentials, DEFAULT_REST_BASE)
    }

    pub fn demo(credentials: ApiCredentials) -> Self {
        Self::with_rest_base(credentials, DEMO_REST_BASE)
    }

    pub fn with_rest_base(credentials: ApiCredentials, rest_base: impl Into<String>) -> Self {
        Self {
            credentials,
            rest_base: rest_base.into().trim_end_matches('/').to_string(),
            client: AsyncClient::new(),
        }
    }

    pub fn rest_base(&self) -> &str {
        &self.rest_base
    }

    pub fn order_url(&self) -> String {
        format!("{}{ORDER_PATH}", self.rest_base)
    }

    pub async fn place_order(&self, request: &NewOrderRequest) -> Result<NewOrderResponse, SpotOrderError> {
        let query = request.signed_query_string(&self.credentials.secret_key)?;
        let response = self
            .client
            .post(self.order_url())
            .header("X-MBX-APIKEY", &self.credentials.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(query)
            .send()
            .await?;

        Self::parse_order_response(response).await
    }

    pub fn place_order_blocking(
        &self,
        request: &NewOrderRequest,
    ) -> Result<NewOrderResponse, SpotOrderError> {
        let query = request.signed_query_string(&self.credentials.secret_key)?;
        let response = BlockingClient::new()
            .post(self.order_url())
            .header("X-MBX-APIKEY", &self.credentials.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(query)
            .send()?;

        Self::parse_order_response_blocking(response)
    }

    async fn parse_order_response(
        response: reqwest::Response,
    ) -> Result<NewOrderResponse, SpotOrderError> {
        let status = response.status();
        let body = response.text().await?;
        Self::decode_order_body(status, &body)
    }

    fn parse_order_response_blocking(
        response: reqwest::blocking::Response,
    ) -> Result<NewOrderResponse, SpotOrderError> {
        let status = response.status();
        let body = response.text()?;
        Self::decode_order_body(status, &body)
    }

    fn decode_order_body(
        status: reqwest::StatusCode,
        body: &str,
    ) -> Result<NewOrderResponse, SpotOrderError> {
        if let Ok(api_err) = serde_json::from_str::<BinanceApiError>(body) {
            return Err(SpotOrderError::Api {
                code: api_err.code,
                msg: api_err.msg,
            });
        }

        if !status.is_success() {
            return Err(SpotOrderError::InvalidResponse(format!(
                "status {status}: {body}"
            )));
        }

        serde_json::from_str(body).map_err(|err| SpotOrderError::InvalidResponse(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_request_params_include_price_and_time_in_force() {
        let request = NewOrderRequest::limit(
            "btcusdt",
            OrderSide::Buy,
            "0.01",
            "100.0",
            TimeInForce::Gtc,
        );
        let params = request.to_unsigned_params().unwrap();
        assert_eq!(params["symbol"], "BTCUSDT");
        assert_eq!(params["side"], "BUY");
        assert_eq!(params["type"], "LIMIT");
        assert_eq!(params["quantity"], "0.01");
        assert_eq!(params["price"], "100.0");
        assert_eq!(params["timeInForce"], "GTC");
    }

    #[test]
    fn market_request_params_omit_price_and_time_in_force() {
        let request = NewOrderRequest::market("ETHUSDT", OrderSide::Sell, "1");
        let params = request.to_unsigned_params().unwrap();
        assert_eq!(params["type"], "MARKET");
        assert!(!params.contains_key("price"));
        assert!(!params.contains_key("timeInForce"));
    }

    #[test]
    fn signed_params_include_timestamp_and_signature() {
        let request = NewOrderRequest::market("BTCUSDT", OrderSide::Buy, "0.01");
        let params = request
            .to_signed_params("test-secret")
            .expect("signed params");
        assert!(params.contains_key("timestamp"));
        assert!(params.contains_key("signature"));
        assert_eq!(params["signature"].len(), 64);
    }

    #[test]
    fn strategy_order_maps_price_to_limit_and_absence_to_market() {
        let limit = NewOrderRequest::from_strategy_order("BTCUSDT", "BUY", "1", Some("10".into()))
            .unwrap();
        assert_eq!(limit.order_type, OrderType::Limit);

        let market =
            NewOrderRequest::from_strategy_order("BTCUSDT", "SELL", "1", None).unwrap();
        assert_eq!(market.order_type, OrderType::Market);
    }
}
