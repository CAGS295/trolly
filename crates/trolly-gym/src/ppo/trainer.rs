//! PPO and WoLF-PPO training step with dual learning-rate selection.

use tch::{nn, nn::OptimizerConfig, Device};

use super::actor_critic::ActorCritic;
use super::batch::RolloutBatch;
use super::config::{PpoConfig, WolfPpoConfig};
use super::loss::ppo_loss;

/// Scalar metrics from one policy update.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateMetrics {
    pub total_loss: f64,
    pub clip_loss: f64,
    pub value_loss: f64,
    pub entropy: f64,
    pub active_learning_rate: f64,
    pub estimated_nes_payoff: f64,
    pub current_mean_return: f64,
    pub wolf_winning: bool,
}

/// PPO trainer (fixed learning rate).
pub struct PpoTrainer {
    pub actor_critic: ActorCritic,
    vs: nn::VarStore,
    config: PpoConfig,
    device: Device,
}

impl PpoTrainer {
    pub fn new(obs_dim: i64, num_actions: i64, config: PpoConfig, device: Device) -> Self {
        let vs = nn::VarStore::new(device);
        let actor_critic = ActorCritic::new(&vs.root(), obs_dim, num_actions, &config.hidden_layers);
        Self {
            actor_critic,
            vs,
            config,
            device,
        }
    }

    pub fn config(&self) -> &PpoConfig {
        &self.config
    }

    pub fn device(&self) -> Device {
        self.device
    }

    pub fn var_store(&self) -> &nn::VarStore {
        &self.vs
    }

    /// Run multi-epoch PPO update on a rollout batch.
    pub fn policy_update(&mut self, batch: &RolloutBatch) -> UpdateMetrics {
        let lr = self.config.learning_rate;
        self.run_update(batch, lr, batch.mean_return, batch.mean_return, false)
    }

    fn run_update(
        &mut self,
        batch: &RolloutBatch,
        learning_rate: f64,
        estimated_nes_payoff: f64,
        current_mean_return: f64,
        wolf_winning: bool,
    ) -> UpdateMetrics {
        let mut opt = if self.config.use_adam {
            nn::Adam::default().build(&self.vs, learning_rate).unwrap()
        } else {
            nn::Sgd::default().build(&self.vs, learning_rate).unwrap()
        };

        let mut last = UpdateMetrics {
            total_loss: 0.0,
            clip_loss: 0.0,
            value_loss: 0.0,
            entropy: 0.0,
            active_learning_rate: learning_rate,
            estimated_nes_payoff,
            current_mean_return,
            wolf_winning,
        };

        for _ in 0..self.config.ppo_epochs {
            let (new_log_probs, entropy, values) =
                self.actor_critic
                    .evaluate_actions(&batch.observations, &batch.actions);
            let (total, clip_l, vf_l, ent) = ppo_loss(
                &new_log_probs,
                &batch.old_log_probs,
                &batch.advantages,
                &values,
                &batch.returns,
                &entropy,
                self.config.clip_epsilon,
                self.config.value_coef,
                self.config.entropy_coef,
            );
            opt.zero_grad();
            total.backward();
            opt.step();

            last = UpdateMetrics {
                total_loss: total.double_value(&[]),
                clip_loss: clip_l.double_value(&[]),
                value_loss: vf_l.double_value(&[]),
                entropy: ent.double_value(&[]),
                active_learning_rate: learning_rate,
                estimated_nes_payoff,
                current_mean_return,
                wolf_winning,
            };
        }
        last
    }
}

/// WoLF-PPO trainer: rolling average payoff + dual learning rates.
pub struct WolfPpoTrainer {
    inner: PpoTrainer,
    wolf_config: WolfPpoConfig,
    /// Estimated NES payoff (rolling average of batch mean returns).
    estimated_nes_payoff: Option<f64>,
    update_count: u64,
}

impl WolfPpoTrainer {
    pub fn new(obs_dim: i64, num_actions: i64, config: WolfPpoConfig, device: Device) -> Self {
        let inner = PpoTrainer::new(obs_dim, num_actions, config.ppo.clone(), device);
        Self {
            inner,
            wolf_config: config,
            estimated_nes_payoff: None,
            update_count: 0,
        }
    }

    pub fn config(&self) -> &WolfPpoConfig {
        &self.wolf_config
    }

    pub fn estimated_nes_payoff(&self) -> Option<f64> {
        self.estimated_nes_payoff
    }

    /// Seed the estimated NES payoff (useful for tests and warm-start).
    pub fn set_estimated_nes_payoff(&mut self, payoff: f64) {
        self.estimated_nes_payoff = Some(payoff);
    }

    pub fn actor_critic(&self) -> &ActorCritic {
        &self.inner.actor_critic
    }

    pub fn actor_critic_mut(&mut self) -> &mut ActorCritic {
        &mut self.inner.actor_critic
    }

    pub fn var_store(&self) -> &nn::VarStore {
        self.inner.var_store()
    }

    /// Select α_WIN when current payoff exceeds estimated NES payoff, else α_LOSE.
    pub fn select_learning_rate(&self, current_mean_return: f64) -> (f64, bool) {
        match self.estimated_nes_payoff {
            None => (self.wolf_config.alpha_lose, false),
            Some(estimate) if current_mean_return > estimate => {
                (self.wolf_config.alpha_win(), true)
            }
            Some(_) => (self.wolf_config.alpha_lose, false),
        }
    }

