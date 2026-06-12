//! Stream-derived feature vectors and rolling observation windows.

use std::collections::VecDeque;

use trolly_strategy::{DepthUpdate, EventKind, StreamEvent};

/// Fixed-size feature vector extracted from a normalized stream event.
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureVector(pub Vec<f32>);

impl FeatureVector {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }
}

/// Rolling window of recent feature frames for one symbol (model input stub).
#[derive(Debug, Clone)]
pub struct ObservationWindow {
    capacity: usize,
    frames: VecDeque<FeatureVector>,
}

impl ObservationWindow {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            frames: VecDeque::with_capacity(capacity),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn push(&mut self, frame: FeatureVector) {
        if self.frames.len() == self.capacity {
            self.frames.pop_front();
        }
        self.frames.push_back(frame);
    }

    pub fn frames(&self) -> &VecDeque<FeatureVector> {
        &self.frames
    }

    /// Flatten frames oldest-to-newest for vectorized model input.
    pub fn flattened(&self) -> Vec<f32> {
        self.frames
            .iter()
            .flat_map(|f| f.0.iter().copied())
            .collect()
    }

    pub fn latest(&self) -> Option<&FeatureVector> {
        self.frames.back()
    }
}

/// Extract gym features from a normalized stream event.
///
/// Depth updates yield bid/ask top-of-book and spread; other kinds yield a
/// one-hot kind tag so the window stays populated across event types.
pub fn features_from_event(event: &StreamEvent) -> Option<FeatureVector> {
    match event {
        StreamEvent::Depth(depth) => Some(depth_features(depth)),
        StreamEvent::Execution(_) | StreamEvent::Account(_) => {
            Some(kind_tag_features(event.kind()))
        }
    }
}

fn parse_level(price: &str, qty: &str) -> (f32, f32) {
    (
        price.parse().unwrap_or(0.0),
        qty.parse().unwrap_or(0.0),
    )
}

fn depth_features(depth: &DepthUpdate) -> FeatureVector {
    let (best_bid, bid_qty) = depth
        .bids
        .first()
        .map(|l| parse_level(&l.price, &l.qty))
        .unwrap_or((0.0, 0.0));
    let (best_ask, ask_qty) = depth
        .asks
        .first()
        .map(|l| parse_level(&l.price, &l.qty))
        .unwrap_or((0.0, 0.0));
    let spread = if best_bid > 0.0 && best_ask > 0.0 {
        best_ask - best_bid
    } else {
        0.0
    };
    let mid = if best_bid > 0.0 && best_ask > 0.0 {
        (best_bid + best_ask) / 2.0
    } else {
        0.0
    };
    FeatureVector(vec![
        best_bid,
        bid_qty,
        best_ask,
        ask_qty,
        spread,
        mid,
        depth.update_id.unwrap_or(0) as f32,
    ])
}

fn kind_tag_features(kind: EventKind) -> FeatureVector {
    let tag = match kind {
        EventKind::Depth => 1.0,
        EventKind::Execution => 2.0,
        EventKind::Account => 3.0,
    };
    FeatureVector(vec![tag])
}

#[cfg(test)]
mod tests {
    use super::*;
    use trolly_strategy::{PriceLevel, StreamEvent};

    #[test]
    fn depth_features_include_spread_and_mid() {
        let event = StreamEvent::Depth(DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel {
                price: "100".into(),
                qty: "2".into(),
            }],
            asks: vec![PriceLevel {
                price: "102".into(),
                qty: "1".into(),
            }],
            update_id: Some(5),
        });
        let features = features_from_event(&event).unwrap();
        assert_eq!(features.len(), 7);
        assert!((features.as_slice()[4] - 2.0).abs() < f32::EPSILON); // spread
        assert!((features.as_slice()[5] - 101.0).abs() < f32::EPSILON); // mid
    }

    #[test]
    fn observation_window_rolls_at_capacity() {
        let mut window = ObservationWindow::new(2);
        window.push(FeatureVector(vec![1.0]));
        window.push(FeatureVector(vec![2.0]));
        window.push(FeatureVector(vec![3.0]));
        assert_eq!(window.len(), 2);
        assert_eq!(window.flattened(), vec![2.0, 3.0]);
    }
}
