//! WoLF-PPO training driver: rollout → update → metrics.
//!
//! [`WolfPpoTrainDriver`] wraps [`WolfPpoTrainer`] and drives a standard
//! collect-then-update loop:
//!
//! 1. Collect `horizon` on-policy transitions via an env-step closure.
//! 2. Compute GAE returns and advantages.
//! 3. Run multi-epoch PPO/WoLF-PPO updates.
//! 4. Return [`TrainMetrics`] with scalar training statistics.
//!
//! The env closure follows the same contract as [`RolloutCollector::collect`]:
//! it receives the current observation and action, and returns a
//! [`StepOutput`] with the next observation, reward, and done flag.

use tch::{Kind, Tensor};

use crate::ppo::{ActorCritic, RolloutBatch, WolfPpoConfig, WolfPpoTrainer};

use super::rollout::{RolloutCollector, StepOutput};

/// Scalar training statistics emitted after each `train_step`.
#[derive(Debug, Clone)]
pub struct TrainMetrics {
    /// Mean combined PPO loss (L^CLIP − c1·L^VF + c2·S) across gradient epochs.
    pub policy_loss: f64,
    /// Separate mean value-MSE loss term (diagnostic; same gradient path as `policy_loss`).
    pub value_loss: f64,
    /// Mean per-sample entropy H[π] across the rollout batch.
    pub entropy: f64,
    /// Euclidean distance to a reference NES target, when provided.
    pub nes_distance: Option<f64>,
    /// Learning rate that was active during this update (α_WIN or α_LOSE).
    pub active_lr: f64,
    /// Number of environment steps collected in this training step.
    pub steps_collected: usize,
}

/// Training driver configuration.
#[derive(Debug, Clone)]
pub struct TrainDriverConfig {
    /// Observation dimensionality (must match actor-critic input).
    pub obs_dim: i64,
    /// Number of discrete actions (must match actor-critic output).
    pub num_actions: i64,
    /// Environment steps per collect phase.
    pub horizon: usize,
    /// Discount factor γ for GAE.
    pub gamma: f64,
    /// GAE lambda λ (1.0 → Monte Carlo, 0.0 → one-step TD).
    pub gae_lambda: f64,
}

impl Default for TrainDriverConfig {
    fn default() -> Self {
        Self {
            obs_dim: 4,
            num_actions: 3,
            horizon: 64,
            gamma: 0.99,
            gae_lambda: 0.95,
        }
    }
}

/// WoLF-PPO training driver.
///
/// Owns the [`WolfPpoTrainer`] and the [`RolloutCollector`].
/// Call [`WolfPpoTrainDriver::train_step`] in a loop to drive training.
pub struct WolfPpoTrainDriver {
    pub trainer: WolfPpoTrainer,
    collector: RolloutCollector,
    config: TrainDriverConfig,
    total_steps: usize,
}

impl WolfPpoTrainDriver {
    /// Create a new driver with the given configs.
    pub fn new(driver_config: TrainDriverConfig, wolf_config: WolfPpoConfig) -> Self {
        let collector = RolloutCollector::new(
            driver_config.horizon,
            driver_config.gamma,
            driver_config.gae_lambda,
        );
        let trainer = WolfPpoTrainer::new(
            driver_config.obs_dim,
            driver_config.num_actions,
            wolf_config,
        );
        Self {
            trainer,
            collector,
            config: driver_config,
            total_steps: 0,
        }
    }

    /// Borrow the inner actor-critic for inference.
    pub fn actor_critic(&self) -> &ActorCritic {
        &self.trainer.inner.actor_critic
    }

    /// Total environment steps taken across all `train_step` calls.
    pub fn total_steps(&self) -> usize {
        self.total_steps
    }

