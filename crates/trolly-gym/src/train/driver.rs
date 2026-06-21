//! WoLF-PPO training driver: rollout → multi-epoch update → metrics.

use tch::{nn, Device, Kind, Tensor};

use crate::games::euclidean_distance;
use crate::ppo::{
    default_hidden_layers, ActorCritic, PpoConfig, PpoTrainer, WolfPpoConfig, WolfPpoTrainer,
};
use crate::replay::OnPolicyRolloutBuffer;

use super::metrics::{TrainMetricsLog, TrainStepMetrics};
use super::rollout::{rollout_buffer_to_batch, DEFAULT_GAE_LAMBDA, DEFAULT_GAMMA};

/// Configuration for the end-to-end training loop.
#[derive(Debug, Clone)]
pub struct TrainingLoopConfig {
    pub wolf: WolfPpoConfig,
    pub num_updates: usize,
    pub gamma: f64,
    pub gae_lambda: f64,
}

impl TrainingLoopConfig {
    pub fn smoke() -> Self {
        Self {
            wolf: WolfPpoConfig {
                ppo: PpoConfig {
                    ppo_epochs: 1,
                    use_adam: false,
                    ..Default::default()
                },
                alpha_lose: 0.01,
                ..Default::default()
            },
            num_updates: 3,
            gamma: DEFAULT_GAMMA,
            gae_lambda: DEFAULT_GAE_LAMBDA,
        }
    }
}

/// Driver wrapping [`WolfPpoTrainer`] with rollout conversion and metric logging.
pub struct WolfPpoTrainingDriver {
    pub trainer: WolfPpoTrainer,
    pub gamma: f64,
    pub gae_lambda: f64,
    pub metrics: TrainMetricsLog,
}

impl WolfPpoTrainingDriver {
    pub fn new(config: WolfPpoConfig, gamma: f64, gae_lambda: f64) -> Self {
        Self {
            trainer: WolfPpoTrainer::new(config),
            gamma,
            gae_lambda,
            metrics: TrainMetricsLog::new(),
        }
    }

    pub fn from_wolf_config(config: WolfPpoConfig) -> Self {
        Self::new(config, DEFAULT_GAMMA, DEFAULT_GAE_LAMBDA)
    }

    /// Convert an on-policy buffer, run WoLF-PPO update, log metrics.
    pub fn update_from_rollout(
        &mut self,
        model: &ActorCritic,
        opt: &mut nn::Optimizer,
        rollout: &OnPolicyRolloutBuffer,
        current_payoff: f64,
        nes_reference: Option<&[f64]>,
        device: Device,
    ) -> TrainStepMetrics {
        let batch = rollout_buffer_to_batch(rollout, self.gamma, self.gae_lambda, device);
        let obs_dim = rollout.steps()[0].observation.len() as i64;
        let nes_distance =
            nes_reference.map(|nes| policy_nes_distance(model, obs_dim, nes, device));
        let losses = self
            .trainer
            .policy_update(model, opt, &batch, current_payoff);
        let metrics = TrainStepMetrics::from_losses(
            &losses,
            nes_distance,
            Some(self.trainer.last_learning_rate),
        );
        self.metrics.record(metrics);
        metrics
    }

    /// Plain PPO update (no WoLF LR) for comparison runs.
    pub fn update_from_rollout_ppo(
        &mut self,
        model: &ActorCritic,
        opt: &mut nn::Optimizer,
        rollout: &OnPolicyRolloutBuffer,
        nes_reference: Option<&[f64]>,
        device: Device,
    ) -> TrainStepMetrics {
        let batch = rollout_buffer_to_batch(rollout, self.gamma, self.gae_lambda, device);
        let obs_dim = rollout.steps()[0].observation.len() as i64;
        let nes_distance =
            nes_reference.map(|nes| policy_nes_distance(model, obs_dim, nes, device));
        let inner = PpoTrainer::new(self.trainer.config.ppo);
        let losses = inner.policy_update(model, opt, &batch);
        let metrics = TrainStepMetrics::from_losses(&losses, nes_distance, None);
        self.metrics.record(metrics);
        metrics
    }

    /// Run `num_updates` train steps from a rollout supplier closure.
    pub fn run<F>(
        &mut self,
        model: &ActorCritic,
        opt: &mut nn::Optimizer,
        num_updates: usize,
        mut supply_rollout: F,
        payoff_fn: impl Fn(&ActorCritic, Device) -> f64,
        nes_reference: Option<&[f64]>,
        device: Device,
    ) where
        F: FnMut(usize) -> OnPolicyRolloutBuffer,
    {
        for step in 0..num_updates {
            let rollout = supply_rollout(step);
            let payoff = payoff_fn(model, device);
            self.update_from_rollout(model, opt, &rollout, payoff, nes_reference, device);
        }
    }
}

