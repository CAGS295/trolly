//! PPO and WoLF-PPO policy update trainers.

use tch::{nn, nn::OptimizerConfig, Device, Kind, Tensor};

use super::actor_critic::ActorCritic;
use super::config::{OptimizerKind, PpoConfig, WolfPpoConfig};
use super::loss::{ppo_loss, PpoLossBreakdown};
use super::rollout::RolloutBatch;
use super::wolf::{NesPayoffEstimate, WolfLearningRate};

/// Result of one `policy_update` call.
#[derive(Debug, Clone)]
pub struct PolicyUpdateMetrics {
    pub epochs: usize,
    pub last_breakdown: PpoLossBreakdown,
    pub learning_rate: f64,
}

/// Vanilla PPO trainer: multi-epoch clipped surrogate updates.
pub struct PpoTrainer {
    pub actor_critic: ActorCritic,
    vs: nn::VarStore,
    config: PpoConfig,
    opt: nn::Optimizer,
}

impl PpoTrainer {
    /// Create a trainer with a fresh actor–critic network on CPU.
    pub fn new(obs_dim: i64, n_actions: i64, config: PpoConfig) -> Self {
        let vs = nn::VarStore::new(Device::Cpu);
        let actor_critic = ActorCritic::new(
            vs.root(),
            obs_dim,
            n_actions,
            &config.hidden_layers,
        );
        let opt = build_optimizer(&config, &vs, config.learning_rate);
        Self {
            actor_critic,
            vs,
            config,
            opt,
        }
    }

    /// Set optimizer learning rate (used by WoLF-PPO).
    pub fn set_learning_rate(&mut self, lr: f64) {
        self.opt.set_lr(lr);
    }

    /// Run multi-epoch PPO update on a rollout batch.
    pub fn policy_update(&mut self, batch: &RolloutBatch) -> PolicyUpdateMetrics {
        let mut last = PpoLossBreakdown {
            policy_loss: 0.0,
            value_loss: 0.0,
            entropy: 0.0,
            total_loss: 0.0,
        };

        for _ in 0..self.config.ppo_epochs {
            last = self.single_epoch(batch);
        }

        PolicyUpdateMetrics {
            epochs: self.config.ppo_epochs,
            last_breakdown: last,
            learning_rate: self.config.learning_rate,
        }
    }

    fn single_epoch(&mut self, batch: &RolloutBatch) -> PpoLossBreakdown {
        let (log_probs, entropy, values) =
            self.actor_critic
                .evaluate_actions(&batch.obs, &batch.actions);
        let (loss, breakdown) = ppo_loss(
            &log_probs,
            &batch.old_log_probs,
            &batch.advantages,
            &values,
            &batch.returns,
            &entropy,
            self.config.clip_epsilon,
            self.config.value_coef,
            self.config.entropy_coef,
        );

        self.opt.zero_grad();
        loss.backward();
        self.opt.step();

        breakdown
    }

    /// Expose variable store for checkpoint I/O (WP-020).
    pub fn var_store(&self) -> &nn::VarStore {
        &self.vs
    }

    /// Mutable variable store for checkpoint load (WP-020).
    pub fn var_store_mut(&mut self) -> &mut nn::VarStore {
        &mut self.vs
    }

    /// Current PPO hyperparameters.
    pub fn config(&self) -> &PpoConfig {
        &self.config
    }
}

/// WoLF-PPO trainer: PPO with dual learning rates from rolling NES payoff estimate.
pub struct WolfPpoTrainer {
    inner: PpoTrainer,
    wolf_config: WolfPpoConfig,
    nes_estimate: NesPayoffEstimate,
}

/// WoLF-specific metrics from a policy update.
#[derive(Debug, Clone)]
pub struct WolfPolicyUpdateMetrics {
    pub base: PolicyUpdateMetrics,
    pub wolf_lr: WolfLearningRate,
    pub current_payoff: f64,
    pub estimated_nes_payoff: f64,
}

impl WolfPpoTrainer {
    /// Create a WoLF-PPO trainer with paper-default α_LOSE and hidden layers `[20, 20]`.
    pub fn new(obs_dim: i64, n_actions: i64, config: WolfPpoConfig) -> Self {
        let mut ppo_config = config.ppo.clone();
        ppo_config.learning_rate = config.alpha_lose;
        let inner = PpoTrainer::new(obs_dim, n_actions, ppo_config);
        Self {
            inner,
            wolf_config: config,
            nes_estimate: NesPayoffEstimate::default(),
        }
    }

