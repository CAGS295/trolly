//! Synthetic order-book microstructure for offline RL development.
//!
//! Generates [`StreamEvent::Depth`]-shaped observations (via the same feature
//! extractor as the stream [`Env`](crate::env::Env)) and mark-to-market rewards
//! on discrete hold/buy/sell actions.

use trolly_strategy::{DepthUpdate, PriceLevel, StreamEvent};

use crate::action::Action;
use crate::env::StepResult;
use crate::observation::{features_from_event, ObservationWindow};

/// Features per depth frame (see [`crate::observation::depth_features`]).
pub const FEATURES_PER_FRAME: usize = 7;

/// Configuration for [`MicrostructureSim`].
#[derive(Debug, Clone)]
pub struct MicrostructureConfig {
    pub symbol: String,
    /// Rolling observation window length (flattened obs = `FEATURES_PER_FRAME * window_frames`).
    pub window_frames: usize,
    /// Steps until `done` is set.
    pub episode_steps: usize,
    pub initial_mid: f32,
    pub half_spread: f32,
    /// Deterministic mid drift per step (before noise).
    pub mid_drift: f32,
    /// Uniform noise amplitude in `[-mid_noise, mid_noise]` per step.
    pub mid_noise: f32,
    /// Half-spread paid when changing net position (buy at ask, sell at bid).
    pub trade_cost: f32,
    pub seed: u64,
}

impl Default for MicrostructureConfig {
    fn default() -> Self {
        Self {
            symbol: "SYNTHUSDT".into(),
            window_frames: 1,
            episode_steps: 128,
            initial_mid: 100.0,
            half_spread: 0.5,
            mid_drift: 0.0,
            mid_noise: 0.25,
            trade_cost: 0.5,
            seed: 42,
        }
    }
}

impl MicrostructureConfig {
    pub fn obs_dim(&self) -> i64 {
        microstructure_obs_dim(self.window_frames)
    }
}

/// Flattened observation size for the given window length.
pub fn microstructure_obs_dim(window_frames: usize) -> i64 {
    (FEATURES_PER_FRAME * window_frames.max(1)) as i64
}

/// Aggregate stats from a completed episode.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MicrostructureStats {
    pub total_reward: f32,
    pub steps: usize,
    pub final_mid: f32,
    pub final_position: i8,
}

/// Synthetic single-instrument order book with unit position {-1, 0, 1}.
#[derive(Debug, Clone)]
pub struct MicrostructureSim {
    config: MicrostructureConfig,
    window: ObservationWindow,
    mid: f32,
    position: i8,
    step: usize,
    rng_state: u64,
    episode_reward: f32,
}

impl MicrostructureSim {
    pub fn new(config: MicrostructureConfig) -> Self {
        let mut sim = Self {
            rng_state: config.seed,
            config,
            window: ObservationWindow::new(1),
            mid: 0.0,
            position: 0,
            step: 0,
            episode_reward: 0.0,
        };
        sim.window = ObservationWindow::new(sim.config.window_frames);
        let _ = sim.reset();
        sim
    }

    pub fn config(&self) -> &MicrostructureConfig {
        &self.config
    }

    pub fn position(&self) -> i8 {
        self.position
    }

    pub fn mid(&self) -> f32 {
        self.mid
    }

    pub fn observation(&self) -> Vec<f32> {
        self.window.flattened()
    }

    /// Start a new episode; returns the initial observation.
    pub fn reset(&mut self) -> Vec<f32> {
        self.mid = self.config.initial_mid;
        self.position = 0;
        self.step = 0;
        self.episode_reward = 0.0;
        self.rng_state = self.config.seed;
        self.window = ObservationWindow::new(self.config.window_frames);
        for _ in 0..self.config.window_frames.max(1) {
            self.push_depth_frame();
        }
        self.observation()
    }

    /// Apply a discrete action, advance the latent mid, and return step output.
    pub fn step(&mut self, action: Action) -> StepResult {
        let old_mid = self.mid;
        let old_position = self.position;
        self.apply_action(action);
        let trade_cost = if self.position != old_position {
            self.config.trade_cost
        } else {
            0.0
        };
        self.advance_mid();
        self.push_depth_frame();
        let reward = self.position as f32 * (self.mid - old_mid) - trade_cost;
        self.episode_reward += reward;
        self.step += 1;
        let done = self.step >= self.config.episode_steps;
        StepResult {
            observation: self.observation(),
            reward,
            done,
        }
    }

    /// Consume stats when an episode ends.
    pub fn finish_episode(&self) -> MicrostructureStats {
        MicrostructureStats {
            total_reward: self.episode_reward,
            steps: self.step,
            final_mid: self.mid,
            final_position: self.position,
        }
    }

    fn apply_action(&mut self, action: Action) {
        self.position = match action {
            Action::Hold => self.position,
            Action::Buy => 1,
            Action::Sell => -1,
        };
    }

    fn advance_mid(&mut self) {
        self.mid += self.config.mid_drift + self.config.mid_noise * self.next_unit_noise();
    }

    fn push_depth_frame(&mut self) {
        let event = self.depth_event();
        if let Some(frame) = features_from_event(&event) {
            self.window.push(frame);
        }
    }

    fn depth_event(&self) -> StreamEvent {
        let bid = self.mid - self.config.half_spread;
        let ask = self.mid + self.config.half_spread;
        StreamEvent::Depth(DepthUpdate {
            symbol: self.config.symbol.clone(),
            bids: vec![PriceLevel {
                price: format!("{bid:.4}"),
                qty: "1".into(),
            }],
            asks: vec![PriceLevel {
                price: format!("{ask:.4}"),
                qty: "1".into(),
            }],
            update_id: Some(self.step as u64),
        })
    }

    fn next_unit_noise(&mut self) -> f32 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        ((self.rng_state >> 33) as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_observation_matches_feature_dim() {
        let sim = MicrostructureSim::new(MicrostructureConfig {
            window_frames: 2,
            ..Default::default()
        });
        assert_eq!(sim.observation().len(), FEATURES_PER_FRAME * 2);
    }

    #[test]
    fn long_position_earns_when_mid_rises() {
        let mut sim = MicrostructureSim::new(MicrostructureConfig {
            mid_drift: 1.0,
            mid_noise: 0.0,
            trade_cost: 0.0,
            episode_steps: 4,
            ..Default::default()
        });
        sim.reset();
        let r = sim.step(Action::Buy).reward;
        assert!(r > 0.0, "long should profit when mid rises: {r}");
    }

    #[test]
    fn episode_terminates_at_configured_horizon() {
        let mut sim = MicrostructureSim::new(MicrostructureConfig {
            episode_steps: 3,
            mid_noise: 0.0,
            ..Default::default()
        });
        sim.reset();
        assert!(!sim.step(Action::Hold).done);
        assert!(!sim.step(Action::Hold).done);
        assert!(sim.step(Action::Hold).done);
    }

    #[test]
    fn depth_features_include_mid_and_spread() {
        let sim = MicrostructureSim::new(MicrostructureConfig {
            initial_mid: 200.0,
            half_spread: 1.0,
            ..Default::default()
        });
        let obs = sim.observation();
        assert_eq!(obs.len(), FEATURES_PER_FRAME);
        assert!((obs[4] - 2.0).abs() < f32::EPSILON); // spread
        assert!((obs[5] - 200.0).abs() < f32::EPSILON); // mid
    }
}
