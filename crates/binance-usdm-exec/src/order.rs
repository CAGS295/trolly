//! Signed REST order placement (`POST /fapi/v1/order`).

use std::collections::BTreeMap;
use std::fmt;

use reqwest::blocking::Client as BlockingClient;
use reqwest::Client as AsyncClient;
use serde::{Deserialize, Serialize};

use crate::auth::{sign_params, signed_params_payload};
use crate::endpoints::ApiCredentials;

pub const ORDER_PATH: &str = "/fapi/v1/order";
pub const DEFAULT_REST_BASE: &str = "https://fapi.binance.com";
pub const DEMO_REST_BASE: &str = "https://demo-fapi.binance.com";

/// Order side for USDM placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    pub fn parse(value: &str) -> Result<Self, UsdmOrderError> {
        match value.to_ascii_uppercase().as_str() {
            "BUY" => Ok(Self::Buy),
            "SELL" => Ok(Self::Sell),
            other => Err(UsdmOrderError::InvalidSide(other.into())),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }
}

/// Supported USDM order types for outbound placement.
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
    Gtx,
}

impl TimeInForce {
    pub fn parse(value: &str) -> Result<Self, UsdmOrderError> {
        match value.to_ascii_uppercase().as_str() {
            "GTC" => Ok(Self::Gtc),
            "IOC" => Ok(Self::Ioc),
            "FOK" => Ok(Self::Fok),
            "GTX" => Ok(Self::Gtx),
            other => Err(UsdmOrderError::InvalidTimeInForce(other.into())),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Gtc => "GTC",
            Self::Ioc => "IOC",
            Self::Fok => "FOK",
            Self::Gtx => "GTX",
        }
    }
}

/// Position side for hedge-mode accounts (`LONG` / `SHORT`) or one-way (`BOTH`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionSide {
    Both,
    Long,
    Short,
}

impl PositionSide {
    pub fn parse(value: &str) -> Result<Self, UsdmOrderError> {
        match value.to_ascii_uppercase().as_str() {
            "BOTH" => Ok(Self::Both),
            "LONG" => Ok(Self::Long),
            "SHORT" => Ok(Self::Short),
            other => Err(UsdmOrderError::InvalidPositionSide(other.into())),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Both => "BOTH",
            Self::Long => "LONG",
            Self::Short => "SHORT",
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
    pub position_side: Option<PositionSide>,
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
            position_side: None,
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
            position_side: None,
        }
    }

    pub fn with_position_side(mut self, position_side: PositionSide) -> Self {
        self.position_side = Some(position_side);
        self
    }

    pub fn from_strategy_order(
        symbol: impl Into<String>,
        side: &str,
        quantity: impl Into<String>,
        price: Option<String>,
        position_side: Option<String>,
    ) -> Result<Self, UsdmOrderError> {
        let side = OrderSide::parse(side)?;
        let quantity = quantity.into();
        let symbol = symbol.into();
        let position_side = position_side
            .map(|value| PositionSide::parse(&value))
            .transpose()?;

        let mut request = match price {
            Some(price) => Self::limit(symbol, side, quantity, price, TimeInForce::Gtc),
            None => Self::market(symbol, side, quantity),
        };
        request.position_side = position_side;
        Ok(request)
    }

    pub fn to_unsigned_params(&self) -> Result<BTreeMap<String, String>, UsdmOrderError> {
        let mut params = BTreeMap::new();
        params.insert("symbol".into(), self.symbol.to_ascii_uppercase());
        params.insert("side".into(), self.side.as_str().into());
        params.insert("type".into(), self.order_type.as_str().into());
        params.insert("quantity".into(), self.quantity.clone());

        if let Some(position_side) = self.position_side {
            params.insert("positionSide".into(), position_side.as_str().into());
        }

        match self.order_type {
            OrderType::Market => {}
            OrderType::Limit => {
                let price = self
                    .price
                    .as_ref()
                    .ok_or(UsdmOrderError::MissingLimitPrice)?;
                let tif = self
                    .time_in_force
                    .ok_or(UsdmOrderError::MissingTimeInForce)?;
                params.insert("price".into(), price.clone());
                params.insert("timeInForce".into(), tif.as_str().into());
            }
        }

        Ok(params)
    }

    pub fn to_signed_params(&self, secret_key: &str) -> Result<BTreeMap<String, String>, UsdmOrderError> {
        Ok(sign_params(secret_key, self.to_unsigned_params()?))
    }

    pub fn signed_query_string(&self, secret_key: &str) -> Result<String, UsdmOrderError> {
        Ok(signed_params_payload(&self.to_signed_params(secret_key)?))
    }
}

