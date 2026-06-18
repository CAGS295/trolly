//! Binance USDM/futures user-data stream event types (local to this crate).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Marker type for [`trolly_stream::Endpoints`] wiring (analogous to depth `Depth`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsdmExec;

/// Normalized user-data events after venue parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsdmStreamEvent {
    OrderTradeUpdate(OrderTradeUpdate),
    AccountUpdate(AccountUpdate),
    MarginCall(MarginCall),
    ListenKeyExpired(ListenKeyExpired),
}

impl UsdmStreamEvent {
    /// Symbols that should receive this event via multiplexor routing.
    pub fn route_symbols(&self) -> Vec<String> {
        match self {
            Self::OrderTradeUpdate(o) => vec![o.order.symbol.clone()],
            Self::AccountUpdate(a) => a.route_symbols(),
            Self::MarginCall(m) => m
                .positions
                .iter()
                .map(|p| p.symbol.clone())
                .collect(),
            Self::ListenKeyExpired(_) => Vec::new(),
        }
    }
}

/// Per-symbol routing wrapper used by [`crate::ledger::UsdmSymbolLedger`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedUsdmEvent {
    pub symbol: String,
    pub event: UsdmStreamEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderTradeUpdate {
    pub event_time: u64,
    pub transaction_time: u64,
    pub order: OrderDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderDetail {
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
    pub cumulative_filled_qty: String,
    pub last_filled_price: String,
    pub commission_asset: Option<String>,
    pub commission: Option<String>,
    pub trade_time: u64,
    pub trade_id: i64,
    pub is_maker: bool,
    pub position_side: String,
    pub realized_profit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountUpdate {
    pub event_time: u64,
    pub transaction_time: u64,
    pub reason: String,
    pub balances: Vec<BalanceUpdate>,
    pub positions: Vec<PositionUpdate>,
}

impl AccountUpdate {
    pub fn route_symbols(&self) -> Vec<String> {
        if self.positions.is_empty() {
            return Vec::new();
        }
        let mut symbols: Vec<String> = self
            .positions
            .iter()
            .map(|p| p.symbol.clone())
            .collect();
        symbols.sort();
        symbols.dedup();
        symbols
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalanceUpdate {
    pub asset: String,
    pub wallet_balance: String,
    pub cross_wallet_balance: String,
    pub balance_change: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionUpdate {
    pub symbol: String,
    pub position_amount: String,
    pub entry_price: String,
    pub accumulated_realized: String,
    pub unrealized_pnl: String,
    pub margin_type: String,
    pub isolated_wallet: String,
    pub position_side: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarginCall {
    pub event_time: u64,
    pub cross_wallet_balance: String,
    pub positions: Vec<MarginCallPosition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenKeyExpired {
    pub event_time: u64,
}

/// In-memory per-symbol execution and position ledger updated from user-data events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SymbolLedgerSnapshot {
    pub symbol: String,
    pub orders: HashMap<i64, OrderDetail>,
    pub positions: HashMap<String, PositionUpdate>,
    pub last_account_reason: Option<String>,
}

impl SymbolLedgerSnapshot {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            ..Default::default()
        }
    }

    pub fn apply(&mut self, event: &UsdmStreamEvent) {
        match event {
            UsdmStreamEvent::OrderTradeUpdate(o) if o.order.symbol == self.symbol => {
                self.orders.insert(o.order.order_id, o.order.clone());
            }
            UsdmStreamEvent::AccountUpdate(a) => {
                self.last_account_reason = Some(a.reason.clone());
                for position in &a.positions {
                    if position.symbol == self.symbol {
                        self.positions
                            .insert(position.position_side.clone(), position.clone());
                    }
                }
            }
            UsdmStreamEvent::MarginCall(m) => {
                for position in &m.positions {
                    if position.symbol == self.symbol {
                        self.positions.insert(
                            position.position_side.clone(),
                            PositionUpdate {
                                symbol: position.symbol.clone(),
                                position_amount: position.position_amount.clone(),
                                entry_price: String::new(),
                                accumulated_realized: String::new(),
                                unrealized_pnl: position.unrealized_pnl.clone(),
                                margin_type: position.margin_type.clone(),
                                isolated_wallet: position.isolated_wallet.clone(),
                                position_side: position.position_side.clone(),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
}
