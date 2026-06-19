use serde::{Deserialize, Serialize};

/// Multiplexor route id for account-wide events (balances, not tied to one symbol).
pub const ACCOUNT_ROUTE_ID: &str = "__account__";

/// Marker type for [`trolly_stream::Endpoints`] / [`trolly_stream::EventHandler`] wiring.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpotExec;

/// Parsed Binance spot user-data stream event (execution + account bookkeeping).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpotUserDataEvent {
    ExecutionReport(ExecutionReport),
    OutboundAccountPosition(OutboundAccountPosition),
    BalanceUpdate(BalanceUpdate),
    ListStatus(ListStatus),
    ExternalLockUpdate(ExternalLockUpdate),
    EventStreamTerminated(EventStreamTerminated),
}

impl SpotUserDataEvent {
    /// Routing key for [`trolly_stream::EventHandler::to_id`] / multiplexor fan-in.
    pub fn route_id(&self) -> &str {
        match self {
            Self::ExecutionReport(e) => &e.symbol,
            Self::ListStatus(e) => &e.symbol,
            Self::OutboundAccountPosition(_)
            | Self::BalanceUpdate(_)
            | Self::ExternalLockUpdate(_)
            | Self::EventStreamTerminated(_) => ACCOUNT_ROUTE_ID,
        }
    }

    pub fn subscription_id(&self) -> Option<u64> {
        match self {
            Self::ExecutionReport(e) => e.subscription_id,
            Self::OutboundAccountPosition(e) => e.subscription_id,
            Self::BalanceUpdate(e) => e.subscription_id,
            Self::ListStatus(e) => e.subscription_id,
            Self::ExternalLockUpdate(e) => e.subscription_id,
            Self::EventStreamTerminated(e) => e.subscription_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<u64>,
    pub event_time: u64,
    pub symbol: String,
    pub client_order_id: String,
    pub side: OrderSide,
    pub order_type: String,
    pub time_in_force: String,
    pub order_quantity: String,
    pub order_price: String,
    pub stop_price: String,
    pub iceberg_quantity: String,
    pub order_list_id: i64,
    pub original_client_order_id: String,
    pub execution_type: String,
    pub order_status: String,
    pub reject_reason: String,
    pub order_id: u64,
    pub last_executed_quantity: String,
    pub cumulative_filled_quantity: String,
    pub last_executed_price: String,
    pub commission_amount: String,
    pub commission_asset: Option<String>,
    pub transaction_time: u64,
    pub trade_id: i64,
    pub prevented_match_id: Option<i64>,
    pub execution_id: u64,
    pub is_on_book: bool,
    pub is_maker: bool,
    pub ignore_field: bool,
    pub order_creation_time: u64,
    pub cumulative_quote_quantity: String,
    pub last_quote_quantity: String,
    pub quote_order_quantity: String,
    pub working_time: u64,
    pub self_trade_prevention_mode: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundAccountPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<u64>,
    pub event_time: u64,
    pub last_account_update: u64,
    pub balances: Vec<AssetBalance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetBalance {
    pub asset: String,
    pub free: String,
    pub locked: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalanceUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<u64>,
    pub event_time: u64,
    pub asset: String,
    pub balance_delta: String,
    pub clear_time: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<u64>,
    pub event_time: u64,
    pub symbol: String,
    pub order_list_id: i64,
    pub contingency_type: String,
    pub list_status_type: String,
    pub list_order_status: String,
    pub list_reject_reason: String,
    pub list_client_order_id: String,
    pub transaction_time: u64,
    pub orders: Vec<ListStatusOrder>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListStatusOrder {
    pub symbol: String,
    pub order_id: u64,
    pub client_order_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalLockUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<u64>,
    pub event_time: u64,
    pub asset: String,
    pub delta: String,
    pub transaction_time: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventStreamTerminated {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<u64>,
    pub event_time: u64,
}
