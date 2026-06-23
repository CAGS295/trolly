//! On-policy rollout minibatch tensors for a PPO update step.

use tch::Tensor;

/// One on-policy rollout slice ready for multi-epoch PPO optimization.
#[derive(Debug)]
pub struct RolloutBatch {
    /// Observations `[batch, obs_dim]`.
    pub obs: Tensor,
    /// Sampled discrete actions `[batch]`.
    pub actions: Tensor,
    /// Log π_old(a|s) under the behavior policy `[batch]`.
    pub old_log_probs: Tensor,
    /// Advantage estimates `[batch]`.
    pub advantages: Tensor,
    /// Value targets (returns) `[batch]`.
    pub returns: Tensor,
    /// Mean step reward in this batch (WoLF payoff signal).
    pub mean_reward: f64,
}

impl RolloutBatch {
    /// Batch size (number of transitions).
    pub fn batch_size(&self) -> i64 {
        self.obs.size()[0]
    }
}
