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
        /// Hedge-mode position leg (`LONG` / `SHORT` / `BOTH`); omit for one-way mode.
        position_side: Option<String>,
    },
    /// Request an additional stream subscription.
    Subscribe { symbol: String, channel: String },
    /// Pre-serialized websocket payload (escape hatch for venue adapters).
    Raw(Message),
}

impl OutboundMessage {
    /// Normalized spot/futures place-order command consumed by venue egress adapters.
    pub fn order_request(
        symbol: impl Into<String>,
        side: impl Into<String>,
        qty: impl Into<String>,
        price: Option<String>,
        position_side: Option<String>,
    ) -> Self {
        Self::OrderRequest {
            symbol: symbol.into(),
            side: side.into(),
            qty: qty.into(),
            price,
            position_side,
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
