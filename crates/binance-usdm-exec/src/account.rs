use std::collections::HashMap;

use crate::types::{BalanceChange, PositionChange, PositionKey};

/// Account-wide balances and positions from `ACCOUNT_UPDATE` (and related) events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsdmAccountBookkeeping {
    pub balances: HashMap<String, BalanceChange>,
    pub positions: HashMap<PositionKey, PositionChange>,
}

impl UsdmAccountBookkeeping {
    pub fn apply_balance(&mut self, balance: &BalanceChange) {
        self.balances.insert(balance.asset.clone(), balance.clone());
    }

    pub fn apply_position(&mut self, position: &PositionChange) {
        let key = PositionKey::from(position);
        if is_position_closed(&position.position_amount) {
            self.positions.remove(&key);
        } else {
            self.positions.insert(key, position.clone());
        }
    }

    pub fn balance(&self, asset: &str) -> Option<&BalanceChange> {
        self.balances.get(asset)
    }

    pub fn position(&self, symbol: &str, position_side: &str) -> Option<&PositionChange> {
        self.positions
            .get(&PositionKey::new(symbol, position_side))
    }

    pub fn positions_for_symbol<'a>(
        &'a self,
        symbol: &'a str,
    ) -> impl Iterator<Item = &'a PositionChange> + 'a {
        self.positions
            .values()
            .filter(move |p| p.symbol == symbol)
    }
}

/// Returns true when a position amount represents a closed/flat leg.
pub fn is_position_closed(position_amount: &str) -> bool {
    let trimmed = position_amount.trim();
    if trimmed.is_empty() || trimmed == "0" {
        return true;
    }
    trimmed
        .parse::<f64>()
        .map(|v| v == 0.0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_position(symbol: &str, side: &str, amount: &str) -> PositionChange {
        PositionChange {
            event_time: 1,
            reason: "ORDER".into(),
            symbol: symbol.into(),
            position_amount: amount.into(),
            entry_price: "0".into(),
            unrealized_pnl: "0".into(),
            margin_type: "cross".into(),
            isolated_wallet: "0".into(),
            position_side: side.into(),
        }
    }

    #[test]
    fn closed_amounts_remove_position() {
        assert!(is_position_closed("0"));
        assert!(is_position_closed("0.0"));
        assert!(is_position_closed("0.000"));
        assert!(is_position_closed(""));
        assert!(!is_position_closed("1"));
        assert!(!is_position_closed("-10"));
    }

    #[test]
    fn apply_position_open_then_flatten() {
        let mut book = UsdmAccountBookkeeping::default();
        book.apply_position(&sample_position("BTCUSDT", "LONG", "20"));
        assert_eq!(book.positions.len(), 1);

        book.apply_position(&sample_position("BTCUSDT", "LONG", "0"));
        assert!(book.positions.is_empty());
        assert!(book.position("BTCUSDT", "LONG").is_none());
    }
}
