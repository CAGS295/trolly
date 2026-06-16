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

/// Composite key for the latest position row per `(symbol, position_side)`.
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

    pub fn from_change(position: &PositionChange) -> Self {
        Self::new(&position.symbol, &position.position_side)
    }
}

/// Returns true when the position amount represents a flat/closed leg.
pub fn position_is_flat(position_amount: &str) -> bool {
    position_amount
        .trim()
        .parse::<f64>()
        .map(|amount| amount == 0.0)
        .unwrap_or(false)
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

/// Per-symbol execution and account-wide position bookkeeping state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SymbolBookkeeping {
    pub open_orders: HashMap<i64, OrderTradeUpdate>,
    /// Latest position row per `(symbol, position_side)`; flat legs are omitted.
    pub positions: HashMap<PositionKey, PositionChange>,
    /// Cross wallet balance from the most recent [`MarginCall`] (when provided).
    pub cross_wallet_balance: String,
    /// Latest margin-call payload for strategy inspection.
    ///
    /// Supersede semantics: a call replaces the stored payload when its
    /// [`MarginCall::event_time`] is greater than or equal to the previous one;
    /// older calls are ignored.
    pub latest_margin_call: Option<MarginCall>,
}

impl SymbolBookkeeping {
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
            .filter(move |position| position.symbol == symbol)
    }

    pub fn margin_call(&self) -> Option<&MarginCall> {
        self.latest_margin_call.as_ref()
    }

    /// Persist cross wallet balance and the margin-call positions snapshot.
    ///
    /// Newer calls (by [`MarginCall::event_time`]) replace older ones; equal
    /// timestamps are treated as newer (last-write-wins). Stale calls are ignored
    /// entirely (balance and snapshot are unchanged).
    pub fn apply_margin_call(&mut self, call: &MarginCall) {
        if self
            .latest_margin_call
            .as_ref()
            .is_some_and(|existing| existing.event_time > call.event_time)
        {
            return;
        }

        if !call.cross_wallet_balance.is_empty() {
            self.cross_wallet_balance = call.cross_wallet_balance.clone();
        }
        self.latest_margin_call = Some(call.clone());
    }
}

impl UsdmExecUpdate {
    pub fn routing_id(&self) -> &str {
        match self {
            Self::OrderTrade(o) => &o.symbol,
            Self::PositionChange(_)
            | Self::BalanceChange(_)
            | Self::ListenKeyExpired
            | Self::MarginCall(_) => crate::handler::ACCOUNT_ROUTING_ID,
        }
    }
}
