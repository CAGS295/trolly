//! WoLF-PPO training loop, rollout collection, and checkpoint I/O.

mod checkpoint;
mod driver;
mod env_rollout;
mod metrics;
mod rollout;

pub use checkpoint::{
    actor_critic_from_checkpoint, load_checkpoint, save_checkpoint, CheckpointMeta,
    CheckpointPaths,
};
pub use driver::{smoke_train_loop, TrainingLoopConfig, WolfPpoTrainingDriver};
pub use env_rollout::collect_env_rollout;
pub use metrics::{TrainMetricsLog, TrainStepMetrics};
pub use rollout::{normalize_advantages, rollout_buffer_to_batch, DEFAULT_GAE_LAMBDA, DEFAULT_GAMMA};
