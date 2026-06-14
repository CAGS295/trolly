//! Discrete actions mapped to strategy egress commands.

use trolly_strategy::{OutboundMessage, StreamEgress};

/// Discrete action space stub for the training gym.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Hold,
    Buy,
    Sell,
}

impl Action {
    pub fn to_outbound(&self, symbol: &str, qty: &str, price: Option<&str>) -> OutboundMessage {
        match self {
            Self::Hold => OutboundMessage::Subscribe {
                symbol: symbol.into(),
                channel: "depth".into(),
            },
            Self::Buy => OutboundMessage::OrderRequest {
                symbol: symbol.into(),
                side: "BUY".into(),
                qty: qty.into(),
                price: price.map(str::to_string),
                position_side: None,
            },
            Self::Sell => OutboundMessage::OrderRequest {
                symbol: symbol.into(),
                side: "SELL".into(),
                qty: qty.into(),
                price: price.map(str::to_string),
                position_side: None,
            },
        }
    }

    pub fn dispatch<E: StreamEgress>(
        &self,
        egress: &mut E,
        symbol: &str,
        qty: &str,
        price: Option<&str>,
    ) -> Result<(), E::Error> {
        egress.dispatch(self.to_outbound(symbol, qty, price))
    }
}
