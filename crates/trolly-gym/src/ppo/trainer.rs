//! PPO and WoLF-PPO trainers wrapping actor–critic updates.

use tch::nn::{self, OptimizerConfig};
use tch::Device;

use super::actor_critic::ActorCritic;
use super::batch::RolloutBatch;
use super::config::{OptimizerKind, PpoConfig, WolfPpoConfig};
use super::loss::{ppo_loss, PpoLossBreakdown};

/// Standard PPO trainer with clipped surrogate updates.
pub struct PpoTrainer {
    vs: nn::VarStore,
    actor_critic: ActorCritic,
    opt: nn::Optimizer,
    config: PpoConfig,
}

impl PpoTrainer {
    /// Create a CPU-backed trainer with freshly initialized weights.
    pub fn new(config: PpoConfig) -> Self {
        Self::new_on_device(config, Device::Cpu)
    }

    /// Create a trainer on the given device (CPU for unit tests; CUDA when available).
    pub fn new_on_device(config: PpoConfig, device: Device) -> Self {
        let vs = nn::VarStore::new(device);
        let actor_critic = ActorCritic::new(vs.root(), &config);
        let opt = build_optimizer(&config, &vs);
        Self {
            vs,
            actor_critic,
            opt,
            config,
        }
    }

    /// Read-only access to hyperparameters.
    pub fn config(&self) -> &PpoConfig {
        &self.config
    }

    /// Borrow the actor–critic for inference / rollout collection.
    pub fn actor_critic(&self) -> &ActorCritic {
        &self.actor_critic
    }

    /// Variable store backing the actor–critic (for save/load in WP-020).
    pub fn var_store(&self) -> &nn::VarStore {
        &self.vs
    }

    /// Mutable variable store (checkpoint load).
    pub fn var_store_mut(&mut self) -> &mut nn::VarStore {
        &mut self.vs
    }

    /// Set optimizer learning rate (used by WoLF-PPO dual-rate schedule).
    pub fn set_learning_rate(&mut self, lr: f64) {
        self.opt.set_lr(lr);
    }

    /// Run PPO epochs over `batch`, returning the last epoch's loss breakdown.
    pub fn policy_update(&mut self, batch: &RolloutBatch) -> PpoLossBreakdown {
        let mut last = PpoLossBreakdown {
            clip_loss: 0.0,
            value_loss: 0.0,
            entropy: 0.0,
            total: 0.0,
        };

        for _ in 0..self.config.ppo_epochs {
            self.opt.zero_grad();
            let (values, logits) = self.actor_critic.forward(&batch.observations);
            let (loss, breakdown) = ppo_loss(
                &self.config,
                &values,
                &logits,
                &batch.actions,
                &batch.old_log_probs,
                &batch.returns,
                &batch.advantages,
            );
            loss.backward();
            self.opt.step();
            last = breakdown;
        }

        last
    }
}

/// WoLF-PPO trainer: rolling average payoff as NES estimate and dual learning rates.
pub struct WolfPpoTrainer {
    inner: PpoTrainer,
    config: WolfPpoConfig,
    /// Rolling mean payoff (estimated NES payoff).
    estimated_nes_payoff: f64,
    payoff_updates: u64,
}

impl WolfPpoTrainer {
    pub fn new(config: WolfPpoConfig) -> Self {
        let mut ppo = config.ppo.clone();
        ppo.learning_rate = config.alpha_lose;
        Self {
            inner: PpoTrainer::new(ppo),
            config,
            estimated_nes_payoff: 0.0,
            payoff_updates: 0,
        }
    }

    pub fn config(&self) -> &WolfPpoConfig {
        &self.config
    }

    pub fn actor_critic(&self) -> &ActorCritic {
        self.inner.actor_critic()
    }

    pub fn var_store(&self) -> &nn::VarStore {
        self.inner.var_store()
    }

    pub fn var_store_mut(&mut self) -> &mut nn::VarStore {
        self.inner.var_store_mut()
    }

    /// Rolling average payoff used as the estimated NES payoff (WoLF-PHC analogue).
    pub fn estimated_nes_payoff(&self) -> f64 {
        self.estimated_nes_payoff
    }