fn policy_nes_distance(model: &ActorCritic, obs_dim: i64, nes: &[f64], device: Device) -> f64 {
    let probs = tch::no_grad(|| {
        let obs = Tensor::ones(&[1, obs_dim], (Kind::Float, device));
        let (logits, _) = model.forward(&obs);
        logits.softmax(-1, Kind::Float).squeeze()
    });
    let learned: Vec<f64> = probs.iter::<f64>().unwrap().collect();
    euclidean_distance(&learned, nes)
}

/// Short end-to-end smoke: matrix-game-style constant obs rollout + WoLF updates.
pub fn smoke_train_loop(device: Device) -> TrainMetricsLog {
    let config = TrainingLoopConfig::smoke();
    let vs = nn::VarStore::new(device);
    let obs_dim = 1_i64;
    let action_count = 2_i64;
    let ppo = config.wolf.ppo;
    let model = ActorCritic::new(
        &vs.root(),
        obs_dim,
        action_count,
        &default_hidden_layers(),
        &ppo,
    );
    let mut opt = ActorCritic::build_optimizer(&vs, &ppo);
    let mut driver = WolfPpoTrainingDriver::from_wolf_config(config.wolf);

    driver.run(
        &model,
        &mut opt,
        config.num_updates,
        |_step| synthetic_rollout(obs_dim, action_count, 8, device),
        |model, dev| {
            tch::no_grad(|| {
                let obs = Tensor::ones(&[1, obs_dim], (Kind::Float, dev));
                let (logits, _) = model.forward(&obs);
                logits
                    .softmax(-1, Kind::Float)
                    .squeeze()
                    .double_value(&[0])
            })
        },
        Some(&[0.5, 0.5]),
        device,
    );

    driver.metrics
}

fn synthetic_rollout(
    obs_dim: i64,
    action_count: i64,
    steps: usize,
    device: Device,
) -> OnPolicyRolloutBuffer {
    let vs = nn::VarStore::new(device);
    let model = ActorCritic::new(
        &vs.root(),
        obs_dim,
        action_count,
        &default_hidden_layers(),
        &PpoConfig::default(),
    );
    let obs = Tensor::ones(&[steps as i64, obs_dim], (Kind::Float, device));

    let mut buffer = OnPolicyRolloutBuffer::with_capacity(steps);
    tch::no_grad(|| {
        let (actions, log_probs, values, _) = model.act(&obs);
        for i in 0..steps as i64 {
            buffer.push(crate::replay::OnPolicyStep {
                observation: vec![1.0; obs_dim as usize],
                action: actions.int64_value(&[i]),
                log_prob: log_probs.double_value(&[i]),
                value: values.double_value(&[i]),
                reward: if actions.int64_value(&[i]) == 0 {
                    1.0
                } else {
                    -1.0
                },
                done: false,
            });
        }
    });
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use tch::Device;

    #[test]
    fn smoke_train_loop_produces_finite_metrics() {
        let log = smoke_train_loop(Device::Cpu);
        assert_eq!(log.len(), 3);
        for m in &log.steps {
            assert!(m.policy_loss.is_finite());
            assert!(m.value_loss.is_finite());
            assert!(m.entropy.is_finite());
            assert!(m.learning_rate.unwrap().is_finite());
            assert!(m.nes_distance.unwrap().is_finite());
        }
    }

    #[test]
    fn update_from_rollout_records_wolf_lr() {
        let device = Device::Cpu;
        let vs = nn::VarStore::new(device);
        let model = ActorCritic::new(
            &vs.root(),
            2,
            2,
            &default_hidden_layers(),
            &PpoConfig::default(),
        );
        let mut opt = ActorCritic::build_optimizer(&vs, &PpoConfig::default());
        let mut driver = WolfPpoTrainingDriver::from_wolf_config(WolfPpoConfig {
            ppo: PpoConfig {
                ppo_epochs: 1,
                ..Default::default()
            },
            alpha_lose: 0.04,
            ..Default::default()
        });

        let rollout = synthetic_rollout(2, 2, 4, device);
        let metrics = driver.update_from_rollout(&model, &mut opt, &rollout, 1.0, None, device);
        assert!(metrics.learning_rate.is_some());
        assert!(metrics.total_loss.is_finite());
    }
}
