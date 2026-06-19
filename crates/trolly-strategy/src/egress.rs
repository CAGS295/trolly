//! Outbound messages dispatched back through the stream egress API.
//!
//! Venue adapters (spot and USDM exec crates) consume [`OutboundMessage::OrderRequest`]
//! and translate to signed REST calls.

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
        /// Limit order time in force (`GTC`, `IOC`, `FOK`). Defaults to `GTC` when omitted.
        time_in_force: Option<String>,
        /// USDM hedge-mode position side (`LONG`, `SHORT`, `BOTH`). Omitted for spot.
        position_side: Option<String>,
    },
    /// Request an additional stream subscription.
    Subscribe { symbol: String, channel: String },
    /// Pre-serialized websocket payload (escape hatch for venue adapters).
    Raw(Message),
}

impl OutboundMessage {
    /// Build a normalized place-order command (`price: None` => market).
    pub fn order_request(
        symbol: impl Into<String>,
        side: impl Into<String>,
        qty: impl Into<String>,
        price: Option<impl Into<String>>,
    ) -> Self {
        Self::OrderRequest {
            symbol: symbol.into(),
            side: side.into(),
            qty: qty.into(),
            price: price.map(Into::into),
            time_in_force: None,
            position_side: None,
        }
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
