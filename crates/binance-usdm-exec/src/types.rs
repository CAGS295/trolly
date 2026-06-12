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

/// Per-symbol execution and position bookkeeping state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SymbolBookkeeping {
    pub open_orders: HashMap<i64, OrderTradeUpdate>,
    pub positions: HashMap<String, PositionChange>,
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
