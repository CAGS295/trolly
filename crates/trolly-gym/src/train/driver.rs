//! PPO / WoLF-PPO training drivers with scalar metrics logging.

use tch::Device;

use crate::ppo::{PpoConfig, PpoTrainer, RolloutBatch, WolfPpoConfig, WolfPpoTrainer};
use crate::replay::RolloutCollector;

use super::collector::rollout_collector_to_batch;

/// Shared training loop settings.
#[derive(Debug, Clone)]
pub struct TrainConfig {
    /// Number of policy updates (rollout → multi-epoch PPO).
    pub num_policy_updates: usize,
    /// On-policy steps collected before each update.
    pub rollout_steps: usize,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self {
            num_policy_updates: 4,
            rollout_steps: 8,
        }
    }
}

/// Scalar metrics logged after one policy update.
#[derive(Debug, Clone, PartialEq)]
pub struct TrainStepMetrics {
    pub update: usize,
    pub policy_loss: f64,
    pub value_loss: f64,
    pub entropy: f64,
    pub total_loss: f64,
    pub mean_reward: f64,
    pub wolf_learning_rate: Option<f64>,
    pub nes_distance: Option<f64>,
}

impl TrainStepMetrics {
    fn from_ppo_breakdown(
        update: usize,
        breakdown: &crate::ppo::PpoLossBreakdown,
        batch: &RolloutBatch,
        wolf_lr: Option<f64>,
        nes_distance: Option<f64>,
    ) -> Self {
        Self {
            update,
            policy_loss: breakdown.clip_loss,
            value_loss: breakdown.value_loss,
            entropy: breakdown.entropy,
            total_loss: breakdown.total,
            mean_reward: batch.mean_reward(),
            wolf_learning_rate: wolf_lr,
            nes_distance,
        }
    }
}

/// Standard PPO training driver.
pub struct PpoTrainingDriver {
    trainer: PpoTrainer,
    device: Device,
}

impl PpoTrainingDriver {
    pub fn new(config: PpoConfig) -> Self {
        Self {
            trainer: PpoTrainer::new(config),
            device: Device::Cpu,
        }
    }

    pub fn trainer(&self) -> &PpoTrainer {
        &self.trainer
    }

    pub fn trainer_mut(&mut self) -> &mut PpoTrainer {
        &mut self.trainer
    }

    /// Collect rollouts via `collect`, run PPO updates, and return per-step metrics.
    pub fn train<F>(
        &mut self,
        config: &TrainConfig,
        mut collect: F,
        nes_distance: Option<impl Fn(&PpoTrainer) -> f64>,
    ) -> Vec<TrainStepMetrics>
    where
        F: FnMut(&crate::ppo::ActorCritic, &mut RolloutCollector),
    {
        let mut metrics = Vec::with_capacity(config.num_policy_updates);
        let mut collector = RolloutCollector::with_capacity(config.rollout_steps);

        for update in 0..config.num_policy_updates {
            collector.clear();
            collect(self.trainer.actor_critic(), &mut collector);
            let batch = rollout_collector_to_batch(&collector, self.device);
            let breakdown = self.trainer.policy_update(&batch);
            let nes = nes_distance.as_ref().map(|f| f(&self.trainer));
            metrics.push(TrainStepMetrics::from_ppo_breakdown(
                update,
                &breakdown,
                &batch,
                None,
                nes,
            ));
        }

        metrics
    }
}

/// WoLF-PPO training driver with dual learning-rate logging.
pub struct WolfPpoTrainingDriver {
    trainer: WolfPpoTrainer,
    device: Device,
}

impl WolfPpoTrainingDriver {
    pub fn new(config: WolfPpoConfig) -> Self {
        Self {
            trainer: WolfPpoTrainer::new(config),
            device: Device::Cpu,
        }
    }

    pub fn trainer(&self) -> &WolfPpoTrainer {
        &self.trainer
    }

    pub fn trainer_mut(&mut self) -> &mut WolfPpoTrainer {
        &mut self.trainer
    }

