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

/// Bridges policy harness output into order-placement egress adapters.
///
/// `Action::dispatch` may emit non-order messages such as `Subscribe` for
/// `Hold`. Venue execution adapters only understand order requests, so this
/// wrapper forwards [`OutboundMessage::OrderRequest`] and treats everything
/// else as an ignored side effect.
#[derive(Debug, Clone)]
pub struct OrderOnlyEgress<E> {
    inner: E,
}

impl<E> OrderOnlyEgress<E> {
    pub fn new(inner: E) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &E {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut E {
        &mut self.inner
    }

    pub fn into_inner(self) -> E {
        self.inner
    }
}

impl<E> From<E> for OrderOnlyEgress<E> {
    fn from(inner: E) -> Self {
        Self::new(inner)
    }
}

impl<E> StreamEgress for OrderOnlyEgress<E>
where
    E: StreamEgress,
{
    type Error = E::Error;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        if matches!(&message, OutboundMessage::OrderRequest { .. }) {
            self.inner.dispatch(message)
        } else {
            Ok(())
        }
    }
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
    use trolly_stream::Message;

    #[test]
    fn order_only_egress_forwards_orders_and_ignores_non_orders() {
        let mut egress = OrderOnlyEgress::new(RecordingEgress::default());

        egress
            .dispatch(OutboundMessage::Subscribe {
                symbol: "BTCUSDT".into(),
                channel: "depth".into(),
            })
            .unwrap();
        egress
            .dispatch(OutboundMessage::Raw(Message::Text("{}".into())))
            .unwrap();
        egress
            .dispatch(OutboundMessage::order_request(
                "BTCUSDT",
                "BUY",
                "0.01",
                None::<&str>,
            ))
            .unwrap();

        assert_eq!(
            egress.inner().dispatched,
            vec![OutboundMessage::order_request(
                "BTCUSDT",
                "BUY",
                "0.01",
                None::<&str>,
            )]
        );
    }
}
