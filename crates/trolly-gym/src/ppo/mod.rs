//! Proximal Policy Optimization (PPO) and WoLF-PPO (`torch` feature).
//!
//! Implements the clipped surrogate objective from Ratcliffe et al. (IEEE CoG 2019)
//! using [`tch`] actor–critic networks.

mod actor_critic;
mod config;
mod loss;
mod rollout;
mod trainer;
mod wolf;

pub use actor_critic::ActorCritic;
pub use config::{OptimizerKind, PpoConfig, WolfPpoConfig};
pub use loss::{is_finite_breakdown, ppo_loss, PpoLossBreakdown};
pub use rollout::RolloutBatch;
pub use trainer::{
    synthetic_rollout_batch, PolicyUpdateMetrics, PpoTrainer, WolfPolicyUpdateMetrics,
    WolfPpoTrainer,
};
pub use wolf::{NesPayoffEstimate, WolfLearningRate, WolfMode};
