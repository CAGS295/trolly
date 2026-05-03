//! Shared parsing for Binance-style depth WebSocket payloads.
//!
//! Both the live [`crate::monitor::order_book::OrderBook`] handler and lightweight
//! handlers (for example [`crate::monitor::echo_depth::EchoDepth`]) use the same
//! rules so routing keys (`symbol` / `RPI:` prefix) stay consistent.

use crate::providers::{NullResponse, RPI_PREFIX};
use lob::DepthUpdate;
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message;

#[derive(Deserialize, Debug)]
pub(crate) struct StreamEnvelope {
    pub stream: String,
    pub data: DepthUpdate,
}

/// Parse a Binance-style depth WebSocket message.
///
/// Returns [`None`] for benign control frames such as subscription acks.
pub fn parse_depth_message(message: Message) -> color_eyre::eyre::Result<Option<DepthUpdate>> {
    let data = message.into_data();
    parse_depth_bytes(&data)
}

/// Parse a depth payload from raw JSON bytes (same semantics as [`parse_depth_message`]).
pub fn parse_depth_bytes(data: &[u8]) -> color_eyre::eyre::Result<Option<DepthUpdate>> {
    if let Ok(mut envelope) = serde_json::from_slice::<StreamEnvelope>(data) {
        if envelope.stream.contains("rpiDepth") {
            envelope.data.event.symbol.insert_str(0, RPI_PREFIX);
        }
        return Ok(Some(envelope.data));
    }

    if let Ok(update) = serde_json::from_slice::<DepthUpdate>(data) {
        return Ok(Some(update));
    }

    let is_ack = serde_json::from_slice::<NullResponse>(data)
        .map(|r| r.result.is_none())
        .unwrap_or(false);

    if is_ack {
        Ok(None)
    } else {
        Err(color_eyre::eyre::eyre!(
            "unexpected depth message: {}",
            String::from_utf8_lossy(data)
        ))
    }
}
