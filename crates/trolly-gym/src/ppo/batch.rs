//! On-policy rollout batches consumed by PPO / WoLF-PPO update steps.

use tch::{Kind, Tensor};

/// One on-policy rollout slice ready for a PPO policy update.
///
/// Compatible with WP-020 rollout collection (obs, action, log-prob, value, reward, done).
#[derive(Debug)]
pub struct RolloutBatch {
    /// `[batch, obs_dim]` observation tensor.
    pub observations: Tensor,
    /// `[batch]` sampled action indices (int64).
    pub actions: Tensor,
    /// `[batch]` log π_old(a|s) from the behaviour policy.
    pub old_log_probs: Tensor,
    /// `[batch]` discounted return targets (or GAE targets).
    pub returns: Tensor,
    /// `[batch]` advantage estimates (returns − baseline).
    pub advantages: Tensor,
    /// `[batch]` step rewards (used by WoLF-PPO payoff tracking).
    pub rewards: Tensor,
}

impl RolloutBatch {
    /// Mean step reward in this batch (WoLF current expected payoff).
    pub fn mean_reward(&self) -> f64 {
        f64::try_from(self.rewards.mean(Kind::Float)).unwrap_or(0.0)
    }
}