    /// Dual-rate selector: α_WIN when winning, α_LOSE otherwise.
    pub fn select_learning_rate(&self, current_payoff: f64) -> f64 {
        self.config
            .select_learning_rate(current_payoff, self.estimated_nes_payoff)
    }

    /// Update rolling NES payoff estimate with the batch mean reward.
    pub fn record_payoff(&mut self, batch: &RolloutBatch) {
        let batch_mean = batch.mean_reward();
        self.payoff_updates += 1;
        let n = self.payoff_updates as f64;
        self.estimated_nes_payoff += (batch_mean - self.estimated_nes_payoff) / n;
    }

    /// WoLF-PPO policy update: pick LR from payoff vs estimate, then run PPO epochs.
    ///
    /// Returns `(loss_breakdown, learning_rate_used)`.
    pub fn policy_update(&mut self, batch: &RolloutBatch) -> (PpoLossBreakdown, f64) {
        let current_payoff = batch.mean_reward();
        let lr = self.select_learning_rate(current_payoff);
        self.inner.set_learning_rate(lr);
        let breakdown = self.inner.policy_update(batch);
        self.record_payoff(batch);
        (breakdown, lr)
    }
}

fn build_optimizer(config: &PpoConfig, vs: &nn::VarStore) -> nn::Optimizer {
    match config.optimizer {
        OptimizerKind::Sgd => nn::Sgd::default()
            .build(vs, config.learning_rate)
            .expect("SGD optimizer"),
        OptimizerKind::Adam => nn::Adam::default()
            .build(vs, config.learning_rate)
            .expect("Adam optimizer"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::config::PpoConfig;
    use tch::{Kind, Tensor};

    fn synthetic_batch(config: &PpoConfig, batch: i64, mean_reward: f64) -> RolloutBatch {
        let device = Device::Cpu;
        RolloutBatch {
            observations: Tensor::randn([batch, config.obs_dim], (Kind::Float, device)),
            actions: Tensor::zeros([batch], (Kind::Int64, device)),
            old_log_probs: Tensor::zeros([batch], (Kind::Float, device)),
            returns: Tensor::ones([batch], (Kind::Float, device)),
            advantages: Tensor::ones([batch], (Kind::Float, device)),
            rewards: Tensor::full([batch], mean_reward, (Kind::Float, device)),
        }
    }

    #[test]
    fn policy_update_produces_finite_loss() {
        let config = PpoConfig::default();
        let mut trainer = PpoTrainer::new(config.clone());
        let batch = synthetic_batch(&config, 16, 0.5);
        let breakdown = trainer.policy_update(&batch);
        assert!(breakdown.total.is_finite());
    }

    #[test]
    fn wolf_lr_switches_on_payoff_vs_estimate() {
        let config = WolfPpoConfig {
            alpha_lose: 0.04,
            win_lose_ratio: 4.0,
            ..Default::default()
        };
        let trainer = WolfPpoTrainer::new(config.clone());

        // Before any updates the estimate is 0 — positive payoff selects α_WIN.
        assert!((trainer.select_learning_rate(1.0) - config.alpha_win()).abs() < 1e-9);

        let mut trainer = WolfPpoTrainer::new(config.clone());
        let low_batch = synthetic_batch(&config.ppo, 8, -1.0);
        trainer.record_payoff(&low_batch);
        assert!(trainer.estimated_nes_payoff() < 0.0);

        // Current payoff above estimate → α_WIN.
        assert!((trainer.select_learning_rate(0.0) - config.alpha_win()).abs() < 1e-9);
        // Current payoff below estimate → α_LOSE.
        assert!((trainer.select_learning_rate(-2.0) - config.alpha_lose).abs() < 1e-9);
    }

    #[test]
    fn wolf_policy_update_applies_selected_lr() {
        let config = WolfPpoConfig::default();
        let mut trainer = WolfPpoTrainer::new(config.clone());
        let batch = synthetic_batch(&config.ppo, 8, 1.0);
        let (breakdown, lr) = trainer.policy_update(&batch);
        assert!((lr - config.alpha_win()).abs() < 1e-9);
        assert!(breakdown.total.is_finite());
        assert!(breakdown.clip_loss.is_finite());
        assert!(breakdown.entropy.is_finite());
    }
}
