//! Synthetic order-book microstructure for offline RL development.
//!
//! Stream observations stay on the 7-D [`features_from_event`] extractor used
//! by the live [`Env`](crate::env::Env). Trading cost is a linear bid/ask
//! **depth ladder** `α(v) = δ + λ v` (WP-032). Mid is used only to mark
//! inventory: `r = q_new · Δs − Δcost`. Discrete `{Hold,Buy,Sell}` unit lots
//! remain the live action set; [`MicrostructureSim::step_target`] walks
//! continuous inventory `q → a ∈ (-1, 1)` for the WP-033 Gaussian policy.
//! `λ = 0` is the WP-022 flat-fee compatibility path.

use trolly_strategy::{DepthUpdate, PriceLevel, StreamEvent};

use crate::action::Action;
use crate::env::StepResult;
use crate::observation::{features_from_event, ladder_features, ObservationWindow};
use crate::ticks::{now_ts_ms, TickRow, TickTape};

pub use crate::observation::{DepthLadderSpec, LADDER_FEATURES_PER_RUNG};

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
    /// Ladder intercept δ. WP-022 name: flat unit-lot fee when `lambda == 0`.
    pub trade_cost: f32,
    /// Ladder slope λ. Default is a learnable impact; set `0` for unit-lot snap.
    pub lambda: f32,
    /// Number of parallel ladder rungs `V`.
    pub rung_count: usize,
    /// Rung width `Δv` (inventory units).
    pub rung_width: f32,
    /// When true, each [`MicrostructureSim::reset`] draws a new mid-path seed.
    pub resample_episode_seeds: bool,
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
            lambda: 0.25,
            rung_count: 8,
            rung_width: 0.25,
            resample_episode_seeds: true,
            seed: 42,
        }
    }
}

impl MicrostructureConfig {
    /// WP-022 unit-lot snap: `α(v) = δ` (λ = 0), fixed episode seed.
    pub fn unit_lot_compat() -> Self {
        Self {
            lambda: 0.0,
            resample_episode_seeds: false,
            ..Default::default()
        }
    }

    pub fn obs_dim(&self) -> i64 {
        microstructure_obs_dim(self.window_frames)
    }

    pub fn ladder_obs_dim(&self) -> i64 {
        ladder_obs_dim(self.rung_count)
    }

    pub fn ladder_spec(&self) -> DepthLadderSpec {
        DepthLadderSpec {
            delta: self.trade_cost,
            lambda: self.lambda,
            rung_count: self.rung_count,
            rung_width: self.rung_width,
        }
    }
}

/// Flattened stream observation size for the given window length (7-D frames).
pub fn microstructure_obs_dim(window_frames: usize) -> i64 {
    (FEATURES_PER_FRAME * window_frames.max(1)) as i64
}

/// Flattened parallel ladder observation size (`V × 5`).
pub fn ladder_obs_dim(rung_count: usize) -> i64 {
    (rung_count.max(1) * LADDER_FEATURES_PER_RUNG) as i64
}

/// Aggregate stats from a completed episode.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MicrostructureStats {
    pub total_reward: f32,
    pub steps: usize,
    pub final_mid: f32,
    pub final_position: i8,
}

/// Eval stats including position-change count (for completion checks).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MicrostructureEvalStats {
    pub total_reward: f32,
    pub steps: usize,
    pub trades: usize,
    pub final_position: i8,
    /// Mean `|q|` over the episode (WP-033 mean-reversion diagnostic).
    pub mean_abs_inventory: f32,
    /// Terminal inventory (continuous).
    pub final_inventory: f32,
}

/// Fixed baseline policies for oracle / hold comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselinePolicy {
    Hold,
    Long,
    Short,
    Oracle,
    Random,
}

/// Analytic oracle reward: enter once in the drift direction, then hold.
///
/// Entry pays the ladder walk `0 → sign(drift)`, not a flat fee unless `λ = 0`.
pub fn oracle_reward_estimate(config: &MicrostructureConfig) -> f32 {
    if config.mid_drift.abs() <= f32::EPSILON {
        0.0
    } else {
        let target = if config.mid_drift > 0.0 { 1.0 } else { -1.0 };
        let entry_cost = config.ladder_spec().walk_cost(0.0, target);
        config.episode_steps as f32 * config.mid_drift.abs() - entry_cost
    }
}

