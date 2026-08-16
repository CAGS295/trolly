//! Discrete actions mapped to strategy egress commands.

use trolly_strategy::{OutboundMessage, StreamEgress};

/// Discrete action space for the training gym.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Hold,
    Buy,
    Sell,
}

impl Action {
    pub const COUNT: i64 = 3;

    /// Map categorical policy index back to a gym action.
    pub fn from_index(index: i64) -> Self {
        match index.rem_euclid(Self::COUNT) {
            1 => Self::Buy,
            2 => Self::Sell,
            _ => Self::Hold,
        }
    }

    /// Map this action to its categorical policy index.
    pub fn as_index(self) -> i64 {
        match self {
            Self::Hold => 0,
            Self::Buy => 1,
            Self::Sell => 2,
        }
    }

    pub(crate) fn target_position(self, current_position: i8) -> i8 {
        match self {
            Self::Hold => current_position,
            Self::Buy => 1,
            Self::Sell => -1,
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
