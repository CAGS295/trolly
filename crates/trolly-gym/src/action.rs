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

    /// Map a tanh-Gaussian inventory target onto the live discrete action set.
    ///
    /// `|a| ≤ hold_deadzone` is Hold (mean-reversion / no-trade band).
    /// Positive targets Buy; negative targets Sell.
    pub fn quantize_inventory(target: f32, hold_deadzone: f32) -> Self {
        let band = hold_deadzone.abs();
        if target.abs() <= band {
            Self::Hold
        } else if target > 0.0 {
            Self::Buy
        } else {
            Self::Sell
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

#[cfg(test)]
mod tests {
    use super::*;
    use trolly_strategy::{OutboundMessage, RecordingEgress};

    #[test]
    fn quantize_inventory_deadzone_and_signs() {
        assert_eq!(Action::quantize_inventory(0.0, 0.25), Action::Hold);
        assert_eq!(Action::quantize_inventory(0.25, 0.25), Action::Hold);
        assert_eq!(Action::quantize_inventory(0.26, 0.25), Action::Buy);
        assert_eq!(Action::quantize_inventory(-0.26, 0.25), Action::Sell);
    }

    #[test]
    fn quantized_buy_dispatches_same_order_request() {
        let mut egress = RecordingEgress::default();
        let action = Action::quantize_inventory(0.8, 0.25);
        action
            .dispatch(&mut egress, "SYNTHUSDT", "1", None)
            .unwrap();
        assert_eq!(
            egress.dispatched.last(),
            Some(&OutboundMessage::OrderRequest {
                symbol: "SYNTHUSDT".into(),
                side: "BUY".into(),
                qty: "1".into(),
                price: None,
                time_in_force: None,
                position_side: None,
            })
        );
    }
}
