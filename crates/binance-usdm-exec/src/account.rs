use std::collections::HashMap;

use crate::types::{BalanceChange, MarginCall, PositionChange, SymbolBookkeeping};

/// Key for account-wide position rows: `(symbol, position_side)`.
pub type PositionKey = (String, String);

/// Account-wide balances and positions from `ACCOUNT_UPDATE` (and related) events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountBookkeeping {
    pub balances: HashMap<String, BalanceChange>,
    pub positions: HashMap<PositionKey, PositionChange>,
    /// Latest `MARGIN_CALL` snapshot; superseded when a newer `event_time` arrives.
    pub latest_margin_call: Option<MarginCall>,
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

    /// Record a margin-call snapshot; newer `event_time` replaces the previous call.
    pub fn apply_margin_call(&mut self, call: &MarginCall) {
        match &self.latest_margin_call {
            Some(existing) if call.event_time <= existing.event_time => {}
            _ => self.latest_margin_call = Some(call.clone()),
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

    pub fn latest_margin_call(&self) -> Option<&MarginCall> {
        self.latest_margin_call.as_ref()
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
    use crate::types::{MarginCall, MarginCallPosition};

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

    #[test]
    fn apply_margin_call_supersedes_on_newer_event_time() {
        let mut book = AccountBookkeeping::default();
        let older = MarginCall {
            event_time: 100,
            cross_wallet_balance: "1.0".into(),
            positions: vec![MarginCallPosition {
                symbol: "ETHUSDT".into(),
                position_side: "LONG".into(),
                position_amount: "1".into(),
                margin_type: "CROSSED".into(),
                isolated_wallet: "0".into(),
                mark_price: "100".into(),
                unrealized_pnl: "-1".into(),
                maintenance_margin_required: "0.5".into(),
            }],
        };
        let newer = MarginCall {
            event_time: 200,
            cross_wallet_balance: "2.0".into(),
            positions: vec![],
        };
        let stale = MarginCall {
            event_time: 50,
            cross_wallet_balance: "9.9".into(),
            positions: vec![],
        };

        book.apply_margin_call(&older);
        assert_eq!(book.latest_margin_call().unwrap().event_time, 100);

        book.apply_margin_call(&stale);
        assert_eq!(book.latest_margin_call().unwrap().event_time, 100);

        book.apply_margin_call(&newer);
        let latest = book.latest_margin_call().unwrap();
        assert_eq!(latest.event_time, 200);
        assert_eq!(latest.cross_wallet_balance, "2.0");
        assert!(latest.positions.is_empty());
    }
}
