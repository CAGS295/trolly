//! Feature vectors derived from normalized [`StreamEvent`] updates.

use trolly_strategy::{StreamEvent, StreamEventKind};

/// Number of scalar features extracted from a single stream event.
pub const FEATURES_PER_EVENT: usize = 3;

/// Encode a stream event as a fixed-size feature vector.
///
/// Layout: `[kind_index, payload_numeric, symbol_len]`
pub fn event_to_features(event: &StreamEvent) -> Vec<f64> {
    let kind = match event.kind {
        StreamEventKind::Depth => 0.0,
        StreamEventKind::Execution => 1.0,
        StreamEventKind::Account => 2.0,
    };
    let payload_numeric = event.payload.parse::<f64>().unwrap_or(0.0);
    let symbol_len = event.symbol.len() as f64;
    vec![kind, payload_numeric, symbol_len]
}

/// Flatten the last `window_len` feature rows, zero-padding when history is short.
pub fn flatten_window(rows: &[Vec<f64>], window_len: usize) -> Vec<f64> {
    let feature_dim = FEATURES_PER_EVENT;
    let mut out = vec![0.0; window_len * feature_dim];
    let start = rows.len().saturating_sub(window_len);
    for (i, row) in rows[start..].iter().enumerate() {
        let offset = i * feature_dim;
        for (j, value) in row.iter().enumerate().take(feature_dim) {
            out[offset + j] = *value;
        }
    }
    out
}
