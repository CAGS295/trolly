//! PPO / WoLF-PPO policy update step.

use tch::{Kind, Tensor};

use super::actor_critic::ActorCritic;
use super::config::{PpoConfig, WolfPpoConfig};
use super::loss::{ppo_losses, PpoLossBreakdown};

/// On-policy rollout batch for a single PPO update.
#[derive(Debug)]
pub struct RolloutBatch {
    pub observations: Tensor,
    pub actions: Tensor,
    pub old_log_probs: Tensor,
    pub advantages: Tensor,
    pub returns: Tensor,
}

impl RolloutBatch {
    pub fn new(
        observations: Tensor,
        actions: Tensor,
        old_log_probs: Tensor,
        advantages: Tensor,
        returns: Tensor,
    ) -> Self {
        Self {
            observations,
            actions,
            old_log_probs,
            advantages,
            returns,
        }
    }
}

/// PPO trainer operating on an [`ActorCritic`] network.
pub struct PpoTrainer {
    pub config: PpoConfig,
}

impl PpoTrainer {
    pub fn new(config: PpoConfig) -> Self {
        Self { config }
    }

    /// Run multi-epoch PPO updates; returns mean losses over epochs.
    pub fn policy_update(
        &self,
        model: &ActorCritic,
        opt: &mut tch::nn::Optimizer,
        batch: &RolloutBatch,
    ) -> PpoLossBreakdown {
        let mut totals = PpoLossBreakdown::default();
        let epochs = self.config.ppo_epochs.max(1) as f64;

        for _ in 0..self.config.ppo_epochs {
            let (log_probs, values, entropy) =
                model.evaluate_actions(&batch.observations, &batch.actions);
            let losses = ppo_losses(
                &log_probs,
                &batch.old_log_probs,
                &batch.advantages,
                &values,
                &batch.returns,
                &entropy,
                &self.config,
            );
            opt.zero_grad();
            losses.total.backward();
            opt.step();
            totals.accumulate(&losses.breakdown);
        }

        totals.scale(1.0 / epochs);
        totals
    }
}

/// WoLF-PPO trainer: selects α_WIN vs α_LOSE from payoff vs rolling NES estimate.
pub struct WolfPpoTrainer {
    pub config: WolfPpoConfig,
    /// Rolling average of expected payoff (NES estimate).
    pub nes_payoff_estimate: f64,
    /// Learning rate used in the most recent update.
    pub last_learning_rate: f64,
}

impl WolfPpoTrainer {
    pub fn new(config: WolfPpoConfig) -> Self {
        Self {
            config,
            nes_payoff_estimate: 0.0,
            last_learning_rate: config.alpha_lose,
        }
    }

    /// Update rolling NES payoff estimate with the current expected payoff.
    pub fn observe_payoff(&mut self, expected_payoff: f64) {
        let decay = self.config.nes_estimate_decay;
        self.nes_payoff_estimate =
            decay * self.nes_payoff_estimate + (1.0 - decay) * expected_payoff;
    }

    /// Select WoLF learning rate: α_WIN when payoff exceeds estimate, else α_LOSE.
    pub fn select_learning_rate(&self, current_payoff: f64) -> f64 {
        if current_payoff > self.nes_payoff_estimate {
            self.config.alpha_win()
        } else {
            self.config.alpha_lose
        }
    }

    /// Run WoLF-PPO update with dynamic learning rate.
    pub fn policy_update(
        &mut self,
        model: &ActorCritic,
        opt: &mut tch::nn::Optimizer,
        batch: &RolloutBatch,
        current_payoff: f64,
    ) -> PpoLossBreakdown {
        self.observe_payoff(current_payoff);
        let lr = self.select_learning_rate(current_payoff);
        self.last_learning_rate = lr;
        opt.set_lr(lr);

        let inner = PpoTrainer::new(self.config.ppo);
        inner.policy_update(model, opt, batch)
    }
}

/// Compute GAE-style advantages (simple Monte Carlo returns when λ=1, γ=1).
pub fn compute_advantages(rewards: &[f64], values: &[f64], gamma: f64, gae_lambda: f64) -> (Vec<f64>, Vec<f64>) {
    let n = rewards.len();
    let mut advantages = vec![0.0; n];
    let mut returns = vec![0.0; n];
    let mut gae = 0.0;
    let mut next_value = 0.0;

    for t in (0..n).rev() {
        let delta = rewards[t] + gamma * next_value - values[t];
        gae = delta + gamma * gae_lambda * gae;
        advantages[t] = gae;
        returns[t] = advantages[t] + values[t];
        next_value = values[t];
    }

    (advantages, returns)
}

pub fn tensor_from_vec_f64(data: &[f64]) -> Tensor {
    Tensor::from_slice(data).to_kind(Kind::Float)
}

pub fn tensor_from_vec_i64(data: &[i64]) -> Tensor {
    Tensor::from_slice(data)
}
