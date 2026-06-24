//! On-policy rollout batch consumed by PPO / WoLF-PPO updates.

use tch::{Device, Kind, Tensor};

/// Flattened rollout tensors for one policy update step.
#[derive(Debug)]
pub struct RolloutBatch {
    pub observations: Tensor,
    pub actions: Tensor,
    pub old_log_probs: Tensor,
    pub advantages: Tensor,
    pub returns: Tensor,
    pub old_values: Tensor,
    /// Mean episodic return in this batch (WoLF payoff signal).
    pub mean_return: f64,
}

impl RolloutBatch {
    pub fn from_slices(
        observations: &[f32],
        obs_dim: i64,
        actions: &[i64],
        old_log_probs: &[f64],
        advantages: &[f64],
        returns: &[f64],
        old_values: &[f64],
        device: Device,
    ) -> Self {
        let batch = actions.len() as i64;
        let obs = Tensor::from_slice(observations)
            .to_device(device)
            .reshape(&[batch, obs_dim]);
        let acts = Tensor::from_slice(actions).to_kind(Kind::Int64).to_device(device);
        let old_lp = Tensor::from_slice(old_log_probs).to_device(device);
        let adv = Tensor::from_slice(advantages).to_device(device);
        let ret = Tensor::from_slice(returns).to_device(device);
        let old_v = Tensor::from_slice(old_values).to_device(device);
        let mean_return = returns.iter().sum::<f64>() / returns.len().max(1) as f64;
        Self {
            observations: obs,
            actions: acts,
            old_log_probs: old_lp,
            advantages: adv,
            returns: ret,
            old_values: old_v,
            mean_return,
        }
    }

    pub fn batch_size(&self) -> i64 {
        self.actions.size()[0]
    }
}

impl Clone for RolloutBatch {
    fn clone(&self) -> Self {
        Self {
            observations: self.observations.shallow_clone(),
            actions: self.actions.shallow_clone(),
            old_log_probs: self.old_log_probs.shallow_clone(),
            advantages: self.advantages.shallow_clone(),
            returns: self.returns.shallow_clone(),
            old_values: self.old_values.shallow_clone(),
            mean_return: self.mean_return,
        }
    }
}
