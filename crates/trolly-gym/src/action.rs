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

    /// Map a user-stream / receipt side string onto `{Hold,Buy,Sell}`.
    pub fn from_side(side: &str) -> Option<Self> {
        match side.trim().to_ascii_uppercase().as_str() {
            "BUY" => Some(Self::Buy),
            "SELL" => Some(Self::Sell),
            "HOLD" => Some(Self::Hold),
            _ => None,
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

    /// Pin this side onto a tracked observation symbol.
    ///
    /// Qty is never chosen here: [`crate::Env`] always uses
    /// [`crate::EnvConfig::default_qty`]. Side is `Buy` / `Sell` / `Hold`.
    /// Unknown symbols fall back to the primary Env pair.
    pub fn on_symbol(self, symbol: impl Into<String>) -> ActionDecision {
        ActionDecision {
            action: self,
            symbol: Some(symbol.into()),
        }
    }
}

/// Side plus optional non-primary dispatch symbol.
///
/// `symbol = None` keeps the primary `EnvConfig.symbol` (single-symbol and
/// WP-046 Gaussian paths). `symbol = Some` must name a tracked observation
/// pair or Env falls back to primary. Qty is always `EnvConfig.default_qty`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionDecision {
    pub action: Action,
    pub symbol: Option<String>,
}

impl From<Action> for ActionDecision {
    fn from(action: Action) -> Self {
        Self {
            action,
            symbol: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trolly_strategy::{OutboundMessage, RecordingEgress};

    #[test]
    fn from_side_maps_user_stream_strings() {
        assert_eq!(Action::from_side("buy"), Some(Action::Buy));
        assert_eq!(Action::from_side("SELL"), Some(Action::Sell));
        assert_eq!(Action::from_side("Hold"), Some(Action::Hold));
        assert_eq!(Action::from_side("unknown"), None);
    }

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

    #[test]
    fn on_symbol_keeps_side_and_names_pair() {
        let decision = Action::Sell.on_symbol("ETHUSDT");
        assert_eq!(decision.action, Action::Sell);
        assert_eq!(decision.symbol.as_deref(), Some("ETHUSDT"));
        assert_eq!(ActionDecision::from(Action::Hold).symbol, None);
    }
}
