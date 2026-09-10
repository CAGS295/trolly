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
    (price.parse().unwrap_or(0.0), qty.parse().unwrap_or(0.0))
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

/// Features per ladder rung: `[v, α_ask, α_bid, Δα, q]`.
///
/// This is a **parallel** frame to the 7-D stream depth extractor.
/// Stream `Env` observations stay 7-D; microstructure policies that need
/// the inventory ladder read this layout instead.
pub const LADDER_FEATURES_PER_RUNG: usize = 5;

/// Linear bid/ask depth ladder `α(v) = δ + λ v`.
///
/// `v` is cumulative size already taken on that side (inventory depth), not
/// mid and not wall-clock time. Mid is used only to mark inventory.
#[derive(Debug, Clone, PartialEq)]
pub struct DepthLadderSpec {
    /// Touch offset δ (`MicrostructureConfig::trade_cost` WP-022 alias).
    pub delta: f32,
    /// Linear impact λ. Zero recovers the unit-lot flat fee.
    pub lambda: f32,
    /// Number of rungs `V` along depth.
    pub rung_count: usize,
    /// Width `Δv` of each rung.
    pub rung_width: f32,
}

impl Default for DepthLadderSpec {
    fn default() -> Self {
        Self {
            delta: 0.5,
            lambda: 0.25,
            rung_count: 8,
            rung_width: 0.25,
        }
    }
}

impl DepthLadderSpec {
    pub fn rung_count(&self) -> usize {
        self.rung_count.max(1)
    }

    pub fn rung_width(&self) -> f32 {
        if self.rung_width > 0.0 {
            self.rung_width
        } else {
            1.0
        }
    }

    /// Depth coordinate of rung `k` (start of the rung).
    pub fn rung_v(&self, k: usize) -> f32 {
        k as f32 * self.rung_width()
    }

    /// Half-spread / impact at depth `v`: `α(v) = δ + λ v`.
    pub fn alpha(&self, v: f32) -> f32 {
        self.delta + self.lambda * v.max(0.0)
    }

    /// Level-indexed `Δα` (same on every rung for a linear ladder): `λ Δv`.
    pub fn d_alpha(&self) -> f32 {
        self.lambda * self.rung_width()
    }

    /// Continuous integral `∫_{v0}^{v1} (δ + λ v) dv` with `v ≥ 0`.
    pub fn integral_alpha(&self, v0: f32, v1: f32) -> f32 {
        let v0 = v0.max(0.0);
        let v1 = v1.max(0.0);
        if (v1 - v0).abs() <= f32::EPSILON {
            return 0.0;
        }
        let (lo, hi, sign) = if v1 >= v0 {
            (v0, v1, 1.0)
        } else {
            (v1, v0, -1.0)
        };
        sign * (self.delta * (hi - lo) + 0.5 * self.lambda * (hi * hi - lo * lo))
    }

    /// Discrete rung-sum of `α(v_k) Δv` on `[v0, v1)` (same sign as the walk).
    pub fn rung_sum_alpha(&self, v0: f32, v1: f32) -> f32 {
        let v0 = v0.max(0.0);
        let v1 = v1.max(0.0);
        if (v1 - v0).abs() <= f32::EPSILON {
            return 0.0;
        }
        let (lo, hi, sign) = if v1 >= v0 {
            (v0, v1, 1.0)
        } else {
            (v1, v0, -1.0)
        };
        let dv = self.rung_width();
        let mut sum = 0.0;
        let start = (lo / dv).floor() as i32;
        let end = (hi / dv).ceil() as i32;
        for k in start..end {
            let k = k.max(0) as usize;
            let left = (k as f32 * dv).max(lo);
            let right = ((k + 1) as f32 * dv).min(hi);
            if right > left {
                sum += self.alpha(k as f32 * dv) * (right - left);
            }
        }
        sign * sum
    }

    /// Trading cost of walking inventory `q_from → q_to`.
    ///
    /// Buying (`q` increases) consumes the ask ladder at `v = max(q, 0)`.
    /// Selling consumes the bid ladder at `v = max(-q, 0)`. Crossing zero
    /// splits at the origin (no refund of the side just left).
    pub fn walk_cost(&self, q_from: f32, q_to: f32) -> f32 {
        if (q_to - q_from).abs() <= f32::EPSILON {
            return 0.0;
        }
        if q_from * q_to < 0.0 {
            return self.walk_cost(q_from, 0.0) + self.walk_cost(0.0, q_to);
        }
        if q_to > q_from {
            let v0 = q_from.max(0.0);
            self.integral_alpha(v0, v0 + (q_to - q_from))
        } else {
            let v0 = (-q_from).max(0.0);
            self.integral_alpha(v0, v0 + (q_from - q_to))
        }
    }

    pub fn obs_dim(&self) -> usize {
        self.rung_count() * LADDER_FEATURES_PER_RUNG
    }
}