/// Hold-path book ticks from several mid-path seeds (not one 64-tick tape).
pub fn generate_resampled_tick_rows(
    config: &MicrostructureConfig,
    session_id: &str,
    episode_count: usize,
    steps_per_episode: usize,
) -> Vec<TickRow> {
    let mut rows = Vec::new();
    let episodes = episode_count.max(1);
    let steps = steps_per_episode.max(1);
    for episode in 0..episodes {
        let mut cfg = config.clone();
        cfg.seed = config.seed.wrapping_add(episode as u64);
        cfg.resample_episode_seeds = false;
        cfg.episode_steps = steps;
        let mut sim = MicrostructureSim::new(cfg);
        sim.reset();
        rows.push(sim.last_tick(session_id, "sim"));
        for _ in 0..steps {
            sim.step(Action::Hold);
            rows.push(sim.last_tick(session_id, "sim"));
        }
    }
    rows
}

/// Run a full episode with a baseline policy and eval seed.
pub fn run_baseline_episode(
    config: &MicrostructureConfig,
    seed: u64,
    policy: BaselinePolicy,
) -> MicrostructureEvalStats {
    run_episode_with_actions(config, seed, |step, _obs| baseline_action(config, policy, step))
}

/// Run a full episode; `choose` receives `(step_index, stream observation)`.
pub fn run_episode_with_actions<F>(
    config: &MicrostructureConfig,
    seed: u64,
    mut choose: F,
) -> MicrostructureEvalStats
where
    F: FnMut(usize, &[f32]) -> Action,
{
    let mut cfg = config.clone();
    cfg.seed = seed;
    let mut sim = MicrostructureSim::new(cfg);
    let mut obs = sim.reset();
    let mut trades = 0usize;
    let mut step_idx = 0usize;
    let mut abs_q_sum = 0.0_f32;

    loop {
        let action = choose(step_idx, &obs);
        let old_q = sim.inventory();
        let result = sim.step(action);
        if (sim.inventory() - old_q).abs() > 1e-6 {
            trades += 1;
        }
        abs_q_sum += sim.inventory().abs();
        step_idx += 1;
        obs = result.observation.clone();
        if result.done {
            return finish_eval_stats(&sim, trades, abs_q_sum);
        }
    }
}

/// Run a full episode with continuous target inventory `a ∈ (-1, 1)`.
///
/// `choose` receives `(step_index, ladder_observation)`. Walking `q → a`
/// pays the WP-032 level integral.
pub fn run_episode_with_targets<F>(
    config: &MicrostructureConfig,
    seed: u64,
    mut choose: F,
) -> MicrostructureEvalStats
where
    F: FnMut(usize, &[f32]) -> f32,
{
    let mut cfg = config.clone();
    cfg.seed = seed;
    let mut sim = MicrostructureSim::new(cfg);
    let _ = sim.reset();
    let mut trades = 0usize;
    let mut step_idx = 0usize;
    let mut abs_q_sum = 0.0_f32;

    loop {
        let ladder = sim.ladder_observation();
        let target = choose(step_idx, &ladder);
        let old_q = sim.inventory();
        let result = sim.step_target(target);
        if (sim.inventory() - old_q).abs() > 1e-6 {
            trades += 1;
        }
        abs_q_sum += sim.inventory().abs();
        step_idx += 1;
        let _ = result.observation;
        if result.done {
            return finish_eval_stats(&sim, trades, abs_q_sum);
        }
    }
}

fn finish_eval_stats(
    sim: &MicrostructureSim,
    trades: usize,
    abs_q_sum: f32,
) -> MicrostructureEvalStats {
    let stats = sim.finish_episode();
    MicrostructureEvalStats {
        total_reward: stats.total_reward,
        steps: stats.steps,
        trades,
        final_position: stats.final_position,
        mean_abs_inventory: if stats.steps == 0 {
            0.0
        } else {
            abs_q_sum / stats.steps as f32
        },
        final_inventory: sim.inventory(),
    }
}

/// Absolute inventory snap for a live discrete action (Hold flattens to `0`).
pub fn discrete_target(action: Action) -> f32 {
    match action {
        Action::Hold => 0.0,
        Action::Buy => 1.0,
        Action::Sell => -1.0,
    }
}

/// Clamp a target inventory into `[-1, 1]`.
///
/// Tanh-Gaussian samples already live in `(-1, 1)`; discrete Buy/Sell snap
/// to the closed endpoints.
pub fn clamp_inventory_target(action: f32) -> f32 {
    action.clamp(-1.0, 1.0)
}

