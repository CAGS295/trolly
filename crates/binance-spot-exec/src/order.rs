//! Request builder for Binance spot REST `POST /api/v3/order`.

use std::collections::BTreeMap;

use crate::auth::{build_signed_rest_params, current_timestamp_ms};
use trolly_strategy::OutboundMessage;

/// Order side (`BUY` / `SELL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            other => Err(OrderBuildError::InvalidSide(other.into())),
        }
    }
}

/// Order type (`MARKET` / `LIMIT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Time in force for limit orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

    pub fn parse(value: &str) -> Result<Self, OrderBuildError> {
        match value.to_ascii_uppercase().as_str() {
            "GTC" => Ok(Self::Gtc),
            "IOC" => Ok(Self::Ioc),
            "FOK" => Ok(Self::Fok),
            other => Err(OrderBuildError::InvalidTimeInForce(other.into())),
        }
    }
}

/// Signed REST order request (market or limit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOrderRequest {
    pub symbol: String,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub quantity: String,
    pub price: Option<String>,
    pub time_in_force: Option<TimeInForce>,
    pub client_order_id: Option<String>,
    pub timestamp_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderBuildError {
    InvalidSide(String),
    InvalidTimeInForce(String),
    LimitRequiresPrice,
    MarketMustNotHavePrice,
    NotAnOrderRequest,
}

impl std::fmt::Display for OrderBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSide(v) => write!(f, "invalid order side: {v}"),
            Self::InvalidTimeInForce(v) => write!(f, "invalid time in force: {v}"),
            Self::LimitRequiresPrice => write!(f, "limit order requires price"),
            Self::MarketMustNotHavePrice => write!(f, "market order must not include price"),
            Self::NotAnOrderRequest => write!(f, "expected OrderRequest outbound message"),
        }
    }
}

impl std::error::Error for OrderBuildError {}

impl NewOrderRequest {
    pub fn market(symbol: impl Into<String>, side: OrderSide, quantity: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into().to_ascii_uppercase(),
            side,
            order_type: OrderType::Market,
            quantity: quantity.into(),
            price: None,
            time_in_force: None,
            client_order_id: None,
            timestamp_ms: None,
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
            symbol: symbol.into().to_ascii_uppercase(),
            side,
            order_type: OrderType::Limit,
            quantity: quantity.into(),
            price: Some(price.into()),
            time_in_force: Some(time_in_force),
            client_order_id: None,
            timestamp_ms: None,
        }
    }

    pub fn client_order_id(mut self, client_order_id: impl Into<String>) -> Self {
        self.client_order_id = Some(client_order_id.into());
        self
    }

    pub fn timestamp_ms(mut self, timestamp_ms: u64) -> Self {
        self.timestamp_ms = Some(timestamp_ms);
        self
    }

    pub fn validate(&self) -> Result<(), OrderBuildError> {
        match self.order_type {
            OrderType::Limit if self.price.is_none() => Err(OrderBuildError::LimitRequiresPrice),
            OrderType::Market if self.price.is_some() => {
                Err(OrderBuildError::MarketMustNotHavePrice)
            }
            _ => Ok(()),
        }
    }

    /// Build unsigned REST query params (signature added separately).
    pub fn to_params(&self) -> Result<BTreeMap<String, String>, OrderBuildError> {
        self.validate()?;

        let mut params = BTreeMap::new();
        params.insert("symbol".into(), self.symbol.clone());
        params.insert("side".into(), self.side.as_str().into());
        params.insert("type".into(), self.order_type.as_str().into());
        params.insert("quantity".into(), self.quantity.clone());

        if let Some(price) = &self.price {
            params.insert("price".into(), price.clone());
        }

        if let Some(tif) = self.time_in_force {
            params.insert("timeInForce".into(), tif.as_str().into());
        }

        if let Some(client_order_id) = &self.client_order_id {
            params.insert("newClientOrderId".into(), client_order_id.clone());
        }

        let timestamp = self.timestamp_ms.unwrap_or_else(current_timestamp_ms);
        params.insert("timestamp".into(), timestamp.to_string());
        Ok(params)
    }

    pub fn to_signed_params(
        &self,
        secret_key: &str,
    ) -> Result<BTreeMap<String, String>, OrderBuildError> {
        let params = self.to_params()?;
        Ok(build_signed_rest_params(params, secret_key))
    }
}

impl TryFrom<&OutboundMessage> for NewOrderRequest {
    type Error = OrderBuildError;

    fn try_from(message: &OutboundMessage) -> Result<Self, Self::Error> {
        let OutboundMessage::OrderRequest {
            symbol,
            side,
            qty,
            price,
        } = message
        else {
            return Err(OrderBuildError::NotAnOrderRequest);
        };

        let side = OrderSide::parse(side)?;
        if let Some(price) = price {
            Ok(NewOrderRequest::limit(
                symbol,
                side,
                qty,
                price,
                TimeInForce::Gtc,
            ))
        } else {
            Ok(NewOrderRequest::market(symbol, side, qty))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_order_params_include_price_and_time_in_force() {
        let order = NewOrderRequest::limit(
            "btcusdt",
            OrderSide::Buy,
            "0.01",
            "100.0",
            TimeInForce::Ioc,
        )
        .timestamp_ms(1_700_000_000_000);

        let params = order.to_params().unwrap();
        assert_eq!(params.get("symbol"), Some(&"BTCUSDT".into()));
        assert_eq!(params.get("side"), Some(&"BUY".into()));
        assert_eq!(params.get("type"), Some(&"LIMIT".into()));
        assert_eq!(params.get("quantity"), Some(&"0.01".into()));
        assert_eq!(params.get("price"), Some(&"100.0".into()));
        assert_eq!(params.get("timeInForce"), Some(&"IOC".into()));
        assert_eq!(params.get("timestamp"), Some(&"1700000000000".into()));
        assert!(!params.contains_key("signature"));
    }

    #[test]
    fn market_order_params_omit_price_and_tif() {
        let order =
            NewOrderRequest::market("ETHBTC", OrderSide::Sell, "1.0").timestamp_ms(1_700_000_000_000);

        let params = order.to_params().unwrap();
        assert_eq!(params.get("type"), Some(&"MARKET".into()));
        assert!(!params.contains_key("price"));
        assert!(!params.contains_key("timeInForce"));
    }

    #[test]
    fn signed_params_include_hmac_signature() {
        let order = NewOrderRequest::market("BTCUSDT", OrderSide::Buy, "0.01")
            .timestamp_ms(1_700_000_000_000);
        let signed = order.to_signed_params("test-secret").unwrap();
        assert!(signed.contains_key("signature"));
        assert_ne!(signed.get("signature"), Some(&String::new()));
    }

    #[test]
    fn converts_strategy_order_request() {
        let outbound = OutboundMessage::OrderRequest {
            symbol: "BTCUSDT".into(),
            side: "BUY".into(),
            qty: "0.01".into(),
            price: Some("100".into()),
        };
        let order = NewOrderRequest::try_from(&outbound).unwrap();
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.price.as_deref(), Some("100"));
    }
}