/// Acknowledgement from `POST /fapi/v1/order`. Fills reconcile via user-data `ORDER_TRADE_UPDATE`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewOrderResponse {
    pub symbol: String,
    pub order_id: u64,
    pub client_order_id: String,
    pub price: String,
    pub orig_qty: String,
    pub executed_qty: String,
    pub status: String,
    #[serde(default)]
    pub position_side: Option<String>,
    #[serde(default)]
    pub update_time: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct BinanceApiError {
    code: i64,
    msg: String,
}

/// Errors from outbound order placement.
#[derive(Debug)]
pub enum UsdmOrderError {
    InvalidSide(String),
    InvalidTimeInForce(String),
    InvalidPositionSide(String),
    MissingLimitPrice,
    MissingTimeInForce,
    UnsupportedOutbound(trolly_strategy::OutboundMessage),
    Http(reqwest::Error),
    Api { code: i64, msg: String },
    InvalidResponse(String),
}

impl fmt::Display for UsdmOrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSide(value) => write!(f, "invalid order side: {value}"),
            Self::InvalidTimeInForce(value) => write!(f, "invalid time in force: {value}"),
            Self::InvalidPositionSide(value) => write!(f, "invalid position side: {value}"),
            Self::MissingLimitPrice => write!(f, "limit order missing price"),
            Self::MissingTimeInForce => write!(f, "limit order missing time in force"),
            Self::UnsupportedOutbound(message) => {
                write!(f, "unsupported outbound message for usdm order egress: {message:?}")
            }
            Self::Http(err) => write!(f, "http error: {err}"),
            Self::Api { code, msg } => write!(f, "binance api error {code}: {msg}"),
            Self::InvalidResponse(msg) => write!(f, "invalid order response: {msg}"),
        }
    }
}

impl std::error::Error for UsdmOrderError {}

impl From<reqwest::Error> for UsdmOrderError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

/// Async REST client for signed USDM order placement.
#[derive(Clone, Debug)]
pub struct UsdmOrderClient {
    credentials: ApiCredentials,
    rest_base: String,
    client: AsyncClient,
}

impl UsdmOrderClient {
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

    pub async fn place_order(&self, request: &NewOrderRequest) -> Result<NewOrderResponse, UsdmOrderError> {
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
    ) -> Result<NewOrderResponse, UsdmOrderError> {
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
    ) -> Result<NewOrderResponse, UsdmOrderError> {
        let status = response.status();
        let body = response.text().await?;
        Self::decode_order_body(status, &body)
    }

    fn parse_order_response_blocking(
        response: reqwest::blocking::Response,
    ) -> Result<NewOrderResponse, UsdmOrderError> {
        let status = response.status();
        let body = response.text()?;
        Self::decode_order_body(status, &body)
    }

    fn decode_order_body(
        status: reqwest::StatusCode,
        body: &str,
    ) -> Result<NewOrderResponse, UsdmOrderError> {
        if let Ok(api_err) = serde_json::from_str::<BinanceApiError>(body) {
            return Err(UsdmOrderError::Api {
                code: api_err.code,
                msg: api_err.msg,
            });
        }

        if !status.is_success() {
            return Err(UsdmOrderError::InvalidResponse(format!(
                "status {status}: {body}"
            )));
        }

        serde_json::from_str(body).map_err(|err| UsdmOrderError::InvalidResponse(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_request_params_include_price_time_in_force_and_position_side() {
        let request = NewOrderRequest::limit(
            "btcusdt",
            OrderSide::Buy,
            "0.01",
            "100.0",
            TimeInForce::Gtc,
        )
        .with_position_side(PositionSide::Long);
        let params = request.to_unsigned_params().unwrap();
        assert_eq!(params["symbol"], "BTCUSDT");
        assert_eq!(params["side"], "BUY");
        assert_eq!(params["type"], "LIMIT");
        assert_eq!(params["quantity"], "0.01");
        assert_eq!(params["price"], "100.0");
        assert_eq!(params["timeInForce"], "GTC");
        assert_eq!(params["positionSide"], "LONG");
    }

    #[test]
    fn market_request_params_omit_price_and_time_in_force() {
        let request = NewOrderRequest::market("ETHUSDT", OrderSide::Sell, "1");
        let params = request.to_unsigned_params().unwrap();
        assert_eq!(params["type"], "MARKET");
        assert!(!params.contains_key("price"));
        assert!(!params.contains_key("timeInForce"));
        assert!(!params.contains_key("positionSide"));
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
        let limit = NewOrderRequest::from_strategy_order(
            "BTCUSDT",
            "BUY",
            "1",
            Some("10".into()),
            Some("SHORT".into()),
        )
        .unwrap();
        assert_eq!(limit.order_type, OrderType::Limit);
        assert_eq!(limit.position_side, Some(PositionSide::Short));

        let market =
            NewOrderRequest::from_strategy_order("BTCUSDT", "SELL", "1", None, None).unwrap();
        assert_eq!(market.order_type, OrderType::Market);
    }
}
