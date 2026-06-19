//! Outbound commands dispatched by the strategy runtime through the egress API.

use std::fmt;

/// An outbound action produced by a strategy in response to a stream event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Send a websocket text frame on the outbound stream path.
    SendMessage { text: String },
    /// Request subscription to a symbol/channel pair (normalized, not venue-specific).
    Subscribe {
        symbol: String,
        channel: String,
    },
}

impl Command {
    pub fn send_message(text: impl Into<String>) -> Self {
        Self::SendMessage { text: text.into() }
    }

    pub fn subscribe(symbol: impl Into<String>, channel: impl Into<String>) -> Self {
        Self::Subscribe {
            symbol: symbol.into(),
            channel: channel.into(),
        }
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SendMessage { text } => write!(f, "send:{text}"),
            Self::Subscribe { symbol, channel } => {
                write!(f, "subscribe:{symbol}:{channel}")
            }
        }
    }
}
