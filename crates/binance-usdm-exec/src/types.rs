use std::collections::HashMap;

/// Marker type for [`crate::handler::UsdmExecHandler`] routing in `trolly-stream`.
#[derive(Debug, Clone, Copy, Default)]
pub struct UsdmExec;

/// Normalized USDM user-data stream update routed by symbol (or account key).
#[derive(Debug, Clone, PartialEq)]
pub enum UsdmExecUpdate {
    OrderTrade(OrderTradeUpdate),
    BalanceChange(BalanceChange),
    PositionChange(PositionChange),
    ListenKeyExpired,
    MarginCall(MarginCall),
}

/// `ORDER_TRADE_UPDATE` execution report for a single symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderTradeUpdate {
    pub event_time: u64,
    pub transaction_time: u64,
    pub symbol: String,
    pub client_order_id: String,
    pub side: String,
    pub order_type: String,
    pub time_in_force: String,
    pub original_qty: String,
    pub original_price: String,
    pub average_price: String,
    pub execution_type: String,
    pub order_status: String,
    pub order_id: i64,
    pub last_filled_qty: String,
    pub accumulated_filled_qty: String,
    pub last_filled_price: String,
    pub trade_id: i64,
    pub position_side: String,
    pub realized_profit: String,
}

/// Balance row from an `ACCOUNT_UPDATE` event.
#[derive(Debug, Clone, PartialEq)]
pub struct BalanceChange {
    pub event_time: u64,
    pub reason: String,
    pub asset: String,
    pub wallet_balance: String,
    pub cross_wallet_balance: String,
    pub balance_change: String,
}

/// Position row from an `ACCOUNT_UPDATE` event.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionChange {
    pub event_time: u64,
    pub reason: String,
    pub symbol: String,
    pub position_amount: String,
    pub entry_price: String,
    pub unrealized_pnl: String,
    pub margin_type: String,
    pub isolated_wallet: String,
    pub position_side: String,
}

impl PositionChange {
    /// Composite key for durable bookkeeping.
    pub fn key(&self) -> PositionKey {
        PositionKey {
            symbol: self.symbol.clone(),
            position_side: self.position_side.clone(),
        }
    }

    /// True when the position amount is zero (closed / flat).
    pub fn is_flat(&self) -> bool {
        is_zero_decimal(&self.position_amount)
    }
}

/// Stable key for `(symbol, position_side)` bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PositionKey {
    pub symbol: String,
    pub position_side: String,
}

impl PositionKey {
    pub fn new(symbol: impl Into<String>, position_side: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            position_side: position_side.into(),
        }
    }
}

/// `MARGIN_CALL` event (account-wide).
#[derive(Debug, Clone, PartialEq)]
pub struct MarginCall {
    pub event_time: u64,
    pub cross_wallet_balance: String,
    pub positions: Vec<MarginCallPosition>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarginCallPosition {
    pub symbol: String,
    pub position_side: String,
    pub position_amount: String,
    pub margin_type: String,
    pub isolated_wallet: String,
    pub mark_price: String,
    pub unrealized_pnl: String,
    pub maintenance_margin_required: String,
}

/// Execution and account bookkeeping state.
///
/// Per-symbol handlers populate [`Self::open_orders`] only. The account handler (and
/// shared account book) also stores [`Self::balances`] and [`Self::positions`], keyed
/// by asset and `(symbol, position_side)` respectively.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SymbolBookkeeping {
    pub open_orders: HashMap<i64, OrderTradeUpdate>,
    pub balances: HashMap<String, BalanceChange>,
    pub positions: HashMap<PositionKey, PositionChange>,
    /// Latest `MARGIN_CALL` snapshot (cross wallet balance + affected positions).
    pub latest_margin_call: Option<MarginCall>,
}

impl SymbolBookkeeping {
    /// Persist the latest balance row for an asset.
    pub fn apply_balance(&mut self, change: BalanceChange) {
        self.balances.insert(change.asset.clone(), change);
    }

    /// Persist the latest position row for `(symbol, position_side)`.
    ///
    /// Flat (zero amount) rows remove the leg so closed positions are absent from
    /// [`Self::positions`]; use [`PositionChange::is_flat`] on stream rows before apply
    /// when callers need to observe explicit zero updates on the wire.
    pub fn apply_position(&mut self, change: PositionChange) {
        let key = change.key();
        if change.is_flat() {
            self.positions.remove(&key);
        } else {
            self.positions.insert(key, change);
        }
    }

