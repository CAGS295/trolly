use std::collections::BTreeMap;

use crate::auth::{sign_params, signed_params_payload};
use crate::endpoints::ApiCredentials;

/// Order side for USDM placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
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

/// USDM order type.
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

/// Time-in-force for limit orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            other => Err(OrderBuildError::InvalidTimeInForce(other.into())),
        }
    }
}

/// Hedge-mode position leg (`LONG` / `SHORT`). One-way mode uses `BOTH` or omits the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionSide {
    Long,
    Short,
    Both,
}

impl PositionSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Long => "LONG",
            Self::Short => "SHORT",
            Self::Both => "BOTH",
        }
    }

    pub fn parse(value: &str) -> Result<Self, OrderBuildError> {
        match value.to_ascii_uppercase().as_str() {
            "LONG" => Ok(Self::Long),
            "SHORT" => Ok(Self::Short),
            "BOTH" => Ok(Self::Both),
            other => Err(OrderBuildError::InvalidPositionSide(other.into())),
        }
    }
}

/// Signed REST `POST /fapi/v1/order` request payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceOrderRequest {
    pub symbol: String,
    pub side: Side,
    pub order_type: OrderType,
    pub quantity: String,
    pub price: Option<String>,
    pub time_in_force: Option<TimeInForce>,
    pub position_side: Option<PositionSide>,
}

impl PlaceOrderRequest {
    pub fn market(
        symbol: impl Into<String>,
        side: Side,
        quantity: impl Into<String>,
    ) -> Self {
        Self {
            symbol: symbol.into().to_ascii_uppercase(),
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
        side: Side,
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
            position_side: None,
        }
    }

    pub fn with_position_side(mut self, position_side: PositionSide) -> Self {
        self.position_side = Some(position_side);
        self
    }

    pub fn unsigned_params(&self) -> BTreeMap<String, String> {
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
        if let Some(ps) = self.position_side {
            params.insert("positionSide".into(), ps.as_str().into());
        }
        params
    }

    pub fn signed_query(&self, credentials: &ApiCredentials) -> String {
        let params = sign_params(&credentials.secret_key, self.unsigned_params());
        signed_params_payload(&params)
    }
}

/// Build a [`PlaceOrderRequest`] from strategy egress fields.
pub fn order_from_outbound(
    symbol: String,
    side: &str,
    qty: &str,
    price: Option<&str>,
    time_in_force: Option<&str>,
    position_side: Option<&str>,
) -> Result<PlaceOrderRequest, OrderBuildError> {
    let side = Side::parse(side)?;
    let position_side = match position_side {
        Some(value) => Some(PositionSide::parse(value)?),
        None => None,
    };

    let mut order = match price {
        Some(price) => {
            let tif = match time_in_force {
                Some(value) => TimeInForce::parse(value)?,
                None => TimeInForce::Gtc,
            };
            PlaceOrderRequest::limit(symbol, side, qty, price, tif)
        }
        None => {
            if time_in_force.is_some() {
                return Err(OrderBuildError::TimeInForceOnMarket);
            }
            PlaceOrderRequest::market(symbol, side, qty)
        }
    };

    order.position_side = position_side;
    Ok(order)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderBuildError {
    InvalidSide(String),
    InvalidTimeInForce(String),
    InvalidPositionSide(String),
    TimeInForceOnMarket,
}

impl std::fmt::Display for OrderBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSide(value) => write!(f, "invalid order side: {value}"),
            Self::InvalidTimeInForce(value) => write!(f, "invalid time in force: {value}"),
            Self::InvalidPositionSide(value) => write!(f, "invalid position side: {value}"),
            Self::TimeInForceOnMarket => {
                write!(f, "time in force is only valid for limit orders")
            }
        }
    }
}

impl std::error::Error for OrderBuildError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::sign_hmac_sha256_hex;

    #[test]
    fn market_order_params_exclude_price_tif_and_position_side() {
        let order = PlaceOrderRequest::market("btcusdt", Side::Buy, "0.01");
        let params = order.unsigned_params();
        assert_eq!(params["symbol"], "BTCUSDT");
        assert_eq!(params["side"], "BUY");
        assert_eq!(params["type"], "MARKET");
        assert_eq!(params["quantity"], "0.01");
        assert!(!params.contains_key("price"));
        assert!(!params.contains_key("timeInForce"));
        assert!(!params.contains_key("positionSide"));
    }

    #[test]
    fn limit_order_params_include_price_tif_and_position_side() {
        let order = PlaceOrderRequest::limit("ETHUSDT", Side::Sell, "1", "3000", TimeInForce::Ioc)
            .with_position_side(PositionSide::Short);
        let params = order.unsigned_params();
        assert_eq!(params["type"], "LIMIT");
        assert_eq!(params["price"], "3000");
        assert_eq!(params["timeInForce"], "IOC");
        assert_eq!(params["positionSide"], "SHORT");
    }

    #[test]
    fn signed_query_matches_hmac_fixture() {
        let order = PlaceOrderRequest::limit("BTCUSDT", Side::Buy, "0.01", "100", TimeInForce::Gtc)
            .with_position_side(PositionSide::Long);
        let credentials = ApiCredentials {
            api_key: "test-key".into(),
            secret_key: "test-secret".into(),
        };
        let mut params = order.unsigned_params();
        params.insert("timestamp".into(), "1747385641636".into());
        let expected_sig = sign_hmac_sha256_hex(
            "test-secret",
            "positionSide=LONG&price=100&quantity=0.01&side=BUY&symbol=BTCUSDT&timeInForce=GTC&timestamp=1747385641636&type=LIMIT",
        );

        let signed = sign_params("test-secret", params);
        assert_eq!(signed["signature"], expected_sig);
        assert!(order.signed_query(&credentials).contains("signature="));
    }

    #[test]
    fn order_from_outbound_defaults_limit_tif_to_gtc() {
        let order = order_from_outbound(
            "BTCUSDT".into(),
            "buy",
            "0.1",
            Some("50000"),
            None,
            Some("LONG"),
        )
        .unwrap();
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.time_in_force, Some(TimeInForce::Gtc));
        assert_eq!(order.position_side, Some(PositionSide::Long));
    }
}
