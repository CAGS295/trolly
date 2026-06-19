use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::auth::{sign_rest_params, signed_params_payload};
use crate::endpoints::ApiCredentials;

/// Order side for Binance spot REST `POST /api/v3/order`.
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

    pub fn parse(value: &str) -> Result<Self, OrderBuildError> {
        match value.to_ascii_uppercase().as_str() {
            "BUY" => Ok(Self::Buy),
            "SELL" => Ok(Self::Sell),
            other => Err(OrderBuildError::InvalidSide(other.to_string())),
        }
    }
}

/// Supported spot order types for outbound placement.
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

/// Time-in-force for limit orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TimeInForce {
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

    pub fn parse(value: &str) -> Result<Self, OrderBuildError> {
        match value.to_ascii_uppercase().as_str() {
            "GTC" => Ok(Self::Gtc),
            "IOC" => Ok(Self::Ioc),
            "FOK" => Ok(Self::Fok),
            other => Err(OrderBuildError::InvalidTimeInForce(other.to_string())),
        }
    }
}

/// Signed outbound new-order request for `POST /api/v3/order`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOrderRequest {
    pub symbol: String,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub quantity: String,
    pub price: Option<String>,
    pub time_in_force: Option<TimeInForce>,
    pub new_client_order_id: Option<String>,
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
            new_client_order_id: None,
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
            new_client_order_id: None,
        }
    }

    pub fn with_client_order_id(mut self, client_order_id: impl Into<String>) -> Self {
        self.new_client_order_id = Some(client_order_id.into());
        self
    }

    /// Build unsigned REST params (timestamp + signature added by [`sign_rest_params`]).
    pub fn unsigned_params(&self) -> Result<BTreeMap<String, String>, OrderBuildError> {
        let mut params = BTreeMap::new();
        params.insert("symbol".into(), self.symbol.to_uppercase());
        params.insert("side".into(), self.side.as_str().into());
        params.insert("type".into(), self.order_type.as_str().into());
        params.insert("quantity".into(), self.quantity.clone());

        match self.order_type {
            OrderType::Market => {
                if self.price.is_some() || self.time_in_force.is_some() {
                    return Err(OrderBuildError::MarketOrderExtraFields);
                }
            }
            OrderType::Limit => {
                let price = self
                    .price
                    .as_ref()
                    .ok_or(OrderBuildError::LimitMissingPrice)?;
                let tif = self
                    .time_in_force
                    .ok_or(OrderBuildError::LimitMissingTimeInForce)?;
                params.insert("price".into(), price.clone());
                params.insert("timeInForce".into(), tif.as_str().into());
            }
        }

        if let Some(client_id) = &self.new_client_order_id {
            params.insert("newClientOrderId".into(), client_id.clone());
        }

        Ok(params)
    }

    /// Build signed query/form body for `POST /api/v3/order`.
    pub fn signed_form_body(&self, secret_key: &str) -> Result<String, OrderBuildError> {
        let params = sign_rest_params(self.unsigned_params()?, secret_key);
        Ok(signed_params_payload(&params))
    }
}

/// Binance REST new-order acknowledgement (fill/reject reconciliation stays on user-data stream).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewOrderResponse {
    pub symbol: String,
    pub order_id: i64,
    pub client_order_id: String,
    pub transact_time: i64,
    pub price: String,
    pub orig_qty: String,
    pub executed_qty: String,
    pub status: String,
    #[serde(default)]
    pub time_in_force: Option<String>,
    #[serde(rename = "type")]
    pub order_type: String,
    pub side: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderBuildError {
    InvalidSide(String),
    InvalidTimeInForce(String),
    MarketOrderExtraFields,
    LimitMissingPrice,
    LimitMissingTimeInForce,
}

impl fmt::Display for OrderBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSide(value) => write!(f, "invalid order side: {value}"),
            Self::InvalidTimeInForce(value) => write!(f, "invalid time in force: {value}"),
            Self::MarketOrderExtraFields => {
                write!(f, "market orders must not include price or timeInForce")
            }
            Self::LimitMissingPrice => write!(f, "limit orders require price"),
            Self::LimitMissingTimeInForce => write!(f, "limit orders require timeInForce"),
        }
    }
}

impl std::error::Error for OrderBuildError {}

/// Convert a normalized strategy [`trolly_strategy::OutboundMessage::OrderRequest`] into a spot order.
pub fn new_order_from_outbound(
    symbol: &str,
    side: &str,
    qty: &str,
    price: Option<&str>,
    time_in_force: Option<&str>,
) -> Result<NewOrderRequest, OrderBuildError> {
    let side = OrderSide::parse(side)?;
    match price {
        Some(price) => {
            let tif = time_in_force
                .map(TimeInForce::parse)
                .transpose()?
                .unwrap_or(TimeInForce::Gtc);
            Ok(NewOrderRequest::limit(symbol, side, qty, price, tif))
        }
        None => {
            if time_in_force.is_some() {
                return Err(OrderBuildError::MarketOrderExtraFields);
            }
            Ok(NewOrderRequest::market(symbol, side, qty))
        }
    }
}

/// Convenience helper using [`ApiCredentials`] only for signing (no network I/O).
pub fn signed_order_form_body(
    request: &NewOrderRequest,
    credentials: &ApiCredentials,
) -> Result<String, OrderBuildError> {
    request.signed_form_body(&credentials.secret_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_order_signed_body_contains_required_fields() {
        let request = NewOrderRequest::market("btcusdt", OrderSide::Buy, "0.01");
        let body = request.signed_form_body("test-secret").unwrap();
        assert!(body.contains("symbol=BTCUSDT"));
        assert!(body.contains("side=BUY"));
        assert!(body.contains("type=MARKET"));
        assert!(body.contains("quantity=0.01"));
        assert!(body.contains("timestamp="));
        assert!(body.contains("signature="));
    }

    #[test]
    fn limit_order_includes_price_and_time_in_force() {
        let request = NewOrderRequest::limit(
            "ETHUSDT",
            OrderSide::Sell,
            "1.5",
            "2500.00",
            TimeInForce::Ioc,
        );
        let body = request.signed_form_body("secret").unwrap();
        assert!(body.contains("symbol=ETHUSDT"));
        assert!(body.contains("side=SELL"));
        assert!(body.contains("type=LIMIT"));
        assert!(body.contains("quantity=1.5"));
        assert!(body.contains("price=2500.00"));
        assert!(body.contains("timeInForce=IOC"));
    }

    #[test]
    fn new_order_from_outbound_limit_defaults_gtc() {
        let request = new_order_from_outbound("BTCUSDT", "BUY", "1", Some("100"), None).unwrap();
        assert_eq!(request.order_type, OrderType::Limit);
        assert_eq!(request.time_in_force, Some(TimeInForce::Gtc));
    }

    #[test]
    fn new_order_from_outbound_market() {
        let request = new_order_from_outbound("BTCUSDT", "SELL", "0.5", None, None).unwrap();
        assert_eq!(request.order_type, OrderType::Market);
        assert!(request.price.is_none());
    }
}