fn baseline_action(config: &MicrostructureConfig, policy: BaselinePolicy, step: usize) -> Action {
    match policy {
        BaselinePolicy::Hold => Action::Hold,
        BaselinePolicy::Long => Action::Buy,
        BaselinePolicy::Short => Action::Sell,
        BaselinePolicy::Oracle => {
            if step == 0 {
                if config.mid_drift > 0.0 {
                    Action::Buy
                } else if config.mid_drift < 0.0 {
                    Action::Sell
                } else {
                    Action::Hold
                }
            } else {
                Action::Hold
            }
        }
        BaselinePolicy::Random => {
            let mut state = config.seed.wrapping_add(step as u64 + 1);
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1);
            match state % 3 {
                0 => Action::Hold,
                1 => Action::Buy,
                _ => Action::Sell,
            }
        }
    }
}

/// Synthetic single-instrument order book.
///
/// Discrete `{Hold,Buy,Sell}` snaps inventory to `{-1, 0, 1}`. The WP-033
/// Gaussian path walks a continuous `q ∈ (-1, 1)` via [`Self::step_target`].
#[derive(Debug, Clone)]
pub struct MicrostructureSim {
    config: MicrostructureConfig,
    window: ObservationWindow,
    mid: f32,
    inventory: f32,
    step: usize,
    rng_state: u64,
    episode_index: u64,
    episode_reward: f32,
    tape: Option<TickTape>,
}

impl MicrostructureSim {
    pub fn new(config: MicrostructureConfig) -> Self {
        let mut sim = Self {
            rng_state: config.seed,
            config,
            window: ObservationWindow::new(1),
            mid: 0.0,
            inventory: 0.0,
            step: 0,
            episode_index: 0,
            episode_reward: 0.0,
            tape: None,
        };
        sim.window = ObservationWindow::new(sim.config.window_frames);
        let _ = sim.reset();
        sim
    }

    /// Drive the same MDP from stored ClickHouse / sim ticks (no second market model).
    pub fn with_tape(config: MicrostructureConfig, tape: TickTape) -> Self {
        let mut sim = Self::new(config);
        sim.tape = Some(tape);
        let _ = sim.reset();
        sim
    }

    pub fn config(&self) -> &MicrostructureConfig {
        &self.config
    }

    pub fn position(&self) -> i8 {
        self.inventory.round().clamp(-1.0, 1.0) as i8
    }

    /// Continuous inventory `q` used by the ladder walk and Gaussian policy.
    pub fn inventory(&self) -> f32 {
        self.inventory
    }

    pub fn mid(&self) -> f32 {
        self.mid
    }

    pub fn observation(&self) -> Vec<f32> {
        self.window.flattened()
    }

    /// Parallel ladder frame (`V × [v, α_ask, α_bid, Δα, q]`). Stream 7-D obs stay separate.
    pub fn ladder_observation(&self) -> Vec<f32> {
        ladder_features(&self.config.ladder_spec(), self.inventory).0
    }

    pub fn ladder_spec(&self) -> DepthLadderSpec {
        self.config.ladder_spec()
    }

    pub fn episode_seed(&self) -> u64 {
        self.rng_state
    }

    /// Start a new episode; returns the initial stream observation.
    pub fn reset(&mut self) -> Vec<f32> {
        self.mid = self.config.initial_mid;
        self.inventory = 0.0;
        self.step = 0;
        self.episode_reward = 0.0;
        if self.config.resample_episode_seeds {
            self.rng_state = self.config.seed.wrapping_add(self.episode_index);
            self.episode_index = self.episode_index.wrapping_add(1);
        } else {
            self.rng_state = self.config.seed;
        }
        self.window = ObservationWindow::new(self.config.window_frames);
        if let Some(tape) = &mut self.tape {
            tape.reset();
        }
        for _ in 0..self.config.window_frames.max(1) {
            self.push_depth_frame();
        }
        self.observation()
    }

    /// Apply a discrete action, advance the latent mid, and return step output.
    ///
    /// Hold keeps the current inventory; Buy/Sell snap to `±1` (WP-022).
    pub fn step(&mut self, action: Action) -> StepResult {
        let target = match action {
            Action::Hold => self.inventory,
            Action::Buy => 1.0,
            Action::Sell => -1.0,
        };
        self.step_target(target)
    }