    /// Run one collect-update cycle.
    ///
    /// `initial_obs` must have length == `config.obs_dim`.
    /// `env_step` is called `horizon` times to advance the environment.
    /// `bootstrap_value` is the critic estimate for the state after the last
    /// transition (pass 0.0 if the episode ended).
    /// `nes_target` is an optional reference policy for computing NES distance.
    pub fn train_step<F>(
        &mut self,
        initial_obs: Vec<f32>,
        env_step: F,
        bootstrap_value: f32,
        nes_target: Option<&[f64]>,
    ) -> TrainMetrics
    where
        F: FnMut(Vec<f32>, i64) -> StepOutput,
    {
        // Collect rollout — borrow trainer and collector as distinct fields.
        let ac = &self.trainer.inner.actor_critic;
        self.collector.collect(ac, initial_obs, env_step);
        let steps = self.collector.len();
        self.total_steps += steps;

        let batch = self.collector.into_batch(bootstrap_value);

        // Compute separate diagnostic losses before the gradient update
        let (value_loss, entropy) = compute_diagnostics(&self.trainer.inner.actor_critic, &batch);

        // Episode return for WoLF LR selection = mean batch return
        let episode_return = batch.returns.mean(Kind::Float).double_value(&[]);
        let active_lr = self.trainer.active_lr();

        // WoLF-PPO update
        let policy_loss = self.trainer.policy_update(&batch, episode_return);

        // Optional NES distance
        let nes_distance = nes_target.map(|nes| {
            let probs = policy_probs(&self.trainer.inner.actor_critic, self.config.obs_dim);
            euclidean_distance(&probs, nes)
        });

        TrainMetrics {
            policy_loss,
            value_loss,
            entropy,
            nes_distance,
            active_lr,
            steps_collected: steps,
        }
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Extract softmax probabilities from the actor for a zero observation.
fn policy_probs(ac: &ActorCritic, obs_dim: i64) -> Vec<f64> {
    let _g = tch::no_grad_guard();
    let obs = Tensor::zeros(&[1, obs_dim], (tch::Kind::Float, tch::Device::Cpu));
    let (logits, _) = ac.forward(&obs);
    let probs = logits.softmax(-1, Kind::Float).squeeze();
    let n = probs.size()[0] as usize;
    (0..n).map(|i| probs.double_value(&[i as i64])).collect()
}

/// Euclidean distance between two probability vectors.
fn euclidean_distance(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// Compute separate value-MSE and mean entropy diagnostics (no gradient).
fn compute_diagnostics(ac: &ActorCritic, batch: &RolloutBatch) -> (f64, f64) {
    let _g = tch::no_grad_guard();
    let (log_probs, entropy) = ac.evaluate_actions(&batch.observations, &batch.actions);
    let (_, values) = ac.forward(&batch.observations);
    let _ = log_probs; // diagnostic call — we only use entropy here
    let value_loss = (&values - &batch.returns)
        .pow_tensor_scalar(2)
        .mean(Kind::Float)
        .double_value(&[]);
    let entropy_mean = entropy.mean(Kind::Float).double_value(&[]);
    (value_loss, entropy_mean)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::WolfPpoConfig;

    fn make_driver(obs_dim: i64, num_actions: i64, horizon: usize) -> WolfPpoTrainDriver {
        let driver_cfg = TrainDriverConfig {
            obs_dim,
            num_actions,
            horizon,
            gamma: 0.99,
            gae_lambda: 0.95,
        };
        WolfPpoTrainDriver::new(driver_cfg, WolfPpoConfig::default())
    }

    #[test]
    fn train_step_metrics_are_finite() {
        let obs_dim = 4_i64;
        let num_actions = 3_i64;
        let horizon = 16;
        let mut driver = make_driver(obs_dim, num_actions, horizon);

        let initial_obs = vec![0.0_f32; obs_dim as usize];
        let metrics = driver.train_step(
            initial_obs,
            |obs, _action| StepOutput {
                next_observation: obs,
                reward: 1.0,
                done: false,
            },
            0.0,
            None,
        );

        assert!(
            metrics.policy_loss.is_finite(),
            "policy_loss not finite: {}",
            metrics.policy_loss
        );
        assert!(
            metrics.value_loss.is_finite(),
            "value_loss not finite: {}",
            metrics.value_loss
        );
        assert!(
            metrics.entropy.is_finite(),
            "entropy not finite: {}",
            metrics.entropy
        );
        assert_eq!(metrics.steps_collected, horizon);
    }

    #[test]
    fn train_step_with_nes_target_produces_distance() {
        let obs_dim = 4_i64;
        let num_actions = 3_i64;
        let mut driver = make_driver(obs_dim, num_actions, 8);

        let nes = vec![1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0];
        let metrics = driver.train_step(
            vec![0.0_f32; obs_dim as usize],
            |obs, _a| StepOutput {
                next_observation: obs,
                reward: 0.0,
                done: false,
            },
            0.0,
            Some(&nes),
        );

        let dist = metrics.nes_distance.expect("expected NES distance");
        assert!(dist.is_finite() && dist >= 0.0, "invalid NES distance: {dist}");
    }

    #[test]
    fn wolf_lr_switch_reflected_in_metrics() {
        let obs_dim = 4_i64;
        let num_actions = 3_i64;
        let wolf_cfg = WolfPpoConfig::default();
        let alpha_lose = wolf_cfg.alpha_lose;
        let mut driver = make_driver(obs_dim, num_actions, 8);
        // After first step, current == rolling → losing
        let m = driver.train_step(
            vec![0.0_f32; obs_dim as usize],
            |obs, _a| StepOutput { next_observation: obs, reward: 1.0, done: false },
            0.0,
            None,
        );
        assert_eq!(m.active_lr, alpha_lose, "first step should use alpha_lose");
    }

    #[test]
    fn end_to_end_short_train_loop() {
        let obs_dim = 4_i64;
        let num_actions = 3_i64;
        let mut driver = make_driver(obs_dim, num_actions, 16);

        for step in 0..5_usize {
            let reward = if step % 2 == 0 { 1.0_f32 } else { -1.0_f32 };
            let metrics = driver.train_step(
                vec![0.0_f32; obs_dim as usize],
                move |obs, _a| StepOutput {
                    next_observation: obs,
                    reward,
                    done: false,
                },
                0.0,
                None,
            );
            assert!(
                metrics.policy_loss.is_finite(),
                "step {step} policy_loss NaN"
            );
        }
        assert_eq!(driver.total_steps(), 5 * 16);
    }
}
