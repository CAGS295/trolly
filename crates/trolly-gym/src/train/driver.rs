//! WoLF-PPO training driver loop and scalar metrics logging.

use crate::ppo::{RolloutBatch, WolfPpoTrainer, WolfPolicyUpdateMetrics};
use crate::replay::OnPolicyRolloutBuffer;
use crate::train::rollout::finish_rollout_batch;

/// Configuration for the stream/env training driver.
#[derive(Debug, Clone)]
pub struct WolfPpoTrainConfig {
    /// Optional NES distance callback (e.g. matrix-game harness).
    pub log_nes_distance: bool,
}

impl Default for WolfPpoTrainConfig {
    fn default() -> Self {
        Self {
            log_nes_distance: false,
        }
    }
}

/// Scalar metrics from one collect → update cycle.
#[derive(Debug, Clone, PartialEq)]
pub struct TrainStepMetrics {
    pub policy_loss: f64,
    pub value_loss: f64,
    pub entropy: f64,
    pub total_loss: f64,
    pub wolf_learning_rate: f64,
    pub current_payoff: f64,
    pub estimated_nes_payoff: f64,
    pub rollout_steps: usize,
    pub nes_distance: Option<f64>,
}

impl TrainStepMetrics {
    fn from_wolf_update(
        update: &WolfPolicyUpdateMetrics,
        rollout_steps: usize,
        nes_distance: Option<f64>,
    ) -> Self {
        let breakdown = &update.base.last_breakdown;
        Self {
            policy_loss: breakdown.policy_loss,
            value_loss: breakdown.value_loss,
            entropy: breakdown.entropy,
            total_loss: breakdown.total_loss,
            wolf_learning_rate: update.base.learning_rate,
            current_payoff: update.current_payoff,
            estimated_nes_payoff: update.estimated_nes_payoff,
            rollout_steps,
            nes_distance,
        }
    }
}

/// Driver: on-policy rollout → multi-epoch WoLF-PPO update → metrics.
pub struct WolfPpoTrainLoop {
    trainer: WolfPpoTrainer,
    config: WolfPpoTrainConfig,
    history: Vec<TrainStepMetrics>,
}

impl WolfPpoTrainLoop {
    pub fn new(trainer: WolfPpoTrainer, config: WolfPpoTrainConfig) -> Self {
        Self {
            trainer,
            config: config,
            history: Vec::new(),
        }
    }

    pub fn with_trainer(trainer: WolfPpoTrainer) -> Self {
        Self::new(trainer, WolfPpoTrainConfig::default())
    }

    pub fn trainer(&self) -> &WolfPpoTrainer {
        &self.trainer
    }

    pub fn trainer_mut(&mut self) -> &mut WolfPpoTrainer {
        &mut self.trainer
    }

    pub fn config(&self) -> &WolfPpoTrainConfig {
        &self.config
    }

    pub fn history(&self) -> &[TrainStepMetrics] {
        &self.history
    }

    pub fn last_metrics(&self) -> Option<&TrainStepMetrics> {
        self.history.last()
    }

    /// Run multi-epoch WoLF-PPO update on a prepared rollout batch.
    pub fn train_on_batch(
        &mut self,
        batch: &RolloutBatch,
        nes_distance: Option<f64>,
    ) -> TrainStepMetrics {
        let steps = batch.batch_size() as usize;
        let update = self.trainer.policy_update(batch);
        let metrics = TrainStepMetrics::from_wolf_update(&update, steps, nes_distance);
        self.history.push(metrics.clone());
        metrics
    }

    /// Convert an on-policy buffer and run one update.
    pub fn train_on_rollout(
        &mut self,
        rollout: &OnPolicyRolloutBuffer,
        nes_distance: Option<f64>,
    ) -> TrainStepMetrics {
        let batch = finish_rollout_batch(rollout);
        self.train_on_batch(&batch, nes_distance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::WolfPpoConfig;
    use crate::ppo::synthetic_rollout_batch;
    use crate::train::RolloutCollector;

    #[test]
    fn train_on_batch_logs_finite_metrics() {
        let trainer = WolfPpoTrainer::new(4, 3, WolfPpoConfig::default());
        let mut driver = WolfPpoTrainLoop::with_trainer(trainer);
        let batch = synthetic_rollout_batch(8, 4, 3, 0.25);
        let metrics = driver.train_on_batch(&batch, None);

        assert_eq!(metrics.rollout_steps, 8);
        assert!(metrics.policy_loss.is_finite());
        assert!(metrics.value_loss.is_finite());
        assert!(metrics.entropy.is_finite());
        assert!(metrics.wolf_learning_rate > 0.0);
        assert_eq!(driver.history().len(), 1);
    }

    #[test]
    fn short_end_to_end_train_loop() {
        tch::manual_seed(7);
        let trainer = WolfPpoTrainer::new(7, 3, WolfPpoConfig::default());
        let mut driver = WolfPpoTrainLoop::with_trainer(trainer);

        for step_idx in 0..3 {
            let mut collector = RolloutCollector::with_capacity(4);
            for i in 0..4 {
                let obs = vec![step_idx as f32; 7]
                    .into_iter()
                    .enumerate()
                    .map(|(j, v)| v + j as f32 * 0.1)
                    .collect::<Vec<_>>();
                collector.record_step(
                    obs,
                    crate::action::Action::Hold,
                    -1.1,
                    0.0,
                    0.01 * i as f32,
                    false,
                );
            }
            let metrics = driver.train_on_rollout(collector.buffer(), None);
            assert!(metrics.total_loss.is_finite());
            assert_eq!(metrics.rollout_steps, 4);
        }

        assert_eq!(driver.history().len(), 3);
        assert!(driver.last_metrics().unwrap().estimated_nes_payoff.is_finite());
    }
}
