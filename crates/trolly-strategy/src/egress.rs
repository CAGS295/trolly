//! Outbound messages dispatched back through the stream egress API.

use trolly_stream::Message;

/// Normalized command emitted by a strategy runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum OutboundMessage {
    /// Place or amend an order on a symbol.
    OrderRequest {
        symbol: String,
        side: String,
        qty: String,
        price: Option<String>,
        /// Limit order time-in-force (`GTC`, `IOC`, `FOK`). Ignored for market orders.
        time_in_force: Option<String>,
        /// USDM hedge-mode position leg (`LONG`, `SHORT`, `BOTH`). Ignored by spot adapters.
        position_side: Option<String>,
    },
    /// Request an additional stream subscription.
    Subscribe { symbol: String, channel: String },
    /// Pre-serialized websocket payload (escape hatch for venue adapters).
    Raw(Message),
}

impl OutboundMessage {
    pub fn market_order(
        symbol: impl Into<String>,
        side: impl Into<String>,
        qty: impl Into<String>,
    ) -> Self {
        Self::OrderRequest {
            symbol: symbol.into(),
            side: side.into(),
            qty: qty.into(),
            price: None,
            time_in_force: None,
            position_side: None,
        }
    }

    pub fn limit_order(
        symbol: impl Into<String>,
        side: impl Into<String>,
        qty: impl Into<String>,
        price: impl Into<String>,
        time_in_force: Option<impl Into<String>>,
    ) -> Self {
        Self::OrderRequest {
            symbol: symbol.into(),
            side: side.into(),
            qty: qty.into(),
            price: Some(price.into()),
            time_in_force: time_in_force.map(Into::into),
            position_side: None,
        }
    }

    /// Attach a USDM `positionSide` leg to an order request.
    pub fn with_position_side(mut self, position_side: impl Into<String>) -> Self {
        if let Self::OrderRequest {
            position_side: ref mut leg,
            ..
        } = &mut self
        {
            *leg = Some(position_side.into());
        }
        self
    }
}

/// Dispatches outbound stream messages (websocket writes, fan-in queues, etc.).
pub trait StreamEgress {
    type Error;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error>;
}

/// Records dispatched commands for tests.
#[derive(Debug, Default)]
pub struct RecordingEgress {
    pub dispatched: Vec<OutboundMessage>,
}

impl StreamEgress for RecordingEgress {
    type Error = std::convert::Infallible;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        self.dispatched.push(message);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_and_limit_order_constructors() {
        assert_eq!(
            OutboundMessage::market_order("BTCUSDT", "BUY", "1"),
            OutboundMessage::OrderRequest {
                symbol: "BTCUSDT".into(),
                side: "BUY".into(),
                qty: "1".into(),
                price: None,
                time_in_force: None,
                position_side: None,
            }
        );
        assert_eq!(
            OutboundMessage::limit_order("ETHUSDT", "SELL", "2", "3000", Some("IOC")),
            OutboundMessage::OrderRequest {
                symbol: "ETHUSDT".into(),
                side: "SELL".into(),
                qty: "2".into(),
                price: Some("3000".into()),
                time_in_force: Some("IOC".into()),
                position_side: None,
            }
        );
        assert_eq!(
            OutboundMessage::market_order("BTCUSDT", "BUY", "1").with_position_side("LONG"),
            OutboundMessage::OrderRequest {
                symbol: "BTCUSDT".into(),
                side: "BUY".into(),
                qty: "1".into(),
                price: None,
                time_in_force: None,
                position_side: Some("LONG".into()),
            }
        );
    }
}
