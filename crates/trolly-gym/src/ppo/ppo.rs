//! PPO clipped surrogate objective (Schulman et al., 2017).
//!
//! Implements:
//! ```text
//! L^CLIP(θ) = E[min(r_t(θ)·Â_t, clip(r_t(θ), 1−ε, 1+ε)·Â_t)]
//! L^VF(θ)  = E[(V_θ(s_t) − R_t)²]
//! L(θ)     = L^CLIP − c1·L^VF + c2·S[π_θ](s_t)
//! ```

use tch::{nn, nn::OptimizerConfig, Kind, Tensor};

use super::actor_critic::ActorCritic;
use super::config::PpoConfig;

/// Rollout batch collected under the behaviour policy.
pub struct RolloutBatch {
    /// Observations tensor `[T, obs_dim]`.
    pub observations: Tensor,
    /// Action indices `[T]` (i64).
    pub actions: Tensor,
    /// Log-probabilities under the behaviour policy `[T]`.
    pub old_log_probs: Tensor,
    /// Discounted returns (targets for value head) `[T]`.
    pub returns: Tensor,
    /// Advantage estimates `[T]` (e.g. GAE or Monte Carlo).
    pub advantages: Tensor,
}

/// PPO trainer with clipped surrogate, value, and entropy losses.
pub struct PpoTrainer {
    pub vs: nn::VarStore,
    pub actor_critic: ActorCritic,
    pub config: PpoConfig,
    pub optimizer: nn::Optimizer,
}

impl PpoTrainer {
    /// Construct on CPU with the given observation and action dimensions.
    pub fn new(obs_dim: i64, num_actions: i64, config: PpoConfig) -> Self {
        let vs = nn::VarStore::new(tch::Device::Cpu);
        let actor_critic = ActorCritic::new(&vs, obs_dim, num_actions, &config);
        let optimizer = build_optimizer(&vs, &config);
        Self {
            vs,
            actor_critic,
            config,
            optimizer,
        }
    }

    /// Override the optimizer learning rate (used by WoLF-PPO dual-rate selection).
    pub fn set_lr(&mut self, lr: f64) {
        self.optimizer.set_lr(lr);
    }

    /// Run `ppo_epochs` gradient updates on `batch`.
    ///
    /// Returns the mean combined loss (L^CLIP − c1·L^VF + c2·S) across epochs.
    pub fn policy_update(&mut self, batch: &RolloutBatch) -> f64 {
        let mut total_loss = 0.0_f64;
        for _ in 0..self.config.ppo_epochs {
            let (log_probs, entropy) = self
                .actor_critic
                .evaluate_actions(&batch.observations, &batch.actions);
            let (_, values) = self.actor_critic.forward(&batch.observations);

            // Importance ratio r_t(θ) = π_θ(a|s) / π_old(a|s)
            let ratio = (&log_probs - &batch.old_log_probs).exp();

            // Clipped surrogate
            let surr1 = &ratio * &batch.advantages;
            let surr2 = ratio.clamp(
                1.0 - self.config.clip_epsilon,
                1.0 + self.config.clip_epsilon,
            ) * &batch.advantages;
            let policy_loss = -surr1.min_other(&surr2).mean(Kind::Float);

            // Value MSE loss
            let value_loss = (&values - &batch.returns)
                .pow_tensor_scalar(2)
                .mean(Kind::Float);

            // Entropy bonus (mean over batch)
            let entropy_bonus = entropy.mean(Kind::Float);

            // Combined loss: L^CLIP − c1·L^VF + c2·S
            let loss = &policy_loss + self.config.value_coef * &value_loss
                - self.config.entropy_coef * &entropy_bonus;

            self.optimizer.backward_step(&loss);
            total_loss += f64::try_from(loss.detach()).unwrap_or(f64::NAN);
        }
        total_loss / self.config.ppo_epochs as f64
    }
}

fn build_optimizer(vs: &nn::VarStore, config: &PpoConfig) -> nn::Optimizer {
    if config.use_adam {
        nn::Adam::default()
            .build(vs, config.lr)
            .expect("build Adam optimizer")
    } else {
        nn::Sgd::default()
            .build(vs, config.lr)
            .expect("build SGD optimizer")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn ppo_loss_no_nan_sgd() {
        let mut trainer = PpoTrainer::new(4, 3, PpoConfig::default());
        let batch = synthetic_batch(16, 4);
        let loss = trainer.policy_update(&batch);
        assert!(loss.is_finite(), "PPO SGD loss is not finite: {loss}");
    }

    #[test]
    fn ppo_loss_no_nan_adam() {
        let config = PpoConfig {
            use_adam: true,
            ..Default::default()
        };
        let mut trainer = PpoTrainer::new(4, 3, config);
        let batch = synthetic_batch(16, 4);
        let loss = trainer.policy_update(&batch);
        assert!(loss.is_finite(), "PPO Adam loss is not finite: {loss}");
    }

    #[test]
    fn set_lr_does_not_panic() {
        let mut trainer = PpoTrainer::new(4, 3, PpoConfig::default());
        trainer.set_lr(0.001);
        let batch = synthetic_batch(8, 4);
        let loss = trainer.policy_update(&batch);
        assert!(loss.is_finite());
    }
}