    fn update_payoff_estimate(&mut self, batch_mean: f64) {
        match self.estimated_nes_payoff {
            None => self.estimated_nes_payoff = Some(batch_mean),
            Some(prev) => {
                let decay = self.wolf_config.payoff_ema_decay;
                if decay <= 0.0 {
                    // Arithmetic running mean.
                    let n = (self.update_count + 1) as f64;
                    self.estimated_nes_payoff = Some(prev + (batch_mean - prev) / n);
                } else {
                    self.estimated_nes_payoff = Some(decay * prev + (1.0 - decay) * batch_mean);
                }
            }
        }
        self.update_count += 1;
    }

    /// WoLF-PPO policy update: pick learning rate, run PPO epochs, refresh payoff estimate.
    pub fn policy_update(&mut self, batch: &RolloutBatch) -> UpdateMetrics {
        let (lr, winning) = self.select_learning_rate(batch.mean_return);
        let estimate = self.estimated_nes_payoff.unwrap_or(batch.mean_return);
        let metrics = self.inner.run_update(
            batch,
            lr,
            estimate,
            batch.mean_return,
            winning,
        );
        self.update_payoff_estimate(batch.mean_return);
        metrics
    }
}

/// Convenience: build normalized advantages from rewards (simple Monte Carlo returns).
pub fn compute_advantages(rewards: &[f64], values: &[f64], gamma: f64) -> (Vec<f64>, Vec<f64>) {
    let n = rewards.len();
    let mut returns = vec![0.0; n];
    let mut g = 0.0;
    for i in (0..n).rev() {
        g = rewards[i] + gamma * g;
        returns[i] = g;
    }
    let advantages: Vec<f64> = returns
        .iter()
        .zip(values.iter())
        .map(|(r, v)| r - v)
        .collect();
    (advantages, returns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::actor_critic::cpu_device;
    use tch::{Kind, Tensor};

    fn synthetic_batch(device: Device, mean_return: f64) -> RolloutBatch {
        RolloutBatch {
            observations: Tensor::zeros(&[4, 2], (Kind::Float, device)),
            actions: Tensor::from_slice(&[0_i64, 1, 0, 1]).to_kind(Kind::Int64).to_device(device),
            old_log_probs: Tensor::from_slice(&[-0.5_f64, -0.6, -0.4, -0.7]).to_device(device),
            advantages: Tensor::from_slice(&[0.1_f64, -0.2, 0.05, 0.0]).to_device(device),
            returns: Tensor::from_slice(&[0.5_f64, 0.3, 0.4, 0.2]).to_device(device),
            old_values: Tensor::from_slice(&[0.4_f64, 0.5, 0.35, 0.2]).to_device(device),
            mean_return,
        }
    }

    #[test]
    fn wolf_selects_winning_lr_when_above_estimate() {
        let config = WolfPpoConfig::default();
        let mut trainer = WolfPpoTrainer::new(2, 2, config, cpu_device());
        trainer.set_estimated_nes_payoff(0.0);
        let (lr, winning) = trainer.select_learning_rate(1.0);
        assert!(winning);
        assert!((lr - 0.0025).abs() < 1e-9); // 0.01 / 4
    }

    #[test]
    fn wolf_selects_losing_lr_when_below_estimate() {
        let config = WolfPpoConfig::default();
        let mut trainer = WolfPpoTrainer::new(2, 2, config, cpu_device());
        trainer.estimated_nes_payoff = Some(1.0);
        let (lr, winning) = trainer.select_learning_rate(0.0);
        assert!(!winning);
        assert!((lr - 0.01).abs() < 1e-9);
    }

    #[test]
    fn policy_update_produces_finite_losses() {
        let device = cpu_device();
        let config = WolfPpoConfig {
            ppo: PpoConfig {
                ppo_epochs: 2,
                learning_rate: 0.01,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut trainer = WolfPpoTrainer::new(2, 2, config, device);
        let batch = synthetic_batch(device, 0.35);
        let metrics = trainer.policy_update(&batch);
        assert!(metrics.total_loss.is_finite());
        assert!(metrics.clip_loss.is_finite());
        assert!(metrics.value_loss.is_finite());
        assert!(metrics.entropy.is_finite());
        assert!(trainer.estimated_nes_payoff.is_some());
    }

    #[test]
    fn wolf_lr_switches_after_payoff_estimate_updates() {
        let device = cpu_device();
        let config = WolfPpoConfig {
            ppo: PpoConfig {
                ppo_epochs: 1,
                ..Default::default()
            },
            alpha_lose: 0.04,
            payoff_ema_decay: 0.0, // running mean
        };
        let mut trainer = WolfPpoTrainer::new(2, 2, config, device);

        let low = synthetic_batch(device, -1.0);
        let m1 = trainer.policy_update(&low);
        assert!(!m1.wolf_winning);
        assert!((m1.active_learning_rate - 0.04).abs() < 1e-9);

        let high = synthetic_batch(device, 1.0);
        let m2 = trainer.policy_update(&high);
        assert!(m2.wolf_winning);
        assert!((m2.active_learning_rate - 0.01).abs() < 1e-9);
    }
}
