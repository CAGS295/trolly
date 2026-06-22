//! Convert [`RolloutCollector`] steps into a [`RolloutBatch`] for PPO updates.

use tch::{Device, Kind, Tensor};

use crate::ppo::RolloutBatch;
use crate::replay::RolloutCollector;

/// Build a PPO batch from collected on-policy steps.
///
/// Uses one-step returns (`returns = rewards`) and advantage `returns − value` (detached),
/// matching the matrix-game harness.
pub fn rollout_collector_to_batch(collector: &RolloutCollector, device: Device) -> RolloutBatch {
    let steps = collector.steps();
    let batch = steps.len() as i64;
    let obs_dim = steps.first().map(|s| s.observation.len()).unwrap_or(0) as i64;

    let flat_obs: Vec<f32> = steps.iter().flat_map(|s| s.observation.iter().copied()).collect();
    let observations = if batch > 0 {
        Tensor::from_slice(&flat_obs)
            .view([batch, obs_dim])
            .to_device(device)
    } else {
        Tensor::zeros([0, obs_dim], (Kind::Float, device))
    };

    let actions: Vec<i64> = steps.iter().map(|s| s.action_index()).collect();
    let old_log_probs: Vec<f32> = steps.iter().map(|s| s.log_prob).collect();
    let rewards: Vec<f32> = steps.iter().map(|s| s.reward).collect();
    let values: Vec<f32> = steps.iter().map(|s| s.value).collect();

    let actions = Tensor::from_slice(&actions).to_device(device);
    let old_log_probs = Tensor::from_slice(&old_log_probs).to_device(device);
    let rewards = Tensor::from_slice(&rewards).to_device(device);
    let values = Tensor::from_slice(&values).to_device(device);

    let returns = rewards.shallow_clone();
    let advantages = &returns - &values.detach();

    RolloutBatch {
        observations,
        actions,
        old_log_probs,
        returns,
        advantages,
        rewards,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::replay::RolloutStep;

    #[test]
    fn rollout_collector_batch_shapes_match_steps() {
        let mut collector = RolloutCollector::with_capacity(3);
        for obs in [[1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
            collector.push(RolloutStep {
                observation: obs.to_vec(),
                action: Action::Buy,
                log_prob: -0.2,
                value: 0.05,
                reward: 0.1,
                done: false,
            });
        }

        let batch = rollout_collector_to_batch(&collector, Device::Cpu);
        assert_eq!(batch.observations.size(), [3, 2]);
        assert_eq!(batch.actions.size(), [3]);
        assert_eq!(batch.old_log_probs.size(), [3]);
        assert_eq!(batch.returns.size(), [3]);
        assert_eq!(batch.advantages.size(), [3]);
        assert!((batch.mean_reward() - 0.1).abs() < 1e-6);
    }
}
