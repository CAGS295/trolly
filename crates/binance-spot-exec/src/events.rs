use serde::{Deserialize, Serialize};

/// Multiplexor route id for account-wide user-data events.
pub const ACCOUNT_ROUTE_ID: &str = "__account__";

/// Parsed Binance spot user-data stream event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpotUserEvent {
    ExecutionReport(ExecutionReport),
    OutboundAccountPosition(OutboundAccountPosition),
    BalanceUpdate(BalanceUpdate),
    ListStatus(ListStatus),
    ExternalLockUpdate(ExternalLockUpdate),
    EventStreamTerminated(EventStreamTerminated),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub event_time: u64,
    pub symbol: String,
    pub client_order_id: String,
    pub side: String,
    pub order_type: String,
    pub time_in_force: String,
    pub quantity: String,
    pub price: String,
    pub execution_type: String,
    pub order_status: String,
    pub order_id: i64,
    pub last_qty: String,
    pub cumulative_qty: String,
    pub last_price: String,
    pub transaction_time: u64,
    pub trade_id: i64,
    pub is_on_book: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetBalance {
    pub asset: String,
    pub free: String,
    pub locked: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboundAccountPosition {
    pub event_time: u64,
    pub last_update_time: u64,
    pub balances: Vec<AssetBalance>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalanceUpdate {
    pub event_time: u64,
    pub asset: String,
    pub delta: String,
    pub clear_time: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListStatusOrder {
    pub symbol: String,
    pub order_id: i64,
    pub client_order_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListStatus {
    pub event_time: u64,
    pub symbol: String,
    pub list_status_type: String,
    pub list_order_status: String,
    pub orders: Vec<ListStatusOrder>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalLockUpdate {
    pub event_time: u64,
    pub asset: String,
    pub delta: String,
    pub transaction_time: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventStreamTerminated {
    pub event_time: u64,
}

impl SpotUserEvent {
    pub fn route_id(&self) -> &str {
        match self {
            SpotUserEvent::ExecutionReport(r) => &r.symbol,
            SpotUserEvent::ListStatus(r) => &r.symbol,
            SpotUserEvent::OutboundAccountPosition(_)
            | SpotUserEvent::BalanceUpdate(_)
            | SpotUserEvent::ExternalLockUpdate(_)
            | SpotUserEvent::EventStreamTerminated(_) => ACCOUNT_ROUTE_ID,
        }
    }
}