    /// Run WoLF-PPO update: observe batch payoff, pick α_WIN/α_LOSE, then PPO epochs.
    pub fn policy_update(&mut self, batch: &RolloutBatch) -> WolfPolicyUpdateMetrics {
        let current_payoff = batch.mean_reward;
        let estimated_before = self.nes_estimate.average();
        let wolf_lr = self
            .nes_estimate
            .select_learning_rate(&self.wolf_config, current_payoff);

        self.inner.set_learning_rate(wolf_lr.lr);

        let base = self.inner.policy_update(batch);
        self.nes_estimate.observe(current_payoff);

        WolfPolicyUpdateMetrics {
            base,
            wolf_lr,
            current_payoff,
            estimated_nes_payoff: estimated_before,
        }
    }

    /// Rolling average payoff estimate (NES proxy).
    pub fn nes_estimate(&self) -> &NesPayoffEstimate {
        &self.nes_estimate
    }

    /// Underlying actor–critic.
    pub fn actor_critic(&self) -> &ActorCritic {
        &self.inner.actor_critic
    }

    /// WoLF + PPO configuration.
    pub fn config(&self) -> &WolfPpoConfig {
        &self.wolf_config
    }

    /// Expose variable store for checkpoint I/O (WP-020).
    pub fn var_store(&self) -> &nn::VarStore {
        self.inner.var_store()
    }

    /// Mutable variable store for checkpoint load (WP-020).
    pub fn var_store_mut(&mut self) -> &mut nn::VarStore {
        self.inner.var_store_mut()
    }
}

fn build_optimizer(config: &PpoConfig, vs: &nn::VarStore, lr: f64) -> nn::Optimizer {
    match config.optimizer {
        OptimizerKind::Sgd => nn::Sgd::default().build(vs, lr).expect("SGD optimizer"),
        OptimizerKind::Adam => nn::Adam::default().build(vs, lr).expect("Adam optimizer"),
    }
}

/// Build a synthetic rollout batch for tests and offline harnesses.
pub fn synthetic_rollout_batch(
    batch_size: i64,
    obs_dim: i64,
    n_actions: i64,
    mean_reward: f64,
) -> RolloutBatch {
    let obs = Tensor::randn([batch_size, obs_dim], (Kind::Float, Device::Cpu));
    let actions = Tensor::randint(n_actions, [batch_size], (Kind::Int64, Device::Cpu));
    let old_log_probs = Tensor::full([batch_size], -0.5, (Kind::Float, Device::Cpu));
    let advantages = Tensor::randn([batch_size], (Kind::Float, Device::Cpu));
    let returns = Tensor::randn([batch_size], (Kind::Float, Device::Cpu));

    RolloutBatch {
        obs,
        actions,
        old_log_probs,
        advantages,
        returns,
        mean_reward,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::loss::is_finite_breakdown;
    use crate::ppo::wolf::WolfMode;

    #[test]
    fn actor_critic_forward_shapes() {
        let vs = nn::VarStore::new(Device::Cpu);
        let net = ActorCritic::new(vs.root(), 4, 2, &[20, 20]);
        let obs = Tensor::randn([8, 4], (Kind::Float, Device::Cpu));
        let (logits, values) = net.forward(&obs);
        assert_eq!(logits.size(), vec![8, 2]);
        assert_eq!(values.size(), vec![8, 1]);
    }

    #[test]
    fn ppo_loss_finite_on_synthetic_batch() {
        let batch = synthetic_rollout_batch(16, 4, 2, 0.0);
        let mut trainer = PpoTrainer::new(4, 2, PpoConfig::default());
        let metrics = trainer.policy_update(&batch);
        assert!(is_finite_breakdown(&metrics.last_breakdown));
    }

    #[test]
    fn wolf_lr_switches_on_payoff_vs_estimate() {
        let config = WolfPpoConfig {
            alpha_lose: 0.1,
            win_lose_ratio: 4.0,
            ..WolfPpoConfig::default()
        };
        assert!((config.alpha_win() - 0.025).abs() < 1e-9);

        let mut estimate = NesPayoffEstimate::default();
        estimate.observe(0.0);

        let win = estimate.select_learning_rate(&config, 1.0);
        assert_eq!(win.mode, WolfMode::Win);
        assert!((win.lr - config.alpha_win()).abs() < 1e-9);

        let lose = estimate.select_learning_rate(&config, -1.0);
        assert_eq!(lose.mode, WolfMode::Lose);
        assert!((lose.lr - config.alpha_lose).abs() < 1e-9);
    }

    #[test]
    fn wolf_ppo_policy_update_runs() {
        let batch = synthetic_rollout_batch(8, 3, 2, 0.5);
        let mut trainer = WolfPpoTrainer::new(3, 2, WolfPpoConfig::default());
        let metrics = trainer.policy_update(&batch);
        assert!(is_finite_breakdown(&metrics.base.last_breakdown));
        assert!(metrics.base.learning_rate > 0.0);
    }
}