    /// Latest open position for `(symbol, position_side)`, if any.
    pub fn position(&self, symbol: &str, position_side: &str) -> Option<&PositionChange> {
        let key = PositionKey::new(symbol, position_side);
        self.positions
            .get(&key)
            .filter(|p| !p.is_flat())
    }

    /// All stored position legs for a symbol.
    pub fn positions_for_symbol<'a>(
        &'a self,
        symbol: &'a str,
    ) -> impl Iterator<Item = &'a PositionChange> + 'a {
        self.positions
            .values()
            .filter(move |p| p.symbol == symbol)
    }

    /// Open (non-flat) positions across all symbols.
    pub fn open_positions(&self) -> impl Iterator<Item = &PositionChange> {
        self.positions.values().filter(|p| !p.is_flat())
    }

    /// Latest balance row for an asset.
    pub fn balance(&self, asset: &str) -> Option<&BalanceChange> {
        self.balances.get(asset)
    }

    /// Persist the latest margin-call snapshot; newer calls supersede older ones.
    pub fn apply_margin_call(&mut self, call: MarginCall) {
        if self
            .latest_margin_call
            .as_ref()
            .is_some_and(|existing| existing.event_time > call.event_time)
        {
            return;
        }
        self.latest_margin_call = Some(call);
    }

    /// Latest margin-call payload for strategy inspection, if any.
    pub fn margin_call(&self) -> Option<&MarginCall> {
        self.latest_margin_call.as_ref()
    }
}

impl UsdmExecUpdate {
    pub fn routing_id(&self) -> &str {
        match self {
            Self::OrderTrade(o) => &o.symbol,
            Self::PositionChange(p) => &p.symbol,
            Self::BalanceChange(_) | Self::ListenKeyExpired | Self::MarginCall(_) => {
                crate::handler::ACCOUNT_ROUTING_ID
            }
        }
    }
}

fn is_zero_decimal(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return true;
    }
    trimmed
        .parse::<f64>()
        .map(|v| v == 0.0)
        .unwrap_or(matches!(trimmed, "0" | "0.0" | "0.00" | "-0" | "-0.0"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_position(amount: &str, side: &str) -> PositionChange {
        PositionChange {
            event_time: 1,
            reason: "ORDER".into(),
            symbol: "BTCUSDT".into(),
            position_amount: amount.into(),
            entry_price: "100".into(),
            unrealized_pnl: "0".into(),
            margin_type: "cross".into(),
            isolated_wallet: "0".into(),
            position_side: side.into(),
        }
    }

    #[test]
    fn apply_position_removes_flat_legs() {
        let mut book = SymbolBookkeeping::default();
        book.apply_position(sample_position("1.5", "LONG"));
        assert_eq!(book.positions.len(), 1);

        book.apply_position(sample_position("0", "LONG"));
        assert!(book.positions.is_empty());
        assert!(book.position("BTCUSDT", "LONG").is_none());
    }

    #[test]
    fn open_positions_excludes_flat_rows() {
        let mut book = SymbolBookkeeping::default();
        book.apply_position(sample_position("2", "LONG"));
        book.apply_position(sample_position("1", "SHORT"));
        assert_eq!(book.open_positions().count(), 2);
    }

    fn sample_margin_call(event_time: u64, cw: &str) -> MarginCall {
        MarginCall {
            event_time,
            cross_wallet_balance: cw.into(),
            positions: vec![MarginCallPosition {
                symbol: "ETHUSDT".into(),
                position_side: "LONG".into(),
                position_amount: "1.327".into(),
                margin_type: "CROSSED".into(),
                isolated_wallet: "0".into(),
                mark_price: "187.17".into(),
                unrealized_pnl: "-1.16".into(),
                maintenance_margin_required: "1.61".into(),
            }],
        }
    }

    #[test]
    fn apply_margin_call_supersedes_older() {
        let mut book = SymbolBookkeeping::default();
        book.apply_margin_call(sample_margin_call(100, "1.0"));
        book.apply_margin_call(sample_margin_call(200, "2.0"));
        assert_eq!(book.margin_call().unwrap().event_time, 200);
        assert_eq!(book.margin_call().unwrap().cross_wallet_balance, "2.0");

        book.apply_margin_call(sample_margin_call(150, "1.5"));
        assert_eq!(book.margin_call().unwrap().event_time, 200);
        assert_eq!(book.margin_call().unwrap().cross_wallet_balance, "2.0");
    }
}