    /// Walk inventory `q → a`, pay the WP-032 level integral, then mark mid.
    ///
    /// `a` is clamped into `(-1, 1)`. Reward is `q_new · Δs − Δcost`.
    pub fn step_target(&mut self, target: f32) -> StepResult {
        let old_mid = self.mid;
        let old_q = self.inventory;
        let new_q = clamp_inventory_target(target);
        let delta_cost = self.config.ladder_spec().walk_cost(old_q, new_q);
        self.inventory = new_q;
        if self.tape.is_some() {
            self.advance_from_tape();
        } else {
            self.advance_mid();
            self.push_depth_frame();
        }
        let reward = self.inventory * (self.mid - old_mid) - delta_cost;
        self.episode_reward += reward;
        self.step += 1;
        let tape_done = self
            .tape
            .as_ref()
            .is_some_and(|t| t.remaining() == 0);
        let done = self.step >= self.config.episode_steps || tape_done;
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
            final_position: self.position(),
        }
    }

    fn advance_mid(&mut self) {
        self.mid += self.config.mid_drift + self.config.mid_noise * self.next_unit_noise();
    }

    /// Current book as the shared [`TickRow`] (same bag written to ClickHouse).
    pub fn last_tick(&self, session_id: &str, source: &str) -> TickRow {
        TickRow::from_depth_event(
            &self.depth_event(),
            "sim",
            source,
            session_id,
            now_ts_ms() + self.step as i64,
        )
        .expect("sim depth always yields a tick")
    }

    fn advance_from_tape(&mut self) {
        if let Some(tape) = &mut self.tape {
            if let Some(row) = tape.next() {
                self.mid = row.mid as f32;
                if let Some(frame) = row.to_features() {
                    self.window.push(frame);
                    return;
                }
            }
        }
        self.push_depth_frame();
    }

    fn push_depth_frame(&mut self) {
        if let Some(tape) = &mut self.tape {
            if let Some(row) = tape.next() {
                self.mid = row.mid as f32;
                if let Some(frame) = row.to_features() {
                    self.window.push(frame);
                    return;
                }
            }
        }
        let event = self.depth_event();
        if let Some(frame) = features_from_event(&event) {
            self.window.push(frame);
        }
    }

    fn depth_event(&self) -> StreamEvent {
        let spec = self.config.ladder_spec();
        let qty = format!("{:.4}", spec.rung_width());
        let mut bids = Vec::with_capacity(spec.rung_count());
        let mut asks = Vec::with_capacity(spec.rung_count());
        for k in 0..spec.rung_count() {
            let extra = spec.lambda * spec.rung_v(k);
            let bid = self.mid - self.config.half_spread - extra;
            let ask = self.mid + self.config.half_spread + extra;
            bids.push(PriceLevel {
                price: format!("{bid:.4}"),
                qty: qty.clone(),
            });
            asks.push(PriceLevel {
                price: format!("{ask:.4}"),
                qty: qty.clone(),
            });
        }
        StreamEvent::Depth(DepthUpdate {
            symbol: self.config.symbol.clone(),
            bids,
            asks,
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

    #[test]
    fn hold_baseline_zero_reward_on_zero_drift() {
        let config = MicrostructureConfig {
            episode_steps: 64,
            mid_noise: 0.0,
            ..Default::default()
        };
        let stats = run_baseline_episode(&config, 1000, BaselinePolicy::Hold);
        assert_eq!(stats.steps, 64);
        assert_eq!(stats.trades, 0);
        assert!((stats.total_reward).abs() < f32::EPSILON);
    }

    #[test]
    fn tape_replays_stored_mids() {
        let mut gen = MicrostructureSim::new(MicrostructureConfig {
            mid_noise: 0.0,
            mid_drift: 0.5,
            episode_steps: 4,
            ..Default::default()
        });
        gen.reset();
        let mut rows = vec![gen.last_tick("tape-test", "sim")];
        for _ in 0..4 {
            gen.step(Action::Hold);
            rows.push(gen.last_tick("tape-test", "sim"));
        }
        let expected_mid = rows.last().unwrap().mid;
        let mut replayed = MicrostructureSim::with_tape(
            MicrostructureConfig {
                mid_noise: 0.0,
                episode_steps: 8,
                ..Default::default()
            },
            TickTape::new(rows),
        );
        while !replayed.step(Action::Hold).done {}
        assert!((replayed.mid() as f64 - expected_mid).abs() < 1e-3);
    }

    #[test]
    fn oracle_matches_analytic_estimate_without_noise() {
        let config = MicrostructureConfig {
            episode_steps: 128,
            mid_drift: 0.1,
            mid_noise: 0.0,
            trade_cost: 0.5,
            resample_episode_seeds: false,
            ..Default::default()
        };
        let stats = run_baseline_episode(&config, 2000, BaselinePolicy::Oracle);
        let expected = oracle_reward_estimate(&config);
        assert!((stats.total_reward - expected).abs() < 0.05, "{} vs {expected}", stats.total_reward);
        assert_eq!(stats.trades, 1);
        let flat = config.trade_cost + 0.5 * config.lambda;
        assert!((expected - (12.8 - flat)).abs() < 1e-5);
    }

    #[test]
    fn unit_lot_compat_matches_flat_trade_cost() {
        let config = MicrostructureConfig {
            lambda: 0.0,
            trade_cost: 0.5,
            mid_drift: 0.1,
            mid_noise: 0.0,
            episode_steps: 16,
            resample_episode_seeds: false,
            ..Default::default()
        };
        let stats = run_baseline_episode(&config, 7, BaselinePolicy::Oracle);
        let expected = oracle_reward_estimate(&config);
        assert!(
            (stats.total_reward - expected).abs() < 1e-4,
            "oracle {} vs estimate {expected} (steps={} trades={})",
            stats.total_reward,
            stats.steps,
            stats.trades
        );
        assert!((expected - (config.episode_steps as f32 * 0.1 - 0.5)).abs() < 1e-5);
    }

    #[test]
    fn buy_pays_ladder_integral_not_mid() {
        let mut sim = MicrostructureSim::new(MicrostructureConfig {
            mid_drift: 0.0,
            mid_noise: 0.0,
            trade_cost: 0.5,
            lambda: 0.25,
            episode_steps: 4,
            resample_episode_seeds: false,
            ..Default::default()
        });
        sim.reset();
        let result = sim.step(Action::Buy);
        let expected_cost = 0.5 + 0.5 * 0.25;
        assert!(
            (result.reward + expected_cost).abs() < 1e-5,
            "reward {} should be -∫α = -{expected_cost} when Δmid=0",
            result.reward
        );
        assert_eq!(sim.position(), 1);
    }

    #[test]
    fn ladder_observation_is_parallel_to_stream_features() {
        let sim = MicrostructureSim::new(MicrostructureConfig {
            window_frames: 2,
            rung_count: 4,
            rung_width: 0.5,
            lambda: 0.25,
            trade_cost: 0.5,
            resample_episode_seeds: false,
            ..Default::default()
        });
        assert_eq!(sim.observation().len(), FEATURES_PER_FRAME * 2);
        let ladder = sim.ladder_observation();
        assert_eq!(ladder.len(), 4 * LADDER_FEATURES_PER_RUNG);
        assert!((ladder[3] - 0.125).abs() < 1e-6);
        assert!((ladder[4] - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn reset_resamples_mid_path_seeds() {
        let mut sim = MicrostructureSim::new(MicrostructureConfig {
            mid_noise: 1.0,
            mid_drift: 0.0,
            episode_steps: 2,
            resample_episode_seeds: true,
            seed: 11,
            ..Default::default()
        });
        let mut mids = Vec::new();
        for _ in 0..4 {
            sim.reset();
            sim.step(Action::Hold);
            mids.push(sim.mid());
        }
        let unique = mids
            .iter()
            .map(|m| (m * 1e4).round() as i32)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            unique.len() > 1,
            "resampled episodes must not share one mid path: {mids:?}"
        );
    }

    #[test]
    fn step_target_pays_ladder_integral() {
        let mut sim = MicrostructureSim::new(MicrostructureConfig {
            mid_drift: 0.0,
            mid_noise: 0.0,
            trade_cost: 0.5,
            lambda: 0.25,
            episode_steps: 4,
            resample_episode_seeds: false,
            ..Default::default()
        });
        sim.reset();
        let target = 0.4;
        let expected = sim.ladder_spec().walk_cost(0.0, target);
        let result = sim.step_target(target);
        assert!((sim.inventory() - target).abs() < 1e-6);
        assert!(
            (result.reward + expected).abs() < 1e-5,
            "reward {} should be -∫α = -{expected}",
            result.reward
        );
    }

    #[test]
    fn hold_target_zero_keeps_flat_inventory() {
        let config = MicrostructureConfig {
            episode_steps: 8,
            mid_noise: 0.0,
            lambda: 0.25,
            resample_episode_seeds: false,
            ..Default::default()
        };
        let stats = run_episode_with_targets(&config, 3, |_, _| 0.0);
        assert_eq!(stats.trades, 0);
        assert!(stats.mean_abs_inventory.abs() < 1e-6);
        assert!(stats.total_reward.abs() < 1e-6);
    }

    #[test]
    fn resampled_tick_rows_use_several_seeds() {
        let config = MicrostructureConfig {
            mid_noise: 0.5,
            episode_steps: 4,
            resample_episode_seeds: true,
            ..Default::default()
        };
        let rows = generate_resampled_tick_rows(&config, "ladder-test", 3, 4);
        assert!(rows.len() >= 15);
        let mids: std::collections::BTreeSet<i64> = rows
            .iter()
            .map(|r| (r.mid * 1e4).round() as i64)
            .collect();
        assert!(
            mids.len() > 2,
            "expected several mid values from resampled seeds, got {mids:?}"
        );
    }
}
