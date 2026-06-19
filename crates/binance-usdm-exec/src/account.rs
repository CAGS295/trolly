use std::collections::HashMap;

use crate::types::{BalanceChange, MarginCall, PositionChange, PositionKey};

/// Account-wide balances and positions from `ACCOUNT_UPDATE` (and related) events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsdmAccountBookkeeping {
    pub balances: HashMap<String, BalanceChange>,
    pub positions: HashMap<PositionKey, PositionChange>,
    /// Latest `MARGIN_CALL` snapshot (`cross_wallet_balance` + affected legs).
    ///
    /// Supersede semantics: a newer call replaces the stored snapshot when its
    /// `event_time` is greater than or equal to the previous call's `event_time`.
    /// Older calls are ignored for state but may still be forwarded on the outbound
    /// channel.
    pub latest_margin_call: Option<MarginCall>,
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

    pub fn margin_call(&self) -> Option<&MarginCall> {
        self.latest_margin_call.as_ref()
    }

    /// Persist a `MARGIN_CALL` snapshot when it is newer than or equal to the stored call.
    ///
    /// Returns `true` when state was updated.
    pub fn apply_margin_call(&mut self, call: &MarginCall) -> bool {
        if self
            .latest_margin_call
            .as_ref()
            .is_some_and(|existing| call.event_time < existing.event_time)
        {
            return false;
        }
        self.latest_margin_call = Some(call.clone());
        true
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

    fn sample_margin_call(event_time: u64, cross_wallet: &str, symbol: &str) -> MarginCall {
        use crate::types::MarginCallPosition;

        MarginCall {
            event_time,
            cross_wallet_balance: cross_wallet.into(),
            positions: vec![MarginCallPosition {
                symbol: symbol.into(),
                position_side: "LONG".into(),
                position_amount: "1".into(),
                margin_type: "CROSSED".into(),
                isolated_wallet: "0".into(),
                mark_price: "100".into(),
                unrealized_pnl: "-1".into(),
                maintenance_margin_required: "2".into(),
            }],
        }
    }

    #[test]
    fn apply_margin_call_supersedes_newer() {
        let mut book = UsdmAccountBookkeeping::default();
        let older = sample_margin_call(100, "1.0", "ETHUSDT");
        let newer = sample_margin_call(200, "2.0", "BTCUSDT");

        assert!(book.apply_margin_call(&older));
        assert_eq!(book.margin_call().unwrap().cross_wallet_balance, "1.0");

        assert!(book.apply_margin_call(&newer));
        let stored = book.margin_call().unwrap();
        assert_eq!(stored.event_time, 200);
        assert_eq!(stored.cross_wallet_balance, "2.0");
        assert_eq!(stored.positions[0].symbol, "BTCUSDT");
    }

    #[test]
    fn apply_margin_call_ignores_older() {
        let mut book = UsdmAccountBookkeeping::default();
        let newer = sample_margin_call(200, "2.0", "BTCUSDT");
        let older = sample_margin_call(100, "1.0", "ETHUSDT");

        assert!(book.apply_margin_call(&newer));
        assert!(!book.apply_margin_call(&older));

        let stored = book.margin_call().unwrap();
        assert_eq!(stored.event_time, 200);
        assert_eq!(stored.cross_wallet_balance, "2.0");
    }

    #[test]
    fn apply_margin_call_equal_timestamp_replaces() {
        let mut book = UsdmAccountBookkeeping::default();
        let first = sample_margin_call(100, "1.0", "ETHUSDT");
        let second = sample_margin_call(100, "3.0", "BTCUSDT");

        assert!(book.apply_margin_call(&first));
        assert!(book.apply_margin_call(&second));
        assert_eq!(book.margin_call().unwrap().cross_wallet_balance, "3.0");
    }
}
