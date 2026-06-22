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
    /// Number of discrete actions (Hold, Buy, Sell).
    pub const COUNT: i64 = 3;

    /// Map a discrete action to a categorical policy index.
    pub fn to_index(self) -> i64 {
        match self {
            Self::Hold => 0,
            Self::Buy => 1,
            Self::Sell => 2,
        }
    }

    /// Decode a policy index back to an action.
    pub fn from_index(index: i64) -> Option<Self> {
        match index {
            0 => Some(Self::Hold),
            1 => Some(Self::Buy),
            2 => Some(Self::Sell),
            _ => None,
        }
    }

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
                time_in_force: None,
                position_side: None,
            },
            Self::Sell => OutboundMessage::OrderRequest {
                symbol: symbol.into(),
                side: "SELL".into(),
                qty: qty.into(),
                price: price.map(str::to_string),
                time_in_force: None,
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
