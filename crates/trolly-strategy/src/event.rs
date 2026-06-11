//! Normalized stream events consumed by strategies (venue-agnostic).

use std::fmt;

/// Kind of market or account update carried in a [`StreamEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamEventKind {
    Depth,
    Execution,
    Account,
}

impl fmt::Display for StreamEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Depth => write!(f, "depth"),
            Self::Execution => write!(f, "execution"),
            Self::Account => write!(f, "account"),
        }
    }
}

/// A normalized update from `trolly-stream` ingress (depth, execution, or account).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEvent {
    pub symbol: String,
    pub kind: StreamEventKind,
    /// Opaque payload (JSON or other normalized form from upstream parsers).
    pub payload: String,
}

impl StreamEvent {
    pub fn new(
        symbol: impl Into<String>,
        kind: StreamEventKind,
        payload: impl Into<String>,
    ) -> Self {
        Self {
            symbol: symbol.into(),
            kind,
            payload: payload.into(),
        }
    }
}