/// Flattened ladder frame: `V` rungs of `[v_k, α_ask, α_bid, Δα, q]`.
pub fn ladder_features(spec: &DepthLadderSpec, inventory: f32) -> FeatureVector {
    let d_alpha = spec.d_alpha();
    let mut values = Vec::with_capacity(spec.obs_dim());
    for k in 0..spec.rung_count() {
        let v = spec.rung_v(k);
        let alpha = spec.alpha(v);
        values.extend_from_slice(&[v, alpha, alpha, d_alpha, inventory]);
    }
    FeatureVector(values)
}

/// Half-spread from a depth update, when both sides have a positive price.
pub fn depth_half_spread(depth: &DepthUpdate) -> Option<f32> {
    let (best_bid, _) = depth
        .bids
        .first()
        .map(|level| parse_level(&level.price, &level.qty))?;
    let (best_ask, _) = depth
        .asks
        .first()
        .map(|level| parse_level(&level.price, &level.qty))?;
    if best_bid > 0.0 && best_ask > best_bid {
        Some((best_ask - best_bid) / 2.0)
    } else {
        None
    }
}

/// Ladder frame from an ingested depth tick: book half-spread becomes δ,
/// inventory is the current `q`, and λ / `V` stay on the trained spec.
pub fn ladder_features_from_depth(
    depth: &DepthUpdate,
    spec: &DepthLadderSpec,
    inventory: f32,
) -> FeatureVector {
    let mut spec = spec.clone();
    if let Some(half) = depth_half_spread(depth) {
        spec.delta = half;
    }
    ladder_features(&spec, inventory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trolly_strategy::{DepthUpdate, PriceLevel, StreamEvent};

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

    #[test]
    fn ladder_features_expose_rungs_inventory_and_level_indexed_d_alpha() {
        let spec = DepthLadderSpec {
            delta: 0.5,
            lambda: 0.25,
            rung_count: 4,
            rung_width: 0.5,
        };
        let frame = ladder_features(&spec, 1.0);
        assert_eq!(frame.len(), 4 * LADDER_FEATURES_PER_RUNG);
        let s = frame.as_slice();
        assert!((s[0] - 0.0).abs() < f32::EPSILON);
        assert!((s[1] - spec.alpha(0.0)).abs() < 1e-6);
        assert!((s[2] - spec.alpha(0.0)).abs() < 1e-6);
        assert!((s[3] - spec.d_alpha()).abs() < 1e-6);
        assert!((s[4] - 1.0).abs() < f32::EPSILON);
        assert!((s[5] - 0.5).abs() < f32::EPSILON);
        for k in 0..4 {
            let d_alpha = s[k * LADDER_FEATURES_PER_RUNG + 3];
            assert!(
                (d_alpha - 0.125).abs() < 1e-6,
                "Δα must be level-indexed λΔv, got {d_alpha} at rung {k}"
            );
            assert!((s[k * LADDER_FEATURES_PER_RUNG + 4] - 1.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn walk_cost_integral_matches_closed_form() {
        let spec = DepthLadderSpec {
            delta: 0.5,
            lambda: 0.25,
            ..Default::default()
        };
        let expected = spec.delta + 0.5 * spec.lambda; // ∫_0^1 (δ + λv) dv
        assert!((spec.walk_cost(0.0, 1.0) - expected).abs() < 1e-6);
        assert!((spec.walk_cost(0.0, -1.0) - expected).abs() < 1e-6);
        assert!((spec.walk_cost(1.0, -1.0) - 2.0 * expected).abs() < 1e-6);
        assert_eq!(spec.walk_cost(1.0, 1.0), 0.0);
        assert!((spec.integral_alpha(0.0, 1.0) - spec.rung_sum_alpha(0.0, 1.0)).abs() < 0.15);
    }

    #[test]
    fn zero_lambda_recovers_flat_unit_lot_fee() {
        let spec = DepthLadderSpec {
            delta: 0.5,
            lambda: 0.0,
            ..Default::default()
        };
        assert!((spec.walk_cost(0.0, 1.0) - 0.5).abs() < f32::EPSILON);
        assert!((spec.walk_cost(1.0, 0.0) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn ladder_features_from_depth_use_book_half_spread_as_delta() {
        let depth = DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel {
                price: "100".into(),
                qty: "2".into(),
            }],
            asks: vec![PriceLevel {
                price: "104".into(),
                qty: "1".into(),
            }],
            update_id: Some(1),
        };
        let spec = DepthLadderSpec {
            delta: 0.5,
            lambda: 0.25,
            rung_count: 2,
            rung_width: 0.25,
        };
        let frame = ladder_features_from_depth(&depth, &spec, -0.5);
        let s = frame.as_slice();
        assert_eq!(s.len(), 10);
        assert!((s[1] - 2.0).abs() < 1e-6); // δ = half-spread = 2
        assert!((s[4] + 0.5).abs() < f32::EPSILON); // q
        assert!((s[6] - (2.0 + 0.25 * 0.25)).abs() < 1e-6);
    }
}
