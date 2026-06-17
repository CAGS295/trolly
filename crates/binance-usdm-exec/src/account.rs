use std::collections::HashMap;

use crate::types::{BalanceChange, PositionChange, SymbolBookkeeping};

/// Key for account-wide position rows: `(symbol, position_side)`.
pub type PositionKey = (String, String);

/// Account-wide balances and positions from `ACCOUNT_UPDATE` (and related) events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountBookkeeping {
    pub balances: HashMap<String, BalanceChange>,
    pub positions: HashMap<PositionKey, PositionChange>,
}

impl AccountBookkeeping {
    pub fn apply_balance(&mut self, balance: &BalanceChange) {
        self.balances.insert(balance.asset.clone(), balance.clone());
    }

    pub fn apply_position(&mut self, position: &PositionChange) {
        let key = (
            position.symbol.clone(),
            position.position_side.clone(),
        );
        if position_is_closed(&position.position_amount) {
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
            .get(&(symbol.to_owned(), position_side.to_owned()))
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

/// Apply a position row to per-symbol [`SymbolBookkeeping`] (keyed by `position_side`).
pub fn apply_position_to_symbol(state: &mut SymbolBookkeeping, position: &PositionChange) {
    if position_is_closed(&position.position_amount) {
        state.positions.remove(&position.position_side);
    } else {
        state
            .positions
            .insert(position.position_side.clone(), position.clone());
    }
}

/// Returns true when a position amount represents a closed/flat leg.
pub fn position_is_closed(position_amount: &str) -> bool {
    let trimmed = position_amount.trim();
    if trimmed.is_empty() {
        return true;
    }
    match trimmed.parse::<f64>() {
        Ok(v) => v == 0.0,
        Err(_) => trimmed == "0",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_is_closed_detects_zero_forms() {
        assert!(position_is_closed("0"));
        assert!(position_is_closed("0.0"));
        assert!(position_is_closed("0.000"));
        assert!(position_is_closed(""));
        assert!(!position_is_closed("1"));
        assert!(!position_is_closed("-0.5"));
    }

    #[test]
    fn apply_position_removes_closed_legs_from_account() {
        let mut book = AccountBookkeeping::default();
        let open = PositionChange {
            event_time: 1,
            reason: "ORDER".into(),
            symbol: "BTCUSDT".into(),
            position_amount: "10".into(),
            entry_price: "100".into(),
            unrealized_pnl: "0".into(),
            margin_type: "cross".into(),
            isolated_wallet: "0".into(),
            position_side: "LONG".into(),
        };
        book.apply_position(&open);
        assert_eq!(book.positions.len(), 1);

        let closed = PositionChange {
            position_amount: "0".into(),
            ..open
        };
        book.apply_position(&closed);
        assert!(book.positions.is_empty());
    }
}