    /// Collect rollouts via `collect`, run WoLF-PPO updates, and return per-step metrics.
    pub fn train<F>(
        &mut self,
        config: &TrainConfig,
        mut collect: F,
        nes_distance: Option<impl Fn(&WolfPpoTrainer) -> f64>,
    ) -> Vec<TrainStepMetrics>
    where
        F: FnMut(&crate::ppo::ActorCritic, &mut RolloutCollector),
    {
        let mut metrics = Vec::with_capacity(config.num_policy_updates);
        let mut collector = RolloutCollector::with_capacity(config.rollout_steps);

        for update in 0..config.num_policy_updates {
            collector.clear();
            collect(self.trainer.actor_critic(), &mut collector);
            let batch = rollout_collector_to_batch(&collector, self.device);
            let (breakdown, lr) = self.trainer.policy_update(&batch);
            let nes = nes_distance.as_ref().map(|f| f(&self.trainer));
            metrics.push(TrainStepMetrics::from_ppo_breakdown(
                update,
                &breakdown,
                &batch,
                Some(lr),
                nes,
            ));
        }

        metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::env::{Env, EnvConfig};
    use crate::games::{euclidean_distance, MatrixGame, MatrixGameKind};
    use crate::replay::RolloutStep;
    use crate::train::collect_env_rollout_step;
    use tch::{Kind, Tensor};
    use trolly_strategy::{DepthUpdate, PriceLevel, RecordingEgress, StreamEvent};

    fn depth_event(symbol: &str, bid: &str, ask: &str) -> StreamEvent {
        StreamEvent::Depth(DepthUpdate {
            symbol: symbol.into(),
            bids: vec![PriceLevel {
                price: bid.into(),
                qty: "1".into(),
            }],
            asks: vec![PriceLevel {
                price: ask.into(),
                qty: "1".into(),
            }],
            update_id: Some(1),
        })
    }

    #[test]
    fn short_env_train_loop_logs_finite_metrics() {
        let obs_frames = 1;
        let mut env = Env::new(
            EnvConfig::new("BTCUSDT").with_window_frames(obs_frames),
            RecordingEgress::default(),
        );
        for (bid, ask) in [("100", "102"), ("101", "103"), ("102", "104")] {
            assert!(env.ingest_event(&depth_event("BTCUSDT", bid, ask)));
        }

        let obs_dim = env.current_observation().len() as i64;
        let wolf_config = WolfPpoConfig {
            ppo: PpoConfig::default()
                .with_obs_dim(obs_dim)
                .with_num_actions(Action::COUNT),
            ..Default::default()
        };

        tch::manual_seed(42);
        let mut driver = WolfPpoTrainingDriver::new(wolf_config);
        let train_config = TrainConfig {
            num_policy_updates: 3,
            rollout_steps: 2,
        };

        let metrics = driver.train(&train_config, |actor_critic, collector| {
            while collector.len() < train_config.rollout_steps {
                let _ = collect_env_rollout_step(&mut env, actor_critic, collector);
                assert!(env.ingest_event(&depth_event("BTCUSDT", "103", "105")));
            }
        }, None::<fn(&WolfPpoTrainer) -> f64>);

        assert_eq!(metrics.len(), train_config.num_policy_updates);
        for m in &metrics {
            assert!(m.policy_loss.is_finite());
            assert!(m.value_loss.is_finite());
            assert!(m.entropy.is_finite());
            assert!(m.total_loss.is_finite());
            assert!(m.wolf_learning_rate.is_some());
        }
    }

    #[test]
    fn short_matrix_game_train_loop_with_nes_distance() {
        let game = MatrixGame::new(MatrixGameKind::MatchingPennies);
        let wolf_config = WolfPpoConfig {
            ppo: PpoConfig {
                obs_dim: game.obs_dim(),
                num_actions: game.num_actions() as i64,
                hidden_dims: vec![20, 20],
                ppo_epochs: 2,
                ..Default::default()
            },
            ..Default::default()
        };

        tch::manual_seed(7);
        let mut driver = WolfPpoTrainingDriver::new(wolf_config);
        let train_config = TrainConfig {
            num_policy_updates: 4,
            rollout_steps: 8,
        };

        let nes_probs = game.nes_probabilities();
        let metrics = driver.train(
            &train_config,
            |actor_critic, collector| {
                let batch = 8_i64;
                let obs = Tensor::zeros([batch, game.obs_dim()], (Kind::Float, Device::Cpu));
                let (_actions, log_probs, values) = actor_critic.act(&obs);
                let rows = [0_i64, 1];
                let cols = [0_i64, 1];
                for i in 0..batch {
                    let r = rows[i as usize % rows.len()];
                    let c = cols[i as usize % cols.len()];
                    let reward = game.row_payoff(r as usize, c as usize) as f32;
                    collector.push(RolloutStep {
                        observation: vec![0.0; game.obs_dim() as usize],
                        action: Action::from_index(r).unwrap(),
                        log_prob: log_probs.double_value(&[i]) as f32,
                        value: values.double_value(&[i]) as f32,
                        reward,
                        done: false,
                    });
                }
            },
            Some(|trainer: &WolfPpoTrainer| {
                let obs = Tensor::zeros([1, game.obs_dim()], (Kind::Float, Device::Cpu));
                let (_, logits) = trainer.actor_critic().forward(&obs);
                let probs = logits.softmax(-1, Kind::Float).squeeze();
                let learned: Vec<f64> = (0..probs.size()[0])
                    .map(|i| probs.double_value(&[i]))
                    .collect();
                euclidean_distance(&learned, &nes_probs)
            }),
        );

        assert_eq!(metrics.len(), train_config.num_policy_updates);
        assert!(metrics.iter().all(|m| m.nes_distance.is_some()));
        assert!(metrics.last().unwrap().nes_distance.unwrap().is_finite());
    }
}
