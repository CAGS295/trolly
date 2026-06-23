//! WoLF-PPO training driver: rollout collection, checkpoint I/O, and Env hooks.
//!
//! Built behind the `torch` feature. Orchestrates [`crate::ppo::WolfPpoTrainer`]
//! with on-policy rollouts from [`crate::replay::OnPolicyRolloutBuffer`] or
//! stream-fed [`crate::Env`] steps.

mod checkpoint;
mod driver;
mod env_rollout;
mod rollout;

pub use checkpoint::{
    load_actor_critic_checkpoint, save_actor_critic_checkpoint, CheckpointError,
};
pub use driver::{TrainStepMetrics, WolfPpoTrainConfig, WolfPpoTrainLoop};
pub use env_rollout::{collect_env_rollout, EnvRolloutConfig};
pub use rollout::{action_from_index, action_to_index, finish_rollout_batch, RolloutCollector};
