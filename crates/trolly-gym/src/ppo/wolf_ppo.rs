//! WoLF-PPO: Win-or-Learn-Fast Policy Proximal Optimization.
//!
//! Extends PPO with dual learning rates driven by a rolling-average payoff
//! estimate (NES payoff proxy). Reference: Ratcliffe et al., IEEE CoG 2019.
//!
//! # Dual-rate rule
//!
//! Let `V̄` be the rolling average return and `V` the current episode return.
//!
//! - **Winning** (`V > V̄`): use `α_WIN` (small — already near equilibrium).
//! - **Losing** (`V ≤ V̄`): use `α_LOSE` (large — need to adapt quickly).
//!
//! Ratio constraint enforced by [`WolfPpoConfig`]: `α_WIN = α_LOSE / 4`.

use std::collections::VecDeque;

use super::config::WolfPpoConfig;
use super::ppo::{PpoTrainer, RolloutBatch};

/// WoLF-PPO trainer with dual learning-rate selection.
pub struct WolfPpoTrainer {
    /// Inner PPO trainer (owns VarStore, ActorCritic, Optimizer).
    pub inner: PpoTrainer,
    config: WolfPpoConfig,
    /// Sliding window of episode returns for NES payoff estimate.
    payoff_history: VecDeque<f64>,
    /// Exponential rolling average of the payoff window.
    rolling_avg_payoff: f64,
    /// Most recent episode return.
    current_payoff: f64,
}

impl WolfPpoTrainer {
    pub fn new(obs_dim: i64, num_actions: i64, config: WolfPpoConfig) -> Self {
        let inner = PpoTrainer::new(obs_dim, num_actions, config.ppo.clone());
        Self {
            inner,
            config,
            payoff_history: VecDeque::new(),
            rolling_avg_payoff: 0.0,
            current_payoff: 0.0,
        }
    }

    /// Rolling average payoff — the NES payoff estimate `V̄`.
    pub fn rolling_avg_payoff(&self) -> f64 {
        self.rolling_avg_payoff
    }

    /// Most recent episode payoff `V`.
    pub fn current_payoff(&self) -> f64 {
        self.current_payoff
    }

    /// `true` when the agent is in the winning regime (`V > V̄`).
    pub fn is_winning(&self) -> bool {
        self.current_payoff > self.rolling_avg_payoff
    }

    /// Learning rate that will be applied on the next update.
    pub fn active_lr(&self) -> f64 {
        if self.is_winning() {
            self.config.alpha_win
        } else {
            self.config.alpha_lose
        }
    }

    fn update_payoff_estimate(&mut self, episode_return: f64) {
        self.current_payoff = episode_return;
        self.payoff_history.push_back(episode_return);
        if self.payoff_history.len() > self.config.payoff_window {
            self.payoff_history.pop_front();
        }
        let n = self.payoff_history.len() as f64;
        self.rolling_avg_payoff = self.payoff_history.iter().sum::<f64>() / n;
    }

    /// WoLF-PPO policy update.
    ///
    /// 1. Updates rolling payoff estimate with `episode_return`.
    /// 2. Selects `α_WIN` or `α_LOSE` based on win/lose condition.
    /// 3. Runs PPO gradient epochs at selected rate.
    ///
    /// Returns mean combined loss across PPO epochs.
    pub fn policy_update(&mut self, batch: &RolloutBatch, episode_return: f64) -> f64 {
        self.update_payoff_estimate(episode_return);
        let lr = self.active_lr();
        self.inner.set_lr(lr);
        self.inner.policy_update(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::config::WolfPpoConfig;
    use crate::ppo::ppo::RolloutBatch;
    use tch::{Kind, Tensor};

    fn synthetic_batch(batch_size: i64, obs_dim: i64) -> RolloutBatch {
        RolloutBatch {
            observations: Tensor::randn(&[batch_size, obs_dim], (Kind::Float, tch::Device::Cpu)),
            actions: Tensor::zeros(&[batch_size], (Kind::Int64, tch::Device::Cpu)),
            old_log_probs: Tensor::full(
                &[batch_size],
                -1.0_f64,
                (Kind::Float, tch::Device::Cpu),
            ),
            returns: Tensor::randn(&[batch_size], (Kind::Float, tch::Device::Cpu)),
            advantages: Tensor::randn(&[batch_size], (Kind::Float, tch::Device::Cpu)),
        }
    }

    #[test]
    fn wolf_config_alpha_ratio() {
        let config = WolfPpoConfig::default();
        let ratio = config.alpha_lose / config.alpha_win;
        assert!(
            (ratio - 4.0).abs() < 1e-12,
            "α_WIN must be α_LOSE / 4, got ratio {ratio}"
        );
    }

    #[test]
    fn wolf_config_with_alpha_lose() {
        let config = WolfPpoConfig::default().with_alpha_lose(0.08);
        assert!((config.alpha_lose - 0.08).abs() < 1e-12);
        assert!((config.alpha_win - 0.02).abs() < 1e-12);
    }

    #[test]
    fn wolf_lr_switches_on_payoff_vs_estimate() {
        let config = WolfPpoConfig::default();
        let alpha_win = config.alpha_win;
        let alpha_lose = config.alpha_lose;
        let mut trainer = WolfPpoTrainer::new(4, 3, config);
        let batch = synthetic_batch(8, 4);

        // First episode: history = [1.0], rolling = 1.0, current = 1.0
        // current (1.0) > rolling (1.0) is false → losing
        trainer.policy_update(&batch, 1.0);
        assert!(
            !trainer.is_winning(),
            "should be losing when current == rolling"
        );
        assert_eq!(trainer.active_lr(), alpha_lose);

        // High-return episode: history = [1.0, 10.0], rolling = 5.5
        // current (10.0) > rolling (5.5) → winning
        trainer.policy_update(&batch, 10.0);
        assert!(trainer.is_winning(), "should be winning after high return");
        assert_eq!(trainer.active_lr(), alpha_win);

        // Low-return episode: history = [1.0, 10.0, 0.0], rolling ≈ 3.67
        // current (0.0) > rolling (3.67) is false → losing
        trainer.policy_update(&batch, 0.0);
        assert!(!trainer.is_winning(), "should be losing after low return");
        assert_eq!(trainer.active_lr(), alpha_lose);
    }

    #[test]
    fn wolf_loss_finite_after_updates() {
        let mut trainer = WolfPpoTrainer::new(4, 3, WolfPpoConfig::default());
        let batch = synthetic_batch(16, 4);
        for ret in [1.0, 0.5, 2.0, 0.1, 3.0] {
            let loss = trainer.policy_update(&batch, ret);
            assert!(
                loss.is_finite(),
                "WoLF-PPO loss not finite (return={ret}): {loss}"
            );
        }
    }
}
