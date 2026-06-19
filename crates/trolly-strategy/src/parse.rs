//! Parse injectable websocket text frames into normalized [`StreamEvent`] values.

use crate::event::{StreamEvent, StreamEventKind};
use trolly_stream::Message;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    NotText,
    InvalidFormat,
    UnknownKind(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotText => write!(f, "expected text websocket frame"),
            Self::InvalidFormat => {
                write!(f, "expected `<kind>:<symbol>:<payload>` text frame")
            }
            Self::UnknownKind(kind) => write!(f, "unknown stream event kind: {kind}"),
        }
    }
}

impl std::error::Error for ParseError {}

fn kind_from_str(raw: &str) -> Result<StreamEventKind, ParseError> {
    match raw {
        "depth" => Ok(StreamEventKind::Depth),
        "execution" => Ok(StreamEventKind::Execution),
        "account" => Ok(StreamEventKind::Account),
        other => Err(ParseError::UnknownKind(other.to_string())),
    }
}

/// Parse a synthetic or normalized text frame: `depth:BTCUSDT:{"b":1}`.
pub fn parse_stream_event(message: &Message) -> Result<StreamEvent, ParseError> {
    let Message::Text(text) = message else {
        return Err(ParseError::NotText);
    };

    let mut parts = text.splitn(3, ':');
    let kind_raw = parts.next().ok_or(ParseError::InvalidFormat)?;
    let symbol = parts.next().ok_or(ParseError::InvalidFormat)?;
    let payload = parts.next().ok_or(ParseError::InvalidFormat)?;

    Ok(StreamEvent::new(
        symbol,
        kind_from_str(kind_raw)?,
        payload,
    ))
}
