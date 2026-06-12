//! Normalized stream events consumed by strategies (venue-agnostic).

use serde::{Deserialize, Serialize};

/// High-level category for multiplexed stream traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Depth,
    Execution,
    Account,
}

/// Order-book delta or snapshot fields (already normalized upstream).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DepthUpdate {
    pub symbol: String,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_id: Option<u64>,
}

/// Execution / fill report (user-data stream).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionUpdate {
    pub symbol: String,
    pub order_id: String,
    pub side: String,
    pub fill_qty: String,
    pub fill_price: String,
}

/// Balance or position bookkeeping update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountUpdate {
    pub asset: String,
    pub balance: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: String,
    pub qty: String,
}

/// Venue-neutral event envelope routed by symbol (or account asset).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamEvent {
    Depth(DepthUpdate),
    Execution(ExecutionUpdate),
    Account(AccountUpdate),
}

impl StreamEvent {
    pub fn kind(&self) -> EventKind {
        match self {
            Self::Depth(_) => EventKind::Depth,
            Self::Execution(_) => EventKind::Execution,
            Self::Account(_) => EventKind::Account,
        }
    }

    /// Key used by [`trolly_stream::MonitorMultiplexor`] for per-symbol routing.
    pub fn routing_id(&self) -> &str {
        match self {
            Self::Depth(d) => &d.symbol,
            Self::Execution(e) => &e.symbol,
            Self::Account(a) => a.symbol.as_deref().unwrap_or(&a.asset),
        }
    }
}

// Fix typo - I used ExecutionKind instead of EventKind